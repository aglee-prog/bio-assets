use bio_assets::{
    index::Library,
    models::{Asset, ComposeArgs, ComposeElement},
};
use serde_json::json;
use std::fs;
use std::path::PathBuf;

const FIXTURES: &str = "tests/fixtures";
const ENV: &str = "BIO_ASSETS_PUBLIC_URL";

#[derive(serde::Deserialize)]
struct ManifestEntry {
    file: String,
}

/// A tempdir-backed `Library` with every fixture stored as an asset. The tempdir
/// is held so the on-disk state (including `results/`) outlives the test.
struct Harness {
    #[allow(dead_code)]
    dir: tempfile::TempDir,
    lib: Library,
    id: String,
}

fn asset_for(name: &str) -> Asset {
    Asset {
        id: format!("test:public-url-{name}"),
        source: "test".into(),
        upstream_key: name.into(),
        name: name.into(),
        category: "fixture".into(),
        tags: vec!["public-url".into()],
        description: format!("Public URL test fixture {name}"),
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
    let entry = &manifest[0];
    let name = entry.file.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(&entry.file);
    let bytes = fs::read(PathBuf::from(FIXTURES).join(&entry.file)).unwrap();
    let asset = asset_for(name);
    lib.store(&asset, &bytes, &json!({})).unwrap();
    let id = asset.id;
    Harness { dir, lib, id }
}

fn single(asset_id: &str) -> ComposeArgs {
    ComposeArgs {
        width: 100.0,
        height: 100.0,
        background: None,
        elements: vec![ComposeElement {
            asset_id: asset_id.into(),
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            rotation: 0.0,
        }],
    }
}

#[test]
fn public_url_env_var_controls_result_url() {
    // All env-var mutation lives in this single test function, mutated
    // sequentially, because `cargo test` runs test threads in parallel within
    // one binary and env vars are process-global. `Library::compose` reads the
    // variable per call, so sequential mutation is safe.
    unsafe { std::env::remove_var(ENV) };

    let h = new_harness();
    let args = single(&h.id);

    // unset => relative artifact path
    let r = h.lib.compose(&args).expect("compose must succeed");
    assert_eq!(
        r.url,
        format!("/results/{}.svg", r.result_id),
        "unset BIO_ASSETS_PUBLIC_URL must give the relative artifact path"
    );

    // absolute base without trailing slash
    unsafe { std::env::set_var(ENV, "https://assets.example.com") };
    let r = h.lib.compose(&args).expect("compose must succeed");
    assert_eq!(
        r.url,
        format!("https://assets.example.com/results/{}.svg", r.result_id),
        "BIO_ASSETS_PUBLIC_URL must prefix the artifact path"
    );

    // absolute base with trailing slash => no doubled slash
    unsafe { std::env::set_var(ENV, "https://assets.example.com/") };
    let r = h.lib.compose(&args).expect("compose must succeed");
    assert_eq!(
        r.url,
        format!("https://assets.example.com/results/{}.svg", r.result_id),
        "trailing slash on BIO_ASSETS_PUBLIC_URL must not double the slash"
    );

    unsafe { std::env::remove_var(ENV) };
}
