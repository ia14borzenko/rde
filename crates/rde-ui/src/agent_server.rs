//! Встроенный headless MCP-сервер по streamable HTTP для внешних агентов (M4.1, R-MCP-02).
//!
//! GUI — это удобная точка включения «Разрешить внешним агентам»: пользователь ставит галочку в
//! настройках, RDE поднимает **фоновый** MCP-сервер на **loopback** с **обязательным bearer-токеном**
//! и показывает адрес/токен в панели «Агент», чтобы их можно было скопировать во внешний клиент
//! (Claude Code и др.). Политика — **read-only** (`approvals=deny`): внешний агент может читать
//! проект/документ/артефакты, но мутации через GUI-диалог approvals — M4.2; так включение доступа
//! остаётся fail-closed (R-MCP-05).
//!
//! Сервер живёт в фоновом потоке ([`rde_mcp::start_http_background`]) и останавливается при
//! выключении настройки, смене проекта или выходе из RDE (`Drop`).

use std::path::{Path, PathBuf};

use rde_host::HeadlessHost;
use rde_mcp::{HttpOptions, HttpServerHandle, HttpServerInfo, McpPolicy};

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

    /// Запустить фоновый MCP HTTP для проекта `root` (если уже запущен — ничего не делает).
    ///
    /// Политика — read-only (`allow_mutation=false`): внешний агент только читает проект, мутации
    /// требуют GUI-approvals (M4.2). Auth — bearer-токен, `bind=127.0.0.1:0` (loopback, эфемерный порт).
    ///
    /// # Errors
    ///
    /// Строка с причиной (проект не открывается / сервер не стартовал).
    pub fn start(&mut self, root: &Path) -> Result<(), String> {
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
        let hooks = rde_mcp::HostHooks::jsonl(host.root()).with_approvals(std::sync::Arc::new(
            rde_mcp::DenyAllHook,
        ));
        let policy = McpPolicy::read_only();
        let options = HttpOptions {
            bind: "127.0.0.1:0".to_owned(),
            token: None,
            // GUI показывает адрес/токен в панели «Агент»; Ctrl+C обрабатывает приложение.
            announce: false,
            handle_ctrl_c: false,
        };
        match rde_mcp::start_http_background(std::sync::Arc::new(host), hooks, policy, options) {
            Ok(handle) => {
                self.root = Some(root.to_path_buf());
                self.handle = Some(handle);
                self.last_error = None;
                Ok(())
            }
            Err(error) => {
                self.last_error = Some(error.to_string());
                Err(error.to_string())
            }
        }
    }

    /// Остановить сервер (идемпотентно) и закрыть порт.
    pub fn stop(&mut self) {
        if let Some(mut handle) = self.handle.take() {
            handle.stop();
        }
        self.root = None;
    }

    /// Применить настройку «Разрешить внешним агентам» для проекта `root` (вкл/выкл).
    ///
    /// Возвращает `Err` только при включении, когда сервер не удалось поднять (настройка при этом
    /// остаётся выключенной в UI — решает вызывающий слой).
    pub fn apply(&mut self, enabled: bool, root: &Path) -> Result<(), String> {
        if enabled {
            self.start(root)
        } else {
            self.stop();
            Ok(())
        }
    }

    /// Сменить проект: остановить сервер и (если включён) поднять для нового корня.
    pub fn on_project_switched(&mut self, enabled: bool, root: &Path) -> Result<(), String> {
        self.stop();
        if enabled {
            self.start(root)
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
        dir
    }

    /// Сырой HTTP-запрос к серверу (loopback): только статус-строка.
    fn raw_status(addr: std::net::SocketAddr, request: &str) -> String {
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_secs(3))
            .expect("TCP connect");
        stream.set_read_timeout(Some(std::time::Duration::from_secs(5))).unwrap();
        stream.write_all(request.as_bytes()).expect("запрос");
        stream.flush().unwrap();
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        String::from_utf8_lossy(&response).lines().next().unwrap_or_default().to_owned()
    }

    #[test]
    fn disabled_by_default_and_starts_on_demand() {
        let root = temp_project("start");
        let mut server = ExternalAgentServer::new();
        assert_eq!(server.status(false), ExternalAgentStatus::Disabled);
        assert!(!server.is_running());
        // Разрешено, но не запущено.
        assert_eq!(server.status(true), ExternalAgentStatus::NotRunning);
        // Запуск: loopback + токен-секрет + URL.
        server.start(&root).expect("сервер стартует");
        let status = server.status(true);
        assert!(status.is_running(), "статус: {status:?}");
        assert!(status.label().contains("MCP HTTP"));
        let info = match &status {
            ExternalAgentStatus::Running(info) => info.clone(),
            other => panic!("ожидался Running, получено {other:?}"),
        };
        assert!(info.addr.ip().is_loopback());
        assert_eq!(info.token.len(), 64);
        assert!(info.generated);
        assert!(info.url().ends_with("/mcp"));
        // /health доступен без токена (сервер действительно слушает порт).
        let health = raw_status(
            info.addr,
            "GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
        );
        assert_eq!(health, "HTTP/1.1 200 OK");
        // Повторный start идемпотентен.
        server.start(&root).expect("повторный start");
        // Остановка: порт закрыт, состояние — NotRunning.
        let addr = info.addr;
        server.stop();
        assert_eq!(server.status(true), ExternalAgentStatus::NotRunning);
        let closed = std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(500));
        assert!(closed.is_err(), "порт должен быть закрыт после stop");
    }

    #[test]
    fn apply_and_project_switch_manage_lifecycle() {
        let first = temp_project("switch-a");
        let second = temp_project("switch-b");
        let mut server = ExternalAgentServer::new();
        server.apply(true, &first).expect("включение");
        assert!(server.is_running());
        assert_eq!(server.project_root().map(Path::to_path_buf), Some(first.clone()));
        // Смена проекта: старый сервер остановлен, поднят для нового корня.
        server.on_project_switched(true, &second).expect("смена проекта");
        assert!(server.is_running());
        assert_eq!(server.project_root().map(Path::to_path_buf), Some(second.clone()));
        // Выключение настройки останавливает сервер.
        server.apply(false, &second).expect("выключение");
        assert!(!server.is_running());
        assert_eq!(server.status(false), ExternalAgentStatus::Disabled);
    }

    #[test]
    fn start_error_is_reported_and_state_stays_stopped() {
        let missing = std::env::temp_dir().join(format!("rde-ui-agent-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&missing);
        let mut server = ExternalAgentServer::new();
        let error = server.start(&missing).expect_err("несуществующий проект");
        assert!(!error.is_empty());
        assert!(!server.is_running());
        assert!(server.last_error().is_some());
        assert_eq!(server.status(true), ExternalAgentStatus::NotRunning);
    }
}
