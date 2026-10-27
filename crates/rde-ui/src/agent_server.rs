//! Встроенный headless MCP-сервер по streamable HTTP для внешних агентов (M4.1/M4.2, R-MCP-02).
//!
//! GUI — точка включения «Разрешить внешним агентам»: пользователь ставит галочку в настройках, RDE
//! поднимает **фоновый** MCP-сервер на **loopback** с **обязательным bearer-токеном** и показывает
//! адрес/токен в панели «Агент», чтобы их можно было скопировать во внешний клиент (Claude Code и др.).
//!
//! Режимы (M4.2):
//! - по умолчанию — **только чтение** (`approvals=deny`) — M4.1;
//! - при включённой настройке «Разрешить агенту изменения» — грант `project_write` есть, но **каждое**
//!   действие проходит через GUI-диалог [`GuiApprovalHook`] (разрешить один раз / в сессии / отклонить),
//!   с превью аргументов («было/станет»), чекпоинтом и аудитом; `run_script`/`git_commit` из GUI пока
//!   выключены (M4.2b). Решение пользователя — единственный источник `allow` (fail-closed: таймаут/
//!   закрытие → [`ApprovalDecision::Deny`]).
//!
//! Сервер живёт в фоновом потоке ([`rde_mcp::start_http_background`]) и останавливается при
//! выключении настройки, смене проекта или выходе из RDE (`Drop`); ожидающие approvals при остановке
//! получают `Deny`.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rde_host::HeadlessHost;
use rde_mcp::{
    ApprovalDecision, ApprovalHook, ApprovalRequest, HostHooks, HttpOptions, HttpServerHandle, HttpServerInfo,
    McpPolicy,
};

/// Сколько ждать решения пользователя, прежде чем отказать (fail-closed), по умолчанию.
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(300);

/// Запрос агента, ожидающий решения пользователя в GUI (M4.2).
pub struct PendingApproval {
    /// Порядковый номер (для UI/тестов).
    pub id: u64,
    /// Имя инструмента (`note_write`, `doc_replace_range`, …).
    pub tool: String,
    /// Краткая выжимка аргументов (без тел; для заголовка диалога).
    pub args_summary: String,
    /// Полные аргументы — для превью «было/станет» в диалоге (в аудит не попадают).
    pub args: Option<serde_json::Value>,
    /// Действие деструктивно (перезапись/удаление/запуск).
    pub destructive: bool,
    /// Кто просит (`mcp`/`agent`).
    pub requested_by: String,
    reply: std::sync::mpsc::SyncSender<ApprovalDecision>,
    decided: bool,
}

impl PendingApproval {
    /// Имя инструмента для UI.
    pub fn tool(&self) -> &str {
        &self.tool
    }

    /// Отдать решение серверу (идемпотентно; после таймаута/закрытия — игнорируется).
    pub fn decide(&mut self, decision: ApprovalDecision) {
        if self.decided {
            return;
        }
        self.decided = true;
        let _ = self.reply.send(decision);
    }

    /// Превью изменения для диалога: `(было, станет)` для текстовых правок.
    ///
    /// Извлекает `expected_text`/`new_text` (правка блока), `before`/`after` (предложение),
    /// `content`/`text` (заметка/абзац — «было» неизвестно) из аргументов инструмента.
    pub fn preview(&self) -> Option<(Option<String>, String)> {
        let args = self.args.as_ref()?;
        if let Some(new_text) = args.get("new_text").and_then(serde_json::Value::as_str) {
            let old = args.get("expected_text").and_then(serde_json::Value::as_str).map(str::to_owned);
            return Some((old, new_text.to_owned()));
        }
        if let Some(after) = args.get("after").and_then(serde_json::Value::as_str) {
            let before = args.get("before").and_then(serde_json::Value::as_str).map(str::to_owned);
            return Some((before, after.to_owned()));
        }
        if let Some(content) = args.get("content").and_then(serde_json::Value::as_str) {
            return Some((None, content.to_owned()));
        }
        if let Some(text) = args.get("text").and_then(serde_json::Value::as_str) {
            return Some((None, text.to_owned()));
        }
        None
    }

    /// Краткая JSON-выжимка аргументов (строки обрезаются) — для диалога.
    pub fn args_json_summary(&self) -> String {
        let Some(args) = &self.args else { return self.args_summary.clone() };
        let mut value = args.clone();
        truncate_json_strings(&mut value, 240);
        let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| self.args_summary.clone());
        if text.chars().count() > 4000 {
            format!("{}…", text.chars().take(4000).collect::<String>())
        } else {
            text
        }
    }
}

fn truncate_json_strings(value: &mut serde_json::Value, max_chars: usize) {
    match value {
        serde_json::Value::String(text) => {
            if text.chars().count() > max_chars {
                *text = format!("{}…", text.chars().take(max_chars).collect::<String>());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                truncate_json_strings(item, max_chars);
            }
        }
        serde_json::Value::Object(map) => {
            for item in map.values_mut() {
                truncate_json_strings(item, max_chars);
            }
        }
        _ => {}
    }
}

/// Очередь GUI-approvals: сервер кладёт запросы, главный поток (egui) показывает диалог и отвечает.
pub struct GuiApprovals {
    queue: Mutex<VecDeque<PendingApproval>>,
    next_id: AtomicU64,
}

impl Default for GuiApprovals {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for GuiApprovals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuiApprovals")
            .field("pending", &self.pending_count())
            .field("next_id", &self.next_id.load(Ordering::Relaxed))
            .finish()
    }
}

impl GuiApprovals {
    /// Новая пустая очередь.
    pub fn new() -> Self {
        Self { queue: Mutex::new(VecDeque::new()), next_id: AtomicU64::new(1) }
    }

    fn lock_queue(&self) -> std::sync::MutexGuard<'_, VecDeque<PendingApproval>> {
        match self.queue.lock() {
            Ok(queue) => queue,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Поставить запрос в очередь (вызывается из approval-хука и тестов); возвращает канал решения.
    pub(crate) fn enqueue(&self, request: ApprovalRequest) -> std::sync::mpsc::Receiver<ApprovalDecision> {
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let pending = PendingApproval {
            id,
            tool: request.tool,
            args_summary: request.args_summary,
            args: request.args,
            destructive: request.destructive,
            requested_by: request.requested_by,
            reply: sender,
            decided: false,
        };
        self.lock_queue().push_back(pending);
        receiver
    }

    /// Взять самый старый запрос для показа диалога (если есть).
    pub fn take_pending(&self) -> Option<PendingApproval> {
        self.lock_queue().pop_front()
    }

    /// Запросы, ещё не показанные пользователю.
    pub fn pending_count(&self) -> usize {
        self.lock_queue().len()
    }

    /// Отклонить все ожидающие запросы (остановка сервера/выход/выключение доступа — fail-closed).
    pub fn deny_all(&self) {
        let mut queue = self.lock_queue();
        for mut pending in queue.drain(..) {
            pending.decide(ApprovalDecision::Deny);
        }
    }
}

/// In-process `ApprovalHook` для GUI: ждёт решения пользователя (диалог egui), иначе `Deny`.
#[derive(Clone)]
pub struct GuiApprovalHook {
    approvals: Arc<GuiApprovals>,
    timeout: Duration,
}

impl GuiApprovalHook {
    /// Хук с таймаутом ожидания решения (по умолчанию [`APPROVAL_TIMEOUT`]).
    pub fn new(approvals: Arc<GuiApprovals>) -> Self {
        Self { approvals, timeout: APPROVAL_TIMEOUT }
    }

    /// Хук с явным таймаутом (тесты).
    pub fn with_timeout(approvals: Arc<GuiApprovals>, timeout: Duration) -> Self {
        Self { approvals, timeout }
    }
}

#[async_trait]
impl ApprovalHook for GuiApprovalHook {
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision {
        let receiver = self.approvals.enqueue(request);
        let timeout = self.timeout;
        // Ждём в blocking-пуле: tokio-воркеры сервера не блокируем (диалог может быть открыт долго).
        tokio::task::spawn_blocking(move || receiver.recv_timeout(timeout).unwrap_or(ApprovalDecision::Deny))
            .await
            // Паника/отмена blocking-задачи — fail-closed.
            .unwrap_or(ApprovalDecision::Deny)
    }
}

/// Параметры запуска встроенного сервера (M4.2).
#[derive(Clone)]
pub struct ExternalAgentOptions {
    /// Разрешить агенту изменять проект (каждое действие — через GUI-диалог approvals).
    pub allow_mutations: bool,
    /// Очередь GUI-approvals (общая с UI).
    pub approvals: Arc<GuiApprovals>,
}

impl std::fmt::Debug for ExternalAgentOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalAgentOptions")
            .field("allow_mutations", &self.allow_mutations)
            .finish_non_exhaustive()
    }
}

/// Состояние внешнего MCP-доступа для UI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExternalAgentStatus {
    /// Доступ выключен (пользователь не разрешил внешних агентов).
    Disabled,
    /// Разрешён, но сервер ещё/уже не запущен (например, ошибка запуска).
    NotRunning,
    /// Сервер работает: адрес/токен для подключения клиента.
    Running(HttpServerInfo),
}

impl ExternalAgentStatus {
    /// Краткая подпись для панели «Агент».
    pub fn label(&self) -> String {
        match self {
            ExternalAgentStatus::Disabled => "не подключён".to_owned(),
            ExternalAgentStatus::NotRunning => "MCP HTTP: остановлен".to_owned(),
            ExternalAgentStatus::Running(info) => format!("MCP HTTP: {}", info.addr),
        }
    }

    /// Сервер работает.
    pub fn is_running(&self) -> bool {
        matches!(self, ExternalAgentStatus::Running(_))
    }
}

/// Менеджер встроенного MCP-сервера (фоновый streamable HTTP) для GUI.
#[derive(Default)]
pub struct ExternalAgentServer {
    root: Option<PathBuf>,
    handle: Option<HttpServerHandle>,
    last_error: Option<String>,
    mutations_enabled: bool,
    approvals: Option<Arc<GuiApprovals>>,
}

impl ExternalAgentServer {
    /// Новый менеджер (ничего не запущено).
    pub fn new() -> Self {
        Self::default()
    }

    /// Текущее состояние для UI.
    pub fn status(&self, enabled: bool) -> ExternalAgentStatus {
        if let Some(handle) = &self.handle {
            return ExternalAgentStatus::Running(handle.info().clone());
        }
        if enabled {
            ExternalAgentStatus::NotRunning
        } else {
            ExternalAgentStatus::Disabled
        }
    }

    /// Корень проекта, для которого поднят сервер.
    pub fn project_root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// Последняя ошибка запуска (для панели/журнала).
    pub fn last_error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// Сервер работает.
    pub fn is_running(&self) -> bool {
        self.handle.is_some()
    }

    /// Мутации разрешены (через GUI-диалог approvals).
    pub fn mutations_enabled(&self) -> bool {
        self.mutations_enabled
    }

    /// Запустить фоновый MCP HTTP для проекта `root` (если уже запущен — ничего не делает).
    ///
    /// Без `options.allow_mutations` политика read-only (`approvals=deny`). С мутациями — грант
    /// `project_write` и GUI-диалог на каждое действие ([`GuiApprovalHook`]); `run_script`/
    /// `git_commit` из GUI пока выключены (M4.2b). Auth — bearer-токен, `bind=127.0.0.1:0`.
    ///
    /// # Errors
    ///
    /// Строка с причиной (проект не открывается / сервер не стартовал).
    pub fn start(&mut self, root: &Path, options: &ExternalAgentOptions) -> Result<(), String> {
        if self.handle.is_some() {
            return Ok(());
        }
        let host = match HeadlessHost::open(root) {
            Ok(host) => host,
            Err(error) => {
                let message = format!("не удалось открыть проект для MCP-сервера: {error}");
                self.last_error = Some(message.clone());
                return Err(message);
            }
        };
        let approvals: Arc<dyn ApprovalHook> = if options.allow_mutations {
            Arc::new(GuiApprovalHook::new(Arc::clone(&options.approvals)))
        } else {
            Arc::new(rde_mcp::DenyAllHook)
        };
        let hooks = HostHooks::jsonl(host.root()).with_approvals(approvals);
        let mut policy = McpPolicy::read_only();
        if options.allow_mutations {
            // Грант на запись в проект; каждое действие подтверждается GUI-диалогом.
            policy.allow_project_write = true;
            policy.approval_mode = "gui".to_owned();
        }
        let http_options = HttpOptions {
            bind: "127.0.0.1:0".to_owned(),
            token: None,
            // GUI показывает адрес/токен в панели «Агент»; Ctrl+C обрабатывает приложение.
            announce: false,
            handle_ctrl_c: false,
        };
        match rde_mcp::start_http_background(std::sync::Arc::new(host), hooks, policy, http_options) {
            Ok(handle) => {
                self.root = Some(root.to_path_buf());
                self.handle = Some(handle);
                self.mutations_enabled = options.allow_mutations;
                self.approvals = Some(Arc::clone(&options.approvals));
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                Err(error.to_string())
            }
        }
    }

    /// Остановить сервер (идемпотентно) и закрыть порт; ожидающие approvals получают `Deny`.
    pub fn stop(&mut self) {
        if let Some(approvals) = self.approvals.take() {
            approvals.deny_all();
        }
        if let Some(mut handle) = self.handle.take() {
            handle.stop();
        }
        self.root = None;
        self.mutations_enabled = false;
    }

    /// Применить настройку «Разрешить внешним агентам» для проекта `root` (вкл/выкл).
    ///
    /// Если сервер уже запущен, при изменении режима (read-only ↔ мутации) он перезапускается —
    /// политика и хук фиксируются на старте сервера.
    ///
    /// Возвращает `Err` только при включении, когда сервер не удалось поднять (настройка при этом
    /// остаётся выключенной в UI — решает вызывающий слой).
    pub fn apply(&mut self, enabled: bool, root: &Path, options: &ExternalAgentOptions) -> Result<(), String> {
        if !enabled {
            self.stop();
            return Ok(());
        }
        if self.handle.is_some() && self.mutations_enabled == options.allow_mutations {
            return Ok(());
        }
        self.stop();
        self.start(root, options)
    }

    /// Сменить проект: остановить сервер и (если включён) поднять для нового корня.
    pub fn on_project_switched(
        &mut self,
        enabled: bool,
        root: &Path,
        options: &ExternalAgentOptions,
    ) -> Result<(), String> {
        self.stop();
        if enabled {
            self.start(root, options)
        } else {
            Ok(())
        }
    }
}

impl Drop for ExternalAgentServer {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-ui-agent-server-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        rde_core::create_project(&dir, "Тестовый проект").expect("проект создаётся");
        // Заметка для проверки мутации через approval.
        rde_core::create_object(
            &dir,
            &rde_core::CreateRequest { kind: rde_core::NewObjectKind::Note, title: "Заметка агента".to_owned() },
        )
        .expect("заметка создаётся");
        dir
    }

    fn options(approvals: Arc<GuiApprovals>, allow_mutations: bool) -> ExternalAgentOptions {
        ExternalAgentOptions { allow_mutations, approvals }
    }

    /// Сырой HTTP-запрос: полный ответ.
    fn raw_http(addr: std::net::SocketAddr, request: &str) -> String {
        use std::io::{Read, Write};
        let mut stream =
            std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)).expect("TCP connect");
        stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        stream.write_all(request.as_bytes()).expect("запрос");
        stream.flush().unwrap();
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        String::from_utf8_lossy(&response).into_owned()
    }

    fn mcp_post(addr: std::net::SocketAddr, token: &str, method: &str, params: serde_json::Value) -> String {
        let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params.clone()}).to_string();
        // spec 2026-07-28: для tools/call требуется заголовок Mcp-Name с именем инструмента.
        let name_header = if method == "tools/call" {
            format!(
                "Mcp-Name: {}\r\n",
                params.get("name").and_then(serde_json::Value::as_str).unwrap_or_default()
            )
        } else {
            String::new()
        };
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\n\
             Accept: application/json, text/event-stream\r\n\
             MCP-Protocol-Version: 2026-07-28\r\n\
             Mcp-Method: {method}\r\n{name_header}\
             Authorization: Bearer {token}\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        raw_http(addr, &request)
    }

    fn json_body(response: &str) -> serde_json::Value {
        let body = response.split_once("\r\n\r\n").map(|(_, body)| body).unwrap_or_default();
        serde_json::from_str(body).unwrap_or_else(|error| panic!("JSON: {error}; response={response}"))
    }

    fn error_code(result: &serde_json::Value) -> String {
        result
            .get("result")
            .and_then(|result| result.get("structuredContent"))
            .and_then(|content| content.get("error"))
            .and_then(|error| error.get("code"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    fn note_write_params(note_id: &str, content: &str) -> serde_json::Value {
        serde_json::json!({
            "name": "note_write",
            "arguments": {"note_id": note_id, "content": content},
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        })
    }

    /// Прямой вызов инструмента без ожидания approvals (read-only/ошибочные сценарии).
    fn call_tool(server: &ExternalAgentServer, method: &str, params: serde_json::Value) -> serde_json::Value {
        let info = match server.status(true) {
            ExternalAgentStatus::Running(info) => info,
            other => panic!("сервер не запущен: {other:?}"),
        };
        json_body(&mcp_post(info.addr, &info.token, method, params))
    }

    fn call_note_write(
        server: &ExternalAgentServer,
        note_id: &str,
        content: &str,
        approvals: &Arc<GuiApprovals>,
        decision: ApprovalDecision,
    ) -> serde_json::Value {
        let info = match server.status(true) {
            ExternalAgentStatus::Running(info) => info,
            other => panic!("сервер не запущен: {other:?}"),
        };
        let params = note_write_params(note_id, content);
        let handle = std::thread::spawn({
            let info = info.clone();
            move || mcp_post(info.addr, &info.token, "tools/call", params)
        });
        let mut answered = false;
        for _ in 0..300 {
            if let Some(mut pending) = approvals.take_pending() {
                assert_eq!(pending.tool(), "note_write");
                assert!(pending.preview().is_some(), "превью «было/станет» для note_write");
                pending.decide(decision);
                answered = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !answered {
            let response = handle.join().expect("HTTP-поток");
            panic!(
                "approval-запрос не дошёл до GUI-очереди; response: {}",
                response.chars().take(600).collect::<String>()
            );
        }
        json_body(&handle.join().expect("HTTP-поток"))
    }

    /// `(id, file)` единственной заметки тестового проекта.
    fn note_info(root: &Path) -> (String, String) {
        let project = rde_core::load_project(root).expect("проект");
        project
            .objects
            .iter()
            .find_map(|object| match &object.data {
                rde_core::ObjectData::Note(note) => Some((note.id.clone(), note.file.clone())),
                _ => None,
            })
            .expect("заметка в тестовом проекте")
    }

    fn note_id(root: &Path) -> String {
        note_info(root).0
    }

    #[test]
    fn disabled_by_default_and_starts_on_demand() {
        let root = temp_project("start");
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        assert_eq!(server.status(false), ExternalAgentStatus::Disabled);
        assert!(!server.is_running());
        // Разрешено, но не запущено.
        assert_eq!(server.status(true), ExternalAgentStatus::NotRunning);
        // Запуск: loopback + токен-секрет + URL; read-only.
        server.start(&root, &options(Arc::clone(&approvals), false)).expect("сервер стартует");
        let status = server.status(true);
        assert!(status.is_running(), "статус: {status:?}");
        let info = match &status {
            ExternalAgentStatus::Running(info) => info.clone(),
            other => panic!("ожидался Running, получено {other:?}"),
        };
        assert!(info.addr.ip().is_loopback());
        assert_eq!(info.token.len(), 64);
        assert!(info.generated);
        assert!(info.url().ends_with("/mcp"));
        assert!(!server.mutations_enabled());
        // /health доступен без токена.
        let health = raw_http(info.addr, "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(health.starts_with("HTTP/1.1 200"), "{health}");
        server.stop();
        assert_eq!(server.status(true), ExternalAgentStatus::NotRunning);
    }

    #[test]
    fn read_only_mode_still_denies_mutations() {
        let root = temp_project("read-only");
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        server.start(&root, &options(Arc::clone(&approvals), false)).expect("сервер стартует");
        let result = call_tool(&server, "tools/call", note_write_params(&note_id(&root), "x"));
        assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "read-only: {result}");
        assert_eq!(approvals.pending_count(), 0, "в read-only approval-хук не вызывается");
    }

    #[test]
    fn gui_approval_allows_mutation_with_checkpoint() {
        let root = temp_project("allow");
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        server.start(&root, &options(Arc::clone(&approvals), true)).expect("сервер стартует");
        assert!(server.mutations_enabled());
        let id = note_id(&root);
        let result =
            call_note_write(&server, &id, "Текст, одобренный пользователем", &approvals, ApprovalDecision::AllowOnce);
        let content = result
            .get("result")
            .and_then(|result| result.get("structuredContent"))
            .unwrap_or_else(|| panic!("нет structuredContent: {result}"));
        assert!(content.get("error").is_none(), "мутация должна примениться: {result}");
        assert_eq!(content["changed_files"].as_array().map(Vec::len), Some(1));
        assert!(content["checkpoint_id"].as_str().is_some(), "чекпоинт в ответе: {content}");
        // Чекпоинт на диске + файл заметки изменён.
        let checkpoint_id = content["checkpoint_id"].as_str().unwrap();
        assert!(root.join(".rde/agent/checkpoints").join(checkpoint_id).join("manifest.json").is_file());
        let (_, note_file) = note_info(&root);
        let text = std::fs::read_to_string(root.join("objects/notes").join(&id).join(&note_file)).unwrap_or_default();
        assert!(text.contains("Текст, одобренный пользователем"), "файл заметки: {text}");
    }

    #[test]
    fn gui_approval_deny_keeps_project_unchanged() {
        let root = temp_project("deny");
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        server.start(&root, &options(Arc::clone(&approvals), true)).expect("сервер стартует");
        let (id, note_file) = note_info(&root);
        let note_path = root.join("objects/notes").join(&id).join(&note_file);
        let before = std::fs::read_to_string(&note_path).unwrap_or_default();
        let result = call_note_write(&server, &id, "не должно примениться", &approvals, ApprovalDecision::Deny);
        assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "{result}");
        let after = std::fs::read_to_string(&note_path).unwrap_or_default();
        assert_eq!(before, after, "отказ не меняет файл");
    }

    #[test]
    fn gui_approval_timeout_is_deny() {
        let approvals = Arc::new(GuiApprovals::new());
        let hook = GuiApprovalHook::with_timeout(Arc::clone(&approvals), Duration::from_millis(80));
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let decision = runtime.block_on(hook.request(ApprovalRequest {
            tool: "note_write".to_owned(),
            args_summary: "{}".to_owned(),
            args_sha256: "0".repeat(64),
            destructive: true,
            requested_by: "mcp".to_owned(),
            args: Some(serde_json::json!({"content": "x"})),
        }));
        assert_eq!(decision, ApprovalDecision::Deny, "таймаут → Deny (fail-closed)");
        // Запрос остался в очереди (GUI не отвечал) — очищается deny_all при остановке.
        assert_eq!(approvals.pending_count(), 1);
        approvals.deny_all();
        assert_eq!(approvals.pending_count(), 0);
    }

    #[test]
    fn apply_and_project_switch_manage_lifecycle() {
        let first = temp_project("switch-a");
        let second = temp_project("switch-b");
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        let opts = options(Arc::clone(&approvals), false);
        server.apply(true, &first, &opts).expect("включение");
        assert!(server.is_running());
        assert_eq!(server.project_root().map(Path::to_path_buf), Some(first.clone()));
        // Смена проекта: старый сервер остановлен, поднят для нового корня.
        server.on_project_switched(true, &second, &opts).expect("смена проекта");
        assert!(server.is_running());
        assert_eq!(server.project_root().map(Path::to_path_buf), Some(second.clone()));
        // Смена режима (read-only → мутации) перезапускает сервер.
        let mutating = options(Arc::clone(&approvals), true);
        server.apply(true, &second, &mutating).expect("режим мутаций");
        assert!(server.mutations_enabled());
        // Выключение настройки останавливает сервер.
        server.apply(false, &second, &mutating).expect("выключение");
        assert!(!server.is_running());
        assert_eq!(server.status(false), ExternalAgentStatus::Disabled);
    }

    #[test]
    fn start_error_is_reported_and_state_stays_stopped() {
        let missing = std::env::temp_dir().join(format!("rde-ui-agent-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        let error = server.start(&missing, &options(approvals, false)).expect_err("несуществующий проект");
        assert!(!error.is_empty());
        assert!(!server.is_running());
        assert!(server.last_error().is_some());
        assert_eq!(server.status(true), ExternalAgentStatus::NotRunning);
    }
}
