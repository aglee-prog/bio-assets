use super::{Network, Report, same_origin_url, text};
use crate::{index::Library, models::Asset};
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::collections::HashSet;

const BASE: &str = "https://scidraw.io";

pub async fn import(
    library: &Library,
    network: &Network,
    limit: Option<usize>,
    refresh: bool,
) -> Result<Report> {
    let mut next = Some(format!("{BASE}/api/v1/drawings/?image_type=svg"));
    let mut pages = HashSet::new();
    let mut report = Report::default();
    while let Some(url) = next {
        ensure!(
            pages.insert(url.clone()),
            "SciDraw repeated a pagination cursor"
        );
        let page = network.json(&url).await?;
        let results = page["results"]
            .as_array()
            .context("SciDraw catalogue schema changed")?;
        for item in results {
            if report.imported + report.failed >= limit.unwrap_or(usize::MAX) {
                return Ok(report);
            }
            if text(item, "image_type") != "svg" {
                continue;
            }
            let id = format!("scidraw:{}", text(item, "id"));
            let outcome = async {
                let slug = text(item, "slug");
                ensure!(
                    !slug.is_empty() && !slug.contains('/'),
                    "invalid SciDraw slug"
                );
                let detail = network
                    .json(&format!("{BASE}/api/v1/drawings/{slug}/"))
                    .await?;
                let asset = map(&detail)?;
                let download = same_origin_url(BASE, &text(&detail, "image_url"))?;
                let key = crate::svg::digest(asset.id.as_bytes());
                let cached = library.root.join(format!("cache/scidraw/{key}.svg"));
                let bytes = if !refresh && cached.exists() {
                    std::fs::read(&cached)?
                } else {
                    let bytes = network.get(&download).await?;
                    crate::index::atomic_write(&cached, &bytes)?;
                    bytes
                };
                library.store(&asset, &bytes, &detail)
            }
            .await;
            report.record(&id, outcome);
        }
        next = page["next"]
            .as_str()
            .map(|s| same_origin_url(BASE, s))
            .transpose()?;
    }
    Ok(report)
}

pub fn map(detail: &Value) -> Result<Asset> {
    let key = text(detail, "id");
    ensure!(!key.is_empty(), "SciDraw asset has no ID");
    ensure!(text(detail, "image_type") == "svg", "not an SVG");
    let name = text(detail, "name");
    let authors = detail["authors"]
        .as_array()
        .context("missing SciDraw authors")?
        .iter()
        .map(|a| text(a, "full_name"))
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    ensure!(!authors.is_empty(), "missing SciDraw author credits");
    let (license, license_url) = match text(detail, "license").as_str() {
        "cc-by" => ("CC-BY-4.0", "https://creativecommons.org/licenses/by/4.0/"),
        "cc0" => (
            "CC0-1.0",
            "https://creativecommons.org/publicdomain/zero/1.0/",
        ),
        other => anyhow::bail!("unrecognized SciDraw license: {other}"),
    };
    let source_url = format!("{BASE}/drawing/{}", text(detail, "slug"));
    let doi = text(detail, "doi");
    let citation_url = if doi.is_empty() {
        source_url.clone()
    } else {
        format!("https://doi.org/{doi}")
    };
    let created = text(detail, "created_at");
    Ok(Asset {
        id: format!("scidraw:{key}"),
        source: "scidraw".into(),
        upstream_key: key,
        name: name.clone(),
        category: text(&detail["category"], "name").to_lowercase(),
        tags: vec![],
        description: String::new(),
        license: license.into(),
        license_url: Some(license_url.into()),
        author: Some(authors.clone()),
        attribution: format!(
            "{authors}. {name}. SciDraw ({created}). {license} ({license_url}). {citation_url}. SVG internal IDs/classes normalized; geometry unchanged."
        ),
        source_url,
        source_revision: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_coauthors_and_doi() {
        let a=map(&serde_json::json!({"id":"123","image_type":"svg","name":"Mouse","slug":"mouse","authors":[{"full_name":"A"},{"full_name":"B"}],"license":"cc-by","doi":"10.1/example","category":{"name":"Mouse"}})).unwrap();
        assert_eq!(a.author.as_deref(), Some("A; B"));
        assert!(a.attribution.contains("https://doi.org/10.1/example"));
    }
}
