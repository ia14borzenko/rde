//! E2E M1: `rde mcp serve` по stdio с **настоящим** rmcp-клиентом (spec MCP 2026-07-28).
//!
//! Проверяет контракт из ТЗ §10.3/§8.7: внешний MCP-клиент видит проект RDE, получает
//! стабильный `tools/list`, читает дерево/документ/артефакты/источники, получает
//! машиночитаемые ошибки `RDE_*`, все вызовы попадают в `.rde/agent/audit.log`, stdout занят
//! только JSON-RPC, EOF stdin завершает сервер штатно.
//!
//! Тесты работают на временной копии `demo-project/` без `.rde`/`exports`/`out`/`.venv`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ProtocolVersion, ReadResourceRequestParams, ResourceContents,
};
use rmcp::service::RunningService;
use rmcp::transport::TokioChildProcess;
use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt, RoleClient};
use serde_json::{json, Value};

/// Минимальный клиент: сервер не использует server-initiated запросы (M1 без notifications).
#[derive(Clone, Default)]
struct RdeTestClient;

impl ClientHandler for RdeTestClient {}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-mcp-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Рекурсивная копия проекта без локального состояния/выходов/окружений (свой helper).
fn copy_project(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(&*name, ".rde" | "exports" | "out" | "__pycache__" | ".venv" | ".git" | "target")
            || name.ends_with(".bak")
        {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&*name);
        if from.is_dir() {
            copy_project(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn demo_copy(name: &str) -> PathBuf {
    let root = temp_dir(name);
    copy_project(&rde_core::demo_project_dir(), &root);
    root
}

/// Подключиться к `rde mcp serve` через `server/discover` (stateless lifecycle 2026-07-28).
async fn connect(root: &Path) -> RunningService<RoleClient, RdeTestClient> {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rde"));
    command.arg("mcp").arg("serve").arg("--project").arg(root);
    let transport = TokioChildProcess::new(command).expect("запуск rde mcp serve");
    RdeTestClient
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] },
        )
        .await
        .expect("MCP-сервер должен подключиться по stdio (server/discover)")
}

async fn call(client: &RunningService<RoleClient, RdeTestClient>, tool: &str, arguments: Value) -> CallToolResult {
    let arguments = arguments.as_object().cloned().unwrap_or_default();
    client
        .peer()
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap_or_else(|error| panic!("tools/call {tool}: {error}"))
}

fn structured(result: &CallToolResult) -> &Value {
    result.structured_content.as_ref().unwrap_or_else(|| panic!("нет structuredContent: {result:?}"))
}

fn error_code(result: &CallToolResult) -> String {
    structured(result)
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn external_client_reads_project_document_artifacts_and_audits_calls() {
    let root = demo_copy("full");
    let client = connect(&root).await;

    // 1. server/discover уже прошёл в lifecycle; peer info содержит версию/инструкции сервера.
    let peer = client.peer_info().expect("peer info после discover");
    assert_eq!(peer.protocol_version, ProtocolVersion::V_2026_07_28);
    assert_eq!(peer.server_info.as_ref().map(|info| info.name.as_str()), Some("rde"));
    let instructions = peer.instructions.clone().expect("инструкции в discover");
    assert!(instructions.len() <= 32 * 1024, "инструкции {} байт", instructions.len());
    assert!(instructions.contains("RDE MCP"));
    assert!(peer.capabilities.tools.is_some());
    assert!(peer.capabilities.resources.is_some());

    // 2. tools/list: стабильный набор/порядок, только read-only.
    let tools = client.peer().list_tools(None).await.expect("tools/list");
    let names: Vec<&str> = tools.tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(
        names,
        vec![
            "workspace_current",
            "project_tree",
            "project_search",
            "file_read",
            "doc_list",
            "doc_outline",
            "doc_read",
            "artifact_list",
            "artifact_read",
            "source_list",
            "note_list",
            "template_list",
            "git_status",
        ]
    );
    assert_eq!(tools.ttl_ms, Some(300_000));
    for tool in &tools.tools {
        assert_eq!(tool.annotations.as_ref().and_then(|annotations| annotations.read_only_hint), Some(true));
        assert!(tool.input_schema.contains_key("type"));
    }

    // 3. workspace_current / project_tree.
    let workspace = call(&client, "workspace_current", json!({})).await;
    assert_eq!(workspace.is_error, Some(false));
    assert_eq!(structured(&workspace)["name"], "Демо: отчёт по НИР");
    assert_eq!(structured(&workspace)["object_count"], 12);

    let tree = call(&client, "project_tree", json!({})).await;
    let nodes = structured(&tree).as_array().expect("массив узлов дерева").clone();
    assert_eq!(nodes.len(), 12);
    assert!(nodes.iter().any(|node| node["kind"] == "document"));

    // 4. Документ: doc_list → doc_outline → doc_read по block_ids.
    let documents = call(&client, "doc_list", json!({})).await;
    let document = structured(&documents)[0].clone();
    let doc_id = document["id"].as_str().expect("id документа").to_owned();
    assert_eq!(document["migrated"], true);

    let outline = call(&client, "doc_outline", json!({"doc_id": doc_id})).await;
    let blocks = structured(&outline)["blocks"].as_array().expect("блоки оглавления").clone();
    assert!(!blocks.is_empty());
    let block_ids: Vec<Value> = blocks.iter().take(2).map(|block| block["id"].clone()).collect();
    let body = call(&client, "doc_read", json!({"doc_id": doc_id, "block_ids": block_ids})).await;
    assert_eq!(structured(&body)["returned_blocks"], 2);
    assert!(!structured(&body)["text"].as_str().unwrap_or_default().is_empty());

    // 5. Артефакты: list → read (без запуска).
    let artifacts = call(&client, "artifact_list", json!({})).await;
    let artifacts = structured(&artifacts).as_array().expect("массив артефактов").clone();
    assert_eq!(artifacts.len(), 5);
    let artifact_id = artifacts[0]["id"].as_str().expect("id артефакта").to_owned();
    let detail = call(&client, "artifact_read", json!({"artifact_id": artifact_id})).await;
    assert_eq!(detail.is_error, Some(false));
    assert_eq!(structured(&detail)["info"]["id"], artifacts[0]["id"]);

    // 6. Источники/заметки/шаблоны/git (read-only).
    assert_eq!(structured(&call(&client, "source_list", json!({})).await).as_array().map(Vec::len), Some(3));
    assert_eq!(structured(&call(&client, "note_list", json!({})).await).as_array().map(Vec::len), Some(1));
    assert_eq!(structured(&call(&client, "template_list", json!({})).await).as_array().map(Vec::len), Some(3));
    let git = call(&client, "git_status", json!({})).await;
    assert!(structured(&git)["available"].as_bool().unwrap_or(false));

    // 7. file_read: успех + path-sandbox (машиночитаемый код).
    let file = call(&client, "file_read", json!({"path": "rde.toml"})).await;
    assert!(structured(&file)["text"].as_str().unwrap_or_default().contains("rde/project@1"));
    let escape = call(&client, "file_read", json!({"path": "../../../etc/passwd"})).await;
    assert_eq!(escape.is_error, Some(true));
    assert_eq!(error_code(&escape), "RDE_PATH_ESCAPE");
    let dot_rde = call(&client, "file_read", json!({"path": ".rde/agent/audit.log"})).await;
    assert_eq!(error_code(&dot_rde), "RDE_APPROVAL_DENIED");

    // 8. project_search и лимиты.
    let search = call(&client, "project_search", json!({"query": "статистик"}))
        .await;
    assert!(!structured(&search).as_array().expect("результаты поиска").is_empty());
    let too_many = call(&client, "project_search", json!({"query": "a", "limit": 1000})).await;
    assert_eq!(error_code(&too_many), "RDE_OUT_OF_RANGE");

    // 9. Неизвестный инструмент — ошибка JSON-RPC (protocol error), а не результат.
    let unknown = client.peer().call_tool(CallToolRequestParams::new("artifact_run")).await;
    assert!(unknown.is_err(), "неизвестный инструмент должен вернуть ошибку протокола");

    // 10. Ресурсы: list/templates/read.
    let resources = client.peer().list_resources(None).await.expect("resources/list");
    assert!(resources.ttl_ms.is_some());
    let uris: Vec<&str> = resources.resources.iter().map(|resource| resource.uri.as_str()).collect();
    assert!(uris.contains(&"rde://project/tree"));
    assert!(uris.contains(&"rde://templates"));
    assert!(uris.contains(&"rde://doc/01M5VJ1JG04TFF59TDWH9EDD1R/outline"));
    let templates = client.peer().list_resource_templates(None).await.expect("resources/templates/list");
    assert_eq!(templates.resource_templates.len(), 2);
    let tree_resource = client
        .peer()
        .read_resource(ReadResourceRequestParams::new("rde://project/tree"))
        .await
        .expect("resources/read rde://project/tree");
    let tree_value: Value = match &tree_resource.contents[0] {
        ResourceContents::TextResourceContents { text, mime_type, .. } => {
            assert_eq!(mime_type.as_deref(), Some("application/json"));
            serde_json::from_str(text).expect("JSON в ресурсе")
        }
        other => panic!("ожидался текстовый ресурс, получено {other:?}"),
    };
    assert_eq!(tree_value.as_array().map(Vec::len), Some(12));
    let missing_resource = client.peer().read_resource(ReadResourceRequestParams::new("rde://nope")).await;
    assert!(missing_resource.is_err(), "неизвестный ресурс — ошибка ресурса");

    // 11. Аудит: JSONL в .rde/agent/audit.log, по строке на вызов, без тел аргументов.
    let audit_path = root.join(".rde/agent/audit.log");
    let audit = std::fs::read_to_string(&audit_path).expect("файл аудита создан");
    let entries: Vec<Value> = audit
        .lines()
        .map(|line| serde_json::from_str(line).expect("валидная JSONL-строка аудита"))
        .collect();
    assert!(entries.len() >= 12, "аудит должен покрывать все вызовы: {} строк", entries.len());
    for entry in &entries {
        assert!(entry.get("tool").and_then(Value::as_str).is_some());
        assert!(entry.get("ts").and_then(Value::as_u64).is_some());
        assert_eq!(entry.get("params_sha256").and_then(Value::as_str).map(str::len), Some(64));
        assert!(entry.get("decision").and_then(Value::as_str).is_some());
        assert!(entry.get("ok").and_then(Value::as_bool).is_some());
        assert!(entry.get("duration_ms").and_then(Value::as_u64).is_some());
    }
    assert!(entries.iter().any(|entry| entry["tool"] == "project_tree" && entry["ok"] == true));
    assert!(entries
        .iter()
        .any(|entry| entry["tool"] == "artifact_run" && entry["decision"] == "deny"));
    // Тела/аргументы в аудит не пишутся — только хеши.
    assert!(!audit.contains("../../../etc/passwd"), "аргументы не должны попадать в аудит");

    // 12. EOF/завершение: клиент отменяет сервис, child завершается (graceful shutdown).
    client.cancel().await.expect("корректное завершение клиента");
}

#[test]
fn stdout_is_jsonrpc_only_and_eof_shuts_down_server() {
    let root = demo_copy("raw");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rde"))
        .arg("mcp")
        .arg("serve")
        .arg("--project")
        .arg(&root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("запуск rde mcp serve");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let mut reader = BufReader::new(stdout);
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    let requests = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "server/discover", "params": {"_meta": meta}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {"_meta": meta}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "project_tree", "arguments": {}, "_meta": meta}}),
    ];
    for request in &requests {
        writeln!(stdin, "{request}").expect("запись JSON-RPC");
    }
    stdin.flush().unwrap();
    for expected_id in 1..=3 {
        let mut line = String::new();
        reader.read_line(&mut line).expect("чтение ответа");
        let value: Value = serde_json::from_str(line.trim()).unwrap_or_else(|error| {
            panic!("stdout должен содержать только JSON-RPC (id {expected_id}): {error}; строка: {line:?}")
        });
        assert_eq!(value["jsonrpc"], "2.0");
        assert_eq!(value["id"], expected_id);
    }
    // EOF stdin: сервер обязан выйти сам, штатно и быстро.
    drop(stdin);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let status = child.wait();
        let _ = sender.send(status);
    });
    let status = receiver
        .recv_timeout(Duration::from_secs(15))
        .expect("сервер не завершился по EOF за 15 с")
        .expect("wait на процесс");
    assert!(status.success(), "EOF-завершение должно быть кодом 0, получено {status:?}");
    assert!(root.join(".rde/agent/audit.log").is_file(), "tools/call должен попасть в аудит");
}

#[test]
fn invalid_project_exits_with_code_2_and_clear_stderr() {
    let dir = temp_dir("missing-project");
    let output = Command::new(env!("CARGO_BIN_EXE_rde"))
        .arg("mcp")
        .arg("serve")
        .arg("--project")
        .arg(dir.join("nonexistent"))
        .stdin(Stdio::null())
        .output()
        .expect("запуск rde mcp serve");
    assert_eq!(output.status.code(), Some(2), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("RDE MCP"), "понятная ошибка в stderr: {stderr}");
    assert!(output.stdout.is_empty(), "stdout при ошибке проекта должен быть пустым");
}

#[test]
fn non_mcp_arguments_do_not_touch_mcp_path() {
    // `--help` в MCP-ветке невозможен: команда не `mcp` — ветка не активируется;
    // проверяем только, что `mcp` без `serve` даёт код 2 и подсказку (без запуска GUI).
    let output = Command::new(env!("CARGO_BIN_EXE_rde"))
        .arg("mcp")
        .arg("api")
        .stdin(Stdio::null())
        .output()
        .expect("запуск rde mcp api");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("mcp serve"), "подсказка использования: {stderr}");
}
