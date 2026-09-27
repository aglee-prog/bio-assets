use super::{Report, text};
use crate::{
    index::{Library, atomic_write},
    models::Asset,
};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{collections::HashMap, fs, path::Path, process::Command};

fn git(dir: Option<&Path>, args: &[&str]) -> Result<String> {
    let mut command = Command::new("git");
    if let Some(dir) = dir {
        command.arg("-C").arg(dir);
    }
    let out = command
        .args(args)
        .output()
        .context("git must be installed for Bioicons imports")?;
    ensure!(
        out.status.success(),
        "git failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_owned())
}

pub fn import(
    library: &Library,
    from: Option<&Path>,
    update: bool,
    limit: Option<usize>,
) -> Result<Report> {
    let cache = library.root.join("cache/bioicons");
    let repo = from.unwrap_or(&cache);
    if !repo.exists() {
        ensure!(from.is_none(), "local repository does not exist");
        git(
            None,
            &[
                "clone",
                "--depth",
                "1",
                "https://github.com/duerrsimon/bioicons.git",
                repo.to_str().context("invalid cache path")?,
            ],
        )?;
    } else if update && from.is_none() {
        git(Some(repo), &["fetch", "--depth", "1", "origin", "main"])?;
        git(Some(repo), &["checkout", "--detach", "FETCH_HEAD"])?;
    }
    let revision = git(Some(repo), &["rev-parse", "HEAD"])?;
    let state_path = library.root.join("data/manifests/bioicons/revision.txt");
    let mut renames = HashMap::new();
    if let Ok(previous) = fs::read_to_string(&state_path)
        && let Ok(diff) = git(
            Some(repo),
            &[
                "diff",
                "--name-status",
                "-M",
                previous.trim(),
                &revision,
                "--",
                "static/icons",
            ],
        )
    {
        for line in diff.lines() {
            let parts: Vec<_> = line.split('\t').collect();
            if parts.len() == 3
                && parts[0].starts_with('R')
                && let Some(id) =
                    library.existing_id("bioicons", parts[1].trim_start_matches("static/icons/"))?
            {
                renames.insert(parts[2].trim_start_matches("static/icons/").to_owned(), id);
            }
        }
    }
    let icons = repo.join("static/icons");
    let authors: Value = serde_json::from_slice(&fs::read(icons.join("authors.json"))?)?;
    let mut report = Report::default();
    for entry in walkdir::WalkDir::new(&icons)
        .sort_by_file_name()
        .into_iter()
    {
        let entry = entry?;
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|s| s.to_str()) != Some("svg")
        {
            continue;
        }
        if report.imported + report.failed >= limit.unwrap_or(usize::MAX) {
            break;
        }
        let key = entry
            .path()
            .strip_prefix(&icons)?
            .to_str()
            .context("non-UTF8 upstream path")?
            .to_owned();
        let parts: Vec<_> = key.split('/').collect();
        if parts.len() != 4 {
            continue;
        }
        let id = library
            .existing_id("bioicons", &key)?
            .or_else(|| renames.get(&key).cloned())
            .unwrap_or_else(|| format!("bioicons:{}", key.trim_end_matches(".svg")));
        let (license, url) = license(parts[0]);
        let author = parts[2].replace('_', " ");
        let author_url = text(&authors, &author);
        let name = parts[3].trim_end_matches(".svg").replace(['_', '-'], " ");
        let source_url =
            format!("https://github.com/duerrsimon/bioicons/blob/{revision}/static/icons/{key}");
        let attribution = format!(
            "{name} by {author} ({author_url}), {license}{}. Source: {source_url}. SVG internal IDs/classes normalized; geometry unchanged.",
            url.as_ref().map(|u| format!(" ({u})")).unwrap_or_default()
        );
        let asset = Asset {
            id: id.clone(),
            source: "bioicons".into(),
            upstream_key: key.clone(),
            name,
            category: parts[1].replace('_', " ").to_lowercase(),
            tags: vec![],
            description: String::new(),
            license,
            license_url: url,
            author: Some(author),
            attribution,
            source_url,
            source_revision: Some(revision.clone()),
        };
        report.record(&id,(||library.store(&asset,&fs::read(entry.path())?,&json!({"path":key,"author_url":author_url,"license_directory":parts[0],"commit":revision})))());
    }
    if limit.is_none() {
        atomic_write(&state_path, revision.as_bytes())?;
    }
    Ok(report)
}

fn license(value: &str) -> (String, Option<String>) {
    match value {
        "cc-0" => (
            "CC0-1.0".into(),
            Some("https://creativecommons.org/publicdomain/zero/1.0/".into()),
        ),
        "cc-by-3.0" => (
            "CC-BY-3.0".into(),
            Some("https://creativecommons.org/licenses/by/3.0/".into()),
        ),
        "cc-by-4.0" => (
            "CC-BY-4.0".into(),
            Some("https://creativecommons.org/licenses/by/4.0/".into()),
        ),
        "cc-by-sa-3.0" => (
            "CC-BY-SA-3.0".into(),
            Some("https://creativecommons.org/licenses/by-sa/3.0/".into()),
        ),
        "cc-by-sa-4.0" => (
            "CC-BY-SA-4.0".into(),
            Some("https://creativecommons.org/licenses/by-sa/4.0/".into()),
        ),
        "mit" => (
            "MIT".into(),
            Some("https://opensource.org/license/mit".into()),
        ),
        // Upstream says only BSD; do not invent a clause count.
        other => (other.to_owned(), None),
    }
}
