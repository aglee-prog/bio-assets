use rmcp::schemars;
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
    pub source: String,
    pub svg: String,
    pub view_box: [f64; 4],
    pub width: f64,
    pub height: f64,
    pub license: String,
    pub license_url: Option<String>,
    pub author: Option<String>,
    pub attribution: String,
    pub source_url: String,
}

/// One placed asset inside a composition: an asset id plus its placement
/// anchored at the asset's viewBox origin.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComposeElement {
    pub asset_id: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_scale")]
    pub scale: f64,
    #[serde(default)]
    pub rotation: f64,
}
fn default_scale() -> f64 {
    1.0
}

/// A deterministic composition request: a canvas plus an ordered list of
/// placed assets.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComposeArgs {
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub background: Option<String>,
    pub elements: Vec<ComposeElement>,
}

/// Compact, deterministic composition result. Carries no SVG markup; retrieve
/// the artifact with `get_composed` using `result_id`.
#[derive(Debug, Serialize, Deserialize)]
pub struct ComposeResult {
    pub result_id: String,
    pub width: f64,
    pub height: f64,
    pub element_count: usize,
    pub attribution: Vec<String>,
}

/// A resolved asset used by the pure `compose` function: the normalized SVG
/// string, its viewBox, and attribution.
#[derive(Debug, Clone)]
pub struct PlacedAsset {
    pub asset_id: String,
    pub svg: String,
    pub view_box: [f64; 4],
    pub attribution: String,
}
