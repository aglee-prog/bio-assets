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

/// One scene element: either a placed asset or a native text/line/rect/circle.
/// Untagged, so existing asset-only JSON (no `type` key) still deserializes;
/// the variant is disambiguated by which required fields are present.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum ComposeElement {
    Asset(AssetElement),
    Text(TextElement),
    Line(LineElement),
    Rect(RectElement),
    Circle(CircleElement),
}

/// A placed asset: an asset id plus its placement anchored at the asset's
/// viewBox origin.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssetElement {
    pub asset_id: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_scale")]
    pub scale: f64,
    #[serde(default)]
    pub rotation: f64,
}
/// A native text element anchored at (x, y).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TextElement {
    pub text: String,
    pub x: f64,
    pub y: f64,
    #[serde(default = "default_font_size")]
    pub font_size: f64,
    #[serde(default = "default_anchor")]
    pub anchor: String,
    #[serde(default)]
    pub fill: Option<String>,
}
/// A native line from (x1, y1) to (x2, y2); `arrow_end` adds a chevron at (x2, y2).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LineElement {
    pub x1: f64,
    pub y1: f64,
    pub x2: f64,
    pub y2: f64,
    #[serde(default = "default_width")]
    pub width: f64,
    #[serde(default)]
    pub stroke: Option<String>,
    #[serde(default)]
    pub arrow_end: bool,
}
/// A native rectangle; `fill` and `stroke` are emitted only when present.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RectElement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub rx: f64,
    #[serde(default)]
    pub fill: Option<String>,
    #[serde(default)]
    pub stroke: Option<String>,
    #[serde(default = "default_width")]
    pub stroke_width: f64,
}
/// A native circle; `fill` and `stroke` are emitted only when present.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CircleElement {
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
    #[serde(default)]
    pub fill: Option<String>,
    #[serde(default)]
    pub stroke: Option<String>,
    #[serde(default = "default_width")]
    pub stroke_width: f64,
}
fn default_scale() -> f64 {
    1.0
}
fn default_font_size() -> f64 {
    12.0
}
fn default_anchor() -> String {
    "start".into()
}
fn default_width() -> f64 {
    1.0
}

/// A deterministic composition request: a canvas plus an ordered list of
/// elements.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ComposeArgs {
    pub width: f64,
    pub height: f64,
    #[serde(default)]
    pub background: Option<String>,
    pub elements: Vec<ComposeElement>,
}

/// Compact, deterministic composition result. Carries no SVG markup. `url` is
/// the artifact location for the client to present to the user (absolute when a
/// public base URL is configured, otherwise a relative server path); the client
/// must not fetch or inspect the generated SVG. `status` is `"ok"` on success.
#[derive(Debug, Serialize, Deserialize)]
pub struct ComposeResult {
    pub status: String,
    pub result_id: String,
    pub url: String,
    pub mime_type: String,
    pub width: f64,
    pub height: f64,
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
