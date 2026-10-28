//! E2E встроенного ACP-агента с **настоящим DSH** (M4.3, R-AGT-01): `dsh --profile acp`.
//!
//! Проверяет клиент `rde-agent` на реальном агенте: `initialize` (версия/возможности),
//! `session/list` (по `cwd` проекта), `session/new` с stdio-декларацией `rde mcp serve
//! --approval=deny` (fail-closed), `session/close`. LLM-тур не проверяется: в песочнице нет
//! провайдерских ключей DSH — это ручная проверка пользователя (см. отчёт/ADR-0014).
//!
//! Если DSH не установлен/профиль `acp` недоступен — тест **пропускается** (как python_available
//! в других тестах) с причиной в stdout: окружение CI может не иметь локальной поставки DSH.
//!
//! DSH требует записываемый `DSH_HOME` (в песочнице `~/.dsh` read-only) — копируем **только**
//! профиль `acp` во временный каталог; `.credentials.yaml` в репозиторий/коммиты не попадает.
//! Соединение с агентом живёт до `shutdown` (SDK держит stdin дочернего процесса открытым).

use std::path::PathBuf;
use std::time::{Duration, Instant};

use rde_agent::{AgentClient, AgentConfig, AgentEvent, McpServerSpec};

/// Сколько ждать события от настоящего DSH (запуск node-процесса + MCP-discovery).
const WAIT: Duration = Duration::from_secs(90);

/// Дождаться события, удовлетворяющего предикату; остальные события копятся и возвращаются.
fn wait_for(
    agent: &mut rde_agent::AgentHandle,
    predicate: impl Fn(&AgentEvent) -> bool,
    context: &str,
) -> AgentEvent {
    let deadline = Instant::now() + WAIT;
    let mut seen = Vec::new();
    loop {
        while let Some(event) = agent.try_recv() {
            if predicate(&event) {
                return event;
            }
            seen.push(event);
        }
        assert!(Instant::now() < deadline, "не дождались «{context}» от DSH; видели: {seen:?}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

/// Строка `file` из вывода команды (без внешних крейтов).
fn command_available(program: &str, args: &[&str]) -> bool {
    std::process::Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Подготовить временный `DSH_HOME` с копией профиля `acp` (возвращает путь или причину пропуска).
fn prepare_dsh_home(name: &str) -> Result<PathBuf, String> {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return Err("HOME не задан".to_owned());
    };
    let profile = home.join(".dsh/profiles/acp");
    if !profile.join("package.json").is_file() {
        return Err(format!("профиль DSH acp не найден: {}", profile.display()));
    }
    let dsh_home = std::env::temp_dir().join(format!("rde-dsh-e2e-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dsh_home);
    let target = dsh_home.join("profiles/acp");
    std::fs::create_dir_all(&target)
        .map_err(|error| format!("создать {}: {error}", target.display()))?;
    for entry in std::fs::read_dir(&profile)
        .map_err(|error| format!("прочитать {}: {error}", profile.display()))?
        .flatten()
    {
        let from = entry.path();
        let to = target.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)
                .map_err(|error| format!("копировать {}: {error}", from.display()))?;
        }
    }
    Ok(dsh_home)
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|error| error.to_string())?;
    for entry in std::fs::read_dir(src).map_err(|error| error.to_string())?.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

/// E2E: настоящий DSH по ACP (initialize/session-list/new/close) через клиент RDE.
#[test]
fn real_dsh_acp_initialize_list_new_and_close_session() {
    if !command_available("dsh", &["--version"]) {
        eprintln!("SKIP: `dsh` не найден в PATH — e2e ACP-агента пропущен (M4.3)");
        return;
    }
    let dsh_home = match prepare_dsh_home("acp") {
        Ok(path) => path,
        Err(reason) => {
            eprintln!("SKIP: DSH-профиль acp недоступен ({reason}) — e2e пропущен (M4.3)");
            return;
        }
    };
    // Минимальный проект RDE (cwd сессии агента).
    let project = std::env::temp_dir().join(format!("rde-agent-e2e-project-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&project);
    rde_core::create_project(&project, "E2E ACP проект").expect("проект создаётся");

    let config = AgentConfig::new("dsh")
        .args(["--profile", "acp"])
        .envs([("DSH_HOME", dsh_home.display().to_string())]);
    let mut agent = AgentClient::spawn(config).expect("клиент агента запускается");

    // 1) initialize: DSH сообщает ACP v1 + session list/resume/close + HTTP-MCP.
    let started = match wait_for(
        &mut agent,
        |event| matches!(event, AgentEvent::Started(_) | AgentEvent::Disconnected { .. }),
        "Started от DSH",
    ) {
        AgentEvent::Started(started) => started,
        AgentEvent::Disconnected { reason } => {
            panic!("DSH отключился до initialize: {reason} (DSH_HOME={})", dsh_home.display())
        }
        other => panic!("ожидался Started, получено {other:?}"),
    };
    assert_eq!(started.protocol_version, 1, "ACP v1");
    assert_eq!(started.agent_name, "deepseek-harness-acp");
    assert!(started.session_list, "DSH поддерживает session/list");
    assert!(started.session_resume, "DSH поддерживает session/resume");
    assert!(started.session_close, "DSH поддерживает session/close");
    assert!(started.mcp_http, "DSH поддерживает streamable HTTP MCP (R-MCP-02)");
    assert!(!started.prompt_image, "DSH ACP: image-промптов нет (ограничение R-AGT-04)");

    // 2) session/list по cwd проекта: DSH переживает рестарты — сессий может быть 0 или больше.
    agent.list_sessions(Some(project.clone())).expect("session/list");
    let sessions = match wait_for(
        &mut agent,
        |event| matches!(event, AgentEvent::SessionsListed { .. } | AgentEvent::Error { .. }),
        "SessionsListed",
    ) {
        AgentEvent::SessionsListed { sessions, .. } => sessions,
        AgentEvent::Error { message, .. } => panic!("session/list не выполнен: {message}"),
        other => panic!("ожидался SessionsListed, получено {other:?}"),
    };
    for info in &sessions {
        assert_eq!(info.cwd, project, "session/list фильтруется по cwd проекта");
    }

    // 3) session/new с stdio-декларацией RDE-MCP (`--approval=deny` — read-only, fail-closed).
    let rde_binary = env!("CARGO_BIN_EXE_rde");
    agent
        .new_session(
            project.clone(),
            vec![McpServerSpec::Stdio {
                name: "rde".to_owned(),
                command: rde_binary.to_owned(),
                args: vec![
                    "mcp".to_owned(),
                    "serve".to_owned(),
                    "--project".to_owned(),
                    project.display().to_string(),
                    "--approval=deny".to_owned(),
                ],
                env: Vec::new(),
            }],
        )
        .expect("session/new");
    let session_id = match wait_for(
        &mut agent,
        |event| matches!(event, AgentEvent::SessionOpened { .. } | AgentEvent::Error { .. }),
        "SessionOpened от DSH",
    ) {
        AgentEvent::SessionOpened { session_id, config_options } => {
            // DSH отдаёт configOptions: модель (провайдер/группы) и reasoning_effort.
            assert!(
                config_options.iter().any(|option| option.id == "model"),
                "configOptions: model (R-AGT-02): {config_options:?}"
            );
            assert!(config_options.iter().any(|option| option.id == "reasoning_effort"));
            session_id
        }
        AgentEvent::Error { message, hint } => panic!(
            "session/new не выполнен: {message} (hint={hint:?}) — проверьте, что `rde mcp serve` стартует"
        ),
        other => panic!("ожидался SessionOpened, получено {other:?}"),
    };
    assert!(!session_id.is_empty());

    // 4) session/close: сессия закрывается, соединение остаётся живым.
    agent.close_session().expect("session/close");
    match wait_for(
        &mut agent,
        |event| matches!(event, AgentEvent::SessionClosed { .. } | AgentEvent::Error { .. }),
        "SessionClosed",
    ) {
        AgentEvent::SessionClosed { session_id: closed } => assert_eq!(closed, session_id),
        AgentEvent::Error { message, .. } => panic!("session/close не выполнен: {message}"),
        other => panic!("ожидался SessionClosed, получено {other:?}"),
    }

    agent.shutdown();
    drop(agent);
    let _ = std::fs::remove_dir_all(&project);
    let _ = std::fs::remove_dir_all(&dsh_home);
}
