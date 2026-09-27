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

#[tokio::test]
async fn real_http_initialize_search_get_batch_and_validation() {
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
    library.store(&asset,b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><path d='M0 0L10 10'/></svg>",&json!({})).unwrap();
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
    let response=client.post(&url).header("accept","application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"integration-test","version":"1"}}})).send().await.unwrap();
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
    let tools = call(
        &client,
        &url,
        &session,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 3);
    let search=call(&client,&url,&session,json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_assets","arguments":{"query":"nerve cell"}}})).await;
    let data: Value =
        serde_json::from_str(search["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data[0]["id"], "test:neuron");
    assert!(data[0].get("svg").is_none());
    let get=call(&client,&url,&session,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"get_asset","arguments":{"id":"test:neuron"}}})).await;
    let data: Value =
        serde_json::from_str(get["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["license"], "CC0-1.0");
    assert!(data["svg"].as_str().unwrap().contains("viewBox"));
    let batch=call(&client,&url,&session,json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"get_assets","arguments":{"ids":["test:missing","test:neuron"]}}})).await;
    let data: Value =
        serde_json::from_str(batch["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data["errors"][0]["code"], "ASSET_NOT_FOUND");
    assert_eq!(data["assets"][0]["author"], "Test Author");
    let invalid=call(&client,&url,&session,json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"search_assets","arguments":{"query":"neuron","limit":0}}})).await;
    assert!(invalid["result"]["isError"] == true || invalid.get("error").is_some());
    task.abort();
}
