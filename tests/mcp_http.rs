use bio_assets::{index::Library, models::Asset, server};
use serde_json::{Value, json};

/// Fixture SVG for the `test:neuron` asset stored by [`start`].
const NEURON_SVG: &str =
    "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 10 10'><path d='M0 0L10 10'/></svg>";

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
        .store(&asset, NEURON_SVG.as_bytes(), &json!({}))
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
async fn real_http_initialize_search_and_validation() {
    let srv = start().await;
    let tools = call(
        &srv.client,
        &srv.url,
        &srv.session,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await;
    assert_eq!(tools["result"]["tools"].as_array().unwrap().len(), 2);
    let tool_names: Vec<String> = tools["result"]["tools"].as_array().unwrap()
        .iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert!(tool_names.contains(&"search_assets".to_string()));
    assert!(tool_names.contains(&"compose_svg".to_string()));
    assert!(!tool_names.contains(&"get_asset".to_string()), "get_asset must be gone: {tool_names:?}");
    assert!(!tool_names.contains(&"get_assets".to_string()), "get_assets must be gone: {tool_names:?}");
    assert!(!tool_names.contains(&"get_composed".to_string()), "get_composed must be gone: {tool_names:?}");
    let search = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"search_assets","arguments":{"query":"nerve cell"}}})).await;
    let data: Value =
        serde_json::from_str(search["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(data[0]["id"], "test:neuron");
    assert!(data[0].get("svg").is_none());
    let invalid = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"search_assets","arguments":{"query":"neuron","limit":0}}})).await;
    assert!(invalid["result"]["isError"] == true || invalid.get("error").is_some());
}

#[tokio::test]
async fn raw_svg_cannot_be_retrieved_through_mcp_tools() {
    let srv = start().await;
    // Every exposed tool, called with valid arguments, must return a
    // successful response that leaks neither the fixture SVG nor any markup.
    let tools = call(
        &srv.client,
        &srv.url,
        &srv.session,
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
    )
    .await;
    let tool_names: Vec<String> = tools["result"]["tools"].as_array().unwrap()
        .iter().map(|t| t["name"].as_str().unwrap().to_string()).collect();
    assert_eq!(tool_names.len(), 2, "expected exactly 2 exposed tools: {tool_names:?}");
    for (i, name) in tool_names.iter().enumerate() {
        let arguments = match name.as_str() {
            "search_assets" => json!({"query":"nerve cell"}),
            "compose_svg" => json!({"width":100,"height":100,"elements":[{"asset_id":"test:neuron","x":5,"y":5,"scale":2,"rotation":0}]}),
            other => panic!("unexpected tool in tools/list: {other:?}; add valid arguments to this test"),
        };
        let response = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":10+i,"method":"tools/call","params":{"name":name,"arguments":arguments}})).await;
        let result = &response["result"];
        assert!(
            result.get("isError").is_none() || result["isError"] == false,
            "{name} call must succeed: {response}"
        );
        let text = result["content"][0]["text"].as_str().unwrap();
        assert!(
            !text.contains(NEURON_SVG),
            "{name} must not return the raw asset SVG: {text}"
        );
        for marker in ["<svg", "<path", "<symbol", "<use", "viewBox"] {
            assert!(
                !text.contains(marker),
                "{name} must not leak markup marker {marker:?}: {text}"
            );
        }
    }
}

#[tokio::test]
async fn compose_svg_returns_compact_schema_and_http_serves_exact_bytes() {
    let srv = start().await;
    let root = srv.url.trim_end_matches("/mcp");
    let compose = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"compose_svg","arguments":{"width":100,"height":100,"elements":[{"asset_id":"test:neuron","x":5,"y":5,"scale":2,"rotation":0}]}}})).await;
    let result = &compose["result"];
    assert!(result.get("isError").is_none() || result["isError"] == false, "compose failed: {compose}");
    let text = result["content"][0]["text"].as_str().unwrap().to_string();
    let data: Value = serde_json::from_str(&text).unwrap();
    // (A) schema
    let result_id = data["result_id"].as_str().expect("result_id present").to_string();
    let re = regex::Regex::new(r"^ba_comp_[0-9a-f]{16}$").unwrap();
    assert!(re.is_match(&result_id), "result_id must match ^ba_comp_[0-9a-f]{{16}}$: {result_id}");
    assert_eq!(data["url"], format!("/results/{result_id}.svg"), "url must be the relative artifact path");
    assert_eq!(data["mime_type"], "image/svg+xml");
    assert_eq!(data["width"], 100.0);
    assert_eq!(data["height"], 100.0);
    let keys: std::collections::BTreeSet<&str> = data
        .as_object()
        .expect("compose result must be a JSON object")
        .keys()
        .map(|k| k.as_str())
        .collect();
    assert_eq!(
        keys,
        ["status", "result_id", "url", "mime_type", "width", "height"]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        "compose result must contain exactly the 6 contract keys"
    );
    assert_eq!(data["status"], "ok");
    // (A) no SVG markup anywhere in the MCP response
    for marker in ["<svg", "<path", "<symbol", "<use", "viewBox=\"0 0"] {
        assert!(!text.contains(marker), "MCP response must not contain {marker:?}: {text}");
    }
    // (B) HTTP retrieval
    let resp = srv.client.get(format!("{root}/results/{result_id}.svg")).send().await.unwrap();
    assert_eq!(resp.status(), 200, "GET composed artifact must be 200");
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).unwrap().to_str().unwrap();
    assert_eq!(content_type, "image/svg+xml", "Content-Type must be image/svg+xml, got {content_type}");
    let body = resp.bytes().await.unwrap();
    let body_str = std::str::from_utf8(&body).unwrap();
    assert!(body_str.contains("<symbol"), "served svg must contain <symbol: {body_str}");
    assert!(body_str.contains("<use"), "served svg must contain <use: {body_str}");
    assert!(body_str.contains("href=\"#"), "served svg <use> must reference a symbol: {body_str}");
    // exact stored bytes
    let stored = std::fs::read(srv.dir.path().join("results").join(format!("{result_id}.svg"))).unwrap();
    assert_eq!(body.to_vec(), stored, "served bytes must equal the stored bytes");
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
async fn http_result_endpoint_rejects_invalid_and_unknown_ids() {
    let srv = start().await;
    let root = srv.url.trim_end_matches("/mcp");
    let cases = vec![
        format!("{root}/results/ba_comp_0000000000000000.svg"), // valid format, never composed
        format!("{root}/results/not-a-valid-id.svg"),
        format!("{root}/results/foo/bar.svg"),
    ];
    for url in cases {
        let resp = srv.client.get(&url).send().await.unwrap();
        assert!(!resp.status().is_success(), "{url} must not be 2xx, got {}", resp.status());
        let body = resp.text().await.unwrap();
        assert!(!body.contains("root:"), "{url} must not leak /etc/passwd");
        assert!(!body.contains("<svg"), "{url} must not return svg");
    }
    // traversal attempts (raw + percent-encoded) must not 200 or leak /etc/passwd
    for url in [
        format!("{root}/results/../../etc/passwd"),
        format!("{root}/results/..%2f..%2fetc%2fpasswd"),
        format!("{root}/results/%2e%2e%2f%2e%2e%2fetc%2fpasswd"),
    ] {
        let resp = srv.client.get(&url).send().await.unwrap();
        assert!(!resp.status().is_success(), "{url} must not be 2xx, got {}", resp.status());
        let body = resp.text().await.unwrap();
        assert!(!body.contains("root:"), "{url} must not leak /etc/passwd: {body}");
    }
    // no directory listing
    for url in [format!("{root}/results"), format!("{root}/results/")] {
        let resp = srv.client.get(&url).send().await.unwrap();
        assert!(!resp.status().is_success(), "{url} must not list, got {}", resp.status());
        let body = resp.text().await.unwrap();
        assert!(!body.contains("ba_comp_"), "{url} must not list artifacts: {body}");
    }
    // POST to an artifact URL must not 200
    let resp = srv.client.post(format!("{root}/results/ba_comp_0000000000000000.svg")).send().await.unwrap();
    assert!(!resp.status().is_success(), "POST to artifact must not be 2xx, got {}", resp.status());
}

#[tokio::test]
async fn compose_same_args_twice_same_result_id_and_bytes() {
    let srv = start().await;
    let args = json!({"width":100,"height":100,"elements":[{"asset_id":"test:neuron","x":5,"y":5,"scale":2,"rotation":0}]});
    let c1 = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"compose_svg","arguments":args.clone()}})).await;
    let d1: Value = serde_json::from_str(c1["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let c2 = call(&srv.client,&srv.url,&srv.session,json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"compose_svg","arguments":args}})).await;
    let d2: Value = serde_json::from_str(c2["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    let id1 = d1["result_id"].as_str().unwrap().to_string();
    let id2 = d2["result_id"].as_str().unwrap().to_string();
    assert_eq!(id1, id2, "identical args must yield the same result_id");
    let b1 = std::fs::read(srv.dir.path().join("results").join(format!("{id1}.svg"))).unwrap();
    let b2 = std::fs::read(srv.dir.path().join("results").join(format!("{id2}.svg"))).unwrap();
    assert_eq!(b1, b2, "stored bytes must be identical");
}
