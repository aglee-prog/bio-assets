pub mod bioicons;
pub mod niaid;
pub mod scidraw;

use crate::{
    index::{Library, atomic_write, contained_path},
    models::Asset,
};
use anyhow::{Context, Result, bail, ensure};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fs, path::Path, time::Duration};

pub struct Network {
    client: Client,
}
impl Network {
    pub fn new() -> Result<Self> {
        Ok(Self {
            client: Client::builder()
                .user_agent("bio-assets/0.1 (explicit local SVG collection importer)")
                .timeout(Duration::from_secs(120))
                .redirect(reqwest::redirect::Policy::limited(5))
                .build()?,
        })
    }
    pub async fn get(&self, url: &str) -> Result<Vec<u8>> {
        self.request(url, None).await
    }
    pub async fn json(&self, url: &str) -> Result<Value> {
        Ok(serde_json::from_slice(&self.get(url).await?)?)
    }
    pub async fn action(&self, url: &str, action: &str, args: &Value) -> Result<String> {
        Ok(String::from_utf8(
            self.request(url, Some((action, args))).await?,
        )?)
    }
    async fn request(&self, url: &str, action: Option<(&str, &Value)>) -> Result<Vec<u8>> {
        for attempt in 0..4 {
            tokio::time::sleep(Duration::from_millis(150 * (1 << attempt))).await;
            let request = if let Some((id, args)) = action {
                self.client
                    .post(url)
                    .header("Next-Action", id)
                    .header("Content-Type", "text/plain;charset=UTF-8")
                    .body(args.to_string())
            } else {
                self.client.get(url)
            };
            let mut response = match request.send().await {
                Ok(r) => r,
                Err(e) if attempt < 3 => {
                    tracing::warn!(%url,%e,"retrying request");
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            if (response.status().as_u16() == 429 || response.status().is_server_error())
                && attempt < 3
            {
                let delay = response
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.parse::<u64>().ok())
                    .unwrap_or(2 << attempt)
                    .min(120);
                tokio::time::sleep(Duration::from_secs(delay)).await;
                continue;
            }
            response = response
                .error_for_status()
                .with_context(|| format!("GET {url}"))?;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await? {
                ensure!(
                    bytes.len() + chunk.len() <= 64 * 1024 * 1024,
                    "upstream response exceeds 64 MiB"
                );
                bytes.extend_from_slice(&chunk);
            }
            return Ok(bytes);
        }
        bail!("request retries exhausted: {url}")
    }
}

/// Source-owned URLs sometimes incorrectly advertise HTTP; use HTTPS on the same host.
pub fn same_origin_url(base: &str, path: &str) -> Result<String> {
    let base = Url::parse(base)?;
    let mut url = base.join(path)?;
    ensure!(
        url.host_str() == base.host_str() && url.port_or_known_default().is_some(),
        "unexpected upstream host"
    );
    ensure!(
        url.username().is_empty() && url.password().is_none(),
        "unexpected URL credentials"
    );
    url.set_scheme("https")
        .map_err(|_| anyhow::anyhow!("invalid URL scheme"))?;
    Ok(url.into())
}

#[derive(Default, Serialize)]
pub struct Report {
    pub imported: usize,
    pub failed: usize,
    pub failures: Vec<Value>,
}
impl Report {
    pub fn record(&mut self, id: &str, outcome: Result<()>) {
        match outcome {
            Ok(()) => self.imported += 1,
            Err(e) => {
                self.failed += 1;
                tracing::debug!(id,error=%e,"asset skipped; previous indexed version retained");
                self.failures
                    .push(serde_json::json!({"id":id,"error":format!("{e:#}")}));
            }
        }
    }
    pub fn save(&self, library: &Library, source: &str) -> Result<()> {
        atomic_write(
            &library
                .root
                .join(format!("data/manifests/{source}/last-import.json")),
            &serde_json::to_vec_pretty(self)?,
        )?;
        println!(
            "{}",
            serde_json::json!({
                "status": if self.failed==0 {"complete"} else if self.imported==0 {"failed"} else {"partial"},
                "imported":self.imported,"failed":self.failed,
                "report":library.root.join(format!("data/manifests/{source}/last-import.json"))
            })
        );
        Ok(())
    }

    pub fn check_outcome(&self, strict: bool) -> Result<()> {
        ensure!(
            self.imported > 0 || self.failed == 0,
            "no assets imported; {} files failed; inspect the import report",
            self.failed
        );
        ensure!(
            !strict || self.failed == 0,
            "partial import: {} assets imported, {} skipped (--strict); inspect the import report",
            self.imported,
            self.failed
        );
        if self.failed > 0 {
            tracing::warn!(
                imported = self.imported,
                skipped = self.failed,
                "Partial import: successful assets are available; inspect the report for skipped files. Use --strict for a nonzero exit on partial imports."
            );
        }
        Ok(())
    }
}

#[derive(Deserialize)]
pub struct ManifestEntry {
    #[serde(flatten)]
    pub asset: Asset,
    pub local_path: String,
}

pub fn import_manifest(
    library: &Library,
    source: &str,
    manifest: &Path,
    limit: Option<usize>,
) -> Result<Report> {
    let entries: Vec<ManifestEntry> = serde_json::from_slice(&fs::read(manifest)?)?;
    let base = manifest
        .parent()
        .context("manifest needs a parent directory")?;
    let mut report = Report::default();
    for entry in entries.into_iter().take(limit.unwrap_or(usize::MAX)) {
        let outcome = (|| {
            ensure!(entry.asset.source == source, "manifest source mismatch");
            let path = contained_path(base, Path::new(&entry.local_path))?;
            library.store(
                &entry.asset,
                &fs::read(path)?,
                &serde_json::json!({"manifest":manifest,"local_path":entry.local_path}),
            )
        })();
        report.record(&entry.asset.id, outcome);
    }
    Ok(report)
}

pub fn text(value: &Value, key: &str) -> String {
    value[key].as_str().unwrap_or_default().to_owned()
}
pub fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
