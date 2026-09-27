use serde::{Deserialize, Serialize};

/// Shared importer format. Source-specific records are preserved separately.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub source: String,
    pub upstream_key: String,
    pub name: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub description: String,
    pub license: String,
    #[serde(default)]
    pub license_url: Option<String>,
    #[serde(default)]
    pub author: Option<String>,
    pub attribution: String,
    pub source_url: String,
    #[serde(default)]
    pub source_revision: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AssetSummary {
    pub id: String,
    pub name: String,
    pub source: String,
    pub category: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AssetContent {
    pub id: String,
    pub name: String,
    pub svg: String,
    pub license: String,
    pub license_url: Option<String>,
    pub author: Option<String>,
    pub attribution: String,
    pub source_url: String,
}
