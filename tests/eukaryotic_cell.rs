use bio_assets::{index::Library, models::Asset, server};
use serde_json::{Value, json};
use xmltree::Element;

/// Mirror of `tests/mcp_http.rs::wire_json` — private to that test binary.
/// Handles both plain JSON-RPC and SSE `data: ` lines from the streamable
/// HTTP MCP transport.
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

/// Mirror of `tests/mcp_http.rs::call` helper.
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

/// A running MCP server backed by a tempdir library holding three eukaryotic
/// cell assets (nucleus, mitochondrion, Golgi apparatus).  Aborts the server
/// task on drop.
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

/// Create a tempdir library, store the three eukaryotic-cell assets, bind a
/// random TCP port, start the MCP server, complete the initialisation handshake,
/// and return a `Server` ready for tool calls.
async fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let library = Library::new(dir.path());
    library.initialize().unwrap();

    let nucleus = Asset {
        id: "test:cell-nucleus".into(),
        source: "test".into(),
        upstream_key: "cell-nucleus".into(),
        name: "Nucleus".into(),
        category: "cell".into(),
        tags: vec!["nucleus".into(), "cell".into()],
        description: "The membrane-bound nucleus of a eukaryotic cell".into(),
        license: "CC0-1.0".into(),
        license_url: None,
        author: Some("Test Author".into()),
        attribution: "Test Author, CC0-1.0".into(),
        source_url: "https://example.org/nucleus".into(),
        source_revision: None,
    };
    library
        .store(
            &nucleus,
            b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 100'><ellipse cx='50' cy='50' rx='40' ry='30'/></svg>",
            &json!({}),
        )
        .unwrap();

    let mitochondrion = Asset {
        id: "test:cell-mitochondrion".into(),
        source: "test".into(),
        upstream_key: "cell-mitochondrion".into(),
        name: "Mitochondrion".into(),
        category: "organelle".into(),
        tags: vec!["mitochondrion".into(), "organelle".into(), "cell".into()],
        description: "Powerhouse of the cell".into(),
        license: "CC0-1.0".into(),
        license_url: None,
        author: Some("Test Author".into()),
        attribution: "Test Author, CC0-1.0".into(),
        source_url: "https://example.org/mitochondrion".into(),
        source_revision: None,
    };
    library
        .store(
            &mitochondrion,
            b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 120 60'><ellipse cx='60' cy='30' rx='50' ry='20'/><path d='M20 30 L100 30'/></svg>",
            &json!({}),
        )
        .unwrap();

    let golgi = Asset {
        id: "test:cell-golgi".into(),
        source: "test".into(),
        upstream_key: "cell-golgi".into(),
        name: "Golgi apparatus".into(),
        category: "organelle".into(),
        tags: vec!["golgi".into(), "organelle".into(), "cell".into()],
        description: "Protein packaging organelle".into(),
        license: "CC0-1.0".into(),
        license_url: None,
        author: Some("Test Author".into()),
        attribution: "Test Author, CC0-1.0".into(),
        source_url: "https://example.org/golgi".into(),
        source_revision: None,
    };
    library
        .store(
            &golgi,
            b"<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 100 80'><path d='M10 20 Q50 5 90 20'/><path d='M10 40 Q50 25 90 40'/><path d='M10 60 Q50 45 90 60'/></svg>",
            &json!({}),
        )
        .unwrap();

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    drop(listener);

    let task = tokio::spawn(server::serve(library, address));

    let client = reqwest::Client::new();
    let url = format!("http://{address}/mcp");

    // Wait for the server to become healthy.
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

    // MCP initialise handshake.
    let response = client
        .post(&url)
        .header("accept", "application/json, text/event-stream")
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-03-26",
                "capabilities": {},
                "clientInfo": { "name": "eukaryotic-cell-test", "version": "1" }
            }
        }))
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

    // notifications/initialized (required by the protocol).
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

/// End-to-end regression test for the eukaryotic-cell scenario:
///   1. Store nucleus + mitochondrion + Golgi assets
///   2. Assert tool list (2 tools: search_assets + compose_svg, no
///      get_asset/get_assets/get_composed)
///   3. search_assets for "organelle cell"
///   4. compose_svg → HTTP GET → XML parse
///   5. Determinism: same args → same result_id + bytes
#[tokio::test]
async fn eukaryotic_cell_end_to_end() {
    let srv = start().await;

    // ── 1. tools/list ──────────────────────────────────────────────────
    let tools_resp = call(
        &srv.client,
        &srv.url,
        &srv.session,
        json!({"jsonrpc":"2.0","id":10,"method":"tools/list"}),
    )
    .await;
    let tools = tools_resp["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2, "exactly 2 tools");
    let names: Vec<String> = tools
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    for expected in ["search_assets", "compose_svg"] {
        assert!(names.contains(&expected.to_string()), "missing tool {expected}; got {names:?}");
    }
    assert!(!names.contains(&"get_asset".to_string()), "get_asset must not be present");
    assert!(!names.contains(&"get_assets".to_string()), "get_assets must not be present");
    assert!(!names.contains(&"get_composed".to_string()), "get_composed must not be present");

    // ── 2. search_assets ───────────────────────────────────────────────
    let search_resp = call(
        &srv.client,
        &srv.url,
        &srv.session,
        json!({"jsonrpc":"2.0","id":11,"method":"tools/call","params":{
            "name":"search_assets",
            "arguments":{"query":"organelle cell"}
        }}),
    )
    .await;
    let search_text = search_resp["result"]["content"][0]["text"].as_str().unwrap();
    let search_data: Value = serde_json::from_str(search_text).unwrap();
    let search_array = search_data.as_array().unwrap();
    // The raw MCP response must not contain SVG markup.
    for marker in ["<svg", "<path", "<ellipse"] {
        assert!(
            !search_text.contains(marker),
            "search must not leak SVG markers — found {marker:?} in: {search_text}"
        );
    }
    // At least one result must be mitochondrion or golgi (both have "organelle" + "cell").
    let found_ids: Vec<&str> = search_array
        .iter()
        .filter_map(|a| a["id"].as_str())
        .collect();
    assert!(
        found_ids.contains(&"test:cell-mitochondrion") || found_ids.contains(&"test:cell-golgi"),
        "expected mitochondrion or golgi in search results; got {found_ids:?}"
    );

    // ── 3. compose_svg ─────────────────────────────────────────────────
    let compose_resp = call(
        &srv.client,
        &srv.url,
        &srv.session,
        json!({"jsonrpc":"2.0","id":13,"method":"tools/call","params":{
            "name":"compose_svg",
            "arguments":{
                "width":300.0,
                "height":200.0,
                "background":"#ffffff",
                "elements":[
                    {"asset_id":"test:cell-nucleus","x":95.0,"y":50.0,"scale":1.5,"rotation":0.0},
                    {"asset_id":"test:cell-mitochondrion","x":15.0,"y":130.0,"scale":1.0,"rotation":15.0},
                    {"asset_id":"test:cell-golgi","x":210.0,"y":10.0,"scale":1.2,"rotation":0.0}
                ]
            }
        }}),
    )
    .await;
    let compose_result = &compose_resp["result"];
    // No isError (or isError == false).
    assert!(
        compose_result.get("isError").is_none() || compose_result["isError"] == false,
        "compose must succeed: {}",
        compose_resp["result"]["content"][0]["text"]
    );
    let compose_text = compose_result["content"][0]["text"].as_str().unwrap();
    let compose_data: Value = serde_json::from_str(compose_text).unwrap();
    // compose_data IS the ComposeResult JSON (no outer "result" wrapper).

    let result_id = compose_data["result_id"].as_str().unwrap();
    let re = regex::Regex::new(r"^ba_comp_[0-9a-f]{16}$").unwrap();
    assert!(re.is_match(result_id), "result_id must match pattern: {result_id}");

    let expected_url = format!("/results/{result_id}.svg");
    assert_eq!(compose_data["url"].as_str().unwrap(), expected_url, "url must match");
    assert_eq!(compose_data["mime_type"].as_str().unwrap(), "image/svg+xml");
    assert_eq!(compose_data["width"].as_f64().unwrap(), 300.0);
    assert_eq!(compose_data["height"].as_f64().unwrap(), 200.0);
    let keys: std::collections::BTreeSet<&str> = compose_data
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
    assert_eq!(
        compose_data["status"].as_str(),
        Some("ok"),
        "status must be ok"
    );
    // MCP response must not contain SVG markup.
    for marker in ["<svg", "<path", "<symbol", "<use", "<ellipse", "viewBox=\""] {
        assert!(
            !compose_text.contains(marker),
            "compose response must not contain {marker:?}: {compose_text}"
        );
    }

    // ── 4. HTTP GET the composed SVG ───────────────────────────────────
    let root = srv.url.trim_end_matches("/mcp");
    let resp = srv
        .client
        .get(format!("{root}{expected_url}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "HTTP GET must return 200");
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(content_type, "image/svg+xml", "Content-Type must be image/svg+xml");
    let body_bytes = resp.bytes().await.unwrap();
    let body_str = std::str::from_utf8(&body_bytes).unwrap();

    // Parse as XML.
    let svg_root = Element::parse(body_str.as_bytes()).unwrap();
    assert_eq!(svg_root.name, "svg");

    // Exactly 3 <symbol> and 3 <use> referencing them.
    let sym_matches: Vec<(usize, &str)> = body_str.match_indices("<symbol id=\"").collect();
    assert_eq!(sym_matches.len(), 3, "exactly 3 <symbol> elements");
    let sym_ids: Vec<String> = sym_matches
        .iter()
        .map(|(i, _)| {
            let rest = &body_str[i + "<symbol id=\"".len()..];
            let end = rest.find('"').unwrap();
            rest[..end].to_string()
        })
        .collect();

    let use_matches: Vec<(usize, &str)> = body_str.match_indices("<use href=\"#").collect();
    assert_eq!(use_matches.len(), 3, "exactly 3 <use href=\"#\"> elements");
    let use_hrefs: Vec<String> = use_matches
        .iter()
        .map(|(i, _)| {
            let rest = &body_str[i + "<use href=\"".len()..];
            let end = rest.find('"').unwrap();
            // rest starts with "#" so the href value includes the hash
            rest[..end].to_string()
        })
        .collect();
    let resolved = use_hrefs
        .iter()
        .filter(|h| sym_ids.iter().any(|s| format!("#{s}") == **h))
        .count();
    assert_eq!(resolved, 3, "all 3 <use> hrefs must resolve to emitted symbol ids");

    // Per-asset SVG markers that survive normalization.
    assert!(body_str.contains("rx=\"40\""), "nucleus ellipse rx=40 must survive");
    assert!(body_str.contains("M20 30"), "mitochondrion path M20 30 must survive");
    assert!(
        body_str.contains("Q50 5 90 20"),
        "golgi path Q50 5 90 20 must survive"
    );
    assert!(
        body_str.contains("fill=\"#ffffff\""),
        "background rect must be present"
    );

    // ── 5. Determinism ─────────────────────────────────────────────────
    let compose_resp2 = call(
        &srv.client,
        &srv.url,
        &srv.session,
        json!({"jsonrpc":"2.0","id":14,"method":"tools/call","params":{
            "name":"compose_svg",
            "arguments":{
                "width":300.0,
                "height":200.0,
                "background":"#ffffff",
                "elements":[
                    {"asset_id":"test:cell-nucleus","x":95.0,"y":50.0,"scale":1.5,"rotation":0.0},
                    {"asset_id":"test:cell-mitochondrion","x":15.0,"y":130.0,"scale":1.0,"rotation":15.0},
                    {"asset_id":"test:cell-golgi","x":210.0,"y":10.0,"scale":1.2,"rotation":0.0}
                ]
            }
        }}),
    )
    .await;
    let compose_text2 = compose_resp2["result"]["content"][0]["text"].as_str().unwrap();
    let compose_data2: Value = serde_json::from_str(compose_text2).unwrap();
    let result_id2 = compose_data2["result_id"].as_str().unwrap();
    assert_eq!(result_id, result_id2, "same args must yield the same result_id");

    let resp2 = srv
        .client
        .get(format!("{root}/results/{result_id2}.svg"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp2.status(), 200);
    let body2_bytes = resp2.bytes().await.unwrap();
    assert_eq!(
        body_bytes.to_vec(),
        body2_bytes.to_vec(),
        "identical args must produce identical SVG bytes"
    );
}
