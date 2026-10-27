//! E2E M2: `rde mcp serve` по stdio с **настоящим** rmcp-клиентом (spec MCP 2026-07-28).
//!
//! Проверяет контракт ТЗ §10.3/§8.7: внешний MCP-клиент видит проект и каталог
//! **21 инструмента** (13 read-only M1 + 8 мутаций M2), получает машиночитаемые ошибки `RDE_*`,
//! а мутации проходят только через гранты/approvals/чекпоинты: по умолчанию (`--approval=deny`)
//! файлы не меняются, с `--approval=auto` — меняются с чекпоинтом в аудите; git-коммит и запуск
//! артефактов (job-модель) проверяются отдельно; все вызовы попадают в `.rde/agent/audit.log`,
//! stdout занят только JSON-RPC, EOF stdin завершает сервер штатно.
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

/// Минимальный клиент: сервер не использует server-initiated запросы.
#[derive(Clone, Default)]
struct RdeTestClient;

impl ClientHandler for RdeTestClient {}

/// Ожидаемый каталог M2.5: M1/M2-наборы не переставлены, новые инструменты — в конце.
const EXPECTED_TOOLS: [&str; 27] = [
    // M1 (read-only)
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
    // M2 (мутации + job-модель)
    "artifact_write_manifest",
    "artifact_write_script",
    "artifact_run",
    "run_status",
    "run_cancel",
    "run_log",
    "note_write",
    "git_commit",
    // M2.5 (документные suggested edits/правки/ревизии)
    "doc_suggest_edit",
    "suggestion_list",
    "suggestion_accept",
    "suggestion_reject",
    "doc_replace_range",
    "revision_list",
];

/// Мутирующие инструменты (readOnlyHint=false).
const MUTATING_TOOLS: [&str; 10] = [
    "artifact_write_manifest",
    "artifact_write_script",
    "artifact_run",
    "run_cancel",
    "note_write",
    "git_commit",
    "doc_suggest_edit",
    "suggestion_accept",
    "suggestion_reject",
    "doc_replace_range",
];

/// Деструктивные мутации (destructiveHint=true).
const DESTRUCTIVE_TOOLS: [&str; 8] = [
    "artifact_write_manifest",
    "artifact_write_script",
    "artifact_run",
    "run_cancel",
    "note_write",
    "git_commit",
    "suggestion_accept",
    "doc_replace_range",
];

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

fn std_command(root: &Path, extra_args: &[&str]) -> std::process::Command {
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_rde"));
    command.arg("mcp").arg("serve").arg("--project").arg(root);
    for arg in extra_args {
        command.arg(arg);
    }
    command
}

fn connect_command(root: &Path, extra_args: &[&str]) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_rde"));
    command.arg("mcp").arg("serve").arg("--project").arg(root);
    for arg in extra_args {
        command.arg(arg);
    }
    command
}

/// Подключиться к `rde mcp serve` через `server/discover` (stateless lifecycle 2026-07-28).
async fn connect_with(root: &Path, extra_args: &[&str]) -> RunningService<RoleClient, RdeTestClient> {
    let transport = TokioChildProcess::new(connect_command(root, extra_args)).expect("запуск rde mcp serve");
    RdeTestClient
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] },
        )
        .await
        .expect("MCP-сервер должен подключиться по stdio (server/discover)")
}

async fn connect(root: &Path) -> RunningService<RoleClient, RdeTestClient> {
    connect_with(root, &[]).await
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

fn audit_entries(root: &Path) -> Vec<Value> {
    let audit = std::fs::read_to_string(root.join(".rde/agent/audit.log")).expect("файл аудита создан");
    audit.lines().map(|line| serde_json::from_str(line).expect("валидный JSONL аудита")).collect()
}

async fn first_artifact_of_type(
    client: &RunningService<RoleClient, RdeTestClient>,
    kind: &str,
) -> Option<String> {
    let list = call(client, "artifact_list", json!({})).await;
    structured(&list)
        .as_array()
        .and_then(|artifacts| artifacts.iter().find(|artifact| artifact["artifact_type"] == kind))
        .and_then(|artifact| artifact["id"].as_str().map(str::to_owned))
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
    assert!(instructions.contains("Мутации (M2"), "инструкции описывают политику мутаций M2: {instructions}");
    assert!(peer.capabilities.tools.is_some());
    assert!(peer.capabilities.resources.is_some());

    // 2. tools/list: 21 инструмент, M1-порядок не переставлен, M2 — в конце.
    let tools = client.peer().list_tools(None).await.expect("tools/list");
    let names: Vec<&str> = tools.tools.iter().map(|tool| tool.name.as_ref()).collect();
    assert_eq!(names, EXPECTED_TOOLS);
    assert_eq!(tools.ttl_ms, Some(300_000));
    let definitions_size = serde_json::to_vec(&tools.tools).expect("сериализация tools").len();
    assert!(definitions_size <= 40 * 1024, "tools/list {definitions_size} байт > 40 KiB (бюджет R-AGT-08)");
    for tool in &tools.tools {
        let name = tool.name.as_ref();
        let annotations = tool.annotations.as_ref().expect("аннотации");
        assert!(tool.input_schema.contains_key("type"));
        assert!(tool.output_schema.is_some(), "{name} без outputSchema");
        if MUTATING_TOOLS.contains(&name) {
            assert_eq!(annotations.read_only_hint, Some(false), "{name}: readOnlyHint");
            assert_eq!(
                annotations.destructive_hint,
                Some(DESTRUCTIVE_TOOLS.contains(&name)),
                "{name}: destructiveHint"
            );
        } else {
            assert_eq!(annotations.read_only_hint, Some(true), "{name}: readOnlyHint");
            assert_eq!(annotations.destructive_hint, Some(false), "{name}: destructiveHint");
        }
    }

    // 3. workspace_current / project_tree.
    let workspace = call(&client, "workspace_current", json!({})).await;
    assert_eq!(workspace.is_error, Some(false));
    assert_eq!(structured(&workspace)["name"], "Демо: отчёт по НИР");
    assert_eq!(structured(&workspace)["object_count"], 12);
    let tree = call(&client, "project_tree", json!({})).await;
    assert_eq!(structured(&tree).as_array().map(Vec::len), Some(12));

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

    // 5. Артефакты: list → read (без запуска).
    let artifacts = call(&client, "artifact_list", json!({})).await;
    let artifacts = structured(&artifacts).as_array().expect("массив артефактов").clone();
    assert_eq!(artifacts.len(), 5);
    let artifact_id = artifacts[0]["id"].as_str().expect("id артефакта").to_owned();
    let detail = call(&client, "artifact_read", json!({"artifact_id": artifact_id})).await;
    assert_eq!(detail.is_error, Some(false));
    assert_eq!(structured(&detail)["info"]["id"], artifacts[0]["id"]);

    // 6. Источники/заметки/шаблоны/git (read-only). Копия вне git: ветки нет (branch=null).
    assert_eq!(structured(&call(&client, "source_list", json!({})).await).as_array().map(Vec::len), Some(3));
    assert_eq!(structured(&call(&client, "note_list", json!({})).await).as_array().map(Vec::len), Some(1));
    assert_eq!(structured(&call(&client, "template_list", json!({})).await).as_array().map(Vec::len), Some(3));
    let git = call(&client, "git_status", json!({})).await;
    let git = structured(&git);
    assert_eq!(git["available"], true);
    assert_eq!(git["is_repo"], false, "копия без .git — не репозиторий");
    assert!(git.get("branch").is_none() || git["branch"].is_null(), "без репозитория ветка null: {git}");

    // 7. file_read: успех + path-sandbox + большой файл без range.
    let file = call(&client, "file_read", json!({"path": "rde.toml"})).await;
    assert!(structured(&file)["text"].as_str().unwrap_or_default().contains("rde/project@1"));
    let escape = call(&client, "file_read", json!({"path": "../../../etc/passwd"})).await;
    assert_eq!(error_code(&escape), "RDE_PATH_ESCAPE");
    let dot_rde = call(&client, "file_read", json!({"path": ".rde/agent/audit.log"})).await;
    assert_eq!(error_code(&dot_rde), "RDE_APPROVAL_DENIED");

    // 8. project_search и лимиты.
    let search = call(&client, "project_search", json!({"query": "статистик"})).await;
    assert!(!structured(&search).as_array().expect("результаты поиска").is_empty());
    let too_many = call(&client, "project_search", json!({"query": "a", "limit": 1000})).await;
    assert_eq!(error_code(&too_many), "RDE_OUT_OF_RANGE");

    // 9. Неизвестный инструмент — ошибка JSON-RPC.
    let unknown = client.peer().call_tool(CallToolRequestParams::new("doc_insert")).await;
    assert!(unknown.is_err(), "неизвестный инструмент должен вернуть ошибку протокола");

    // 10. Ресурсы: list/templates/read.
    let resources = client.peer().list_resources(None).await.expect("resources/list");
    assert!(resources.ttl_ms.is_some());
    let uris: Vec<&str> = resources.resources.iter().map(|resource| resource.uri.as_str()).collect();
    assert!(uris.contains(&"rde://project/tree"));
    assert!(uris.contains(&"rde://templates"));
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

    // 11. Default deny: мутация отклоняется, файлы не меняются, аудит фиксирует deny.
    let artifact_id = artifacts[0]["id"].as_str().expect("id артефакта").to_owned();
    let manifest_path = root.join("objects/artifacts").join(&artifact_id).join("artifact.toml");
    let before = std::fs::read(&manifest_path).unwrap();
    let denied = call(
        &client,
        "artifact_write_manifest",
        json!({"artifact_id": artifact_id, "manifest": {"title": "Не должно примениться"}}),
    )
    .await;
    assert_eq!(denied.is_error, Some(true));
    assert_eq!(error_code(&denied), "RDE_APPROVAL_DENIED");
    assert_eq!(std::fs::read(&manifest_path).unwrap(), before, "deny не меняет файлы");
    let denied_run = call(&client, "artifact_run", json!({})).await;
    assert_eq!(error_code(&denied_run), "RDE_APPROVAL_DENIED");

    // 12. Аудит: JSONL, по строке на вызов, расширенные поля M2.
    let entries = audit_entries(&root);
    assert!(entries.len() >= 15, "аудит покрывает все вызовы: {}", entries.len());
    for entry in &entries {
        assert!(entry.get("tool").and_then(Value::as_str).is_some());
        assert!(entry.get("ts").and_then(Value::as_u64).is_some());
        assert_eq!(entry.get("params_sha256").and_then(Value::as_str).map(str::len), Some(64));
        assert!(entry.get("decision").and_then(Value::as_str).is_some());
        assert!(entry.get("ok").and_then(Value::as_bool).is_some());
        assert!(entry.get("duration_ms").and_then(Value::as_u64).is_some());
    }
    assert!(entries.iter().any(|entry| entry["tool"] == "project_tree" && entry["ok"] == true));
    let denied_entry = entries
        .iter()
        .find(|entry| entry["tool"] == "artifact_write_manifest" && entry["decision"] == "deny")
        .expect("аудит отказа мутации");
    assert_eq!(denied_entry["reason"], "policy", "причина отказа — политика");
    // Тела/аргументы в аудит не пишутся — только хеши.
    let audit_text = std::fs::read_to_string(root.join(".rde/agent/audit.log")).unwrap();
    assert!(!audit_text.contains("../../../etc/passwd"));

    // 13. EOF/завершение: клиент отменяет сервис, child завершается (graceful shutdown).
    client.cancel().await.expect("корректное завершение клиента");
}

#[tokio::test]
async fn approval_auto_applies_mutations_with_checkpoint_and_audit() {
    let root = demo_copy("auto");
    let client = connect_with(&root, &["--approval=auto"]).await;
    let artifacts = call(&client, "artifact_list", json!({})).await;
    let artifact_id = structured(&artifacts)
        .as_array()
        .and_then(|artifacts| artifacts.iter().find(|artifact| artifact["artifact_type"] == "compute"))
        .and_then(|artifact| artifact["id"].as_str())
        .expect("compute-артефакт")
        .to_owned();
    let manifest_path = root.join("objects/artifacts").join(&artifact_id).join("artifact.toml");
    let _before = std::fs::read(&manifest_path).unwrap();

    // 1) Запись манифеста с approval=auto: файл изменён, чекпоинт создан.
    let result = call(
        &client,
        "artifact_write_manifest",
        json!({"artifact_id": artifact_id, "manifest": {"title": "Заголовок из M2-агента"}}),
    )
    .await;
    assert_eq!(result.is_error, Some(false), "{result:?}");
    let outcome = structured(&result);
    assert_eq!(outcome["dry_run"], false);
    let checkpoint_id = outcome["checkpoint_id"].as_str().expect("checkpoint_id").to_owned();
    let manifest_text = std::fs::read_to_string(&manifest_path).unwrap();
    assert!(
        manifest_text.contains("Заголовок из M2-агента"),
        "манифест записан: {manifest_text}"
    );
    assert!(root.join(".rde/agent/checkpoints").join(&checkpoint_id).join("manifest.json").is_file());
    // `artifact_list` видит изменение (проект — файлы как источник истины).
    let list = call(&client, "artifact_list", json!({})).await;
    assert!(structured(&list)
        .as_array()
        .unwrap()
        .iter()
        .any(|artifact| artifact["title"] == "Заголовок из M2-агента"));
    // 2) Аудит: decision=allow + reason=allow_once + checkpoint_id.
    let entries = audit_entries(&root);
    let entry = entries
        .iter()
        .find(|entry| entry["tool"] == "artifact_write_manifest" && entry["decision"] == "allow")
        .expect("аудит разрешённой мутации");
    assert_eq!(entry["checkpoint_id"], checkpoint_id);
    assert_eq!(entry["reason"], "allow_once");

    // 3) note_write: содержимое заметки меняется.
    let notes = call(&client, "note_list", json!({})).await;
    let note = structured(&notes).as_array().and_then(|notes| notes.first()).cloned().expect("заметка");
    let note_id = note["id"].as_str().unwrap().to_owned();
    let note_path = root.join(note["file"].as_str().unwrap());
    let note_result = call(&client, "note_write", json!({"note_id": note_id, "content": "# M2 e2e\n"})).await;
    assert_eq!(note_result.is_error, Some(false), "{note_result:?}");
    assert!(std::fs::read_to_string(&note_path).unwrap().contains("M2 e2e"));

    // 4) dry_run не пишет.
    let dry = call(
        &client,
        "artifact_write_manifest",
        json!({"artifact_id": artifact_id, "manifest": {"title": "dry"}, "dry_run": true}),
    )
    .await;
    assert_eq!(structured(&dry)["dry_run"], true);
    assert!(!std::fs::read_to_string(&manifest_path).unwrap().contains("title = \"dry\""));

    client.cancel().await.expect("корректное завершение клиента");
}

#[tokio::test]
async fn approval_allowlist_limits_tools() {
    let root = demo_copy("allowlist");
    let client = connect_with(&root, &["--approval=allowlist", "--allow-tools=note_write"]).await;
    // note_write разрешён списком.
    let notes = call(&client, "note_list", json!({})).await;
    let note_id = structured(&notes).as_array().and_then(|notes| notes.first()).and_then(|note| note["id"].as_str()).unwrap().to_owned();
    let allowed = call(&client, "note_write", json!({"note_id": note_id, "content": "# allowlist\n"})).await;
    assert_eq!(allowed.is_error, Some(false), "{allowed:?}");
    // artifact_write_manifest не в списке — гранта нет.
    let artifacts = call(&client, "artifact_list", json!({})).await;
    let artifact_id = structured(&artifacts).as_array().and_then(|artifacts| artifacts.first()).and_then(|artifact| artifact["id"].as_str()).unwrap().to_owned();
    let denied = call(
        &client,
        "artifact_write_manifest",
        json!({"artifact_id": artifact_id, "manifest": {"title": "x"}}),
    )
    .await;
    assert_eq!(error_code(&denied), "RDE_APPROVAL_DENIED");
    client.cancel().await.expect("корректное завершение клиента");
}

#[tokio::test]
async fn git_commit_e2e_with_approval() {
    if Command::new("git").arg("--version").output().map(|output| !output.status.success()).unwrap_or(true) {
        eprintln!("git недоступен — e2e git_commit пропущен");
        return;
    }
    let root = demo_copy("git");
    let git = |args: &[&str]| {
        let status = Command::new("git").args(args).current_dir(&root).output().expect("git запускается");
        assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
    };
    git(&["init", "-q"]);
    git(&["config", "user.name", "E2E"]);
    git(&["config", "user.email", "e2e@rde.local"]);
    // Грант git_commit — отдельный (--allow-destructive, A4); project_write тоже нужен для мутации.
    let client = connect_with(&root, &["--approval=auto", "--allow-destructive"]).await;
    let notes = call(&client, "note_list", json!({})).await;
    let note_id = structured(&notes).as_array().and_then(|notes| notes.first()).and_then(|note| note["id"].as_str()).unwrap().to_owned();
    assert_eq!(call(&client, "note_write", json!({"note_id": note_id, "content": "# git e2e\n"})).await.is_error, Some(false));
    let commit = call(&client, "git_commit", json!({"message": "M2: e2e коммит агента"})).await;
    assert_eq!(commit.is_error, Some(false), "{commit:?}");
    let outcome = structured(&commit);
    assert!(outcome["commit"]["id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(outcome["checkpoint_id"].as_str().is_some());
    // В коммите нет `.rde/**`; рабочее дерево чистое.
    let tracked = Command::new("git").args(["ls-files"]).current_dir(&root).output().expect("git ls-files");
    let tracked = String::from_utf8_lossy(&tracked.stdout);
    assert!(!tracked.lines().any(|line| line.starts_with(".rde/")), "в коммите .rde: {tracked}");
    let git_status = call(&client, "git_status", json!({})).await;
    assert_eq!(structured(&git_status)["is_repo"], true);
    assert_eq!(structured(&git_status)["changed"].as_array().map(Vec::len), Some(0));
    // Без гранта git_commit недоступен (отдельный --approval=deny сервер).
    client.cancel().await.expect("корректное завершение клиента");
}

#[tokio::test]
async fn artifact_run_job_model_e2e_on_data_artifact() {
    // Запуск data-артефакта не требует uv/python (нет скрипта) — проверяем job-модель end-to-end.
    let root = demo_copy("run");
    let client = connect_with(&root, &["--approval=auto", "--allow-run"]).await;
    let data_artifact = first_artifact_of_type(&client, "data").await;
    let Some(data_artifact) = data_artifact else {
        eprintln!("в демо нет data-артефакта — e2e artifact_run пропущен");
        return;
    };
    let job = call(&client, "artifact_run", json!({"artifact_id": data_artifact})).await;
    assert_eq!(job.is_error, Some(false), "artifact_run: {job:?}");
    let info = structured(&job);
    let job_id = info["job_id"].as_str().expect("job_id").to_owned();
    assert!(info["checkpoint_id"].as_str().is_some(), "перед запуском создан чекпоинт");
    // Опрос до завершения (job асинхронный; обычно завершается за миллисекунды).
    let mut state = String::new();
    for _ in 0..200 {
        let status = call(&client, "run_status", json!({"job_id": job_id})).await;
        assert_eq!(status.is_error, Some(false), "run_status: {status:?}");
        state = structured(&status)["state"].as_str().unwrap_or_default().to_owned();
        if state != "running" {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    assert!(matches!(state.as_str(), "succeeded" | "failed"), "job не завершился: {state}");
    let log = call(&client, "run_log", json!({"job_id": job_id, "max_bytes": 1024})).await;
    assert_eq!(log.is_error, Some(false), "run_log: {log:?}");
    // Неизвестный job — машиночитаемая ошибка.
    let missing = call(&client, "run_status", json!({"job_id": "01NOSUCHJOB"})).await;
    assert_eq!(error_code(&missing), "RDE_JOB_NOT_FOUND");
    client.cancel().await.expect("корректное завершение клиента");
}

#[test]
fn stdout_is_jsonrpc_only_and_eof_shuts_down_server() {
    let root = demo_copy("raw");
    let mut child = std_command(&root, &[])
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
        json!({"jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": {"name": "artifact_run", "arguments": {}, "_meta": meta}}),
    ];
    for request in &requests {
        writeln!(stdin, "{request}").expect("запись JSON-RPC");
    }
    stdin.flush().unwrap();
    // Ответы могут прийти не в порядке запросов (tools/call выполняется параллельно) —
    // проверяем, что на stdout только JSON-RPC и есть ответ на каждый id.
    let mut responses = std::collections::BTreeMap::new();
    while responses.len() < requests.len() {
        let mut line = String::new();
        reader.read_line(&mut line).expect("чтение ответа");
        let value: Value = serde_json::from_str(line.trim()).unwrap_or_else(|error| {
            panic!("stdout должен содержать только JSON-RPC: {error}; строка: {line:?}")
        });
        assert_eq!(value["jsonrpc"], "2.0");
        let id = value["id"].as_i64().expect("id ответа");
        responses.insert(id, value);
    }
    for expected_id in 1..=4 {
        let value = responses.remove(&expected_id).unwrap_or_else(|| panic!("нет ответа id {expected_id}"));
        assert!(value.get("result").is_some() || value.get("error").is_some(), "ответ id {expected_id}: {value}");
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
    let audit = std::fs::read_to_string(root.join(".rde/agent/audit.log")).expect("аудит создан");
    assert!(audit.contains("artifact_run"), "отказ мутации в аудите: {audit}");
    assert!(audit.contains("\"decision\":\"deny\""), "default deny в аудите: {audit}");
}

#[test]
fn invalid_project_and_bad_approval_flags_exit_with_code_2() {
    let dir = temp_dir("missing-project");
    let output = std_command(&dir.join("nonexistent"), &[])
        .stdin(Stdio::null())
        .output()
        .expect("запуск rde mcp serve");
    assert_eq!(output.status.code(), Some(2), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    assert!(String::from_utf8_lossy(&output.stderr).contains("RDE MCP"));
    assert!(output.stdout.is_empty(), "stdout при ошибке проекта должен быть пустым");

    // Несогласованные флаги approvals — код 2 и понятная подсказка (без запуска сервера).
    let project = demo_copy("bad-flags");
    for args in [
        vec!["--approval=allowlist"],
        vec!["--approval=deny", "--allow-run"],
        vec!["--approval=auto", "--allow-tools=note_write"],
        vec!["--approval=unknown"],
    ] {
        let output = std_command(&project, &args)
            .stdin(Stdio::null())
            .output()
            .expect("запуск rde mcp serve");
        assert_eq!(
            output.status.code(),
            Some(2),
            "флаги {args:?}: stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn non_mcp_arguments_do_not_touch_mcp_path() {
    // `mcp` без `serve` даёт код 2 и подсказку (без запуска GUI).
    let output = Command::new(env!("CARGO_BIN_EXE_rde"))
        .arg("mcp")
        .arg("api")
        .stdin(Stdio::null())
        .output()
        .expect("запуск rde mcp api");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("mcp serve"));
}

// --- M2.5: suggested edits документа / правки блоков / ревизии (e2e) ---------

/// Мигрированный документ демо-проекта: `(doc_id, body_hash, относительный путь файла)`.
async fn migrated_document(
    client: &RunningService<RoleClient, RdeTestClient>,
) -> (String, String, String) {
    let documents = call(client, "doc_list", json!({})).await;
    let document = structured(&documents)
        .as_array()
        .and_then(|documents| documents.iter().find(|document| document["migrated"] == true))
        .expect("мигрированный документ демо-проекта")
        .clone();
    let doc_id = document["id"].as_str().expect("doc id").to_owned();
    let body_hash = document["body_hash"].as_str().expect("body_hash (M2.5)").to_owned();
    let file = document["file"].as_str().expect("file").to_owned();
    (doc_id, body_hash, file)
}

/// Блоки документа с полным текстом.
async fn document_blocks(
    client: &RunningService<RoleClient, RdeTestClient>,
    doc_id: &str,
) -> Vec<Value> {
    let body = call(
        client,
        "doc_read",
        json!({"doc_id": doc_id, "start": 0, "limit": 256}),
    )
    .await;
    structured(&body)["blocks"].as_array().cloned().unwrap_or_default()
}

/// Найти простой редактируемый абзац: `doc_replace_range` с `dry_run` — единственная безопасная
/// проверка редактируемости (вставки/поля/рисунки вернут RDE_UNSUPPORTED).
async fn editable_block(
    client: &RunningService<RoleClient, RdeTestClient>,
    doc_id: &str,
    skip: &[String],
) -> (String, String) {
    for block in document_blocks(client, doc_id).await {
        let id = block["id"].as_str().unwrap_or_default().to_owned();
        let text = block["text"].as_str().unwrap_or_default().to_owned();
        if skip.contains(&id) || text.chars().count() < 10 || text.chars().count() > 200 {
            continue;
        }
        let dry = call(
            client,
            "doc_replace_range",
            json!({"doc_id": doc_id, "block_id": id, "expected_text": text, "new_text": text, "dry_run": true}),
        )
        .await;
        if dry.is_error != Some(true) {
            return (id, text);
        }
    }
    panic!("в демо-документе нет простого редактируемого абзаца");
}

#[tokio::test]
async fn document_suggestions_edits_and_revisions_e2e() {
    let root = demo_copy("doc-m25");
    let client = connect_with(&root, &["--approval=auto"]).await;
    let (doc_id, body_hash, file) = migrated_document(&client).await;
    assert_eq!(body_hash.len(), 64, "body_hash — sha256 hex");
    let package = root.join(&file);
    let before_bytes = std::fs::read(&package).unwrap();
    let (block_id, block_text) = editable_block(&client, &doc_id, &[]).await;

    // 1) Предложение: документ НЕ меняется.
    let suggestion = call(
        &client,
        "doc_suggest_edit",
        json!({
            "doc_id": doc_id,
            "block_id": block_id,
            "expected_text": block_text,
            "new_text": "Текст, предложенный MCP-агентом",
            "note": "e2e M2.5"
        }),
    )
    .await;
    assert_eq!(suggestion.is_error, Some(false), "doc_suggest_edit: {suggestion:?}");
    let suggestion_info = structured(&suggestion).clone();
    let suggestion_id = suggestion_info["id"].as_str().expect("suggestion id").to_owned();
    assert_eq!(suggestion_info["status"], "pending");
    assert_eq!(std::fs::read(&package).unwrap(), before_bytes, "предложение не меняет документ");

    // 2) Список: pending-предложение видно.
    let list = call(&client, "suggestion_list", json!({"doc_id": doc_id, "status": "pending"})).await;
    assert_eq!(structured(&list).as_array().map(Vec::len), Some(1));
    assert_eq!(structured(&list)[0]["id"], suggestion_id);

    // 3) Принятие: чекпоинт + ревизия + правка документа.
    let accepted = call(&client, "suggestion_accept", json!({"suggestion_id": suggestion_id})).await;
    assert_eq!(accepted.is_error, Some(false), "suggestion_accept: {accepted:?}");
    let outcome = structured(&accepted).clone();
    let checkpoint_id = outcome["checkpoint_id"].as_str().expect("checkpoint_id").to_owned();
    let revision_id = outcome["revision_id"].as_str().expect("revision_id").to_owned();
    assert_eq!(outcome["after"], "Текст, предложенный MCP-агентом");
    assert!(root.join(".rde/agent/checkpoints").join(&checkpoint_id).join("manifest.json").is_file());
    let blocks = document_blocks(&client, &doc_id).await;
    let block = blocks.iter().find(|block| block["id"] == block_id.as_str()).expect("блок");
    assert_eq!(block["text"], "Текст, предложенный MCP-агентом");
    // Аудит: allow + checkpoint_id (R-AGT-05).
    let entries = audit_entries(&root);
    let audit_entry = entries
        .iter()
        .find(|entry| entry["tool"] == "suggestion_accept" && entry["decision"] == "allow")
        .expect("аудит применения предложения");
    assert_eq!(audit_entry["checkpoint_id"], checkpoint_id);
    // Ревизия видна и текущая.
    let revisions = call(&client, "revision_list", json!({"doc_id": doc_id})).await;
    let revisions = structured(&revisions).as_array().cloned().unwrap_or_default();
    assert!(revisions.iter().any(|revision| revision["revision_id"] == revision_id.as_str()
        && revision["current"] == true), "ревизии: {revisions:?}");

    // 4) Прямая правка: успех с актуальным body_hash, конфликты при устаревших ожиданиях.
    let documents = call(&client, "doc_list", json!({})).await;
    let current_hash = structured(&documents)
        .as_array()
        .and_then(|documents| documents.iter().find(|document| document["id"] == doc_id.as_str()))
        .and_then(|document| document["body_hash"].as_str())
        .expect("body_hash после правки")
        .to_owned();
    let direct = call(
        &client,
        "doc_replace_range",
        json!({
            "doc_id": doc_id,
            "block_id": block_id,
            "expected_text": "Текст, предложенный MCP-агентом",
            "new_text": "Прямая правка M2.5",
            "expected_body_hash": current_hash
        }),
    )
    .await;
    assert_eq!(direct.is_error, Some(false), "doc_replace_range: {direct:?}");
    let conflict = call(
        &client,
        "doc_replace_range",
        json!({
            "doc_id": doc_id,
            "block_id": block_id,
            "expected_text": "Текст, предложенный MCP-агентом",
            "new_text": "не применится"
        }),
    )
    .await;
    assert_eq!(error_code(&conflict), "RDE_CONFLICT", "устаревший expected_text");
    let conflict_hash = call(
        &client,
        "doc_replace_range",
        json!({
            "doc_id": doc_id,
            "block_id": block_id,
            "expected_text": "Прямая правка M2.5",
            "new_text": "не применится",
            "expected_body_hash": "0".repeat(64)
        }),
    )
    .await;
    assert_eq!(error_code(&conflict_hash), "RDE_CONFLICT", "устаревший body_hash");

    // 5) Отклонение предложения документ не меняет.
    let bytes_before_reject = std::fs::read(&package).unwrap();
    let second = call(
        &client,
        "doc_suggest_edit",
        json!({
            "doc_id": doc_id,
            "block_id": block_id,
            "expected_text": "Прямая правка M2.5",
            "new_text": "Отклоняемый вариант"
        }),
    )
    .await;
    assert_eq!(second.is_error, Some(false), "{second:?}");
    let second_id = structured(&second)["id"].as_str().unwrap().to_owned();
    let rejected = call(&client, "suggestion_reject", json!({"suggestion_id": second_id, "reason": "e2e"})).await;
    assert_eq!(rejected.is_error, Some(false), "{rejected:?}");
    assert_eq!(structured(&rejected)["status"], "rejected");
    assert_eq!(std::fs::read(&package).unwrap(), bytes_before_reject, "reject не меняет документ");
    // Повторное решение по обработанному предложению — RDE_CONFLICT.
    let repeated = call(&client, "suggestion_accept", json!({"suggestion_id": second_id})).await;
    assert_eq!(error_code(&repeated), "RDE_CONFLICT");

    client.cancel().await.expect("корректное завершение клиента");
}

#[tokio::test]
async fn default_deny_blocks_document_mutations_e2e() {
    let root = demo_copy("doc-deny");
    let client = connect(&root).await;
    let (doc_id, _body_hash, file) = migrated_document(&client).await;
    let package = root.join(&file);
    let before = std::fs::read(&package).unwrap();
    let block = document_blocks(&client, &doc_id)
        .await
        .into_iter()
        .find(|block| block["text"].as_str().map(str::len).unwrap_or(0) > 10)
        .expect("блок документа");
    let block_id = block["id"].as_str().unwrap().to_owned();
    let block_text = block["text"].as_str().unwrap().to_owned();

    let suggest = call(
        &client,
        "doc_suggest_edit",
        json!({"doc_id": doc_id, "block_id": block_id, "expected_text": block_text, "new_text": "x"}),
    )
    .await;
    assert_eq!(error_code(&suggest), "RDE_APPROVAL_DENIED");
    let replace = call(
        &client,
        "doc_replace_range",
        json!({"doc_id": doc_id, "block_id": block_id, "expected_text": block_text, "new_text": "x"}),
    )
    .await;
    assert_eq!(error_code(&replace), "RDE_APPROVAL_DENIED");
    let accept = call(&client, "suggestion_accept", json!({"suggestion_id": "01NOPE"})).await;
    assert_eq!(error_code(&accept), "RDE_APPROVAL_DENIED");
    // Read-only инструменты документа работают и ничего не меняют.
    let outline = call(&client, "doc_outline", json!({"doc_id": doc_id})).await;
    assert_eq!(outline.is_error, Some(false));
    assert_eq!(std::fs::read(&package).unwrap(), before, "default deny не меняет документ");
    // В аудите — deny по политике.
    let entries = audit_entries(&root);
    assert!(entries.iter().any(|entry| entry["tool"] == "doc_suggest_edit"
        && entry["decision"] == "deny"
        && entry["reason"] == "policy"));
    client.cancel().await.expect("корректное завершение клиента");
}
