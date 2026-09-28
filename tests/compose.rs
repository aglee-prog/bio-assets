use bio_assets::{
    index::Library,
    models::{Asset, ComposeArgs, ComposeElement},
};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

const FIXTURES: &str = "tests/fixtures";

#[derive(serde::Deserialize)]
struct ManifestEntry {
    file: String,
    construct: String,
}

/// A tempdir-backed `Library` with every fixture stored as an asset. The tempdir
/// is held so the on-disk state (including `data/composed/`) outlives the test.
struct Harness {
    #[allow(dead_code)]
    dir: tempfile::TempDir,
    lib: Library,
    ids: Vec<String>,
}

fn asset_for(name: &str) -> Asset {
    Asset {
        id: format!("test:compose-{name}"),
        source: "test".into(),
        upstream_key: name.into(),
        name: name.into(),
        category: "fixture".into(),
        tags: vec!["compose".into()],
        description: format!("Composed test fixture {name}"),
        license: "CC0 1.0".into(),
        license_url: Some("https://creativecommons.org/publicdomain/zero/1.0/".into()),
        author: Some("Fixture Author".into()),
        attribution: "Fixture Author, CC0 1.0".into(),
        source_url: format!("https://example.org/{name}"),
        source_revision: None,
    }
}

fn new_harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path());
    lib.initialize().unwrap();
    let manifest: Vec<ManifestEntry> =
        serde_json::from_str(&fs::read_to_string(PathBuf::from(FIXTURES).join("manifest.json")).unwrap()).unwrap();
    let mut ids = Vec::new();
    for entry in &manifest {
        let name = entry.file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(&entry.file);
        let bytes = fs::read(PathBuf::from(FIXTURES).join(&entry.file)).unwrap();
        let asset = asset_for(name);
        lib.store(&asset, &bytes, &json!({})).unwrap();
        ids.push(asset.id.clone());
    }
    Harness { dir, lib, ids }
}

fn elem(asset_id: &str, x: f64, y: f64, scale: f64, rotation: f64) -> ComposeElement {
    ComposeElement {
        asset_id: asset_id.into(),
        x,
        y,
        scale,
        rotation,
    }
}

fn single(asset_id: &str) -> ComposeArgs {
    ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![elem(asset_id, 0.0, 0.0, 1.0, 0.0)],
    }
}

fn composed_text(lib: &Library, args: &ComposeArgs) -> String {
    let result = lib.compose(args).expect("compose must succeed");
    lib.get_composed(&result.result_id).expect("stored artifact readable")
}

fn symbol_ids(svg: &str) -> Vec<String> {
    svg.match_indices("<symbol id=\"")
        .map(|(i, _)| {
            let rest = &svg[i + "<symbol id=\"".len()..];
            rest[..rest.find('"').expect("quoted symbol id")].to_string()
        })
        .collect()
}

/// The `<use href="#…">` elements that target one of the emitted `<symbol>` ids,
/// i.e. the *composed* uses. Asset-internal `<use href="#ba_…_iN">` reference the
/// asset's own ids (never a `ba_comp_…` symbol), so this isolates only the
/// composition's own `<use>` elements.
fn composed_use_fragments(svg: &str, syms: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for (i, _) in svg.match_indices("<use href=\"#") {
        let rest = &svg[i + "<use href=\"#".len()..];
        let target = &rest[..rest.find('"').expect("quoted href")];
        if syms.iter().any(|s| s == target) {
            let close = svg[i..].find("/>").expect("composed <use> is self-closing");
            out.push(svg[i..i + close + 2].to_string());
        }
    }
    out
}

#[test]
fn determinism_same_id_and_bytes() {
    let h = new_harness();
    let args = single(&h.ids[0]);
    let r1 = h.lib.compose(&args).unwrap();
    let r2 = h.lib.compose(&args).unwrap();
    assert_eq!(r1.result_id, r2.result_id, "identical args must give the same result_id");
    let b1 = h.lib.get_composed(&r1.result_id).unwrap();
    let b2 = h.lib.get_composed(&r2.result_id).unwrap();
    assert_eq!(b1, b2, "get_composed must return identical bytes");
}

#[test]
fn symbol_use_counts_and_unique_symbol_ids() {
    let h = new_harness();
    let args = ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![
            elem(&h.ids[0], 0.0, 0.0, 1.0, 0.0),
            elem(&h.ids[1], 10.0, 10.0, 1.0, 0.0),
            elem(&h.ids[2], 20.0, 20.0, 2.0, 90.0),
        ],
    };
    let svg = composed_text(&h.lib, &args);
    let n = args.elements.len();
    let syms = symbol_ids(&svg);
    assert_eq!(syms.len(), n, "one <symbol> per element: {svg}");
    // symbol ids must be unique
    {
        let mut sorted = syms.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(syms.len(), sorted.len(), "symbol ids must be unique: {syms:?}");
    }
    // the composed <use> elements are exactly those targeting an emitted symbol id
    let composed_uses = composed_use_fragments(&svg, &syms);
    assert_eq!(composed_uses.len(), n, "one composed <use> per element: {svg}");
    for use_el in &composed_uses {
        assert!(
            use_el.contains("width=\"") && use_el.contains("height=\""),
            "composed <use> must set width and height: {use_el}"
        );
    }
}

#[test]
fn reuse_same_asset_twice_distinct_ids() {
    let h = new_harness();
    let args = ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![
            elem(&h.ids[0], 0.0, 0.0, 1.0, 0.0),
            elem(&h.ids[0], 1.0, 1.0, 1.0, 45.0),
        ],
    };
    let svg = composed_text(&h.lib, &args);
    let syms = symbol_ids(&svg);
    assert_eq!(syms.len(), 2, "two placements => two symbols: {svg}");
    assert_ne!(syms[0], syms[1], "the two placements must use distinct symbol ids: {syms:?}");
    // both placements are referenced by composed <use> elements resolving to the two symbols
    let composed_uses = composed_use_fragments(&svg, &syms);
    assert_eq!(composed_uses.len(), 2, "two composed <use> elements: {svg}");
}

#[test]
fn geometry_exact_transform() {
    let h = new_harness();
    // scale=2, rotation=0 => whole numbers render without a trailing .0
    let args = ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![elem(&h.ids[0], 3.0, 7.0, 2.0, 0.0)],
    };
    let svg = composed_text(&h.lib, &args);
    assert!(
        svg.contains("translate(3 7) scale(2) rotate(0)"),
        "expected exact transform translate(3 7) scale(2) rotate(0); got: {svg}"
    );
    // rotation=90
    let args = ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![elem(&h.ids[0], 1.0, 1.0, 1.5, 90.0)],
    };
    let svg = composed_text(&h.lib, &args);
    assert!(
        svg.contains("translate(1 1) scale(1.5) rotate(90)"),
        "expected exact transform translate(1 1) scale(1.5) rotate(90); got: {svg}"
    );
}

#[test]
fn negative_viewbox_preserved_and_raw_size() {
    let h = new_harness();
    let id = h
        .ids
        .iter()
        .find(|i| i.contains("erlenmeyer"))
        .expect("erlenmeyer fixture must be stored");
    let args = single(id);
    let svg = composed_text(&h.lib, &args);
    // the symbol's viewBox is the asset's original negative/non-zero origin, not `0 0 w h`
    assert!(
        svg.contains("viewBox=\"-120.5 -155.297 880.8 532.8\""),
        "negative/non-zero origin must be preserved in the <symbol> viewBox; got: {svg}"
    );
    // the composed <use> width/height are the raw (un-scaled) viewBox size
    assert!(
        svg.contains("width=\"880.8\" height=\"532.8\""),
        "composed <use> width/height must equal the raw viewBox ow/oh; got: {svg}"
    );
}

#[test]
fn hard_case_fixtures_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path());
    lib.initialize().unwrap();
    let manifest: Vec<ManifestEntry> =
        serde_json::from_str(&fs::read_to_string(PathBuf::from(FIXTURES).join("manifest.json")).unwrap()).unwrap();
    for entry in &manifest {
        let name = entry.file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(&entry.file);
        let bytes = fs::read(PathBuf::from(FIXTURES).join(&entry.file)).unwrap();
        let asset = asset_for(name);
        lib.store(&asset, &bytes, &json!({})).unwrap();

        let args = single(&asset.id);
        let svg = composed_text(&lib, &args);

        // (a) output parses as XML
        let root = xmltree::Element::parse(svg.as_bytes()).unwrap_or_else(|e| panic!("{:?} must parse as XML: {e}", entry.file));
        assert_eq!(root.name, "svg");

        // (b) the named construct survives
        let marker = construct_marker(&entry.construct).unwrap_or("<path");
        if construct_marker(&entry.construct).is_none() {
            // negative/non-zero viewBox origin (erlenmeyer)
            assert!(
                svg.contains("viewBox=\"-120.5 -155.297 880.8 532.8\""),
                "negative origin must be preserved for {}; got: {svg}", entry.file
            );
        } else {
            assert!(svg.contains(marker), "construct marker {marker:?} must be present for {}; got: {svg}", entry.file);
        }

        // (c) the asset markup (its own <svg xmlns root) is embedded inside the composition
        let svg_roots = svg.matches("<svg xmlns").count();
        assert!(svg_roots >= 2, "asset markup must be embedded inside a <symbol> (nested <svg xmlns) for {}; got {svg_roots} svg roots", entry.file);
    }
}

/// Pick a literal substring expected to survive composition, based on the
/// fixture's declared construct. `None` means the construct is the negative /
/// non-zero viewBox origin (checked separately, not a markup marker).
fn construct_marker(construct: &str) -> Option<&'static str> {
    if construct.contains("viewBox")
        || construct.contains("negative")
        || construct.contains("origin")
    {
        return None;
    }
    if construct.contains("clipPath") {
        return Some("<clipPath");
    }
    if construct.contains("filter") {
        return Some("<filter");
    }
    if construct.contains("pattern") {
        return Some("<pattern");
    }
    if construct.contains("mask") {
        return Some("<mask");
    }
    if construct.contains("style") {
        return Some("<style");
    }
    if construct.contains("userSpaceOnUse") {
        return Some("userSpaceOnUse");
    }
    if construct.contains("use") {
        return Some("<use");
    }
    Some("<path")
}

#[test]
fn all_or_nothing_missing_asset_no_file() {
    let h = new_harness();
    let composed_dir = h.dir.path().join("data/composed");
    let before: usize = fs::read_dir(&composed_dir)
        .map(|d| d.count())
        .unwrap_or(0);
    let args = ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![elem("test:compose-never-stored", 0.0, 0.0, 1.0, 0.0)],
    };
    let err = h.lib.compose(&args).unwrap_err();
    assert!(err.to_string().contains("test:compose-never-stored"), "error must name the missing asset: {err}");
    let after: usize = fs::read_dir(&composed_dir)
        .map(|d| d.count())
        .unwrap_or(0);
    assert_eq!(before, after, "a failed composition must not write any file into data/composed");
}

#[test]
fn bounds_rejections() {
    let h = new_harness();
    // 51 elements rejected
    let many = ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: (0..51).map(|i| elem(&h.ids[0], i as f64, 0.0, 1.0, 0.0)).collect(),
    };
    assert!(h.lib.compose(&many).is_err(), ">50 elements must be rejected");
    // scale 0 rejected
    let zero = ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![elem(&h.ids[0], 0.0, 0.0, 0.0, 0.0)],
    };
    assert!(h.lib.compose(&zero).is_err(), "scale:0 must be rejected");
    // negative scale rejected
    let negative = ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![elem(&h.ids[0], 0.0, 0.0, -1.0, 0.0)],
    };
    assert!(h.lib.compose(&negative).is_err(), "negative scale must be rejected");
    // |rotation| > 360 rejected
    let spin = ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![elem(&h.ids[0], 0.0, 0.0, 1.0, 720.0)],
    };
    assert!(h.lib.compose(&spin).is_err(), "|rotation|>360 must be rejected");
    // zero elements rejected
    let empty = ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![],
    };
    assert!(h.lib.compose(&empty).is_err(), "zero elements must be rejected");
}

#[test]
fn get_composed_exact_bytes_and_unknown_error() {
    let h = new_harness();
    let args = single(&h.ids[0]);
    let result = h.lib.compose(&args).unwrap();
    let on_disk = fs::read(h.dir.path().join("data/composed").join(format!("{}.svg", result.result_id))).unwrap();
    let via_api = h.lib.get_composed(&result.result_id).unwrap().into_bytes();
    assert_eq!(on_disk, via_api, "get_composed must return the exact stored bytes");
    // unknown but well-formed result id => error
    let unknown = h.lib.get_composed("ba_comp_0000000000000000");
    assert!(unknown.is_err(), "unknown result_id must be an error");
    // malformed result id => error
    assert!(h.lib.get_composed("not-a-result-id").is_err(), "malformed result_id must be an error");
}

#[test]
fn attribution_deduped_first_seen_order() {
    let dir = tempfile::tempdir().unwrap();
    let lib = Library::new(dir.path());
    lib.initialize().unwrap();
    let svg = b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><path d='M0 0L10 10'/></svg>";
    let store = |id: &str, attribution: &str| {
        let asset = Asset {
            id: id.into(),
            source: "test".into(),
            upstream_key: id.into(),
            name: id.into(),
            category: "fixture".into(),
            tags: vec![],
            description: String::new(),
            license: "CC0 1.0".into(),
            license_url: None,
            author: Some("Fixture Author".into()),
            attribution: attribution.into(),
            source_url: "https://example.org/x".into(),
            source_revision: None,
        };
        lib.store(&asset, svg, &json!({})).unwrap();
    };
    store("test:compose-a", "Shared Author, CC0 1.0");
    store("test:compose-b", "Unique Author, CC-BY-4.0");
    store("test:compose-c", "Shared Author, CC0 1.0");
    let args = ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![
            elem("test:compose-a", 0.0, 0.0, 1.0, 0.0),
            elem("test:compose-b", 1.0, 1.0, 1.0, 0.0),
            elem("test:compose-c", 2.0, 2.0, 1.0, 0.0),
        ],
    };
    let result = lib.compose(&args).unwrap();
    assert_eq!(
        result.attribution,
        vec!["Shared Author, CC0 1.0".to_string(), "Unique Author, CC-BY-4.0".to_string()],
        "attribution must be the deduped first-seen union"
    );
}
