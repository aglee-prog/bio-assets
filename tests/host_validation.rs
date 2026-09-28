use bio_assets::{index::Library, models::Asset, server};
use serde_json::json;

/// A running MCP server backed by a tempdir library holding one `test:neuron`
/// asset. Aborts the server task on drop.
struct Server {
    #[allow(dead_code)]
    dir: tempfile::TempDir,
    client: reqwest::Client,
    base: String,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// This test binary owns `BIO_ASSETS_ALLOWED_HOSTS`. It is set once, here, before
/// the server starts, so Host validation allows `owui.test` (any port) in addition
/// to rmcp's default local allowlist. Keeping a single serialized test means the
/// process-global env var is set exactly once and never raced.
async fn start() -> Server {
    unsafe { std::env::set_var("BIO_ASSETS_ALLOWED_HOSTS", "owui.test") };

    let dir = tempfile::tempdir().unwrap();
    let library = Library::new(dir.path());
    library.initialize().unwrap();
    let asset = Asset {
        id: "test:neuron".into(),
        source: "test".into(),
        upstream_key: "neuron".into(),
        name: "Neuron".into(),
        category: "cell".into(),
        tags: vec!["neuron".into()],
        description: "A nerve cell".into(),
        license: "CC0-1.0".into(),
        license_url: None,
        author: Some("Test Author".into()),
        attribution: "Test Author, CC0-1.0".into(),
        source_url: "https://example.org/neuron".into(),
        source_revision: None,
    };
    library
        .store(
            &asset,
            b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><path d='M0 0L10 10'/></svg>",
            &json!({}),
        )
        .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);
    let task = tokio::spawn(server::serve(library, address));
    let client = reqwest::Client::new();
    let base = format!("http://{address}");
    for _ in 0..50 {
        if client.get(format!("{base}/health")).send().await.is_ok() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Server {
        dir,
        client,
        base,
        task,
    }
}

fn initialize_body(id: u64) -> serde_json::Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": { "name": "host-validation-test", "version": "1" }
        }
    })
}

/// POST an MCP `initialize` request to `/mcp` with an overridden `Host` header.
async fn post_with_host(srv: &Server, host: &str, id: u64) -> reqwest::Response {
    srv.client
        .post(format!("{}/mcp", srv.base))
        .header("accept", "application/json, text/event-stream")
        .header("host", host)
        .json(&initialize_body(id))
        .send()
        .await
        .unwrap()
}

#[tokio::test]
async fn host_validation_allows_configured_host_and_blocks_others() {
    let srv = start().await;

    // A configured bare name matches ANY port, including one that is not the real
    // bind port.
    let allowed = post_with_host(&srv, "owui.test:8092", 1).await;
    assert!(
        allowed.status().is_success(),
        "configured host must be accepted, got {}",
        allowed.status()
    );
    let body = allowed.text().await.unwrap();
    assert!(
        body.contains("\"result\""),
        "expected a JSON-RPC result for the allowed host; got: {body}"
    );

    // The default local allowlist is preserved even though the env var is set.
    let local = post_with_host(&srv, "127.0.0.1:8092", 2).await;
    assert!(
        local.status().is_success(),
        "default local host must still be accepted, got {}",
        local.status()
    );

    // An unlisted host is still rejected with 403.
    let rejected = post_with_host(&srv, "evil.example.com:8092", 3).await;
    assert_eq!(
        rejected.status(),
        reqwest::StatusCode::FORBIDDEN,
        "unlisted host must be rejected with 403"
    );
}
