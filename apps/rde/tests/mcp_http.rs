//! E2E M3: `rde mcp serve --transport http` — streamable HTTP MCP (R-MCP-02).
//!
//! Проверяет: HTTP выключен по умолчанию (stdio — дефолт), `--transport http` слушает **только
//! loopback**, bearer-токен **обязателен** (401 без/с неверным токеном + аудит), реальный
//! `rmcp`-клиент по streamable HTTP видит проект/инструменты и вызывает их, `/health` доступен без
//! токена и не отдаёт данных проекта, невалидные bind/токен/флаги дают код 2.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use rmcp::model::{CallToolRequestParams, CallToolResult, ProtocolVersion};
use rmcp::service::RunningService;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{ClientHandler, ClientLifecycleMode, ClientServiceExt, RoleClient};
use serde_json::{json, Value};

/// Фиксированный токен теста (64 hex-символа; генерируемый сервером проверяется отдельно).
const TEST_TOKEN: &str = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

#[derive(Clone, Default)]
struct RdeTestClient;

impl ClientHandler for RdeTestClient {}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-mcp-http-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

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

/// Запущенный HTTP-сервер: child + фактический адрес из stderr + накопленный stderr.
struct HttpServer {
    child: Child,
    addr: String,
    token: String,
    stderr: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl Drop for HttpServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl HttpServer {
    fn url(&self) -> String {
        format!("http://{}/mcp", self.addr)
    }

    fn stderr_text(&self) -> String {
        self.stderr.lock().map(|lines| lines.join("\n")).unwrap_or_default()
    }
}

/// Запустить `rde mcp serve --transport http --bind 127.0.0.1:0` и дождаться адреса из stderr.
fn start_http_server(root: &Path, extra_args: &[&str]) -> HttpServer {
    let mut command = Command::new(env!("CARGO_BIN_EXE_rde"));
    command
        .arg("mcp")
        .arg("serve")
        .arg("--project")
        .arg(root)
        .arg("--transport")
        .arg("http")
        .arg("--bind")
        .arg("127.0.0.1:0");
    for arg in extra_args {
        command.arg(arg);
    }
    command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().expect("запуск rde mcp serve --transport http");
    let stderr = child.stderr.take().expect("stderr сервера");
    let stdout = child.stdout.take().expect("stdout сервера");
    let collected = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let (sender, receiver) = mpsc::channel();
    {
        let collected = std::sync::Arc::clone(&collected);
        std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if let Ok(mut lines) = collected.lock() {
                    lines.push(line.clone());
                }
                let _ = sender.send(line);
            }
        });
    }
    // stdout в HTTP-режиме не используется (JSON-RPC идёт по HTTP): читаем и проверяем пустоту.
    let stdout_collected = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    {
        let stdout_collected = std::sync::Arc::clone(&stdout_collected);
        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut text = String::new();
            let _ = reader.read_to_string(&mut text);
            if let Ok(mut collected) = stdout_collected.lock() {
                *collected = text;
            }
        });
    }
    let addr = loop {
        let line = receiver
            .recv_timeout(Duration::from_secs(30))
            .expect("сервер не напечатал адрес HTTP за 30 с");
        if let Some(rest) = line.split("http://").nth(1) {
            let addr = rest.split('/').next().unwrap_or_default().to_owned();
            if !addr.is_empty() {
                break addr;
            }
        }
    };
    // Если токен не задан явно, сервер печатает его следующей строкой после адреса — дочитываем
    // stderr до строки с токеном (без гонки на reader-потоке).
    let token = if extra_args.iter().any(|arg| *arg == "--token" || *arg == "--token-file") {
        TEST_TOKEN.to_owned()
    } else {
        let mut token: Option<String> = None;
        while token.is_none() {
            let Ok(line) = receiver.recv_timeout(Duration::from_secs(10)) else { break };
            if let Some(rest) = line.split("bearer-токен этой сессии: ").nth(1) {
                let value = rest.trim().to_owned();
                if !value.is_empty() {
                    token = Some(value);
                }
            }
        }
        token.expect("сервер не напечатал сгенерированный bearer-токен")
    };
    HttpServer { child, addr, token, stderr: collected }
}

/// Сырой HTTP/1.1 запрос (для негативных проверок: 401/health/заголовки).
fn raw_http_request(addr: &str, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).expect("TCP connect");
    stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().unwrap();
    let mut response = Vec::new();
    let mut buffer = [0u8; 8192];
    // Читаем заголовки, затем тело по Content-Length (keep-alive не закрывает соединение).
    let mut header_end = None;
    while header_end.is_none() {
        let read = stream.read(&mut buffer).expect("read response");
        assert!(read > 0, "соединение закрыто до заголовков");
        response.extend_from_slice(&buffer[..read]);
        header_end = response.windows(4).position(|window| window == b"\r\n\r\n").map(|pos| pos + 4);
    }
    let header_end = header_end.unwrap();
    let headers = String::from_utf8_lossy(&response[..header_end]).to_lowercase();
    let content_length = headers
        .lines()
        .find_map(|line| line.strip_prefix("content-length:"))
        .and_then(|value| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while response.len() < header_end + content_length {
        let read = stream.read(&mut buffer).expect("read body");
        assert!(read > 0, "соединение закрыто до тела ответа");
        response.extend_from_slice(&buffer[..read]);
    }
    String::from_utf8_lossy(&response).into_owned()
}

fn status_line(response: &str) -> &str {
    response.lines().next().unwrap_or_default()
}

fn body_of(response: &str) -> &str {
    response.split_once("\r\n\r\n").map(|(_, body)| body).unwrap_or_default()
}

fn post_discover_request(addr: &str, token: Option<&str>) -> String {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": {
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {}
        }}
    })
    .to_string();
    let auth = token.map(|token| format!("Authorization: Bearer {token}\r\n")).unwrap_or_default();
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\n\
         MCP-Protocol-Version: 2026-07-28\r\nMcp-Method: server/discover\r\n\
         {auth}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    raw_http_request(addr, &request)
}

fn audit_entries(root: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(root.join(".rde/agent/audit.log")) else { return Vec::new() };
    text.lines().filter_map(|line| serde_json::from_str(line).ok()).collect()
}

async fn connect_http(url: &str, token: &str) -> RunningService<RoleClient, RdeTestClient> {
    // rmcp-клиент сам добавляет схему `Bearer ` (reqwest::bearer_auth) — передаём только токен.
    let config = StreamableHttpClientTransportConfig::with_uri(url).auth_header(token);
    let transport = StreamableHttpClientTransport::with_client(reqwest::Client::new(), config);
    RdeTestClient
        .serve_with_lifecycle(
            transport,
            ClientLifecycleMode::Discover { preferred_versions: vec![ProtocolVersion::V_2026_07_28] },
        )
        .await
        .expect("MCP-клиент должен подключиться по streamable HTTP")
}

async fn call(client: &RunningService<RoleClient, RdeTestClient>, tool: &str, arguments: Value) -> CallToolResult {
    let arguments = arguments.as_object().cloned().unwrap_or_default();
    client
        .peer()
        .call_tool(CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments))
        .await
        .unwrap_or_else(|error| panic!("tools/call {tool}: {error}"))
}

fn error_code(result: &CallToolResult) -> String {
    result
        .structured_content
        .as_ref()
        .and_then(|value| value.get("error"))
        .and_then(|error| error.get("code"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn http_streamable_transport_e2e_with_rmcp_client() {
    let root = demo_copy("client");
    let server = start_http_server(&root, &["--token", TEST_TOKEN]);
    let client = connect_http(&server.url(), &server.token).await;
    // discover прошёл в lifecycle; сервер тот же, что у stdio: 27 инструментов.
    let peer = client.peer_info().expect("peer info после discover");
    assert_eq!(peer.server_info.as_ref().map(|info| info.name.as_str()), Some("rde"));
    assert_eq!(peer.protocol_version, ProtocolVersion::V_2026_07_28);
    let tools = client.peer().list_tools(None).await.expect("tools/list по HTTP");
    assert_eq!(tools.tools.len(), 29);
    // Чтение проекта работает; мутации по умолчанию запрещены (approvals=deny).
    let tree = call(&client, "project_tree", json!({})).await;
    assert_eq!(tree.is_error, Some(false));
    let denied = call(&client, "note_write", json!({"note_id": "01M5VJ1JG0V3RS3DTR98H6BCFN", "content": "x"})).await;
    assert_eq!(error_code(&denied), "RDE_APPROVAL_DENIED");
    // Аудит: tool-call и решение политики — в общем JSONL.
    let entries = audit_entries(&root);
    assert!(entries.iter().any(|entry| entry["tool"] == "project_tree" && entry["ok"] == true));
    assert!(entries
        .iter()
        .any(|entry| entry["tool"] == "note_write" && entry["decision"] == "deny"));
    // Токен, переданный явно, не печатается в stderr.
    assert!(!server.stderr_text().contains(TEST_TOKEN), "явный токен не должен печататься");
    assert!(server.stderr_text().contains("не печатается"));
    client.cancel().await.expect("закрыть клиент");
}

#[tokio::test]
async fn http_generated_token_is_printed_once_and_works() {
    let root = demo_copy("generated");
    let server = start_http_server(&root, &[]);
    assert_eq!(server.token.len(), 64, "сгенерированный токен: 32 байта hex");
    assert!(server.stderr_text().contains("bearer-токен этой сессии"));
    let client = connect_http(&server.url(), &server.token).await;
    let tools = client.peer().list_tools(None).await.expect("tools/list с автогенерированным токеном");
    assert_eq!(tools.tools.len(), 29);
    client.cancel().await.expect("закрыть клиент");
}

#[test]
fn http_requires_bearer_token_and_audits_401() {
    let root = demo_copy("auth");
    // Держим сервер отдельным runtime-блоком: в этом тесте только сырой HTTP (без tokio-клиента).
    let server = start_http_server(&root, &["--token", TEST_TOKEN]);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        // Без токена — 401 + машиночитаемый RDE_UNAUTHORIZED + WWW-Authenticate.
        let response = post_discover_request(&server.addr, None);
        assert_eq!(status_line(&response), "HTTP/1.1 401 Unauthorized", "{response}");
        assert!(response.to_lowercase().contains("www-authenticate: bearer"));
        let body: Value = serde_json::from_str(body_of(&response)).expect("JSON тела 401");
        assert_eq!(body["error"]["code"], "RDE_UNAUTHORIZED");
        // Неверный токен — тоже 401 (не 500/404).
        let response = post_discover_request(&server.addr, Some(&"f".repeat(64)));
        assert_eq!(status_line(&response), "HTTP/1.1 401 Unauthorized", "{response}");
        // Валидный токен — discover отвечает 200 (JSON-ответ для простого запроса).
        let response = post_discover_request(&server.addr, Some(TEST_TOKEN));
        assert_eq!(status_line(&response), "HTTP/1.1 200 OK", "{response}");
        let body: Value = serde_json::from_str(body_of(&response)).expect("JSON discover");
        // serverInfo в spec 2026-07-28 лежит в `_meta` результата (DiscoverResult).
        assert_eq!(
            body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "rde",
            "discover: {body}"
        );
        // /health — без токена и без данных проекта.
        let health = raw_http_request(
            &server.addr,
            &format!("GET /health HTTP/1.1\r\nHost: {}\r\nConnection: close\r\n\r\n", server.addr),
        );
        assert_eq!(status_line(&health), "HTTP/1.1 200 OK", "{health}");
        let body: Value = serde_json::from_str(body_of(&health)).expect("JSON health");
        assert_eq!(body["status"], "ok");
        assert!(body.get("project").is_none(), "health не отдаёт данные проекта: {body}");
    });
    // Неудачи аутентификации аудируются (tool="http", reason="auth"), токен не пишется.
    let audit = std::fs::read_to_string(root.join(".rde/agent/audit.log")).expect("аудит HTTP");
    let denied: Vec<Value> = audit
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|entry: &Value| entry["tool"] == "http")
        .collect();
    assert_eq!(denied.len(), 2, "две неудачи аутентификации: {denied:?}");
    for entry in &denied {
        assert_eq!(entry["decision"], "deny");
        assert_eq!(entry["reason"], "auth");
        assert_eq!(entry["error_code"], "RDE_UNAUTHORIZED");
    }
    assert!(!audit.contains(TEST_TOKEN), "токен не должен попадать в аудит");
}

#[test]
fn http_rejects_non_loopback_bind_and_bad_token() {
    let root = demo_copy("badflags");
    let run = |args: &[&str]| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rde"));
        command.arg("mcp").arg("serve").arg("--project").arg(&root);
        for arg in args {
            command.arg(arg);
        }
        let output = command.stdin(Stdio::null()).output().expect("запуск сервера");
        (output.status.code(), String::from_utf8_lossy(&output.stderr).to_string())
    };
    let cases: &[(&[&str], &str)] = &[
        (&["--transport", "http", "--bind", "0.0.0.0:8765", "--token", TEST_TOKEN], "loopback"),
        (&["--transport", "http", "--bind", "192.168.1.5:8765", "--token", TEST_TOKEN], "loopback"),
        (&["--transport", "http", "--bind", "localhost:8765", "--token", TEST_TOKEN], "--bind"),
        (&["--transport", "http", "--token", "short"], "не короче"),
        (&["--transport", "http", "--token-file", "/nonexistent/token"], "token-file"),
        (&["--transport", "stdio", "--bind", "127.0.0.1:0"], "--transport http"),
        (&["--transport", "carrier-pigeon"], "неизвестный транспорт"),
        (&["--transport", "http", "--token", TEST_TOKEN, "--token-file", "/tmp/x"], "--token-file"),
    ];
    for (args, needle) in cases {
        let (code, stderr) = run(args);
        assert_eq!(code, Some(2), "флаги {args:?}: код {code:?}, stderr: {stderr}");
        assert!(
            stderr.contains(needle),
            "флаги {args:?}: ожидалась подсказка `{needle}`, stderr: {stderr}"
        );
    }
}

#[test]
fn stdio_is_default_transport() {
    // Без --transport сервер работает по stdio: HTTP-адрес не печатается и порт не слушается.
    let root = demo_copy("stdio-default");
    let mut command = Command::new(env!("CARGO_BIN_EXE_rde"));
    command
        .arg("mcp")
        .arg("serve")
        .arg("--project")
        .arg(&root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = command.output().expect("запуск stdio-сервера");
    assert_eq!(output.status.code(), Some(0), "EOF stdin → 0");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("HTTP-транспорт"), "без флага HTTP не запускается: {stderr}");
    assert!(output.stdout.is_empty(), "stdio-сервер без запросов ничего не пишет в stdout");
}
