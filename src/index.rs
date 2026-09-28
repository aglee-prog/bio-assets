use crate::{
    compose,
    models::{Asset, AssetContent, AssetSummary, ComposeArgs, ComposeResult, PlacedAsset},
    svg,
};
use anyhow::{bail, Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Clone)]
pub struct Library {
    pub root: PathBuf,
}

impl Library {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
    pub fn initialize(&self) -> Result<()> {
        for dir in ["assets", "data/manifests", "data/normalized", "results", "cache"] {
            fs::create_dir_all(self.root.join(dir))?;
        }
        let db = Connection::open(self.root.join("data/assets.sqlite"))?;
        db.busy_timeout(Duration::from_secs(30))?;
        db.execute_batch(include_str!("schema.sql"))?;
        Ok(())
    }
    fn connect(&self, write: bool) -> Result<Connection> {
        let flags = if write {
            OpenFlags::SQLITE_OPEN_READ_WRITE
        } else {
            OpenFlags::SQLITE_OPEN_READ_ONLY
        };
        let db = Connection::open_with_flags(self.root.join("data/assets.sqlite"), flags)
            .context("cannot open index; run an import first")?;
        db.busy_timeout(Duration::from_secs(30))?;
        Ok(db)
    }
    pub fn count(&self) -> Result<i64> {
        Ok(self
            .connect(false)?
            .query_row("SELECT count(*) FROM assets", [], |r| r.get(0))?)
    }

    pub fn existing_id(&self, source: &str, upstream_key: &str) -> Result<Option<String>> {
        Ok(self
            .connect(false)?
            .query_row(
                "SELECT id FROM assets WHERE source=? AND upstream_key=?",
                params![source, upstream_key],
                |r| r.get(0),
            )
            .optional()?)
    }

    /// Save originals and provenance even if normalization rejects the asset.
    /// Content-addressed files make publication of the SQLite row atomic for readers.
    pub fn store(&self, asset: &Asset, bytes: &[u8], raw: &Value) -> Result<()> {
        ensure!(
            asset.id.starts_with(&format!("{}:", asset.source)),
            "asset ID must be namespaced"
        );
        ensure!(
            !asset.source.is_empty()
                && asset
                    .source
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-'),
            "invalid source name"
        );
        ensure!(
            !asset.license.trim().is_empty() && !asset.attribution.trim().is_empty(),
            "license and attribution must be preserved"
        );
        let hash = svg::digest(bytes);
        let key = svg::digest(asset.id.as_bytes());
        let original = format!("assets/{}/{key}/{hash}.svg", asset.source);
        atomic_write(&self.root.join(&original), bytes)?;
        let manifest = json!({"asset":asset,"original_path":original,"sha256":hash,"upstream":raw});
        atomic_write(
            &self
                .root
                .join(format!("data/manifests/{}/{key}/{hash}.json", asset.source)),
            &serde_json::to_vec_pretty(&manifest)?,
        )?;
        let normalized = svg::prepare(bytes, &asset.id)?;
        let normalized_hash = svg::digest(normalized.as_bytes());
        let reusable = format!("data/normalized/{key}/{normalized_hash}.svg");
        atomic_write(&self.root.join(&reusable), normalized.as_bytes())?;
        let aliases = aliases(asset);
        let mut db = self.connect(true)?;
        let tx = db.transaction()?;
        tx.execute("INSERT INTO assets(id,source,upstream_key,name,category,tags,description,aliases,local_path,reusable_path,license,license_url,author,attribution,source_url,sha256,source_revision)
          VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17)
          ON CONFLICT(id) DO UPDATE SET upstream_key=excluded.upstream_key,name=excluded.name,category=excluded.category,tags=excluded.tags,description=excluded.description,aliases=excluded.aliases,local_path=excluded.local_path,reusable_path=excluded.reusable_path,license=excluded.license,license_url=excluded.license_url,author=excluded.author,attribution=excluded.attribution,source_url=excluded.source_url,sha256=excluded.sha256,source_revision=excluded.source_revision,imported_at=CURRENT_TIMESTAMP",
          params![asset.id,asset.source,asset.upstream_key,asset.name,asset.category,serde_json::to_string(&asset.tags)?,asset.description,aliases,original,reusable,asset.license,asset.license_url,asset.author,asset.attribution,asset.source_url,hash,asset.source_revision])?;
        tx.commit()?;
        Ok(())
    }

    pub fn search(
        &self,
        query: &str,
        source: Option<&str>,
        category: Option<&str>,
        limit: u32,
    ) -> Result<Vec<AssetSummary>> {
        ensure!((1..=100).contains(&limit), "limit must be 1..100");
        ensure!(query.len() <= 1024, "query exceeds 1024 bytes");
        let tokens: Vec<_> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .take(32)
            .map(|s| format!("\"{s}\""))
            .collect();
        ensure!(!tokens.is_empty(), "query must contain words");
        let db = self.connect(false)?;
        let mut stmt = db.prepare("SELECT a.id,a.name,a.source,a.category,a.tags FROM assets_fts JOIN assets a ON a.rowid=assets_fts.rowid WHERE assets_fts MATCH ?1 AND (?2 IS NULL OR a.source=?2) AND (?3 IS NULL OR a.category=?3) ORDER BY bm25(assets_fts,8,3,4,1,2),a.id LIMIT ?4")?;
        let rows = stmt.query_map(
            params![tokens.join(" AND "), source, category, limit],
            |r| {
                let tags: String = r.get(4)?;
                Ok(AssetSummary {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    source: r.get(2)?,
                    category: r.get(3)?,
                    tags: serde_json::from_str(&tags).unwrap_or_default(),
                })
            },
        )?;
        Ok(rows.collect::<std::result::Result<_, _>>()?)
    }

    pub fn get(&self, id: &str) -> Result<Option<AssetContent>> {
        let db = self.connect(false)?;
        let result = db
            .query_row(
                "SELECT id,name,source,reusable_path,license,license_url,author,attribution,source_url FROM assets WHERE id=?",
                [id],
                |r| {
                    Ok((
                        AssetContent {
                            id: r.get(0)?,
                            name: r.get(1)?,
                            source: r.get(2)?,
                            svg: String::new(),
                            view_box: [0.0; 4],
                            width: 0.0,
                            height: 0.0,
                            license: r.get(4)?,
                            license_url: r.get(5)?,
                            author: r.get(6)?,
                            attribution: r.get(7)?,
                            source_url: r.get(8)?,
                        },
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        result
            .map(|(mut asset, path)| {
                asset.svg = fs::read_to_string(contained_path(&self.root, Path::new(&path))?)?;
                let view_box = view_box_of(&asset.svg)?;
                asset.view_box = view_box;
                asset.width = view_box[2];
                asset.height = view_box[3];
                Ok(asset)
            })
            .transpose()
    }

    /// Deterministically compose placed assets: resolve them, build the SVG, and
    /// write it to `results/{result_id}.svg`. Fails before writing if any
    /// asset is missing or invalid (all-or-nothing).
    pub fn compose(&self, args: &ComposeArgs) -> Result<ComposeResult> {
        let resolve = |asset_id: &str| -> Result<PlacedAsset> {
            let Some(asset) = self.get(asset_id)? else {
                bail!("asset not found: {asset_id}");
            };
            Ok(PlacedAsset {
                asset_id: asset.id,
                svg: asset.svg,
                view_box: asset.view_box,
                attribution: asset.attribution,
            })
        };
        let (result_id, svg) = compose::compose(args, &resolve)?;
        atomic_write(
            &self
                .root
                .join("results")
                .join(format!("{result_id}.svg")),
            &svg,
        )?;
        let binding = std::env::var("BIO_ASSETS_PUBLIC_URL").ok();
        let public_url = binding
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        let url = artifact_url(&result_id, public_url);
        Ok(ComposeResult {
            status: "ok".into(),
            result_id,
            url,
            mime_type: "image/svg+xml".into(),
            width: args.width,
            height: args.height,
        })
    }

    /// Return the stored composed SVG bytes for a `result_id` produced by
    /// `compose`, read only from `<root>/results/`. Enforces the strict id
    /// format; a missing artifact is an error.
    pub fn read_result(&self, result_id: &str) -> Result<Vec<u8>> {
        ensure!(is_valid_result_id(result_id), "invalid result id");
        let path = self.root.join("results").join(format!("{result_id}.svg"));
        ensure!(path.is_file(), "result not found");
        let root = self.root.canonicalize()?;
        let canonical = path.canonicalize()?;
        ensure!(
            canonical.starts_with(&root),
            "result path escapes library root"
        );
        Ok(fs::read(&canonical)?)
    }
}

/// Parse the root `viewBox` of a normalized asset. `svg::prepare` guarantees a
/// valid `viewBox` is present, so this is a minimal, on-demand read (no column).
fn view_box_of(svg: &str) -> Result<[f64; 4]> {
    let root = xmltree::Element::parse(svg.as_bytes()).context("normalized SVG is not valid XML")?;
    let view_box = root
        .attributes
        .get("viewBox")
        .context("normalized SVG is missing viewBox")?;
    let parts = view_box
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::parse::<f64>)
        .collect::<std::result::Result<Vec<_>, _>>()
        .context("invalid viewBox")?;
    ensure!(
        parts.len() == 4
            && parts.iter().all(|n| n.is_finite())
            && parts[2] > 0.0
            && parts[3] > 0.0,
        "invalid viewBox"
    );
    Ok([parts[0], parts[1], parts[2], parts[3]])
}

/// Strict result-id format: `ba_comp_` + exactly 16 lowercase hex chars.
pub fn is_valid_result_id(result_id: &str) -> bool {
    const PREFIX: &str = "ba_comp_";
    let Some(hex) = result_id.strip_prefix(PREFIX) else {
        return false;
    };
    hex.len() == 16 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Absolute artifact URL for a composed result. When a non-blank `public_url`
/// base is supplied it is prefixed (trailing `/` stripped); otherwise the
/// relative server path is returned.
pub fn artifact_url(result_id: &str, public_url: Option<&str>) -> String {
    match public_url.map(str::trim) {
        Some(base) if !base.is_empty() => {
            format!("{}/results/{result_id}.svg", base.trim_end_matches('/'))
        }
        _ => format!("/results/{result_id}.svg"),
    }
}

pub fn contained_path(root: &Path, path: &Path) -> Result<PathBuf> {
    ensure!(!path.is_absolute(), "path must be relative");
    let root = root.canonicalize()?;
    let path = root.join(path).canonicalize()?;
    ensure!(path.starts_with(root), "path escapes asset directory");
    Ok(path)
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::create_dir_all(path.parent().context("missing parent directory")?)?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temp, bytes)?;
    fs::rename(temp, path)?;
    Ok(())
}

fn aliases(asset: &Asset) -> String {
    let groups: Vec<Vec<String>> =
        serde_json::from_str(include_str!("synonyms.json")).expect("valid built-in synonyms");
    let haystack = format!(
        " {} {} {} ",
        asset.name,
        asset.tags.join(" "),
        asset.description
    )
    .to_lowercase();
    groups
        .into_iter()
        .filter(|group| {
            // The broad phrase is a search alias, not a reverse classification of every receptor.
            group
                .iter()
                .take(if group[0] == "rtk" { 2 } else { group.len() })
                .any(|term| haystack.contains(&format!(" {term} ")))
        })
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_index_updates_and_license_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let lib = Library::new(dir.path());
        lib.initialize().unwrap();
        let mut a = Asset {
            id: "test:1".into(),
            source: "test".into(),
            upstream_key: "1".into(),
            name: "RTK".into(),
            category: "protein".into(),
            tags: vec![],
            description: String::new(),
            license: "CC-BY-4.0".into(),
            license_url: Some("https://creativecommons.org/licenses/by/4.0/".into()),
            author: Some("A; B".into()),
            attribution: "A; B, CC-BY-4.0".into(),
            source_url: "https://example.org/1".into(),
            source_revision: None,
        };
        let svg =
            b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 1 1'><path d='M0 0'/></svg>";
        lib.store(&a, svg, &json!({})).unwrap();
        assert_eq!(
            lib.search("membrane receptor", Some("test"), None, 20)
                .unwrap()
                .len(),
            1
        );
        assert!(
            lib.search("RTK", Some("other"), None, 20)
                .unwrap()
                .is_empty()
        );
        assert_eq!(lib.get(&a.id).unwrap().unwrap().author, a.author);
        assert!(lib.get("test:missing").unwrap().is_none());
        a.name = "neuron".into();
        lib.store(&a, svg, &json!({})).unwrap();
        assert!(lib.search("RTK", None, None, 20).unwrap().is_empty());
        assert_eq!(lib.search("nerve cell", None, None, 20).unwrap().len(), 1);
        assert_eq!(lib.count().unwrap(), 1);
        assert!(lib.search("\" OR *", None, None, 20).is_ok());
    }
}

#[cfg(test)]
mod artifact_url_tests {
    use super::artifact_url;

    const ID: &str = "ba_comp_0123456789abcdef";

    #[test]
    fn none_is_relative() {
        assert_eq!(artifact_url(ID, None), format!("/results/{ID}.svg"));
    }

    #[test]
    fn empty_string_is_relative() {
        assert_eq!(artifact_url(ID, Some("")), format!("/results/{ID}.svg"));
    }

    #[test]
    fn whitespace_only_is_relative() {
        assert_eq!(
            artifact_url(ID, Some("   \t ")),
            format!("/results/{ID}.svg")
        );
    }

    #[test]
    fn one_trailing_slash() {
        assert_eq!(
            artifact_url(ID, Some("https://example.com/")),
            format!("https://example.com/results/{ID}.svg")
        );
    }

    #[test]
    fn multiple_trailing_slashes() {
        assert_eq!(
            artifact_url(ID, Some("https://example.com///")),
            format!("https://example.com/results/{ID}.svg")
        );
    }

    #[test]
    fn no_trailing_slash() {
        assert_eq!(
            artifact_url(ID, Some("https://example.com")),
            format!("https://example.com/results/{ID}.svg")
        );
    }
}
