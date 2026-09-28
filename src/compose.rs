//! Deterministic SVG composition.
//!
//! Pure functions over an injected resolver so the module stays free of
//! `Library` and is unit-testable with in-memory fixtures. Assets are
//! string-embedded inside `<symbol>…</symbol>` (the normalized markup is
//! already namespaced and validated); scaling is applied through the `use`
//! transform, never by inflating `width`/`height`.
use crate::{
    models::{
        AssetElement, CircleElement, ComposeArgs, ComposeElement, LineElement, PlacedAsset,
        RectElement, TextElement,
    },
    svg::{MAX_SVG_BYTES, digest},
};
use anyhow::{Result, ensure};

const MAX_ELEMENTS: usize = 50;

type Resolver<'a> = dyn Fn(&str) -> Result<PlacedAsset> + 'a;

/// Deterministically build a composed SVG from `args`. `resolve` maps an asset
/// id to its normalized content; any failure (missing/invalid asset) aborts
/// before any file is written (all-or-nothing). Returns `(result_id, svg_bytes)`,
/// where `result_id` is content-addressed over the final SVG bytes.
pub fn compose(args: &ComposeArgs, resolve: &Resolver<'_>) -> Result<(String, Vec<u8>)> {
    validate(args)?;
    let seed = seed_of(args);
    let (svg, _attribution) = build(args, resolve, &seed)?;
    let result_id = result_id_of(&svg);
    Ok((result_id, svg))
}

/// The deduped (first-seen order) union of the placed assets' attributions.
/// Resolves every referenced asset, so a missing asset is an error.
pub fn attribution(args: &ComposeArgs, resolve: &Resolver<'_>) -> Result<Vec<String>> {
    validate(args)?;
    let mut seen: Vec<String> = Vec::new();
    for element in &args.elements {
        let ComposeElement::Asset(a) = element else {
            continue;
        };
        let placed = resolve(&a.asset_id)?;
        if !placed.attribution.is_empty() && !seen.iter().any(|s| s == &placed.attribution) {
            seen.push(placed.attribution.clone());
        }
    }
    Ok(seen)
}

/// `ba_comp_{sha256(svg_bytes)[:16]}`: content-addressed over the final
/// composed SVG bytes, so identical bytes always yield the same id.
pub fn result_id_of(svg: &[u8]) -> String {
    format!("ba_comp_{}", &digest(svg)[..16])
}

/// The symbol-id seed: `sha256(canonical_json(args))`. `serde_json::to_string`
/// gives a fixed field order with no whitespace, so identical args always seed
/// the same way. Derived from the args (not the bytes) because the bytes
/// themselves embed the symbol ids, which would be circular.
fn seed_of(args: &ComposeArgs) -> String {
    let canonical = serde_json::to_string(args).expect("ComposeArgs is serializable");
    digest(canonical.as_bytes())
}

/// Resolve every element (all-or-nothing), then emit the SVG deterministically.
/// `seed` is the args-derived symbol-id seed (see `seed_of`).
fn build(
    args: &ComposeArgs,
    resolve: &Resolver<'_>,
    seed: &str,
) -> Result<(Vec<u8>, Vec<String>)> {
    // Resolve every asset element all-or-nothing (a missing asset id aborts
    // before any bytes are produced); non-asset elements are not resolved.
    let placed: Vec<Option<PlacedAsset>> = args
        .elements
        .iter()
        .map(|element| match element {
            ComposeElement::Asset(a) => resolve(&a.asset_id).map(Some),
            _ => Ok(None),
        })
        .collect::<Result<Vec<_>, _>>()?;

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
    let symbol_prefix = format!("ba_comp_{}", &seed[..12]);
    for (i, element) in args.elements.iter().enumerate() {
        match element {
            ComposeElement::Asset(a) => {
                let asset = placed[i]
                    .as_ref()
                    .expect("every asset element is resolved up front");
                emit_asset(&mut out, a, asset, &format!("{symbol_prefix}_{i}"));
            }
            ComposeElement::Text(t) => emit_text(&mut out, t),
            ComposeElement::Line(l) => emit_line(&mut out, l),
            ComposeElement::Rect(r) => emit_rect(&mut out, r),
            ComposeElement::Circle(c) => emit_circle(&mut out, c),
        }
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

/// Emit a placed asset as a `<symbol>` (holding the normalized SVG) plus a
/// `<use>` that anchors it at its placement. `sym` is the element's unique
/// symbol id. The asset markup is passed through unchanged (already namespaced
/// and validated by the importer).
fn emit_asset(out: &mut String, a: &AssetElement, asset: &PlacedAsset, sym: &str) {
    let [ox, oy, ow, oh] = asset.view_box;
    out.push_str(&format!(
        "<symbol id=\"{sym}\" viewBox=\"{ox} {oy} {ow} {oh}\">{inner}</symbol>",
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
        x = fmt_num(a.x),
        y = fmt_num(a.y),
        s = fmt_num(a.scale),
        r = fmt_num(a.rotation)
    ));
}

/// Emit a native `<text>` element. Attribute order is fixed: x, y, font-size,
/// text-anchor, then fill only when present.
fn emit_text(out: &mut String, t: &TextElement) {
    let mut attrs = format!(
        "x=\"{x}\" y=\"{y}\" font-size=\"{fs}\" text-anchor=\"{anchor}\"",
        x = fmt_num(t.x),
        y = fmt_num(t.y),
        fs = fmt_num(t.font_size),
        anchor = xml_escape(&t.anchor)
    );
    if let Some(fill) = &t.fill {
        attrs.push_str(&format!(" fill=\"{fill}\"", fill = xml_escape(fill)));
    }
    out.push_str(&format!(
        "<text {attrs}>{text}</text>",
        text = xml_escape(&t.text)
    ));
}

/// Emit a native `<line>` element. Attribute order is fixed: x1, y1, x2, y2,
/// stroke-width, then stroke only when present. When `arrow_end` is set a
/// two-barb chevron is added at the tip.
fn emit_line(out: &mut String, l: &LineElement) {
    let mut attrs = format!(
        "x1=\"{x1}\" y1=\"{y1}\" x2=\"{x2}\" y2=\"{y2}\" stroke-width=\"{w}\"",
        x1 = fmt_num(l.x1),
        y1 = fmt_num(l.y1),
        x2 = fmt_num(l.x2),
        y2 = fmt_num(l.y2),
        w = fmt_num(l.width)
    );
    if let Some(stroke) = &l.stroke {
        attrs.push_str(&format!(" stroke=\"{stroke}\"", stroke = xml_escape(stroke)));
    }
    out.push_str(&format!("<line {attrs}/>"));
    if l.arrow_end {
        emit_arrowhead(out, l);
    }
}

/// Emit a native `<rect>` element. `rx` is always emitted (plain field); `fill`
/// and `stroke` are emitted only when present, and `stroke-width` only when
/// `stroke` is present.
fn emit_rect(out: &mut String, r: &RectElement) {
    let mut attrs = format!(
        "x=\"{x}\" y=\"{y}\" width=\"{w}\" height=\"{h}\" rx=\"{rx}\"",
        x = fmt_num(r.x),
        y = fmt_num(r.y),
        w = fmt_num(r.width),
        h = fmt_num(r.height),
        rx = fmt_num(r.rx)
    );
    if let Some(fill) = &r.fill {
        attrs.push_str(&format!(" fill=\"{fill}\"", fill = xml_escape(fill)));
    }
    if let Some(stroke) = &r.stroke {
        attrs.push_str(&format!(
            " stroke=\"{stroke}\" stroke-width=\"{sw}\"",
            stroke = xml_escape(stroke),
            sw = fmt_num(r.stroke_width)
        ));
    }
    out.push_str(&format!("<rect {attrs}/>"));
}

/// Emit a native `<circle>` element. `fill` and `stroke` are emitted only when
/// present, and `stroke-width` only when `stroke` is present.
fn emit_circle(out: &mut String, c: &CircleElement) {
    let mut attrs = format!(
        "cx=\"{cx}\" cy=\"{cy}\" r=\"{r}\"",
        cx = fmt_num(c.cx),
        cy = fmt_num(c.cy),
        r = fmt_num(c.r)
    );
    if let Some(fill) = &c.fill {
        attrs.push_str(&format!(" fill=\"{fill}\"", fill = xml_escape(fill)));
    }
    if let Some(stroke) = &c.stroke {
        attrs.push_str(&format!(
            " stroke=\"{stroke}\" stroke-width=\"{sw}\"",
            stroke = xml_escape(stroke),
            sw = fmt_num(c.stroke_width)
        ));
    }
    out.push_str(&format!("<circle {attrs}/>"));
}

/// Emit the two-barb chevron arrowhead at the tip `(x2, y2)` of a line.
///
/// Pure function of the inputs (deterministic). With the line direction angle
/// `theta = atan2(y2 - y1, x2 - x1)` and a barb half-spread of 30 degrees
/// (`spread = PI / 6`), each barb runs from the tip back along
/// `theta + PI -/+ spread`:
///
/// ```text
/// barb_k = (x2 + L * cos(theta + PI -/+ spread), y2 + L * sin(theta + PI -/+ spread))
/// ```
///
/// Both barbs are emitted as `<line>` elements (no `<path>`, no marker) sharing
/// the main line's `stroke` (when present) and `stroke-width`. `L` is the barb
/// length: `max(4.0 * width, 2.0)`, so thin lines still show a visible arrow.
fn emit_arrowhead(out: &mut String, l: &LineElement) {
    let theta = (l.y2 - l.y1).atan2(l.x2 - l.x1);
    const SPREAD: f64 = std::f64::consts::PI / 6.0;
    let len = (4.0 * l.width).max(2.0);
    let stroke = match &l.stroke {
        Some(s) => format!(" stroke=\"{}\"", xml_escape(s)),
        None => String::new(),
    };
    for sign in [-1.0, 1.0] {
        let angle = theta + std::f64::consts::PI + sign * SPREAD;
        let bx = l.x2 + len * angle.cos();
        let by = l.y2 + len * angle.sin();
        out.push_str(&format!(
            "<line x1=\"{x2}\" y1=\"{y2}\" x2=\"{bx}\" y2=\"{by}\" stroke-width=\"{w}\"{stroke}/>",
            x2 = fmt_num(l.x2),
            y2 = fmt_num(l.y2),
            bx = fmt_num(bx),
            by = fmt_num(by),
            w = fmt_num(l.width),
            stroke = stroke
        ));
    }
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
        validate_element(element)?;
    }
    if let Some(background) = &args.background {
        ensure!(
            is_valid_color(background),
            "background must be an XML-escapable color"
        );
    }
    Ok(())
}

/// Per-variant geometry validation. All numeric geometry must be finite and
/// within sensible bounds; every `Some` fill/stroke must be a valid color.
fn validate_element(element: &ComposeElement) -> Result<()> {
    match element {
        ComposeElement::Asset(a) => {
            ensure!(
                a.asset_id
                    .chars()
                    .next()
                    .is_some_and(|c| !c.is_whitespace()),
                "asset_id must not be empty"
            );
            ensure!(a.x.is_finite(), "x must be finite");
            ensure!(a.y.is_finite(), "y must be finite");
            ensure!(a.scale.is_finite() && a.scale > 0.0, "scale must be > 0");
            ensure!(
                a.rotation.is_finite() && a.rotation.abs() <= 360.0,
                "rotation must be within ±360"
            );
        }
        ComposeElement::Text(t) => {
            ensure!(t.x.is_finite(), "x must be finite");
            ensure!(t.y.is_finite(), "y must be finite");
            ensure!(
                t.font_size.is_finite() && t.font_size > 0.0,
                "font_size must be > 0"
            );
            ensure!(
                matches!(t.anchor.as_str(), "start" | "middle" | "end"),
                "anchor must be one of start, middle, end"
            );
            if let Some(fill) = &t.fill {
                ensure!(is_valid_color(fill), "fill must be an XML-escapable color");
            }
        }
        ComposeElement::Line(l) => {
            ensure!(l.x1.is_finite(), "x1 must be finite");
            ensure!(l.y1.is_finite(), "y1 must be finite");
            ensure!(l.x2.is_finite(), "x2 must be finite");
            ensure!(l.y2.is_finite(), "y2 must be finite");
            ensure!(l.width.is_finite() && l.width > 0.0, "width must be > 0");
            if let Some(stroke) = &l.stroke {
                ensure!(is_valid_color(stroke), "stroke must be an XML-escapable color");
            }
        }
        ComposeElement::Rect(r) => {
            ensure!(r.x.is_finite(), "x must be finite");
            ensure!(r.y.is_finite(), "y must be finite");
            ensure!(r.width.is_finite() && r.width > 0.0, "width must be > 0");
            ensure!(r.height.is_finite() && r.height > 0.0, "height must be > 0");
            ensure!(r.rx.is_finite() && r.rx >= 0.0, "rx must be >= 0");
            if let Some(fill) = &r.fill {
                ensure!(is_valid_color(fill), "fill must be an XML-escapable color");
            }
            if let Some(stroke) = &r.stroke {
                ensure!(is_valid_color(stroke), "stroke must be an XML-escapable color");
            }
        }
        ComposeElement::Circle(c) => {
            ensure!(c.cx.is_finite(), "cx must be finite");
            ensure!(c.cy.is_finite(), "cy must be finite");
            ensure!(c.r.is_finite() && c.r > 0.0, "r must be > 0");
            if let Some(fill) = &c.fill {
                ensure!(is_valid_color(fill), "fill must be an XML-escapable color");
            }
            if let Some(stroke) = &c.stroke {
                ensure!(is_valid_color(stroke), "stroke must be an XML-escapable color");
            }
        }
    }
    Ok(())
}

/// A color string is valid when every character is an ASCII graphic (printable
/// non-space) or a space — the same rule `background` always used.
fn is_valid_color(s: &str) -> bool {
    s.chars().all(|c| c.is_ascii_graphic() || c == ' ')
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
    use crate::models::{
        AssetElement, CircleElement, ComposeElement, LineElement, PlacedAsset, RectElement,
        TextElement,
    };
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
        ComposeElement::Asset(AssetElement {
            asset_id: asset_id.into(),
            x,
            y,
            scale,
            rotation,
        })
    }
    fn text_elem(
        text: &str,
        x: f64,
        y: f64,
        font_size: f64,
        anchor: &str,
        fill: Option<&str>,
    ) -> ComposeElement {
        ComposeElement::Text(TextElement {
            text: text.into(),
            x,
            y,
            font_size,
            anchor: anchor.into(),
            fill: fill.map(str::to_owned),
        })
    }
    fn line_elem(
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        width: f64,
        stroke: Option<&str>,
        arrow_end: bool,
    ) -> ComposeElement {
        ComposeElement::Line(LineElement {
            x1,
            y1,
            x2,
            y2,
            width,
            stroke: stroke.map(str::to_owned),
            arrow_end,
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn rect_elem(
        x: f64,
        y: f64,
        width: f64,
        height: f64,
        rx: f64,
        fill: Option<&str>,
        stroke: Option<&str>,
        stroke_width: f64,
    ) -> ComposeElement {
        ComposeElement::Rect(RectElement {
            x,
            y,
            width,
            height,
            rx,
            fill: fill.map(str::to_owned),
            stroke: stroke.map(str::to_owned),
            stroke_width,
        })
    }
    fn circle_elem(
        cx: f64,
        cy: f64,
        r: f64,
        fill: Option<&str>,
        stroke: Option<&str>,
        stroke_width: f64,
    ) -> ComposeElement {
        ComposeElement::Circle(CircleElement {
            cx,
            cy,
            r,
            fill: fill.map(str::to_owned),
            stroke: stroke.map(str::to_owned),
            stroke_width,
        })
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
        // content-addressed: the id is derived from the final bytes
        assert_eq!(id1, result_id_of(&bytes1));
        assert!(id1.starts_with("ba_comp_"));
        assert_eq!(id1.len(), "ba_comp_".len() + 16);
        assert!(
            id1["ba_comp_".len()..].chars().all(|c| c.is_ascii_hexdigit()),
            "id suffix must be 16 hex chars: {id1}"
        );
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

    /// Compose a single element on a 100x100 canvas with no background and
    /// return the produced SVG text.
    fn single_svg(element: ComposeElement) -> String {
        let assets = [("a".to_string(), asset("a", VB, ASSET_A, "A"))];
        let resolve = table(&assets, vec![]);
        let args = ComposeArgs {
            width: 100.0,
            height: 100.0,
            background: None,
            elements: vec![element],
        };
        let (_, svg) = compose(&args, &resolve).unwrap();
        String::from_utf8(svg).unwrap()
    }

    #[test]
    fn native_elements_render_expected_markup() {
        let svg = single_svg(text_elem("Hi", 10.0, 20.0, 16.0, "middle", Some("#000000")));
        assert!(
            svg.contains(
                "<text x=\"10\" y=\"20\" font-size=\"16\" text-anchor=\"middle\" fill=\"#000000\">Hi</text>"
            ),
            "text markup: {svg}"
        );
        xmltree::Element::parse(svg.as_bytes()).unwrap();

        let svg = single_svg(line_elem(0.0, 0.0, 50.0, 25.0, 2.0, Some("#000"), false));
        assert!(
            svg.contains("<line x1=\"0\" y1=\"0\" x2=\"50\" y2=\"25\" stroke-width=\"2\" stroke=\"#000\"/>"),
            "line markup: {svg}"
        );
        xmltree::Element::parse(svg.as_bytes()).unwrap();

        let svg = single_svg(rect_elem(0.0, 0.0, 40.0, 20.0, 4.0, Some("#fff"), Some("#000"), 1.0));
        assert!(
            svg.contains(
                "<rect x=\"0\" y=\"0\" width=\"40\" height=\"20\" rx=\"4\" fill=\"#fff\" stroke=\"#000\" stroke-width=\"1\"/>"
            ),
            "rect markup: {svg}"
        );
        xmltree::Element::parse(svg.as_bytes()).unwrap();

        let svg = single_svg(circle_elem(20.0, 20.0, 10.0, Some("#f00"), Some("#000"), 2.0));
        assert!(
            svg.contains(
                "<circle cx=\"20\" cy=\"20\" r=\"10\" fill=\"#f00\" stroke=\"#000\" stroke-width=\"2\"/>"
            ),
            "circle markup: {svg}"
        );
        xmltree::Element::parse(svg.as_bytes()).unwrap();
    }

    #[test]
    fn text_content_is_escaped() {
        let svg = single_svg(text_elem("A & B < C", 0.0, 0.0, 12.0, "start", None));
        assert!(
            svg.contains("A &amp; B &lt; C"),
            "expected escaped text in markup: {svg}"
        );
        xmltree::Element::parse(svg.as_bytes()).unwrap();
    }

    #[test]
    fn line_arrow_end_emits_barbs() {
        let with_arrow = single_svg(line_elem(0.0, 0.0, 50.0, 25.0, 2.0, Some("#000"), true));
        assert_eq!(
            with_arrow.matches("<line ").count(),
            3,
            "main line + 2 barbs: {with_arrow}"
        );
        xmltree::Element::parse(with_arrow.as_bytes()).unwrap();

        let no_arrow = single_svg(line_elem(0.0, 0.0, 50.0, 25.0, 2.0, Some("#000"), false));
        assert_eq!(
            no_arrow.matches("<line ").count(),
            1,
            "only the main line: {no_arrow}"
        );
        xmltree::Element::parse(no_arrow.as_bytes()).unwrap();
    }

    #[test]
    fn mixed_scene_is_deterministic() {
        let assets = [("a".to_string(), asset("a", VB, ASSET_A, "A"))];
        let resolve = table(&assets, vec![]);
        let make = || ComposeArgs {
            width: 100.0,
            height: 100.0,
            background: Some("#fff".into()),
            elements: vec![
                element("a", 1.0, 2.0, 1.0, 0.0),
                text_elem("Hi", 5.0, 5.0, 16.0, "middle", None),
                line_elem(0.0, 0.0, 50.0, 25.0, 2.0, Some("#000"), true),
                rect_elem(0.0, 0.0, 40.0, 20.0, 4.0, Some("#fff"), None, 1.0),
                circle_elem(20.0, 20.0, 10.0, Some("#f00"), None, 2.0),
            ],
        };
        let (id1, bytes1) = compose(&make(), &resolve).unwrap();
        let (id2, bytes2) = compose(&make(), &resolve).unwrap();
        assert_eq!(id1, id2, "identical mixed args must yield the same result_id");
        assert_eq!(bytes1, bytes2, "identical mixed args must yield the same bytes");
        xmltree::Element::parse(bytes1.as_slice()).unwrap();
    }

    #[test]
    fn asset_only_backward_compat_and_variant_dispatch() {
        // untagged deserialization: asset-shaped JSON (no `type` key) dispatches to Asset
        let parsed: ComposeArgs = serde_json::from_str(
            r#"{"width":10,"height":10,"elements":[{"asset_id":"a","x":0,"y":0,"scale":1,"rotation":0}]}"#,
        )
        .unwrap();
        assert!(
            matches!(&parsed.elements[0], ComposeElement::Asset(_)),
            "asset-shaped JSON must deserialize to the Asset variant"
        );

        let assets = [("a".to_string(), asset("a", VB, ASSET_A, "A"))];
        let resolve = table(&assets, vec![]);
        let args = ComposeArgs {
            width: 100.0,
            height: 100.0,
            background: None,
            elements: vec![element("a", 5.0, 5.0, 2.0, 0.0)],
        };
        let (id, svg) = compose(&args, &resolve).unwrap();
        assert!(id.starts_with("ba_comp_"), "asset-only result_id must start with ba_comp_: {id}");
        let text = String::from_utf8(svg).unwrap();
        assert!(text.contains("<symbol"), "asset-only SVG must contain <symbol: {text}");
        assert!(text.contains("<use"), "asset-only SVG must contain <use: {text}");
    }

    #[test]
    fn new_element_validation() {
        let assets = [("a".to_string(), asset("a", VB, ASSET_A, "A"))];
        let resolve = table(&assets, vec![]);
        let expect_err = |el: ComposeElement| {
            let args = ComposeArgs {
                width: 10.0,
                height: 10.0,
                background: None,
                elements: vec![el],
            };
            compose(&args, &resolve).is_err()
        };
        assert!(
            expect_err(text_elem("a", 0.0, 0.0, 0.0, "start", None)),
            "font_size<=0 must be rejected"
        );
        assert!(
            expect_err(circle_elem(0.0, 0.0, 0.0, None, None, 1.0)),
            "r<=0 must be rejected"
        );
        assert!(
            expect_err(rect_elem(0.0, 0.0, 0.0, 10.0, 0.0, None, None, 1.0)),
            "rect width<=0 must be rejected"
        );
        assert!(
            expect_err(line_elem(0.0, 0.0, 10.0, 0.0, 0.0, None, false)),
            "line width<=0 must be rejected"
        );
        assert!(
            expect_err(text_elem("a", 0.0, 0.0, 12.0, "left", None)),
            "invalid anchor must be rejected"
        );

        // unknown field on any element is rejected via deny_unknown_fields
        assert!(
            serde_json::from_str::<ComposeArgs>(
                r#"{"width":10,"height":10,"elements":[{"text":"a","x":0,"y":0,"bogus":1}]}"#
            )
            .is_err(),
            "unknown field must be rejected by deny_unknown_fields"
        );
    }

    #[test]
    fn fill_stroke_omitted_when_none() {
        let bare = single_svg(rect_elem(0.0, 0.0, 40.0, 20.0, 4.0, None, None, 1.0));
        let rect = bare
            .split_once("<rect ")
            .and_then(|(_, rest)| rest.split_once("/>").map(|(head, _)| format!("<rect {head}/>")))
            .unwrap();
        assert!(!rect.contains("fill="), "no fill attribute when None: {rect}");
        assert!(!rect.contains("stroke="), "no stroke attribute when None: {rect}");
        xmltree::Element::parse(bare.as_bytes()).unwrap();

        let styled = single_svg(rect_elem(0.0, 0.0, 40.0, 20.0, 4.0, Some("#fff"), Some("#000"), 2.0));
        let rect2 = styled
            .split_once("<rect ")
            .and_then(|(_, rest)| rest.split_once("/>").map(|(head, _)| format!("<rect {head}/>")))
            .unwrap();
        assert!(rect2.contains("fill=\"#fff\""), "fill attribute when Some: {rect2}");
        assert!(rect2.contains("stroke=\"#000\""), "stroke attribute when Some: {rect2}");
        assert!(rect2.contains("stroke-width=\"2\""), "stroke-width emitted with stroke: {rect2}");
        xmltree::Element::parse(styled.as_bytes()).unwrap();
    }
}
