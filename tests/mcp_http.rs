use bio_assets::{index::Library, models::Asset, server};
use serde_json::{Value, json};

fn wire_json(body: &str) -> Value {
    if let Ok(value) = serde_json::from_str(body) {
        return value;
    }
    body.lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .find(|v| v.get("result").is_some() || v.get("error").is_some())
        .expect("JSON-RPC response")
}

async fn call(client: &reqwest::Client, url: &str, session: &str, body: Value) -> Value {
    let response = client
        .post(url)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", session)
        .header("mcp-protocol-version", "2025-03-26")
        .json(&body)
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    wire_json(&response.text().await.unwrap())
}

/// A running MCP server backed by a tempdir library holding one `test:neuron`
/// asset. Aborts the server task on drop.
struct Server {
    #[allow(dead_code)]
    dir: tempfile::TempDir,
    client: reqwest::Client,
    url: String,
    session: String,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn start() -> Server {
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
    let url = format!("http://{address}/mcp");
    for _ in 0..50 {
        if client
            .get(format!("http://{address}/health"))
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let response = client
        .post(&url)
        .header("accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"integration-test","version":"1"}}}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    let session = response.headers()["mcp-session-id"]
        .to_str()
        .unwrap()
        .to_owned();
    let init = wire_json(&response.text().await.unwrap());
    assert!(init["result"]["capabilities"]["tools"].is_object());
    let response = client
        .post(&url)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session)
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert!(response.status().is_success());
    Server {
        dir,
        client,
        url,
        session,
        task,
    }
}

#[tokio::test]
async fn real_http_initialize_search_get_batch_and_validation() {
    let srv = start().await;
    let tools = call(
        &srv.client,
        &srv.url,
        &srv.session,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 5);
    let search = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_assets","arguments":{"query":"nerve cell"}}})).await;
    let data: Value =
        serde_json::from_str(search["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data[0]["id"], "test:neuron");
    assert!(data[0].get("svg").is_none());
    let get = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_asset","arguments":{"id":"test:neuron"}}})).await;
    let data: Value =
        serde_json::from_str(get["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["license"], "CC0-1.0");
    assert!(data["svg"].as_str().unwrap().contains("viewBox"));
    let batch = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_assets","arguments":{"ids":["test:missing","test:neuron"]}}})).await;
    let data: Value =
        serde_json::from_str(batch["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["errors"][0]["code"], "ASSET_NOT_FOUND");
    assert_eq!(data["assets"][0]["author"], "Test Author");
    let invalid = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"search_assets","arguments":{"query":"neuron","limit":0}}})).await;
    assert!(invalid["result"]["isError"] == true || invalid.get("error").is_some());
}

#[tokio::test]
async fn get_asset_honors_include_svg_flag() {
    let srv = start().await;
    // Omitted include_svg defaults to true: the full SVG is returned.
    let with_svg = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_asset","arguments":{"id":"test:neuron"}}})).await;
    let data: Value =
        serde_json::from_str(with_svg["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(data["svg"].as_str().is_some(), "include_svg omitted => svg present");
    // include_svg=false: no svg, but the always-present metadata fields remain.
    let meta = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_asset","arguments":{"id":"test:neuron","include_svg":false}}})).await;
    let data: Value =
        serde_json::from_str(meta["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(data.get("svg").is_none(), "include_svg:false => no svg field");
    assert!(data.get("view_box").is_some(), "view_box must be present");
    assert!(data.get("width").is_some(), "width must be present");
    assert!(data.get("height").is_some(), "height must be present");
    assert!(data.get("source").is_some(), "source must be present");
}

#[tokio::test]
async fn compose_svg_then_get_composed_roundtrip() {
    let srv = start().await;
    let compose = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"compose_svg","arguments":{"width":100,"height":100,"elements":[{"asset_id":"test:neuron","x":5,"y":5,"scale":2,"rotation":0}]}}})).await;
    let result = &compose["result"];
    assert!(result.get("isError").is_none() || result["isError"] == false, "compose failed: {compose}");
    let data: Value =
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(data.get("svg").is_none(), "compose_svg must be compact (no svg)");
    let result_id = data["result_id"].as_str().expect("result_id present").to_string();
    assert!(data.get("attribution").is_some(), "attribution must be present");

    let fetch = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_composed","arguments":{"result_id":result_id}}})).await;
    let fdata: Value =
        serde_json::from_str(fetch["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let svg = fdata["svg"].as_str().expect("svg string").to_string();
    assert!(svg.contains("<symbol"), "composed svg must contain a <symbol: {svg}");
    assert!(svg.contains("<use"), "composed svg must contain a <use: {svg}");
    assert!(svg.contains("href=\"#"), "composed svg <use> must reference a symbol: {svg}");
}

#[tokio::test]
async fn compose_svg_rejects_invalid_arguments() {
    let srv = start().await;
    // 51 elements exceeds the 1..=50 bound.
    let elements: Vec<Value> = (0..51)
        .map(|i| json!({"asset_id":"test:neuron","x":i,"y":0,"scale":1,"rotation":0}))
        .collect();
    let too_many = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"compose_svg","arguments":{"width":100,"height":100,"elements":elements}}})).await;
    assert!(
        too_many["result"]["isError"] == true || too_many.get("error").is_some(),
        "51 elements must be rejected: {too_many}"
    );
    // scale:0 is rejected.
    let zero_scale = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"compose_svg","arguments":{"width":100,"height":100,"elements":[{"asset_id":"test:neuron","x":0,"y":0,"scale":0,"rotation":0}]}}})).await;
    assert!(
        zero_scale["result"]["isError"] == true || zero_scale.get("error").is_some(),
        "scale:0 must be rejected: {zero_scale}"
    );
}

#[tokio::test]
async fn get_composed_unknown_result_id_errors() {
    let srv = start().await;
    let unknown = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_composed","arguments":{"result_id":"ba_comp_0000000000000000"}}})).await;
    assert!(
        unknown["result"]["isError"] == true || unknown.get("error").is_some(),
        "unknown result_id must be an error: {unknown}"
    );
}
