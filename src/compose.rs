//! Deterministic SVG composition.
//!
//! Pure functions over an injected resolver so the module stays free of
//! `Library` and is unit-testable with in-memory fixtures. Assets are
//! string-embedded inside `<symbol>…</symbol>` (the normalized markup is
//! already namespaced and validated); scaling is applied through the `use`
//! transform, never by inflating `width`/`height`.
use crate::{
    models::{ComposeArgs, PlacedAsset},
    svg::{MAX_SVG_BYTES, digest},
};
use anyhow::{Result, ensure};

const MAX_ELEMENTS: usize = 50;

type Resolver<'a> = dyn Fn(&str) -> Result<PlacedAsset> + 'a;

/// Deterministically build a composed SVG from `args`. `resolve` maps an asset
/// id to its normalized content; any failure (missing/invalid asset) aborts
/// before any file is written (all-or-nothing). Returns `(result_id, svg_bytes)`.
pub fn compose(args: &ComposeArgs, resolve: &Resolver<'_>) -> Result<(String, Vec<u8>)> {
    validate(args)?;
    let result_id = result_id_of(args);
    let (svg, _attribution) = build(args, resolve, &result_id)?;
    Ok((result_id, svg))
}

/// The deduped (first-seen order) union of the placed assets' attributions.
/// Resolves every referenced asset, so a missing asset is an error.
pub fn attribution(args: &ComposeArgs, resolve: &Resolver<'_>) -> Result<Vec<String>> {
    validate(args)?;
    let mut seen: Vec<String> = Vec::new();
    for element in &args.elements {
        let placed = resolve(&element.asset_id)?;
        if !placed.attribution.is_empty() && !seen.iter().any(|s| s == &placed.attribution) {
            seen.push(placed.attribution.clone());
        }
    }
    Ok(seen)
}

/// `ba_comp_{sha256(canonical_json(args))[:16]}`. `serde_json::to_string` gives
/// a fixed field order with no whitespace, so identical args always hash the
/// same way (and so do the stored bytes).
fn result_id_of(args: &ComposeArgs) -> String {
    let canonical = serde_json::to_string(args).expect("ComposeArgs is serializable");
    let hash = digest(canonical.as_bytes());
    format!("ba_comp_{}", &hash[..16])
}

/// Resolve every element (all-or-nothing), then emit the SVG deterministically.
fn build(
    args: &ComposeArgs,
    resolve: &Resolver<'_>,
    result_id: &str,
) -> Result<(Vec<u8>, Vec<String>)> {
    let placed: Vec<PlacedAsset> = args
        .elements
        .iter()
        .map(|element| resolve(&element.asset_id))
        .collect::<Result<_>>()?;

    let mut out = String::new();
    out.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\">",
        w = fmt_num(args.width),
        h = fmt_num(args.height)
    ));
    if let Some(background) = &args.background {
        out.push_str(&format!(
            "<rect width=\"{w}\" height=\"{h}\" fill=\"{fill}\"/>",
            w = fmt_num(args.width),
            h = fmt_num(args.height),
            fill = xml_escape(background)
        ));
    }
    let symbol_prefix = format!("ba_comp_{}", &digest(result_id.as_bytes())[..12]);
    for (i, element) in args.elements.iter().enumerate() {
        let asset = &placed[i];
        let [ox, oy, ow, oh] = asset.view_box;
        let sym = format!("{symbol_prefix}_{i}");
        out.push_str(&format!(
            "<symbol id=\"{sym}\" viewBox=\"{ox} {oy} {ow} {oh}\">{inner}</symbol>",
            sym = sym,
            ox = fmt_num(ox),
            oy = fmt_num(oy),
            ow = fmt_num(ow),
            oh = fmt_num(oh),
            inner = asset.svg
        ));
        out.push_str(&format!(
            "<use href=\"#{sym}\" width=\"{ow}\" height=\"{oh}\" transform=\"translate({x} {y}) scale({s}) rotate({r})\"/>",
            sym = sym,
            ow = fmt_num(ow),
            oh = fmt_num(oh),
            x = fmt_num(element.x),
            y = fmt_num(element.y),
            s = fmt_num(element.scale),
            r = fmt_num(element.rotation)
        ));
    }
    let attribution = attribution(args, resolve)?;
    if !attribution.is_empty() {
        let joined = attribution
            .iter()
            .map(|s| xml_escape(s))
            .collect::<Vec<_>>()
            .join("; ");
        out.push_str(&format!("<!-- attribution: {joined} -->"));
    }
    out.push_str("</svg>");
    let bytes = out.into_bytes();
    ensure!(bytes.len() <= MAX_SVG_BYTES, "composed SVG exceeds 16 MiB");
    Ok((bytes, attribution))
}

fn validate(args: &ComposeArgs) -> Result<()> {
    ensure!(
        args.width.is_finite() && args.width > 0.0,
        "canvas width must be finite and > 0"
    );
    ensure!(
        args.height.is_finite() && args.height > 0.0,
        "canvas height must be finite and > 0"
    );
    ensure!(
        (1..=MAX_ELEMENTS).contains(&args.elements.len()),
        "elements must contain 1..=50 items"
    );
    for element in &args.elements {
        ensure!(
            element
                .asset_id
                .chars()
                .next()
                .is_some_and(|c| !c.is_whitespace()),
            "asset_id must not be empty"
        );
        ensure!(element.x.is_finite(), "x must be finite");
        ensure!(element.y.is_finite(), "y must be finite");
        ensure!(element.scale.is_finite() && element.scale > 0.0, "scale must be > 0");
        ensure!(
            element.rotation.is_finite() && element.rotation.abs() <= 360.0,
            "rotation must be within ±360"
        );
    }
    if let Some(background) = &args.background {
        ensure!(
            background
                .chars()
                .all(|c| c.is_ascii_graphic() || c == ' '),
            "background must be an XML-escapable color"
        );
    }
    Ok(())
}

/// Format a number for markup: integers without a trailing `.0`, otherwise the
/// shortest exact `Display` form. Deterministic. Callers pass finite values
/// (validated up front); the non-finite branch is only a defensive fallback.
fn fmt_num(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }
    if value.fract() == 0.0 && value.abs() < 1e16 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Escape only the five XML special characters so asset markup passes through
/// unchanged.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{ComposeElement, PlacedAsset};
    use anyhow::bail;

    const VB: [f64; 4] = [0.0, 0.0, 10.0, 10.0];
    const ASSET_A: &str =
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 10 10\"><path d=\"M0 0L10 10\"/></svg>";
    const ASSET_B: &str =
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 20 30\"><path d=\"M0 0L20 30\"/></svg>";

    fn asset(id: &str, vb: [f64; 4], svg: &str, attribution: &str) -> PlacedAsset {
        PlacedAsset {
            asset_id: id.to_string(),
            svg: svg.to_string(),
            view_box: vb,
            attribution: attribution.to_string(),
        }
    }
    fn table<'a>(
        assets: &'a [(String, PlacedAsset)],
        missing: Vec<String>,
    ) -> impl Fn(&str) -> Result<PlacedAsset> + 'a {
        move |id: &str| -> Result<PlacedAsset> {
            if missing.iter().any(|m| m == id) {
                bail!("asset not found: {id}");
            }
            assets
                .iter()
                .find(|(aid, _)| aid == id)
                .map(|(_, a)| a.clone())
                .ok_or_else(|| anyhow::anyhow!("asset not found: {id}"))
        }
    }
    fn element(asset_id: &str, x: f64, y: f64, scale: f64, rotation: f64) -> ComposeElement {
        ComposeElement {
            asset_id: asset_id.into(),
            x,
            y,
            scale,
            rotation,
        }
    }

    #[test]
    fn deterministic_id_and_bytes() {
        let assets = [
            ("a".to_string(), asset("a", VB, ASSET_A, "A, CC0")),
            ("b".to_string(), asset("b", [0.0, 0.0, 20.0, 30.0], ASSET_B, "B, CC-BY-4.0")),
        ];
        let resolve = table(&assets, vec![]);
        let args = ComposeArgs {
            width: 100.0,
            height: 100.0,
            background: None,
            elements: vec![element("a", 1.0, 2.0, 1.0, 0.0), element("b", 10.0, 10.0, 2.0, 90.0)],
        };
        let (id1, bytes1) = compose(&args, &resolve).unwrap();
        let (id2, bytes2) = compose(&args, &resolve).unwrap();
        assert_eq!(id1, id2);
        assert_eq!(bytes1, bytes2);
        assert_eq!(id1, result_id_of(&args));
        assert!(id1.starts_with("ba_comp_"));
        assert_eq!(id1.len(), "ba_comp_".len() + 16);
    }

    #[test]
    fn symbol_use_pairs_and_unique_ids() {
        let assets = [
            ("a".to_string(), asset("a", VB, ASSET_A, "A")),
            ("b".to_string(), asset("b", [0.0, 0.0, 20.0, 30.0], ASSET_B, "B")),
        ];
        let resolve = table(&assets, vec![]);
        let args = ComposeArgs {
            width: 50.0,
            height: 50.0,
            background: Some("#ff0000".into()),
            elements: vec![element("a", 0.0, 0.0, 1.0, 0.0), element("b", 5.0, 5.0, 1.0, 0.0)],
        };
        let (id, svg) = compose(&args, &resolve).unwrap();
        let text = String::from_utf8(svg).unwrap();
        let symbols: Vec<&str> = text.match_indices("<symbol id=\"").map(|(i, _)| &text[i..]).collect();
        assert_eq!(symbols.len(), 2, "one symbol per element: {text}");
        let uses: Vec<&str> = text.match_indices("<use href=\"#").map(|(i, _)| &text[i..]).collect();
        assert_eq!(uses.len(), 2, "one use per element: {text}");
        // every <use> sets width and height
        for use_el in &uses {
            assert!(use_el.contains("width=\"") && use_el.contains("height=\""), "{use_el}");
        }
        // every href resolves to an emitted symbol id, and all ids are unique
        let sym_ids: Vec<String> = text
            .match_indices("<symbol id=\"")
            .map(|(i, _)| {
                let rest = &text[i + "<symbol id=\"".len()..];
                rest[..rest.find('"').unwrap()].to_string()
            })
            .collect();
        let hrefs: Vec<String> = text
            .match_indices("<use href=\"")
            .map(|(i, _)| {
                let rest = &text[i + "<use href=\"".len()..];
                rest[..rest.find('"').unwrap()].to_string()
            })
            .collect();
        // symbol ids must be unique
        {
            let mut sorted = sym_ids.clone();
            sorted.sort();
            sorted.dedup();
            assert_eq!(sym_ids.len(), sorted.len(), "symbol ids must be unique: {sym_ids:?}");
        }
        for href in &hrefs {
            assert!(
                sym_ids.iter().any(|sid| format!("#{sid}") == *href),
                "dangling href {href}; symbols {sym_ids:?}"
            );
        }
        assert!(id.starts_with("ba_comp_"));
        // parses as XML
        let root = xmltree::Element::parse(text.as_bytes()).unwrap();
        assert_eq!(root.name, "svg");
        assert_eq!(root.attributes.get("viewBox").map(String::as_str), Some("0 0 50 50"));
    }

    #[test]
    fn geometry_transform_exact() {
        let assets = [("a".to_string(), asset("a", VB, ASSET_A, "A"))];
        let resolve = table(&assets, vec![]);
        // scale=2, rotation=0
        let args = ComposeArgs {
            width: 10.0,
            height: 10.0,
            background: None,
            elements: vec![element("a", 3.0, 7.0, 2.0, 0.0)],
        };
        let (_, svg) = compose(&args, &resolve).unwrap();
        let text = String::from_utf8(svg).unwrap();
        assert!(
            text.contains("translate(3 7) scale(2) rotate(0)"),
            "expected exact transform, got: {text}"
        );
        // rotation=90
        let args = ComposeArgs {
            width: 10.0,
            height: 10.0,
            background: None,
            elements: vec![element("a", 1.0, 1.0, 1.5, 90.0)],
        };
        let (_, svg) = compose(&args, &resolve).unwrap();
        let text = String::from_utf8(svg).unwrap();
        assert!(
            text.contains("translate(1 1) scale(1.5) rotate(90)"),
            "expected exact transform, got: {text}"
        );
    }

    #[test]
    fn negative_viewbox_preserved() {
        let vb = [-5.0, -10.0, 20.0, 30.0];
        let asset_b = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"-5 -10 20 30\"><path d=\"M0 0\"/></svg>";
        let assets = [("b".to_string(), asset("b", vb, asset_b, "B"))];
        let resolve = table(&assets, vec![]);
        let args = ComposeArgs {
            width: 100.0,
            height: 100.0,
            background: None,
            elements: vec![element("b", 0.0, 0.0, 1.0, 0.0)],
        };
        let (_, svg) = compose(&args, &resolve).unwrap();
        let text = String::from_utf8(svg).unwrap();
        assert!(
            text.contains("viewBox=\"-5 -10 20 30\""),
            "negative/non-zero origin must be preserved: {text}"
        );
        // width/height are the raw ow/oh, never scaled
        assert!(text.contains("width=\"20\" height=\"30\""), "{text}");
    }

    #[test]
    fn reuse_same_asset_no_collision() {
        let assets = [("a".to_string(), asset("a", VB, ASSET_A, "A"))];
        let resolve = table(&assets, vec![]);
        let args = ComposeArgs {
            width: 10.0,
            height: 10.0,
            background: None,
            elements: vec![
                element("a", 0.0, 0.0, 1.0, 0.0),
                element("a", 1.0, 1.0, 1.0, 45.0),
            ],
        };
        let (_, svg) = compose(&args, &resolve).unwrap();
        let text = String::from_utf8(svg).unwrap();
        let sym_ids: Vec<String> = text
            .match_indices("<symbol id=\"")
            .map(|(i, _)| {
                let rest = &text[i + "<symbol id=\"".len()..];
                rest[..rest.find('"').unwrap()].to_string()
            })
            .collect();
        assert_eq!(sym_ids.len(), 2, "{text}");
        assert_ne!(sym_ids[0], sym_ids[1], "two placements must have distinct ids");
    }

    #[test]
    fn missing_asset_fails_and_no_bytes() {
        let assets = [("a".to_string(), asset("a", VB, ASSET_A, "A"))];
        let resolve = table(&assets, vec!["missing".to_string()]);
        let args = ComposeArgs {
            width: 10.0,
            height: 10.0,
            background: None,
            elements: vec![element("a", 0.0, 0.0, 1.0, 0.0), element("missing", 0.0, 0.0, 1.0, 0.0)],
        };
        let err = compose(&args, &resolve).unwrap_err();
        assert!(err.to_string().contains("missing"), "{err}");
        // validation bounds
        let many = ComposeArgs {
            width: 10.0,
            height: 10.0,
            background: None,
            elements: (0..51).map(|i| element("a", i as f64, 0.0, 1.0, 0.0)).collect(),
        };
        assert!(compose(&many, &resolve).is_err(), ">50 elements must be rejected");
        let zero_scale = ComposeArgs {
            width: 10.0,
            height: 10.0,
            background: None,
            elements: vec![element("a", 0.0, 0.0, 0.0, 0.0)],
        };
        assert!(compose(&zero_scale, &resolve).is_err(), "scale<=0 must be rejected");
        let no_elements = ComposeArgs {
            width: 10.0,
            height: 10.0,
            background: None,
            elements: vec![],
        };
        assert!(compose(&no_elements, &resolve).is_err(), "0 elements must be rejected");
        let bad_rotation = ComposeArgs {
            width: 10.0,
            height: 10.0,
            background: None,
            elements: vec![element("a", 0.0, 0.0, 1.0, 720.0)],
        };
        assert!(compose(&bad_rotation, &resolve).is_err(), "|rotation|>360 must be rejected");
    }

    #[test]
    fn attribution_deduped_first_seen_order() {
        let assets = [
            ("a".to_string(), asset("a", VB, ASSET_A, "Shared, CC0")),
            ("b".to_string(), asset("b", [0.0, 0.0, 20.0, 30.0], ASSET_B, "Unique, CC-BY-4.0")),
            ("c".to_string(), asset("c", VB, ASSET_A, "Shared, CC0")),
        ];
        let resolve = table(&assets, vec![]);
        let args = ComposeArgs {
            width: 10.0,
            height: 10.0,
            background: None,
            elements: vec![
                element("a", 0.0, 0.0, 1.0, 0.0),
                element("b", 1.0, 1.0, 1.0, 0.0),
                element("c", 2.0, 2.0, 1.0, 0.0),
            ],
        };
        let attr = attribution(&args, &resolve).unwrap();
        assert_eq!(attr, vec!["Shared, CC0".to_string(), "Unique, CC-BY-4.0".to_string()]);
        // and it is embedded as a comment
        let (_, svg) = compose(&args, &resolve).unwrap();
        let text = String::from_utf8(svg).unwrap();
        assert!(text.contains("<!-- attribution: Shared, CC0; Unique, CC-BY-4.0 -->"), "{text}");
    }

    #[test]
    fn background_rect_emitted() {
        let assets = [("a".to_string(), asset("a", VB, ASSET_A, "A"))];
        let resolve = table(&assets, vec![]);
        let args = ComposeArgs {
            width: 100.0,
            height: 50.0,
            background: Some("#ffffff".into()),
            elements: vec![element("a", 0.0, 0.0, 1.0, 0.0)],
        };
        let (_, svg) = compose(&args, &resolve).unwrap();
        let text = String::from_utf8(svg).unwrap();
        assert!(text.contains("<rect width=\"100\" height=\"50\" fill=\"#ffffff\"/>"), "{text}");
        xmltree::Element::parse(text.as_bytes()).unwrap();
    }
}
