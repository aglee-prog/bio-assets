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
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetArgs {
    #[schemars(length(min = 1))]
    pub id: String,
    /// Include the full SVG markup (default). Set false for metadata only.
    #[serde(default = "default_true")]
    pub include_svg: bool,
}
fn default_true() -> bool {
    true
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetComposedArgs {
    /// A `result_id` returned by `compose_svg`.
    #[schemars(length(min = 1))]
    pub result_id: String,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BatchArgs {
    #[schemars(length(min = 1, max = 100))]
    pub ids: Vec<String>,
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
        description = "Return a local SVG primitive with its full license, author and attribution. Different assets have prefixed internal IDs. Rename IDs again when embedding multiple copies of one asset. No network access.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_asset(
        &self,
        Parameters(args): Parameters<GetArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let lib = self.library.clone();
        Ok(match tokio::task::spawn_blocking(move || -> anyhow::Result<Option<Value>> {
            let Some(asset) = lib.get(&args.id)? else {
                return Ok(None);
            };
            let mut value = serde_json::to_value(asset)?;
            if !args.include_svg && let Some(object) = value.as_object_mut() {
                object.remove("svg");
            }
            Ok(Some(value))
        })
        .await
        {
            Ok(Ok(Some(value))) => result(value),
            Ok(Ok(None)) => failure("ASSET_NOT_FOUND"),
            Ok(Err(e)) => failure(e),
            Err(e) => failure(e),
        })
    }
    #[tool(
        description = "Retrieve up to 100 local SVG primitives in one call. Returns assets in request order and explicit errors for missing items. Includes licensing and attribution for every asset. No network access.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_assets(
        &self,
        Parameters(args): Parameters<BatchArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        if args.ids.is_empty() || args.ids.len() > 100 {
            return Ok(failure("ids must contain 1..100 items"));
        }
        let lib = self.library.clone();
        Ok(match tokio::task::spawn_blocking(move || {
            let mut assets=Vec::new(); let mut errors=Vec::new(); let mut size=0;
            for id in args.ids {
                match lib.get(&id) {
                    Ok(Some(asset))=>{
                        if size+asset.svg.len()>64*1024*1024 {errors.push(json!({"id":id,"code":"BATCH_TOO_LARGE","message":"64 MiB SVG batch limit; retrieve this asset separately"}));}
                        else {size+=asset.svg.len();assets.push(asset);}
                    },
                    Ok(None)=>errors.push(json!({"id":id,"code":"ASSET_NOT_FOUND","message":"Asset is not in the local index"})),
                    Err(e)=>errors.push(json!({"id":id,"code":"READ_FAILED","message":e.to_string()}))
                }
            }
            json!({"assets":assets,"errors":errors})
        }).await {Ok(v)=>result(v),Err(e)=>failure(e)})
    }
    #[tool(
        description = "Deterministically place local assets by asset_id and x/y/scale/rotation onto a canvas. Each asset is anchored at its own viewBox origin; scaling is a transform (width/height stay the raw viewBox size). Returns a compact result id, never SVG markup; retrieve it with get_composed. Preserve every returned attribution. No network access.",
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
    #[tool(
        description = "Retrieve a stored composed SVG by the result id returned by compose_svg. Returns the full SVG markup for that composition. No network access.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_composed(
        &self,
        Parameters(args): Parameters<GetComposedArgs>,
    ) -> Result<CallToolResult, ErrorData> {
        let lib = self.library.clone();
        let result_id = args.result_id.clone();
        Ok(match tokio::task::spawn_blocking(move || lib.get_composed(&result_id)).await {
            Ok(Ok(svg)) => result(json!({"result_id": args.result_id, "svg": svg})),
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
            .with_instructions("Search scientific visual primitives, then either retrieve an asset's SVG and compose it yourself, or use the deterministic composer. Recommended flow: search_assets -> get_asset (include_svg=false for metadata only, with source/view_box/width/height) -> compose_svg to place assets by id + x/y/scale/rotation (each anchored at its own viewBox origin; returns a result id, not markup) -> get_composed to fetch the full SVG. Preserve all returned license and attribution requirements. This service is offline and does not render or lay out figures beyond fixed geometry.")
    }
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
    let service = StreamableHttpService::new(
        move || Ok(AssetServer::new(library.clone())),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = axum::Router::new()
        .route("/health", axum::routing::get(|| async { "ok" }))
        .nest_service("/mcp", service);
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
