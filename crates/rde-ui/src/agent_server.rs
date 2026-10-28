//! Встроенный headless MCP-сервер по streamable HTTP для внешних агентов (M4.1/M4.2, R-MCP-02).
//!
//! GUI — точка включения «Разрешить внешним агентам»: пользователь ставит галочку в настройках, RDE
//! поднимает **фоновый** MCP-сервер на **loopback** с **обязательным bearer-токеном** и показывает
//! адрес/токен в панели «Агент», чтобы их можно было скопировать во внешний клиент (Claude Code и др.).
//!
//! Режимы (M4.2/M4.2b):
//! - по умолчанию — **только чтение** (`approvals=deny`) — M4.1;
//! - при включённых настройках — набор грантов [`ExternalAgentGrants`] (`project_write`,
//!   `run_script`, `git_commit`), и **каждое** действие проходит через GUI-диалог
//!   [`GuiApprovalHook`] (разрешить один раз / в сессии / отклонить), с превью аргументов
//!   («было/станет» для правок, цели/сообщение для запусков и коммитов), чекпоинтом и аудитом.
//!   Решение пользователя — единственный источник `allow` (fail-closed: таймаут/закрытие →
//!   [`ApprovalDecision::Deny`]). `run_script` выдаётся только `trusted`-проекту (R-RUN-06).
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

/// Набор грантов внешнего агента, выданных пользователем в GUI (M4.2b).
///
/// Каждый грант — отдельная галочка в настройках (R-AGT-06: отдельный грант на shell/скрипты,
/// R-MCP-05: гранты, а не «полный доступ»). Эффективный набор может быть уже «желаемого»:
/// `run_script` действует только для `trusted`-проекта (R-RUN-06) и вычисляется при старте сервера.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExternalAgentGrants {
    /// Запись в проект: документ/заметки/манифесты (`project_write`).
    pub project_write: bool,
    /// Запуск скриптов проекта (`artifact_run`/`run_cancel`) — только `trusted` (R-RUN-06).
    pub run_script: bool,
    /// git-коммит (`git_commit`).
    pub git_commit: bool,
}

impl ExternalAgentGrants {
    /// Без грантов: внешний агент только читает (M4.1, fail-closed).
    pub const NONE: Self = Self { project_write: false, run_script: false, git_commit: false };

    /// Есть ли хоть один грант (сервер вообще может менять состояние → нужен GUI-хук).
    pub fn any(&self) -> bool {
        self.project_write || self.run_script || self.git_commit
    }

    /// Короткая подпись действующего режима для панели «Агент»/статус-бара: `rw`, `rw+run+git`,
    /// `read-only`.
    pub fn label(&self) -> String {
        if !self.any() {
            return "read-only".to_owned();
        }
        let mut parts = Vec::new();
        if self.project_write {
            parts.push("rw");
        }
        if self.run_script {
            parts.push("run");
        }
        if self.git_commit {
            parts.push("git");
        }
        parts.join("+")
    }
}

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

    /// Human-readable сводка действия для диалога (M4.2b): что именно запустится/закоммитится.
    ///
    /// Для `artifact_run` — цели/зависимые/`force` + предупреждение о выполнении кода проекта
    /// (R-RUN-06), для `run_cancel` — какой job останавливается, для `git_commit` — сообщение,
    /// что коммитятся все изменения, и что `.rde/**` не попадёт в коммит. Для остальных
    /// инструментов — `None` (диалог показывает превью правки или JSON-аргументы).
    pub fn human_summary(&self) -> Option<Vec<String>> {
        let args = self.args.as_ref()?;
        match self.tool.as_str() {
            "artifact_run" => {
                let mut lines = Vec::new();
                let artifact_id = args.get("artifact_id").and_then(serde_json::Value::as_str).unwrap_or_default();
                let targets: Vec<&str> = args
                    .get("targets")
                    .and_then(serde_json::Value::as_array)
                    .map(|items| items.iter().filter_map(serde_json::Value::as_str).collect())
                    .unwrap_or_default();
                if !artifact_id.is_empty() {
                    lines.push(format!("Артефакт: {artifact_id}"));
                } else if targets.is_empty() {
                    lines.push("Цели: все устаревшие артефакты (targets не задан)".to_owned());
                } else {
                    lines.push(format!("Цели ({}): {}", targets.len(), summarize_list(&targets, 8)));
                }
                let dependents = args.get("include_dependents").and_then(serde_json::Value::as_bool).unwrap_or(false);
                lines.push(format!("С зависимыми артефактами: {}", yes_no(dependents)));
                let force = args.get("force").and_then(serde_json::Value::as_bool).unwrap_or(false);
                lines.push(format!("Принудительный пересчёт (force): {}", yes_no(force)));
                lines.push(
                    "Будут выполнены скрипты проекта (код артефактов) на этой машине — R-RUN-06; \
                     запуск разрешён только для доверенного проекта (trusted = true)."
                        .to_owned(),
                );
                Some(lines)
            }
            "run_cancel" => {
                let job_id = args.get("job_id").and_then(serde_json::Value::as_str).unwrap_or_default();
                Some(vec![
                    format!("Job: {job_id}"),
                    "Текущий запуск будет остановлен (SIGTERM → grace → SIGKILL); уже записанные \
                     выходы и транзакция остаются (R-RUN-05)."
                        .to_owned(),
                ])
            }
            "git_commit" => {
                let message = args.get("message").and_then(serde_json::Value::as_str).unwrap_or_default().trim();
                let message = if message.chars().count() > 200 {
                    format!("{}…", message.chars().take(200).collect::<String>())
                } else {
                    message.to_owned()
                };
                let dry_run = args.get("dry_run").and_then(serde_json::Value::as_bool).unwrap_or(false);
                let mut lines = vec![
                    format!("Сообщение коммита: {message}"),
                    "Будет выполнен `git add -A` + коммит всех изменённых/новых файлов проекта; \
                     `.rde/**` (store/аудит/чекпоинты) в коммит не попадает; автор — из rde.toml \
                     (человек → агент → «MCP client»)."
                        .to_owned(),
                ];
                if dry_run {
                    lines.push("Режим dry_run: коммит НЕ создаётся, только проверка статуса.".to_owned());
                }
                Some(lines)
            }
            _ => None,
        }
    }
}

/// `да`/`нет` для human-readable сводки в диалоге approval.
fn yes_no(value: bool) -> &'static str {
    if value {
        "да"
    } else {
        "нет"
    }
}

/// Список для диалога: первые `max` элементов + «… (+N)», если длиннее.
fn summarize_list(items: &[&str], max: usize) -> String {
    let shown = items.iter().take(max).copied().collect::<Vec<_>>().join(", ");
    if items.len() > max {
        format!("{shown}, … (+{})", items.len() - max)
    } else {
        shown
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

/// Параметры запуска встроенного сервера (M4.2/M4.2b).
#[derive(Clone)]
pub struct ExternalAgentOptions {
    /// Желаемый набор грантов из настроек GUI (эффективный может быть уже: `run_script`
    /// действует только для `trusted`-проекта, R-RUN-06).
    pub grants: ExternalAgentGrants,
    /// Очередь GUI-approvals (общая с UI).
    pub approvals: Arc<GuiApprovals>,
}

impl std::fmt::Debug for ExternalAgentOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExternalAgentOptions")
            .field("grants", &self.grants)
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
    /// Сервер работает: адрес/токен для подключения клиента + действующий набор грантов.
    Running {
        /// Адрес/токен/URL для клиента.
        info: HttpServerInfo,
        /// Фактически выданные гранты (после trusted-гейта run_script).
        grants: ExternalAgentGrants,
    },
}

impl ExternalAgentStatus {
    /// Краткая подпись для панели «Агент».
    pub fn label(&self) -> String {
        match self {
            ExternalAgentStatus::Disabled => "не подключён".to_owned(),
            ExternalAgentStatus::NotRunning => "MCP HTTP: остановлен".to_owned(),
            ExternalAgentStatus::Running { info, grants } => {
                format!("MCP HTTP: {} ({})", info.addr, grants.label())
            }
        }
    }

    /// Сервер работает.
    pub fn is_running(&self) -> bool {
        matches!(self, ExternalAgentStatus::Running { .. })
    }

    /// Действующий набор грантов (пустой, если сервер не работает).
    pub fn grants(&self) -> ExternalAgentGrants {
        match self {
            ExternalAgentStatus::Running { grants, .. } => *grants,
            ExternalAgentStatus::Disabled | ExternalAgentStatus::NotRunning => ExternalAgentGrants::NONE,
        }
    }
}

/// Менеджер встроенного MCP-сервера (фоновый streamable HTTP) для GUI.
#[derive(Default)]
pub struct ExternalAgentServer {
    root: Option<PathBuf>,
    handle: Option<HttpServerHandle>,
    last_error: Option<String>,
    /// Фактически выданные гранты (эффективный набор; `None` при остановленном сервере).
    grants: ExternalAgentGrants,
    approvals: Option<Arc<GuiApprovals>>,
}

impl ExternalAgentServer {
    /// Новый менеджер (ничего не запущено).
    pub fn new() -> Self {
        Self::default()
    }

    /// Эффективный набор грантов для проекта `root`: `run_script` — только для `trusted`
    /// (R-RUN-06), остальные — как задано настройками.
    ///
    /// Проект не читается → считается недоверенным (fail-closed).
    fn effective_grants(root: &Path, desired: &ExternalAgentGrants) -> ExternalAgentGrants {
        let trusted = rde_core::load_project(root).map(|project| project.manifest.trusted).unwrap_or(false);
        ExternalAgentGrants { run_script: desired.run_script && trusted, ..*desired }
    }

    /// Текущее состояние для UI (включая действующий набор грантов).
    pub fn status(&self, enabled: bool) -> ExternalAgentStatus {
        if let Some(handle) = &self.handle {
            return ExternalAgentStatus::Running { info: handle.info().clone(), grants: self.grants };
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

    /// Действующий набор грантов (пустой, если сервер не работает).
    pub fn grants(&self) -> ExternalAgentGrants {
        self.grants
    }

    /// Мутации разрешены хотя бы каким-то грантом (через GUI-диалог approvals).
    pub fn mutations_enabled(&self) -> bool {
        self.grants.any()
    }

    /// Запустить фоновый MCP HTTP для проекта `root` (если уже запущен — ничего не делает).
    ///
    /// Политика собирается из **эффективных** грантов: без грантов — read-only (`approvals=deny`);
    /// с любым грантом — соответствующие `allow_project_write`/`allow_run_script`/`allow_git_commit`,
    /// `approval_mode="gui"` и [`GuiApprovalHook`] (каждое действие подтверждается диалогом).
    /// `run_script` выдаётся только `trusted`-проекту (R-RUN-06). Auth — bearer-токен,
    /// `bind=127.0.0.1:0`.
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
        let grants = Self::effective_grants(root, &options.grants);
        let approvals: Arc<dyn ApprovalHook> = if grants.any() {
            Arc::new(GuiApprovalHook::new(Arc::clone(&options.approvals)))
        } else {
            Arc::new(rde_mcp::DenyAllHook)
        };
        let hooks = HostHooks::jsonl(host.root()).with_approvals(approvals);
        let mut policy = McpPolicy::read_only();
        // Гранты + GUI-approvals: каждое действие подтверждается диалогом (решение — в аудит).
        policy.allow_project_write = grants.project_write;
        policy.allow_run_script = grants.run_script;
        policy.allow_git_commit = grants.git_commit;
        if grants.any() {
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
                self.grants = grants;
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
        self.grants = ExternalAgentGrants::NONE;
    }

    /// Применить настройки внешнего доступа для проекта `root` (вкл/выкл + набор грантов).
    ///
    /// Если сервер уже запущен, при изменении **любого** гранта (включая доверие проекта для
    /// `run_script`) он перезапускается — политика и хук фиксируются на старте сервера.
    ///
    /// Возвращает `Err` только при включении, когда сервер не удалось поднять (настройка при этом
    /// остаётся выключенной в UI — решает вызывающий слой).
    pub fn apply(&mut self, enabled: bool, root: &Path, options: &ExternalAgentOptions) -> Result<(), String> {
        if !enabled {
            self.stop();
            return Ok(());
        }
        let desired = Self::effective_grants(root, &options.grants);
        if self.handle.is_some() && self.grants == desired {
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

    /// Гранты «как из настроек» (эффективные вычисляет сервер: run_script — только для trusted).
    fn grants(project_write: bool, run_script: bool, git_commit: bool) -> ExternalAgentGrants {
        ExternalAgentGrants { project_write, run_script, git_commit }
    }

    fn options(approvals: Arc<GuiApprovals>, grants: ExternalAgentGrants) -> ExternalAgentOptions {
        ExternalAgentOptions { grants, approvals }
    }

    /// Адрес/токен работающего сервера.
    fn server_info(server: &ExternalAgentServer) -> HttpServerInfo {
        match server.status(true) {
            ExternalAgentStatus::Running { info, .. } => info,
            other => panic!("сервер не запущен: {other:?}"),
        }
    }

    /// Текст MCP-инструкций сервера (`server/discover`) — фактическая политика сессии
    /// (grants/approvals).
    ///
    /// `server/discover` (spec 2026-07-28) требует `_meta` с версией протокола и capabilities клиента.
    fn server_instructions(server: &ExternalAgentServer) -> String {
        let info = server_info(server);
        let params = serde_json::json!({
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        });
        let response = json_body(&mcp_post(info.addr, &info.token, "server/discover", params));
        response
            .get("result")
            .and_then(|result| result.get("instructions"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
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
        structured(result)
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned()
    }

    /// `structuredContent` ответа инструмента (успех и ошибки хоста — здесь).
    fn structured(result: &serde_json::Value) -> &serde_json::Value {
        result
            .get("result")
            .and_then(|result| result.get("structuredContent"))
            .unwrap_or_else(|| panic!("нет structuredContent: {result}"))
    }

    /// Параметры вызова инструмента (`tools/call`, spec 2026-07-28).
    fn tool_params(name: &str, arguments: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "arguments": arguments,
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }
        })
    }

    /// Прямой вызов метода без ожидания approvals (read-only/ошибочные сценарии).
    fn call_tool(server: &ExternalAgentServer, method: &str, params: serde_json::Value) -> serde_json::Value {
        let info = server_info(server);
        json_body(&mcp_post(info.addr, &info.token, method, params))
    }

    /// Прямой `tools/call` инструмента (без ожидания approvals).
    fn call_tool_direct(server: &ExternalAgentServer, name: &str, arguments: serde_json::Value) -> serde_json::Value {
        call_tool(server, "tools/call", tool_params(name, arguments))
    }

    /// Вызвать инструмент, дождаться GUI-approval, отдать решение; вернуть ответ и сводку запроса.
    fn call_with_approval(
        server: &ExternalAgentServer,
        name: &str,
        arguments: serde_json::Value,
        approvals: &Arc<GuiApprovals>,
        answer: impl FnOnce(&PendingApproval) -> ApprovalDecision,
    ) -> (serde_json::Value, Option<Vec<String>>) {
        let info = server_info(server);
        let params = tool_params(name, arguments);
        let handle = std::thread::spawn({
            let info = info.clone();
            move || mcp_post(info.addr, &info.token, "tools/call", params)
        });
        let mut summary = None;
        let mut answered = false;
        for _ in 0..300 {
            if let Some(mut pending) = approvals.take_pending() {
                assert_eq!(pending.tool(), name);
                summary = pending.human_summary();
                let decision = answer(&pending);
                pending.decide(decision);
                answered = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if !answered {
            let response = handle.join().expect("HTTP-поток");
            panic!(
                "approval-запрос `{name}` не дошёл до GUI-очереди; response: {}",
                response.chars().take(600).collect::<String>()
            );
        }
        (json_body(&handle.join().expect("HTTP-поток")), summary)
    }

    /// `call_with_approval` с решением «разрешить один раз».
    fn allow_once(
        server: &ExternalAgentServer,
        name: &str,
        arguments: serde_json::Value,
        approvals: &Arc<GuiApprovals>,
    ) -> serde_json::Value {
        call_with_approval(server, name, arguments, approvals, |_| ApprovalDecision::AllowOnce).0
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

    /// id вычислительного артефакта (создаётся при необходимости); `None` — если создать не удалось.
    fn artifact_id(root: &Path) -> String {
        rde_core::create_object(
            root,
            &rde_core::CreateRequest { kind: rde_core::NewObjectKind::ComputeArtifact, title: "Расчёт агента".to_owned() },
        )
        .expect("артефакт создаётся");
        rde_core::load_project(root)
            .expect("проект")
            .objects
            .iter()
            .find_map(|object| match &object.data {
                rde_core::ObjectData::Artifact(artifact) => Some(artifact.id.clone()),
                _ => None,
            })
            .expect("артефакт в тестовом проекте")
    }

    /// `git log --pretty=format:%s` (новые первыми); вне репозитория — пусто.
    fn git_log_subjects(root: &Path) -> Vec<String> {
        let output = std::process::Command::new("git")
            .args(["log", "--pretty=format:%s"])
            .current_dir(root)
            .output()
            .expect("git log");
        String::from_utf8_lossy(&output.stdout).lines().map(str::to_owned).collect()
    }

    /// Файлы последнего коммита (относительно корня репозитория).
    fn git_last_commit_files(root: &Path) -> Vec<String> {
        let output = std::process::Command::new("git")
            .args(["show", "--name-only", "--pretty=format:"])
            .current_dir(root)
            .output()
            .expect("git show");
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect()
    }

    /// Записи в каталоге чекпоинтов агента (хост создаёт чекпоинт перед мутацией/запуском).
    fn checkpoints(root: &Path) -> Vec<String> {
        let dir = root.join(".rde/agent/checkpoints");
        let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
        let mut names: Vec<String> =
            entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
        names.sort();
        names
    }

    #[test]
    fn grants_label_reflects_effective_mode() {
        assert_eq!(ExternalAgentGrants::NONE.label(), "read-only");
        assert!(!ExternalAgentGrants::NONE.any());
        assert_eq!(grants(true, false, false).label(), "rw");
        assert_eq!(grants(true, true, false).label(), "rw+run");
        assert_eq!(grants(true, true, true).label(), "rw+run+git");
        assert_eq!(grants(false, false, true).label(), "git");
        assert!(grants(true, true, true).any());
    }

    #[test]
    fn disabled_by_default_and_starts_on_demand() {
        let root = temp_project("start");
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        assert_eq!(server.status(false), ExternalAgentStatus::Disabled);
        assert_eq!(server.status(false).grants(), ExternalAgentGrants::NONE);
        assert!(!server.is_running());
        // Разрешено, но не запущено.
        assert_eq!(server.status(true), ExternalAgentStatus::NotRunning);
        // Запуск: loopback + токен-секрет + URL; read-only (грантов нет).
        server
            .start(&root, &options(Arc::clone(&approvals), ExternalAgentGrants::NONE))
            .expect("сервер стартует");
        let status = server.status(true);
        assert!(status.is_running(), "статус: {status:?}");
        let info = match &status {
            ExternalAgentStatus::Running { info, .. } => info.clone(),
            other => panic!("ожидался Running, получено {other:?}"),
        };
        assert!(info.addr.ip().is_loopback());
        assert_eq!(info.token.len(), 64);
        assert!(info.generated);
        assert!(info.url().ends_with("/mcp"));
        // Без грантов политика read-only: все три флага выключены, режим виден в статусе.
        assert_eq!(status.grants(), ExternalAgentGrants::NONE);
        assert_eq!(server.grants(), ExternalAgentGrants::NONE);
        assert!(!server.mutations_enabled());
        assert_eq!(status.label(), format!("MCP HTTP: {} (read-only)", info.addr));
        // /health доступен без токена.
        let health = raw_http(info.addr, "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        assert!(health.starts_with("HTTP/1.1 200"), "{health}");
        server.stop();
        assert_eq!(server.status(true), ExternalAgentStatus::NotRunning);
        assert_eq!(server.grants(), ExternalAgentGrants::NONE);
    }

    #[test]
    fn read_only_policy_denies_mutations_of_every_grant() {
        let root = temp_project("read-only");
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        server
            .start(&root, &options(Arc::clone(&approvals), ExternalAgentGrants::NONE))
            .expect("сервер стартует");
        let id = note_id(&root);
        for (tool, args) in [
            ("note_write", serde_json::json!({"note_id": id, "content": "x"})),
            ("artifact_run", serde_json::json!({})),
            ("git_commit", serde_json::json!({"message": "x"})),
        ] {
            let result = call_tool_direct(&server, tool, args);
            assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "read-only `{tool}`: {result}");
        }
        assert_eq!(approvals.pending_count(), 0, "в read-only approval-хук не вызывается");
        // Инструкции сессии отражают read-only: approvals=deny, все три гранта «запрещено».
        let instructions = server_instructions(&server);
        assert!(instructions.contains("режим approvals: `deny`"), "инструкции: {instructions}");
        assert_eq!(instructions.matches("): запрещено;").count(), 3, "все гранты запрещены: {instructions}");
    }

    #[test]
    fn all_grants_are_advertised_with_gui_approvals() {
        let root = temp_project("all-grants");
        rde_core::set_project_trusted(&root, true).expect("проект trusted");
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        let all = grants(true, true, true);
        server.start(&root, &options(Arc::clone(&approvals), all)).expect("сервер стартует");
        // Для trusted-проекта эффективный набор = желаемый: все три гранта в политике.
        assert_eq!(server.grants(), all);
        assert_eq!(server.status(true).grants(), all);
        assert!(server.mutations_enabled());
        let instructions = server_instructions(&server);
        assert!(instructions.contains("режим approvals: `gui`"), "инструкции: {instructions}");
        assert_eq!(instructions.matches("): разрешено;").count(), 3, "все гранты разрешены: {instructions}");
        assert!(instructions.contains("(`git_commit`): разрешено"), "{instructions}");
        assert!(instructions.contains("нужен `trusted`-проект): разрешено"), "{instructions}");
        // Поведенчески: artifact_run и git_commit доходят до GUI-диалога (гранты выданы).
        let result = call_with_approval(&server, "artifact_run", serde_json::json!({}), &approvals, |_| {
            ApprovalDecision::Deny
        })
        .0;
        assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "Deny после диалога: {result}");
        let result = call_with_approval(
            &server,
            "git_commit",
            serde_json::json!({"message": "M4.2b: коммит"}),
            &approvals,
            |_| ApprovalDecision::Deny,
        )
        .0;
        assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "Deny после диалога: {result}");
    }

    #[test]
    fn run_script_grant_requires_trusted_project() {
        let root = temp_project("run-untrusted");
        assert!(!rde_core::load_project(&root).unwrap().manifest.trusted);
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        // Пользователь разрешил запуск скриптов, но проект не trusted (R-RUN-06).
        let run_only = grants(true, true, false);
        server.start(&root, &options(Arc::clone(&approvals), run_only)).expect("сервер стартует");
        assert_eq!(server.grants(), grants(true, false, false), "run-грант не выдан (trusted = false)");
        assert!(server_instructions(&server).contains("нужен `trusted`-проект): запрещено"));
        let old_token = server_info(&server).token;
        // Клиент всё же зовёт artifact_run → отказ по гранту, до диалога не доходит.
        let result = call_tool_direct(&server, "artifact_run", serde_json::json!({}));
        assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "run без trusted: {result}");
        assert_eq!(approvals.pending_count(), 0, "без эффективного гранта approval-хук не вызывается");
        // Явное согласие пользователя (trust_dialog) → trusted=true; apply перезапускает сервер.
        rde_core::set_project_trusted(&root, true).expect("trusted записан");
        server.apply(true, &root, &options(Arc::clone(&approvals), run_only)).expect("перезапуск");
        assert!(server.grants().run_script, "после trusted run-грант включён: {:?}", server.grants());
        assert_ne!(server_info(&server).token, old_token, "сервер перезапущен (новый токен)");
        let result = call_with_approval(&server, "artifact_run", serde_json::json!({}), &approvals, |_| {
            ApprovalDecision::Deny
        })
        .0;
        assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "Deny после диалога: {result}");
    }

    #[test]
    fn run_deny_starts_nothing_and_allow_returns_job() {
        let root = temp_project("run-job");
        rde_core::set_project_trusted(&root, true).expect("проект trusted");
        let artifact = artifact_id(&root);
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        server
            .start(&root, &options(Arc::clone(&approvals), grants(true, true, false)))
            .expect("сервер стартует");
        assert!(server.grants().run_script);
        // Deny: job не стартует, чекпоинт не создаётся.
        let before = checkpoints(&root);
        let (result, summary) = call_with_approval(
            &server,
            "artifact_run",
            serde_json::json!({"targets": [artifact], "include_dependents": true, "force": true}),
            &approvals,
            |_| ApprovalDecision::Deny,
        );
        assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "{result}");
        assert_eq!(checkpoints(&root), before, "отказ не создаёт чекпоинт/job");
        let summary = summary.expect("human-readable сводка artifact_run");
        assert!(summary.iter().any(|line| line.contains(&artifact)), "{summary:?}");
        assert!(summary.iter().any(|line| line.contains("R-RUN-06")), "{summary:?}");
        // AllowOnce: запуск доходит до исполнителя — JobInfo, а не RDE_APPROVAL_DENIED.
        let result = allow_once(&server, "artifact_run", serde_json::json!({"targets": [artifact]}), &approvals);
        assert_ne!(error_code(&result), "RDE_APPROVAL_DENIED", "{result}");
        let content = structured(&result);
        assert!(content.get("error").is_none(), "job должен стартовать: {result}");
        assert!(content["job_id"].as_str().is_some(), "JobInfo в ответе: {content}");
        assert!(content["state"].as_str().is_some(), "state в JobInfo: {content}");
        server.stop();
    }

    #[test]
    fn run_and_commit_previews_are_human_readable() {
        let root = temp_project("previews");
        rde_core::set_project_trusted(&root, true).expect("проект trusted");
        let artifact = artifact_id(&root);
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        server
            .start(&root, &options(Arc::clone(&approvals), grants(true, true, true)))
            .expect("сервер стартует");
        // artifact_run: артефакт/зависимые/force + предупреждение R-RUN-06.
        let (_result, summary) = call_with_approval(
            &server,
            "artifact_run",
            serde_json::json!({"artifact_id": artifact, "include_dependents": true, "force": true}),
            &approvals,
            |_| ApprovalDecision::Deny,
        );
        let text = summary.expect("сводка artifact_run").join("\n");
        assert!(text.contains(&format!("Артефакт: {artifact}")), "{text}");
        assert!(text.contains("С зависимыми артефактами: да"), "{text}");
        assert!(text.contains("Принудительный пересчёт (force): да"), "{text}");
        assert!(text.contains("R-RUN-06"), "{text}");
        // Без аргументов — «все устаревшие».
        let (_result, summary) =
            call_with_approval(&server, "artifact_run", serde_json::json!({}), &approvals, |_| ApprovalDecision::Deny);
        let text = summary.expect("сводка artifact_run").join("\n");
        assert!(text.contains("все устаревшие артефакты"), "{text}");
        assert!(text.contains("С зависимыми артефактами: нет"), "{text}");
        // run_cancel: job_id.
        let (result, summary) = call_with_approval(
            &server,
            "run_cancel",
            serde_json::json!({"job_id": "01JOBUNKNOWN"}),
            &approvals,
            |_| ApprovalDecision::Deny,
        );
        assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "{result}");
        let text = summary.expect("сводка run_cancel").join("\n");
        assert!(text.contains("01JOBUNKNOWN"), "{text}");
        // git_commit: сообщение + git add -A + .rde/** не коммитится.
        let (_result, summary) = call_with_approval(
            &server,
            "git_commit",
            serde_json::json!({"message": "M4.2b: коммит от агента"}),
            &approvals,
            |_| ApprovalDecision::Deny,
        );
        let text = summary.expect("сводка git_commit").join("\n");
        assert!(text.contains("M4.2b: коммит от агента"), "{text}");
        assert!(text.contains("git add -A"), "{text}");
        assert!(text.contains(".rde/**"), "{text}");
        // dry_run отмечается в сводке.
        let (_result, summary) = call_with_approval(
            &server,
            "git_commit",
            serde_json::json!({"message": "проверка", "dry_run": true}),
            &approvals,
            |_| ApprovalDecision::Deny,
        );
        assert!(summary.expect("сводка git_commit").join("\n").contains("dry_run"), "dry_run в сводке");
    }

    #[test]
    fn git_commit_without_grant_is_denied_and_project_untouched() {
        if !rde_vcs::git_available() {
            return;
        }
        let root = temp_project("git-nogrant");
        rde_vcs::init(&root).expect("git init");
        std::fs::write(root.join("agent-change.md"), "правка агента\n").unwrap();
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        // project_write/run выданы, git-грант — нет: коммит обязан быть отклонён политикой.
        server
            .start(&root, &options(Arc::clone(&approvals), grants(true, true, false)))
            .expect("сервер стартует");
        let log_before = git_log_subjects(&root);
        let result = call_tool_direct(&server, "git_commit", serde_json::json!({"message": "не должно закоммититься"}));
        assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "git без гранта: {result}");
        assert_eq!(approvals.pending_count(), 0, "без гранта диалог не показывается");
        assert_eq!(git_log_subjects(&root), log_before, "коммита нет");
        assert_eq!(std::fs::read_to_string(root.join("agent-change.md")).unwrap(), "правка агента\n");
    }

    #[test]
    fn git_commit_deny_and_allow_through_gui() {
        if !rde_vcs::git_available() {
            return;
        }
        let root = temp_project("git-approve");
        rde_vcs::init(&root).expect("git init");
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        server
            .start(&root, &options(Arc::clone(&approvals), grants(false, false, true)))
            .expect("сервер стартует");
        std::fs::write(root.join("agent-change.md"), "правка агента\n").unwrap();
        let log_before = git_log_subjects(&root);
        assert!(!log_before.is_empty(), "стартовый коммит rde_vcs::init");
        // Deny: коммита нет, рабочее дерево не изменилось.
        let (result, _) = call_with_approval(
            &server,
            "git_commit",
            serde_json::json!({"message": "M4.2b: одобренный коммит"}),
            &approvals,
            |_| ApprovalDecision::Deny,
        );
        assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "{result}");
        assert_eq!(git_log_subjects(&root), log_before, "отказ не создаёт коммит");
        assert_eq!(std::fs::read_to_string(root.join("agent-change.md")).unwrap(), "правка агента\n");
        // AllowOnce: коммит с сообщением агента, `.rde/**` в коммит не попадает.
        let result =
            allow_once(&server, "git_commit", serde_json::json!({"message": "M4.2b: одобренный коммит"}), &approvals);
        let content = structured(&result);
        assert!(content.get("error").is_none(), "коммит должен создаться: {result}");
        assert!(content["commit"]["id"].as_str().is_some(), "{content}");
        assert_eq!(content["dry_run"], serde_json::json!(false));
        let log_after = git_log_subjects(&root);
        assert_eq!(log_after.len(), log_before.len() + 1, "ровно один новый коммит: {log_after:?}");
        assert_eq!(log_after.first().map(String::as_str), Some("M4.2b: одобренный коммит"));
        let files = git_last_commit_files(&root);
        assert!(files.iter().any(|file| file == "agent-change.md"), "файл в коммите: {files:?}");
        assert!(
            !files.iter().any(|file| file.starts_with(".rde/") || file.as_str() == ".rde"),
            "`.rde/**` не должен попасть в коммит: {files:?}"
        );
        // dry_run: коммита нет.
        std::fs::write(root.join("agent-change.md"), "вторая правка\n").unwrap();
        let result = allow_once(&server, "git_commit", serde_json::json!({"message": "проверка", "dry_run": true}), &approvals);
        let content = structured(&result);
        assert_eq!(content["dry_run"], serde_json::json!(true), "{content}");
        assert!(content.get("commit").is_none(), "dry_run не создаёт коммит: {content}");
        assert_eq!(git_log_subjects(&root).len(), log_after.len(), "dry_run: без нового коммита");
        // Второй коммит проходит, затем «нечего коммитить» → ошибка хоста (не паника).
        let result = allow_once(&server, "git_commit", serde_json::json!({"message": "M4.2b: второй коммит"}), &approvals);
        assert!(structured(&result).get("error").is_none(), "второй коммит: {result}");
        let result = allow_once(&server, "git_commit", serde_json::json!({"message": "пусто"}), &approvals);
        assert_eq!(error_code(&result), "RDE_NOT_READY", "нечего коммитить: {result}");
        // Пустое сообщение → ошибка валидации инструмента (не паника).
        let result = allow_once(&server, "git_commit", serde_json::json!({"message": "   "}), &approvals);
        assert_eq!(error_code(&result), "RDE_INVALID_ARGUMENT", "пустое сообщение: {result}");
        server.stop();
    }

    #[test]
    fn gui_approval_allows_mutation_with_checkpoint() {
        let root = temp_project("allow");
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        server
            .start(&root, &options(Arc::clone(&approvals), grants(true, false, false)))
            .expect("сервер стартует");
        assert!(server.mutations_enabled());
        let id = note_id(&root);
        let result = allow_once(
            &server,
            "note_write",
            serde_json::json!({"note_id": id, "content": "Текст, одобренный пользователем"}),
            &approvals,
        );
        let content = structured(&result);
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
        server
            .start(&root, &options(Arc::clone(&approvals), grants(true, false, false)))
            .expect("сервер стартует");
        let (id, note_file) = note_info(&root);
        let note_path = root.join("objects/notes").join(&id).join(&note_file);
        let before = std::fs::read_to_string(&note_path).unwrap_or_default();
        let (result, _) = call_with_approval(
            &server,
            "note_write",
            serde_json::json!({"note_id": id, "content": "не должно примениться"}),
            &approvals,
            |pending| {
                assert!(pending.preview().is_some(), "превью «было/станет» для note_write");
                ApprovalDecision::Deny
            },
        );
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
        let read_only = options(Arc::clone(&approvals), ExternalAgentGrants::NONE);
        server.apply(true, &first, &read_only).expect("включение");
        assert!(server.is_running());
        assert_eq!(server.project_root().map(Path::to_path_buf), Some(first.clone()));
        // Смена проекта: старый сервер остановлен, поднят для нового корня.
        server.on_project_switched(true, &second, &read_only).expect("смена проекта");
        assert!(server.is_running());
        assert_eq!(server.project_root().map(Path::to_path_buf), Some(second.clone()));
        // Смена набора грантов (read-only → project_write) перезапускает сервер.
        let old_token = server_info(&server).token;
        let writing = options(Arc::clone(&approvals), grants(true, false, false));
        server.apply(true, &second, &writing).expect("грант записи");
        assert!(server.mutations_enabled());
        assert_eq!(server.grants(), grants(true, false, false));
        assert_ne!(server_info(&server).token, old_token, "новый сервер (новый токен)");
        // Выключение настройки останавливает сервер.
        server.apply(false, &second, &writing).expect("выключение");
        assert!(!server.is_running());
        assert_eq!(server.grants(), ExternalAgentGrants::NONE);
        assert_eq!(server.status(false), ExternalAgentStatus::Disabled);
    }

    #[test]
    fn apply_revokes_removed_grant_and_restarts() {
        let root = temp_project("apply-revoke");
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        let with_write = options(Arc::clone(&approvals), grants(true, false, false));
        server.apply(true, &root, &with_write).expect("включение");
        let first_token = server_info(&server).token;
        // Снятие гранта записи (остаётся только git-грант) → перезапуск, запись запрещена.
        let git_only = options(Arc::clone(&approvals), grants(false, false, true));
        server.apply(true, &root, &git_only).expect("смена грантов");
        assert_eq!(server.grants(), grants(false, false, true));
        assert_ne!(server_info(&server).token, first_token, "сервер перезапущен");
        let result =
            call_tool_direct(&server, "note_write", serde_json::json!({"note_id": note_id(&root), "content": "x"}));
        assert_eq!(error_code(&result), "RDE_APPROVAL_DENIED", "грант снят: {result}");
        // Повторный apply с тем же набором не перезапускает сервер (токен тот же).
        let same_token = server_info(&server).token;
        server.apply(true, &root, &git_only).expect("повторный apply");
        assert_eq!(server_info(&server).token, same_token, "тот же набор грантов → без перезапуска");
        server.stop();
    }

    #[test]
    fn start_error_is_reported_and_state_stays_stopped() {
        let missing = std::env::temp_dir().join(format!("rde-ui-agent-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        let approvals = Arc::new(GuiApprovals::new());
        let mut server = ExternalAgentServer::new();
        let error = server
            .start(&missing, &options(approvals, ExternalAgentGrants::NONE))
            .expect_err("несуществующий проект");
        assert!(!error.is_empty());
        assert!(!server.is_running());
        assert!(server.last_error().is_some());
        assert_eq!(server.status(true), ExternalAgentStatus::NotRunning);
    }
}
