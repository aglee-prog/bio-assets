//! Conservative composition preparation, never a renderer or path optimizer.
use anyhow::{Context, Result, bail, ensure};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::OnceLock,
};
use xmltree::{Element, EmitterConfig, XMLNode};

pub const MAX_SVG_BYTES: usize = 16 * 1024 * 1024;

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn prepare(bytes: &[u8], id: &str) -> Result<String> {
    ensure!(bytes.len() <= MAX_SVG_BYTES, "SVG exceeds 16 MiB");
    let text = std::str::from_utf8(bytes).context("SVG is not UTF-8")?;
    validate_entities(text)?;
    let mut root = Element::parse(bytes).context("invalid SVG XML")?;
    ensure!(root.name == "svg", "document root is not svg");
    ensure!(
        root.namespace.as_deref() == Some("http://www.w3.org/2000/svg"),
        "missing SVG namespace"
    );
    remove_editor_metadata(&mut root);
    distinguish_unused_ids(&mut root)?;
    let prefix = format!("ba_{}", &digest(id.as_bytes())[..16]);
    let mut ids = HashMap::new();
    let mut classes = HashMap::new();
    collect(&root, &prefix, &mut ids, &mut classes)?;
    if !root.attributes.contains_key("viewBox") {
        let dimension = |key: &str| -> Result<f64> {
            let value = root
                .attributes
                .get(key)
                .context("SVG needs viewBox or numeric width/height")?;
            let number: f64 = value
                .trim_end_matches("px")
                .parse()
                .context("cannot infer viewBox from relative dimensions")?;
            ensure!(number.is_finite() && number > 0.0, "invalid SVG dimension");
            Ok(number)
        };
        let (w, h) = (dimension("width")?, dimension("height")?);
        root.attributes
            .insert("viewBox".into(), format!("0 0 {w} {h}"));
    }
    let viewbox: Vec<f64> = root.attributes["viewBox"]
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()?;
    ensure!(
        viewbox.len() == 4
            && viewbox.iter().all(|n| n.is_finite())
            && viewbox[2] > 0.0
            && viewbox[3] > 0.0,
        "invalid viewBox"
    );
    rewrite(&mut root, &ids, &classes)?;
    let mut out = Vec::new();
    root.write_with_config(
        &mut out,
        EmitterConfig::new().write_document_declaration(false),
    )?;
    Ok(String::from_utf8(out)?)
}

/// Illustrator declares literal namespace URIs in its internal DTD. These do
/// not require fetching a DTD. Reject external/recursive/parameter entities;
/// xml-rs itself has no external-entity loader.
fn validate_entities(text: &str) -> Result<()> {
    if !text.contains("<!ENTITY") {
        return Ok(());
    }
    let declarations = Regex::new(
        r#"<!ENTITY\s+[A-Za-z_][A-Za-z0-9_.-]*\s+(?:"(https?://[^"<&%]{1,256})"|'(https?://[^'<&%]{1,256})')\s*>"#,
    )?;
    let count = declarations.find_iter(text).count();
    ensure!(
        count <= 64 && count == text.matches("<!ENTITY").count(),
        "external, recursive or non-URI XML entities are unsupported"
    );
    ensure!(
        text.matches('&').count().saturating_mul(256) <= MAX_SVG_BYTES,
        "XML entity expansion exceeds size limit"
    );
    Ok(())
}

fn distinguish_unused_ids(root: &mut Element) -> Result<()> {
    fn visit(el: &Element, counts: &mut HashMap<String, usize>) {
        if let Some(id) = el.attributes.get("id") {
            *counts.entry(id.clone()).or_default() += 1;
        }
        for child in &el.children {
            if let XMLNode::Element(child) = child {
                visit(child, counts);
            }
        }
    }
    fn styles(el: &Element, out: &mut Vec<String>) {
        if el.name == "style" {
            out.push(el.get_text().unwrap_or_default().into_owned());
        }
        for child in &el.children {
            if let XMLNode::Element(child) = child {
                styles(child, out);
            }
        }
    }
    let mut counts = HashMap::new();
    visit(root, &mut counts);
    let mut style_texts = Vec::new();
    styles(root, &mut style_texts);
    // Browsers resolve url(#id) and aria-* references to the first occurrence
    // in tree order, so keeping the first is unambiguous. Reject only when a
    // stylesheet id selector could target the renamed copy.
    let comments = Regex::new(r"(?s)/\*.*?\*/")?;
    for (id, count) in &counts {
        if *count > 1 {
            // An id selector is `#id` followed by a non-identifier boundary
            // (or end of the selector list part); `#id2` must not match.
            let selector = format!("#{id}");
            ensure!(
                !style_texts.iter().any(|css| {
                    comments.replace_all(css, "").split('}').any(|rule| {
                        rule.split_once('{').is_some_and(|(head, _)| {
                            head.split(',').any(|part| {
                                let Some(rest) = part.trim().strip_prefix(&selector) else {
                                    return false;
                                };
                                rest.is_empty()
                                    || rest
                                        .chars()
                                        .next()
                                        .is_some_and(|c| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
                            })
                        })
                    })
                }),
                "ambiguous duplicate SVG id: {id}"
            );
        }
    }
    let mut used: HashSet<String> = counts.keys().cloned().collect();
    fn rename(el: &mut Element, seen: &mut HashSet<String>, used: &mut HashSet<String>) {
        if let Some(id) = el.attributes.get_mut("id") && !seen.insert(id.clone()) {
            let mut suffix = 2;
            loop {
                let candidate = format!("{id}__duplicate_{suffix}");
                if used.insert(candidate.clone()) {
                    *id = candidate;
                    break;
                }
                suffix += 1;
            }
        }
        for child in &mut el.children {
            if let XMLNode::Element(child) = child {
                rename(child, seen, used);
            }
        }
    }
    rename(root, &mut HashSet::new(), &mut used);
    Ok(())
}

fn collect(
    el: &Element,
    prefix: &str,
    ids: &mut HashMap<String, String>,
    classes: &mut HashMap<String, String>,
) -> Result<()> {
    if let Some(id) = el.attributes.get("id") {
        ensure!(!ids.contains_key(id), "duplicate SVG id: {id}");
        ids.insert(id.clone(), format!("{prefix}_i{}", ids.len()));
    }
    if let Some(value) = el.attributes.get("class") {
        for class in value.split_whitespace() {
            let replacement = format!("{prefix}_c{}", classes.len());
            classes.entry(class.into()).or_insert(replacement);
        }
    }
    for child in &el.children {
        if let XMLNode::Element(child) = child {
            collect(child, prefix, ids, classes)?;
        }
    }
    Ok(())
}

fn remove_editor_metadata(el: &mut Element) {
    // Keep metadata: it can contain copyright and licensing statements.
    el.children.retain(|child| {
        !matches!(child, XMLNode::Element(e) if e.name == "namedview"
            || e.namespace.as_deref()==Some("http://www.inkscape.org/namespaces/inkscape")
            || adobe_private_branch(e))
    });
    // xmltree retains element namespaces but drops attribute prefixes. These
    // exporter hints are not rendering properties and can contain local paths.
    for key in [
        "docname",
        "export-filename",
        "export-xdpi",
        "export-ydpi",
        "version",
        "document-units",
    ] {
        if key != "version" || el.name != "svg" {
            el.attributes.remove(key);
        }
    }
    for child in &mut el.children {
        if let XMLNode::Element(child) = child {
            remove_editor_metadata(child);
        }
    }
}

fn adobe_private_branch(el: &Element) -> bool {
    const AI: &str = "http://ns.adobe.com/AdobeIllustrator/10.0/";
    el.name == "foreignObject"
        && el
            .attributes
            .get("requiredExtensions")
            .is_some_and(|s| s == AI)
        && el.children.iter().all(|node| match node {
            XMLNode::Element(e) => {
                e.namespace.as_deref() == Some(AI)
                    && e.name == "aipgfRef"
                    && e.children
                        .iter()
                        .all(|c| matches!(c,XMLNode::Text(t) if t.trim().is_empty()))
            }
            XMLNode::Text(t) => t.trim().is_empty(),
            XMLNode::Comment(_) => true,
            _ => false,
        })
}

fn fragment(value: &str) -> Result<String> {
    Ok(percent_encoding::percent_decode_str(value)
        .decode_utf8()
        .context("invalid UTF-8 fragment")?
        .into_owned())
}

fn references(value: &str, ids: &HashMap<String, String>) -> Result<String> {
    let lower = value.to_ascii_lowercase();
    ensure!(
        !lower.contains("@import")
            && !lower.contains("javascript:")
            && !lower.contains("expression(")
            && !lower.contains('\\'),
        "unsupported active or escaped SVG/CSS content"
    );
    static URL_RE: OnceLock<Regex> = OnceLock::new();
    let re = URL_RE.get_or_init(|| Regex::new(r"(?i)url\(\s*([^)]*?)\s*\)").unwrap());
    let mut error = None;
    let result = re
        .replace_all(value, |cap: &regex::Captures<'_>| {
            let target = cap[1].trim().trim_matches(['\'', '"']);
            if let Some(mapped) = target
                .strip_prefix('#')
                .and_then(|v| fragment(v).ok())
                .and_then(|v| ids.get(&v))
            {
                format!("url(#{mapped})")
            } else {
                error = Some(format!("external or unresolved SVG reference: {target}"));
                cap[0].to_string()
            }
        })
        .into_owned();
    if let Some(error) = error {
        bail!(error);
    }
    Ok(result)
}

fn stylesheet(
    css: &str,
    ids: &HashMap<String, String>,
    classes: &HashMap<String, String>,
) -> Result<String> {
    let comments = Regex::new(r"(?s)/\*.*?\*/")?;
    let css = comments.replace_all(css, "");
    // External @font-face src would be an unreachable reference at render
    // time; drop those blocks. Local/data font-faces still hit the @ check.
    let font_face = Regex::new(r"(?i)@font-face\s*\{[^{}]*\}")?;
    let external_src = Regex::new(r"(?i)src\s*:[^;}]*(?:https?://)")?;
    let css = font_face.replace_all(&css, |cap: &regex::Captures<'_>| {
        if external_src.is_match(&cap[0]) {
            String::new()
        } else {
            cap[0].to_string()
        }
    });
    ensure!(
        !css.contains('@') && !css.contains('\\'),
        "unsupported CSS at-rule or escape; original retained"
    );
    let selector = Regex::new(r"^([.#])([a-zA-Z_][a-zA-Z0-9_-]*)$")?;
    let mut out = String::new();
    for rule in css.split('}') {
        if rule.trim().is_empty() {
            continue;
        }
        let (head, body) = rule.split_once('{').context("invalid CSS rule")?;
        ensure!(!body.contains('{'), "nested CSS is unsupported");
        let mut scoped = Vec::new();
        for part in head.split(',') {
            let cap = selector
                .captures(part.trim())
                .context("only simple class/id CSS selectors are supported; original retained")?;
            let map = if &cap[1] == "." { classes } else { ids };
            // Unused rules have no visual effect and need not enter the composed SVG.
            if let Some(name) = map.get(&cap[2]) {
                scoped.push(format!("{}{name}", &cap[1]));
            }
        }
        if !scoped.is_empty() {
            out.push_str(&format!(
                "{}{{{}}}",
                scoped.join(","),
                references(body, ids)?
            ));
        }
    }
    Ok(out)
}

fn rewrite(
    el: &mut Element,
    ids: &HashMap<String, String>,
    classes: &HashMap<String, String>,
) -> Result<()> {
    ensure!(
        !matches!(
            el.name.as_str(),
            "script"
                | "foreignObject"
                | "animate"
                | "animateMotion"
                | "animateTransform"
                | "set"
                | "discard"
        ),
        "unsupported active SVG element: {}",
        el.name
    );
    for (key, value) in &mut el.attributes {
        ensure!(
            !key.to_ascii_lowercase().starts_with("on"),
            "SVG event handlers are unsupported"
        );
        if key == "id" {
            *value = ids[value].clone();
        } else if key == "class" {
            *value = value
                .split_whitespace()
                .map(|s| classes[s].as_str())
                .collect::<Vec<_>>()
                .join(" ");
        } else if key == "href" || key.ends_with(":href") {
            if let Some(target) = value.strip_prefix('#') {
                *value = format!(
                    "#{}",
                    ids.get(&fragment(target)?).context("unresolved href")?
                );
            } else {
                ensure!(
                    (el.name == "image"
                        && ["data:image/png;base64,", "data:image/jpeg;base64,"]
                            .iter()
                            .any(|p| value.starts_with(p)))
                        || (el.name == "color-profile"
                            && value.starts_with("data:application/vnd.iccprofile;base64,"))
                        || (el.name == "a"
                            && (value.is_empty()
                                || reqwest::Url::parse(value)
                                    .is_ok_and(|url| matches!(url.scheme(), "http" | "https")))),
                    "external SVG href is unsupported"
                );
            }
        } else if key == "aria-labelledby" || key == "aria-describedby" {
            *value = value
                .split_whitespace()
                .map(|s| {
                    ids.get(s)
                        .cloned()
                        .context("unresolved accessibility reference")
                })
                .collect::<Result<Vec<_>>>()?
                .join(" ");
        } else {
            *value = references(value, ids)?;
        }
    }
    if el.name == "style" {
        let css = el.get_text().unwrap_or_default().into_owned();
        el.children = vec![XMLNode::Text(stylesheet(&css, ids, classes)?)];
    } else {
        for child in &mut el.children {
            if let XMLNode::Element(child) = child {
                rewrite(child, ids, classes)?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn namespaces_gradients_classes_and_references() {
        let raw = br##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 20 30"><style>.st0{fill:url(#g)}</style><defs><linearGradient id="g"/></defs><path class="st0" d="M0 0h10"/><use href="#g"/></svg>"##;
        let a = prepare(raw, "test:a").unwrap();
        let b = prepare(raw, "test:b").unwrap();
        assert!(a.contains("0 0 20 30"));
        assert!(!a.contains("url(#g)"));
        assert!(!a.contains(".st0"));
        assert_ne!(a, b);
        Element::parse(a.as_bytes()).unwrap();
    }
    #[test]
    fn retains_licensing_metadata_but_removes_editor_view_settings() {
        let raw=br#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:s="http://sodipodi.sourceforge.net/DTD/sodipodi-0.dtd" viewBox="0 0 10 10"><metadata><license>Copyright Artist, CC-BY-4.0</license></metadata><s:namedview onionskin="true"/><path d="M0 0"/></svg>"#;
        let prepared = prepare(raw, "test:metadata").unwrap();
        assert!(prepared.contains("Copyright Artist, CC-BY-4.0"));
        assert!(!prepared.contains("namedview"));
    }
    #[test]
    fn rejects_active_and_external_content() {
        for body in [
            "<script/>",
            "<image href='https://example.com/a.png'/>",
            "<path onload='x'/>",
            "<path onclick='x'/>",
            "<style>path{fill:red}</style>",
        ] {
            assert!(
                prepare(
                    format!(
                        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 1 1'>{body}</svg>"
                    )
                    .as_bytes(),
                    "test:x"
                )
                .is_err()
            );
        }
    }
    #[test]
    fn passes_inkscape_path_effects_without_editor_attributes() {
        let raw = br#"<svg xmlns='http://www.w3.org/2000/svg' xmlns:inkscape='http://www.inkscape.org/namespaces/inkscape' viewBox='0 0 10 10'><inkscape:path-effect effect='fillet_chamfer' id='pe1' only_selected='false' lpeversion='1'/><path inkscape:path-effect='#pe1' d='M0 0h10v10z'/></svg>"#;
        let prepared = prepare(raw, "test:inkscape").unwrap();
        assert!(!prepared.contains("only_selected"));
        assert!(!prepared.contains("id=\"pe1\""));
        Element::parse(prepared.as_bytes()).unwrap();
    }
    #[test]
    fn renames_later_duplicate_ids_and_keeps_references_resolvable() {
        let raw = br#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><defs><g id='x'><rect/></g></defs><use href='#x'/><rect id='x'/></svg>"#;
        // Previously rejected as "ambiguous duplicate SVG id"; now the first
        // occurrence keeps a resolvable target and the later one is renamed.
        // collect()/rewrite() then re-map both to distinct opaque ids.
        let prepared = prepare(raw, "test:dup").unwrap();
        let root = Element::parse(prepared.as_bytes()).unwrap();
        let mut target = None;
        let mut ids = Vec::new();
        let mut stack = vec![root];
        while let Some(el) = stack.pop() {
            if el.name == "use" {
                target = el
                    .attributes
                    .get("href")
                    .map(|h| h.trim_start_matches('#').to_string());
            }
            if let Some(id) = el.attributes.get("id") {
                ids.push(id.clone());
            }
            stack.extend(el.children.iter().filter_map(|n| n.as_element().cloned()));
        }
        // The two formerly-duplicate elements ended up with distinct ids.
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert!(unique.len() >= 2, "duplicate ids were not disambiguated: {ids:?}");
        // The reference resolves to one of them (the first occurrence, kept).
        let target = target.expect("use keeps its fragment href");
        assert!(ids.contains(&target), "dangling reference {target}; ids {ids:?}");
    }
    #[test]
    fn rejects_duplicated_id_targeted_by_a_style_selector() {
        let raw = br#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><style>#x{fill:red}</style><rect id='x'/><rect id='x'/></svg>"#;
        assert!(prepare(raw, "test:dupstyle").is_err());
    }
    #[test]
    fn strips_external_font_face_rules() {
        let raw = br#"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><style>@font-face{font-family:V;src:url(https://excalidraw.com/Virgil.woff2) format('woff2')}.a{fill:red}</style><path class='a' d='M0 0'/></svg>"#;
        let prepared = prepare(raw, "test:font").unwrap();
        assert!(!prepared.contains("font-face"));
        assert!(!prepared.contains("excalidraw"));
        assert!(prepared.contains("fill:red"));
        Element::parse(prepared.as_bytes()).unwrap();
    }
    #[test]
    fn still_rejects_local_font_faces_and_other_at_rules() {
        for body in [
            "<style>@font-face{font-family:V;src:url(data:font/woff2;base64,abc) format('woff2')}.a{fill:red}</style>",
            "<style>@font-face{font-family:V;src:local('Virgil')}.a{fill:red}</style>",
            "<style>@media screen{.a{fill:red}}</style>",
            "<style>@keyframes spin{from{opacity:0}}.a{fill:red}</style>",
        ] {
            assert!(
                prepare(
                    format!(
                        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 1 1'>{body}</svg>"
                    )
                    .as_bytes(),
                    "test:x"
                )
                .is_err()
            );
        }
    }
}
