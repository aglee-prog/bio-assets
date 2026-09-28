use crate::{
    index::Library,
    models::ComposeArgs,
};
use anyhow::Result;
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, ServerCapabilities, ServerConfig},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchArgs {
    /// Words to search in names, categories, tags, descriptions and curated aliases.
    #[schemars(length(min = 1, max = 1024))]
    pub query: String,
    pub source: Option<String>,
    pub category: Option<String>,
    #[serde(default = "default_limit")]
    #[schemars(range(min = 1, max = 100))]
    pub limit: u32,
}
fn default_limit() -> u32 {
    20
}

#[derive(Clone)]
pub struct AssetServer {
    library: Library,
    tool_router: ToolRouter<Self>,
}

fn result(value: Value) -> CallToolResult {
    CallToolResult::success(vec![ContentBlock::text(value.to_string())])
}
fn failure(error: impl std::fmt::Display) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(error.to_string())])
}

#[tool_router]
impl AssetServer {
    pub fn new(library: Library) -> Self {
        Self {
            library,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(
        description = "Search imported scientific SVG primitives offline. Returns compact metadata, never SVG markup. Preserve attribution when composing retrieved assets.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn search_assets(
        &self,
        Parameters(args): Parameters<SearchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let lib = self.library.clone();
        let found = tokio::task::spawn_blocking(move || {
            lib.search(
                &args.query,
                args.source.as_deref(),
                args.category.as_deref(),
                args.limit,
            )
        })
        .await;
        Ok(match found {
            Ok(Ok(assets)) => result(json!(assets)),
            Ok(Err(e)) => failure(e),
            Err(e) => failure(e),
        })
    }
    #[tool(
        description = "Deterministically compose a scene of elements onto a canvas. Each element is an untagged union discriminated by which required fields are present (no `type` key); only the listed required fields are mandatory, the rest have defaults, and asset placement is unchanged. Examples — asset: {\"asset_id\":\"sci:neuron\",\"x\":10,\"y\":20,\"scale\":2,\"rotation\":15}; text: {\"text\":\"Hi & <ok>\",\"x\":10,\"y\":10,\"font_size\":16,\"anchor\":\"middle\",\"fill\":\"#000000\"}; line: {\"x1\":0,\"y1\":0,\"x2\":50,\"y2\":25,\"width\":2,\"stroke\":\"#000\",\"arrow_end\":true}; rect: {\"x\":0,\"y\":0,\"width\":40,\"height\":20,\"rx\":4,\"fill\":\"#fff\",\"stroke\":\"#000\",\"stroke_width\":1}; circle: {\"cx\":20,\"cy\":20,\"r\":10,\"fill\":\"#f00\",\"stroke\":\"#000\",\"stroke_width\":2}. Each asset is anchored at its own viewBox origin; scaling is a transform (width/height stay the raw viewBox size). Returns a compact result (status, result_id, url, mime_type, width, height) and never SVG markup; the url is the artifact location, relative by default and absolute when a public base URL is configured. After a successful composition, present the returned url to the user; do not fetch or inspect the generated SVG. No network access.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn compose_svg(
        &self,
        Parameters(args): Parameters<ComposeArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let lib = self.library.clone();
        Ok(match tokio::task::spawn_blocking(move || lib.compose(&args)).await {
            Ok(Ok(res)) => result(json!(res)),
            Ok(Err(e)) => failure(e),
            Err(e) => failure(e),
        })
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AssetServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("bio-assets", env!("CARGO_PKG_VERSION")))
            .with_instructions("Search scientific visual primitives, then use the deterministic composer to build the figure. Recommended flow: search_assets -> compose_svg to place assets (by id + x/y/scale/rotation) or native text/line/rect/circle elements (each an untagged union discriminated by required fields; assets anchored at their own viewBox origin; returns a result_id and an artifact url, never markup) -> present the returned artifact url to the user; do not fetch or inspect the generated SVG. Raw asset SVG is never returned by any tool; compose_svg resolves assets internally. This service is offline and does not render or lay out figures beyond fixed geometry.")
    }
}

/// Serve a stored composed SVG for `GET /results/{name}`. `name` is the
/// URL-decoded path segment and must be a well-formed `result_id` plus a `.svg`
/// suffix; anything else (including traversal, extra path segments, absolute
/// paths, or a bare `/results`) is a 404 with no existence disclosure. Only
/// regular files inside `<root>/results/` are served.
async fn result_artifact(
    axum::extract::State(state): axum::extract::State<Library>,
    axum::extract::Path(name): axum::extract::Path<String>,
) -> axum::response::Response {
    let not_found = || {
        axum::response::Response::builder()
            .status(axum::http::StatusCode::NOT_FOUND)
            .body(axum::body::Body::empty())
            .expect("valid 404 response")
    };
    let root = state.root.clone();
    let bytes = match tokio::task::spawn_blocking(move || -> Result<Vec<u8>, ()> {
        let Some(result_id) = name.strip_suffix(".svg") else {
            return Err(());
        };
        if !crate::index::is_valid_result_id(result_id) {
            return Err(());
        }
        let path = root.join("results").join(name);
        if !path.is_file() {
            return Err(());
        }
        let Ok(canonical) = path.canonicalize() else {
            return Err(());
        };
        let Ok(root_canonical) = root.canonicalize() else {
            return Err(());
        };
        if !canonical.starts_with(&root_canonical) {
            return Err(());
        }
        std::fs::read(&canonical).map_err(|_| ())
    })
    .await
    {
        Ok(Ok(bytes)) => bytes,
        _ => return not_found(),
    };
    axum::response::Response::builder()
        .status(axum::http::StatusCode::OK)
        .header(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("image/svg+xml"),
        )
        .body(axum::body::Body::from(bytes))
        .expect("valid response")
}

pub async fn serve(library: Library, address: SocketAddr) -> Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut config = StreamableHttpServerConfig::default();
    config.allowed_hosts.extend(additional_allowed_hosts(
        std::env::var("BIO_ASSETS_ALLOWED_HOSTS").ok().as_deref(),
    ));

    config.cancellation_token = cancel.child_token();
    let state = library.clone();
    let service = StreamableHttpService::new(
        move || Ok(AssetServer::new(library.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = axum::Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .route("/results/{name}", axum::routing::get(result_artifact))
        .nest_service("/mcp", service)
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(address).await?;
    tracing::info!(%address,"MCP listening at /mcp");
    axum::serve(listener, router)
        .with_graceful_shutdown(async move {
            #[cfg(unix)]
            {
                let mut terminate =
                    tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                        .expect("install SIGTERM handler");
                tokio::select! {_=tokio::signal::ctrl_c()=>{},_=terminate.recv()=>{}}
            }
            #[cfg(not(unix))]
            let _ = tokio::signal::ctrl_c().await;
            cancel.cancel();
        })
        .await?;
    Ok(())
}

/// Parse the `BIO_ASSETS_ALLOWED_HOSTS` value into additional allowed `Host`
/// names for the Streamable HTTP MCP transport.
///
/// This only *adds* to rmcp's built-in local allowlist (`localhost`,
/// `127.0.0.1`, `::1`); it never disables `Host` header validation, and it never
/// introduces a wildcard (`*`) or `0.0.0.0`. An entry may carry a port, in which
/// case rmcp matches that exact `host:port`; a bare name matches any port on that
/// host. Entries are passed through unchanged (no port stripping, no lowercasing,
/// no normalization) so rmcp applies its own `host:port` semantics.
///
/// Split on commas, trim surrounding whitespace on each entry, drop empty entries,
/// and preserve order and duplicates as given. `None` or blank input yields an
/// empty list.
pub fn additional_allowed_hosts(raw: Option<&str>) -> Vec<String> {
    let Some(raw) = raw else {
        return Vec::new();
    };
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod additional_allowed_hosts_tests {
    use super::additional_allowed_hosts;

    #[test]
    fn none_is_empty() {
        assert_eq!(additional_allowed_hosts(None), Vec::<String>::new());
    }

    #[test]
    fn blank_is_empty() {
        assert_eq!(additional_allowed_hosts(Some("")), Vec::<String>::new());
        assert_eq!(additional_allowed_hosts(Some("   ")), Vec::<String>::new());
    }

    #[test]
    fn single_entry() {
        assert_eq!(
            additional_allowed_hosts(Some("host.docker.internal")),
            vec!["host.docker.internal".to_owned()]
        );
    }

    #[test]
    fn trims_whitespace_and_drops_trailing_empty() {
        assert_eq!(
            additional_allowed_hosts(Some(" a , b ,")),
            vec!["a".to_owned(), "b".to_owned()]
        );
    }

    #[test]
    fn drops_internal_empty_entries() {
        assert_eq!(
            additional_allowed_hosts(Some("x,,y, z ,")),
            vec!["x".to_owned(), "y".to_owned(), "z".to_owned()]
        );
    }

    #[test]
    fn preserves_port_verbatim() {
        assert_eq!(
            additional_allowed_hosts(Some("example.com:8080")),
            vec!["example.com:8080".to_owned()]
        );
    }

    #[test]
    fn preserves_duplicates_and_order() {
        assert_eq!(
            additional_allowed_hosts(Some("b, a ,b")),
            vec!["b".to_owned(), "a".to_owned(), "b".to_owned()]
        );
    }

    #[test]
    fn never_emits_wildcard_or_any_on_blank_input() {
        for input in [None, Some(""), Some("   ")] {
            let out = additional_allowed_hosts(input);
            assert!(out.is_empty(), "blank input must yield no entries: {out:?}");
            assert!(!out.contains(&"*".to_owned()), "must never emit `*`: {out:?}");
            assert!(!out.contains(&"0.0.0.0".to_owned()), "must never emit 0.0.0.0: {out:?}");
        }
    }
}
