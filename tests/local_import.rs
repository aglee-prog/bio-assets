use bio_assets::{importers, index::Library};
use serde_json::json;
use std::fs;

#[test]
fn local_manifest_preserves_original_and_failed_update_keeps_previous_asset() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("library");
    let export = dir.path().join("export");
    fs::create_dir(&export).unwrap();
    let library = Library::new(&root);
    library.initialize().unwrap();
    let original=b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 4 4'><path id='p' d='M0 0h4'/></svg>";
    fs::write(export.join("asset.svg"), original).unwrap();
    let manifest = export.join("manifest.json");
    fs::write(
        &manifest,
        serde_json::to_vec(&json!([{
            "id":"niaid:test/1","source":"niaid","upstream_key":"test/1","name":"Example",
            "local_path":"asset.svg","license":"CC-BY-4.0","author":"Artist",
            "attribution":"Artist, CC-BY-4.0","source_url":"https://example.org/asset"
        }]))
        .unwrap(),
    )
    .unwrap();
    let report = importers::import_manifest(&library, "niaid", &manifest, None).unwrap();
    assert_eq!(report.imported, 1);
    let before = library.get("niaid:test/1").unwrap().unwrap();
    assert_ne!(before.svg.as_bytes(), original);
    assert!(
        walkdir::WalkDir::new(root.join("assets"))
            .into_iter()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().is_file())
            .any(|e| fs::read(e.path()).unwrap() == original)
    );
    fs::write(
        export.join("asset.svg"),
        "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 4 4'><script/></svg>",
    )
    .unwrap();
    let report = importers::import_manifest(&library, "niaid", &manifest, None).unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(
        library.get("niaid:test/1").unwrap().unwrap().svg,
        before.svg
    );
    assert_eq!(library.count().unwrap(), 1);
}

#[cfg(unix)]
#[test]
fn manifest_cannot_read_outside_export_through_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let export = dir.path().join("export");
    fs::create_dir(&export).unwrap();
    fs::write(dir.path().join("outside.svg"), "private file").unwrap();
    std::os::unix::fs::symlink(dir.path().join("outside.svg"), export.join("asset.svg")).unwrap();
    assert!(bio_assets::index::contained_path(&export, std::path::Path::new("asset.svg")).is_err());
}
