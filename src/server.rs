use crate::index::Library;
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
        Ok(
            match tokio::task::spawn_blocking(move || lib.get(&args.id)).await {
                Ok(Ok(Some(asset))) => result(json!(asset)),
                Ok(Ok(None)) => failure("ASSET_NOT_FOUND"),
                Ok(Err(e)) => failure(e),
                Err(e) => failure(e),
            },
        )
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
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AssetServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(rmcp::model::Implementation::new("bio-assets", env!("CARGO_PKG_VERSION")))
            .with_instructions("Find a scientific visual primitive, retrieve its SVG, then compose the final SVG yourself. This service only searches and retrieves local assets. Preserve all returned license and attribution requirements. It does not render or lay out figures.")
    }
}

pub async fn serve(library: Library, address: SocketAddr) -> Result<()> {
    use rmcp::transport::streamable_http_server::{
        StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let mut config = StreamableHttpServerConfig::default();
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
