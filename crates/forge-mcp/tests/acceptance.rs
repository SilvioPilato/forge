use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::Value;

fn temp_corpus_for_mcp() -> PathBuf {
    let src = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../fixtures/corpus"
    ));
    let dst = std::env::temp_dir().join("forge-mcp-acceptance-test");
    let _ = std::fs::remove_dir_all(&dst);
    copy_dir(&src, &dst).unwrap();
    dst
}

fn copy_dir(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if ty.is_dir() {
            copy_dir(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

fn empty_corpus_cwd() -> PathBuf {
    let dir = std::env::temp_dir().join("forge-mcp-empty-mode-test");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::create_dir_all(dir.join(".git")).unwrap();
    dir
}

struct MCPClient {
    child: Child,
    stdin: std::process::ChildStdin,
    reader: BufReader<std::process::ChildStdout>,
    next_id: u64,
}

impl MCPClient {
    fn new(config_path: &str) -> Self {
        let bin = env!("CARGO_BIN_EXE_forge-mcp");
        let mut child = Command::new(bin)
            .arg(config_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        MCPClient {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        }
    }

    fn new_empty(cwd: &Path) -> Self {
        let bin = env!("CARGO_BIN_EXE_forge-mcp");
        let mut child = Command::new(bin)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        MCPClient {
            child,
            stdin,
            reader: BufReader::new(stdout),
            next_id: 1,
        }
    }

    fn send(&mut self, method: &str, params: Value) -> Value {
        let id = self.next_id;
        self.next_id += 1;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).unwrap();
        self.stdin.flush().unwrap();

        let mut response = String::new();
        self.reader.read_line(&mut response).unwrap();
        serde_json::from_str(&response).unwrap()
    }

    fn initialize(&mut self) -> Value {
        self.send(
            "initialize",
            serde_json::json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0.1.0"}
            }),
        )
    }

    fn call_tool(&mut self, tool_name: &str, args: Value) -> Value {
        self.send(
            "tools/call",
            serde_json::json!({
                "name": tool_name,
                "arguments": args,
            }),
        )
    }
}

impl Drop for MCPClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

#[test]
fn acceptance_spec_10_e2e() {
    let corpus = temp_corpus_for_mcp();
    let config_path = corpus.join("forge.toml").to_string_lossy().to_string();

    let mut client = MCPClient::new(&config_path);

    let init_resp = client.initialize();
    assert!(
        init_resp.get("result").is_some(),
        "initialize failed: {}",
        init_resp
    );

    let propose_resp = client.call_tool(
        "propose_decision",
        serde_json::json!({
            "title": "Acceptance test decision",
            "body": "Testing the full MCP pipeline.",
            "forces": [
                {"title": "A brand new acceptance force", "force_new": false},
                {"existing_id": "f-rust-stable"}
            ]
        }),
    );
    let result = propose_resp.get("result").unwrap();
    assert!(result.get("content").is_some(), "propose_decision failed");

    let content = result["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    let proposed: Value = serde_json::from_str(text).unwrap();
    assert!(proposed
        .get("problems")
        .unwrap()
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!proposed["decision"]["id"].as_str().unwrap().is_empty());

    let commit_resp = client.call_tool(
        "commit",
        serde_json::json!({
            "proposed": proposed,
        }),
    );
    let commit_result = commit_resp.get("result").unwrap();
    assert!(commit_result.get("content").is_some());
    let commit_text = commit_result["content"][0]["text"].as_str().unwrap();
    let receipt: Value = serde_json::from_str(commit_text).unwrap();
    assert!(!receipt["decision_id"].as_str().unwrap().is_empty());

    let propose2_resp = client.call_tool(
        "propose_decision",
        serde_json::json!({
            "title": "Second acceptance test",
            "body": "Near duplicate force test.",
            "forces": [
                {"title": "A brand new acceptance force", "force_new": false}
            ]
        }),
    );
    let content2 = propose2_resp["result"]["content"].as_array().unwrap();
    let text2 = content2[0]["text"].as_str().unwrap();
    let proposed2: Value = serde_json::from_str(text2).unwrap();

    let commit2_resp = client.call_tool("commit", serde_json::json!({ "proposed": proposed2 }));
    let commit2_text = commit2_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let receipt2: Value = serde_json::from_str(commit2_text).unwrap();
    assert!(
        !receipt2["reused"].as_array().unwrap().is_empty(),
        "second commit should reuse near-dup force"
    );

    let new_force_id = proposed["new_forces"][0]["id"].as_str().unwrap();
    let status_resp = client.call_tool(
        "set_status",
        serde_json::json!({
            "id": new_force_id,
            "status": "changed",
        }),
    );
    let status_text = status_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let status_receipt: Value = serde_json::from_str(status_text).unwrap();
    assert!(!status_receipt["newly_stale"].as_array().unwrap().is_empty());

    let report_resp = client.call_tool("stale_report", serde_json::json!({}));
    let report_text = report_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let report: Value = serde_json::from_str(report_text).unwrap();
    assert!(!report["stale"].as_array().unwrap().is_empty());

    let why_resp = client.call_tool(
        "why",
        serde_json::json!({
            "id": receipt["decision_id"],
        }),
    );
    let why_text = why_resp["result"]["content"][0]["text"].as_str().unwrap();
    let why_result: Value = serde_json::from_str(why_text).unwrap();
    assert!(why_result.get("chain").is_some(), "why should return chain");
}

#[test]
fn acceptance_spec_11_empty_mode() {
    let cwd = empty_corpus_cwd();
    let mut client = MCPClient::new_empty(&cwd);

    let init_resp = client.initialize();
    assert!(
        init_resp.get("result").is_some(),
        "initialize failed in empty mode: {}",
        init_resp
    );

    let search_resp = client.call_tool(
        "search",
        serde_json::json!({"query": "anything", "scope": "both", "limit": 10}),
    );
    let search_text = search_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let search_result: Value = serde_json::from_str(search_text).unwrap();
    assert!(
        search_result["hits"].as_array().unwrap().is_empty(),
        "empty corpus should return no hits"
    );
    assert!(
        search_result.get("hint").is_some(),
        "empty-mode search should include a hint"
    );

    let propose_resp = client.call_tool(
        "propose_decision",
        serde_json::json!({"title": "x", "forces": []}),
    );
    let propose_text = propose_resp["result"]["content"][0]["text"]
        .as_str()
        .unwrap();
    let propose_result: Value = serde_json::from_str(propose_text).unwrap();
    assert!(
        propose_result.get("error").is_some(),
        "propose in empty mode should be refused"
    );
}
