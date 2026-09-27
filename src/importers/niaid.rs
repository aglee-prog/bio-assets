use super::{Network, Report, same_origin_url, strings};
use crate::{
    index::{Library, atomic_write},
    models::Asset,
};
use anyhow::{Context, Result, ensure};
use regex::Regex;
use serde_json::{Value, json};
use std::collections::HashSet;

const BASE: &str = "https://bioart.niaid.nih.gov";

/// Discover the current public website action rather than freezing a build-specific hash.
pub async fn discover_action(network: &Network) -> Result<String> {
    let html = String::from_utf8(network.get(&format!("{BASE}/discover")).await?)?;
    let scripts = Regex::new(r#"<script[^>]*src="([^"]+)""#)?;
    let action = Regex::new(r#"createServerReference\)\("([a-f0-9]+)"[^;]*?"discoverSearch"\)"#)?;
    for cap in scripts.captures_iter(&html) {
        if !cap[1].contains("/app/") || !cap[1].contains("/discover/") {
            continue;
        }
        let js = String::from_utf8(network.get(&same_origin_url(BASE, &cap[1])?).await?)?;
        if let Some(found) = action.captures(&js) {
            return Ok(found[1].to_owned());
        }
    }
    anyhow::bail!(
        "NIAID website interface changed: cannot discover catalogue action; use --manifest for a local export"
    )
}

fn action_result(body: &str) -> Result<Value> {
    for line in body.lines() {
        if let Some((_, json)) = line.split_once(':')
            && let Ok(value) = serde_json::from_str::<Value>(json)
            && value.get("hits").is_some()
        {
            return Ok(value);
        }
    }
    anyhow::bail!("NIAID catalogue response changed or failed")
}

pub async fn import(
    library: &Library,
    network: &Network,
    limit: Option<usize>,
    refresh: bool,
) -> Result<Report> {
    let action = discover_action(network).await?;
    let mut report = Report::default();
    let mut seen = HashSet::new();
    let mut start = 0;
    loop {
        let args = json!([format!(
            "type:bioart?start={start}&size=100&sort=created asc"
        )]);
        let page = action_result(
            &network
                .action(&format!("{BASE}/discover"), &action, &args)
                .await?,
        )?;
        let hits = page["hits"]["hit"]
            .as_array()
            .context("missing NIAID hits")?;
        let total = page["hits"]["found"]
            .as_u64()
            .context("missing NIAID total")?;
        ensure!(
            !hits.is_empty() || start >= total,
            "NIAID returned an incomplete catalogue page"
        );
        for hit in hits {
            let fields = &hit["fields"];
            let entry = first(fields, "id");
            ensure!(
                !entry.is_empty() && entry.chars().all(|c| c.is_ascii_digit()),
                "invalid NIAID entry ID"
            );
            if !seen.insert(entry.clone()) {
                continue;
            }
            for file in svg_files(fields)? {
                if report.imported + report.failed >= limit.unwrap_or(usize::MAX) {
                    return Ok(report);
                }
                let id = format!("niaid:{entry}/{file}");
                let outcome = async {
                    let asset = map(fields, &file)?;
                    let cache = library.root.join(format!("cache/niaid/{entry}/{file}.svg"));
                    let bytes = if !refresh && cache.exists() {
                        std::fs::read(&cache)?
                    } else {
                        let bytes = network
                            .get(&format!("{BASE}/api/bioarts/{entry}/files/{file}"))
                            .await?;
                        atomic_write(&cache, &bytes)?;
                        bytes
                    };
                    library.store(&asset, &bytes, hit)
                }
                .await;
                report.record(&id, outcome);
            }
        }
        start += hits.len() as u64;
        if start >= total {
            break;
        }
    }
    Ok(report)
}

fn first(value: &Value, key: &str) -> String {
    strings(&value[key]).into_iter().next().unwrap_or_default()
}
fn svg_files(fields: &Value) -> Result<Vec<String>> {
    let info = first(fields, "filesinfo");
    let files = info
        .split('|')
        .find_map(|part| part.strip_prefix("SVG:"))
        .unwrap_or_default();
    files
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| {
            ensure!(
                s.chars().all(|c| c.is_ascii_digit()),
                "invalid NIAID SVG file ID"
            );
            Ok(s.to_owned())
        })
        .collect()
}
pub fn map(fields: &Value, file: &str) -> Result<Asset> {
    let entry = first(fields, "id");
    let name = first(fields, "title").trim().to_owned();
    let license = first(fields, "license");
    ensure!(!license.is_empty(), "missing NIAID license");
    let creators = strings(&fields["creator"]).join("; ");
    let collection = first(fields, "collection");
    let keys = strings(&fields["ontologykey"]);
    let types = strings(&fields["ontologytype"]);
    let category = keys
        .iter()
        .zip(&types)
        .find(|(_, kind)| kind.as_str() == "Bioart Category")
        .map(|(key, _)| key.to_lowercase())
        .unwrap_or_default();
    let mut tags = strings(&fields["keywords"]);
    tags.extend(keys);
    tags.sort();
    tags.dedup();
    let source_url = format!("{BASE}/bioart/{entry}");
    let date = first(fields, "created");
    // The catalogue's CC-BY label has no version. Retain it verbatim and link
    // the entry instead of silently assigning CC-BY-4.0.
    let attribution = format!(
        "{creators}. {collection}. ({date}). {name}. NIH BioArt Source. {source_url}. License: {license} (see entry for license details). SVG variant {file}; internal IDs/classes normalized; geometry unchanged."
    );
    Ok(Asset {
        id: format!("niaid:{entry}/{file}"),
        source: "niaid".into(),
        upstream_key: format!("{entry}/{file}"),
        name,
        category,
        tags,
        description: Regex::new("<[^>]*>")?
            .replace_all(&first(fields, "description"), " ")
            .into_owned(),
        license,
        license_url: Some(source_url.clone()),
        author: if creators.is_empty() {
            None
        } else {
            Some(creators)
        },
        attribution,
        source_url,
        source_revision: Some(first(fields, "updated")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn variants_have_distinct_stable_ids_and_unversioned_license_stays_unversioned() {
        let fields = json!({"id":["103"],"title":["Tick"],"license":["CC-BY"],"creator":["Artist"],"filesinfo":["PNG:1|SVG:2,3"],"ontologykey":["Arthropods"],"ontologytype":["Bioart Category"]});
        let files = svg_files(&fields).unwrap();
        assert_eq!(files, vec!["2", "3"]);
        let a = map(&fields, &files[0]).unwrap();
        let b = map(&fields, &files[1]).unwrap();
        assert_ne!(a.id, b.id);
        assert_eq!(a.license, "CC-BY");
        assert_eq!(a.category, "arthropods");
    }
    #[test]
    fn reads_server_action_envelope() {
        let value =
            action_result("0:{\"a\":\"$@1\"}\n1:{\"hits\":{\"found\":1,\"hit\":[]}}\n").unwrap();
        assert_eq!(value["hits"]["found"], 1);
    }
}
