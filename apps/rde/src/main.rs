//! RDE — Research Development Environment (desktop, prototype).
//!
//! Запуск: `cargo run -p rde` (нужно графическое окружение X11/Wayland).
//! Headless MCP-сервер (v0.3/M2): `rde mcp serve --project <dir> [--approval=…]` — транспорт stdio;
//! мутации — только через grants/approvals/чекпоинты (по умолчанию `--approval=deny`, fail-closed).
//! Концепт-скриншот без окна: `cargo run -p rde-ui --example screenshot -- out.png`.

use std::path::PathBuf;

use eframe::egui;
use rde_ui::{APP_TITLE, RdeApp};

fn main() -> eframe::Result<()> {
    // Проект: аргумент `--project <dir>` / первый путь / переменная RDE_PROJECT / демо-проект.
    let args: Vec<String> = std::env::args().skip(1).collect();
    // MCP-ветка (v0.3/M1): headless-сервер, GUI не создаётся.
    if let Some(exit_code) = maybe_run_mcp(&args) {
        std::process::exit(exit_code);
    }
    let from_args = args
        .iter()
        .position(|a| a == "--project")
        .and_then(|i| args.get(i + 1).cloned())
        .or_else(|| args.iter().find(|a| !a.starts_with('-')).cloned());
    let root = from_args
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("RDE_PROJECT").map(PathBuf::from))
        .unwrap_or_else(rde_core::demo_project_dir);
    let project = match rde_core::load(&root) {
        Ok(project) => project,
        Err(e) => {
            eprintln!("RDE: не удалось загрузить проект {}: {e}", root.display());
            std::process::exit(1);
        }
    };
    eprintln!(
        "RDE: проект «{}» — {} объектов, {} устаревших, предупреждений: {}",
        project.name,
        project.object_count(),
        project.stale_count(),
        project.issues.len()
    );
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1600.0, 1000.0])
            .with_min_inner_size([1100.0, 700.0])
            .with_title(APP_TITLE),
        ..Default::default()
    };
    eframe::run_native(
        "RDE",
        options,
        Box::new(|cc| {
            let _ = cc;
            Ok(Box::new(RdeFrame(RdeApp::new(project))))
        }),
    )
}

struct RdeFrame(RdeApp);

impl eframe::App for RdeFrame {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.0.ui(ui);
    }
}

const MCP_USAGE: &str = "использование: rde mcp serve --project <dir> [--transport stdio|http] [--bind 127.0.0.1:8765] [--token <…>|--token-file <…>] [--approval=deny|auto|allowlist] [--allow-tools=a,b] [--allow-run] [--allow-destructive]\n  stdio — дефолт (встроенный агент); http — streamable HTTP только на loopback + обязательный bearer-токен (M3/R-MCP-02), по умолчанию выключен; approvals по умолчанию deny (fail-closed)";

/// Ошибка MCP-ветки: сообщение в stderr + код возврата (2 — проект/аргументы, 1 — сервер).
struct ServeFailure {
    message: String,
    code: i32,
}

impl ServeFailure {
    fn usage(message: impl Into<String>) -> Self {
        Self { message: format!("{}; {MCP_USAGE}", message.into()), code: 2 }
    }

    fn project(message: impl Into<String>) -> Self {
        Self { message: message.into(), code: 2 }
    }

    fn server(message: impl Into<String>) -> Self {
        Self { message: message.into(), code: 1 }
    }
}

/// Распознать `rde mcp …` до `eframe`: `Some(код выхода)`, если MCP-ветка запускалась.
fn maybe_run_mcp(args: &[String]) -> Option<i32> {
    if args.first().map(String::as_str) != Some("mcp") {
        return None;
    }
    match run_mcp_serve(&args[1..]) {
        Ok(()) => Some(0),
        Err(failure) => {
            eprintln!("RDE MCP: {}", failure.message);
            Some(failure.code)
        }
    }
}

/// Значение флага `--name=value` или `--name value`.
fn flag_value(args: &[String], index: usize, name: &str) -> Result<(String, usize), ServeFailure> {
    let current = &args[index];
    if let Some(value) = current.strip_prefix(&format!("{name}=")) {
        return Ok((value.to_owned(), index + 1));
    }
    let value = args.get(index + 1).ok_or_else(|| ServeFailure::usage(format!("{name} без значения")))?;
    Ok((value.clone(), index + 2))
}

/// `rde mcp serve --project <dir>`: headless MCP-сервер (stdio по умолчанию; M3 — streamable HTTP).
///
/// Approvals — **headless и fail-closed** (A4, O2-a): по умолчанию `--approval=deny` (никаких
/// мутаций), `auto` включает гранты (явный флаг!), `allowlist` — только перечисленные инструменты.
/// Интерактивных prompt'ов нет (stdin занят JSON-RPC); GUI/ACP-диалоги — M4 через тот же хук.
///
/// Транспорт M3 (`--transport http`) — streamable HTTP **только на loopback** и только с
/// bearer-токеном (R-MCP-02): без `--token`/`--token-file`/`RDE_MCP_TOKEN` токен генерируется и
/// печатается в stderr один раз; HTTP выключен по умолчанию (никаких открытых портов без флага).
fn run_mcp_serve(args: &[String]) -> Result<(), ServeFailure> {
    if args.first().map(String::as_str) != Some("serve") {
        return Err(ServeFailure::usage("неизвестная команда"));
    }
    let mut project: Option<PathBuf> = None;
    let mut approval_mode: Option<rde_mcp::ApprovalMode> = None;
    let mut allow_tools: Vec<String> = Vec::new();
    let mut allow_run = false;
    let mut allow_destructive = false;
    let mut legacy_read_only = false;
    let mut transport = "stdio".to_owned();
    let mut bind: Option<String> = None;
    let mut token: Option<String> = None;
    let mut token_file: Option<PathBuf> = None;
    let mut index = 1;
    while index < args.len() {
        let arg = args[index].clone();
        let arg_name = arg.split('=').next().unwrap_or(&arg).to_owned();
        match arg_name.as_str() {
            "--project" => {
                let (value, next) = flag_value(args, index, "--project")?;
                project = Some(PathBuf::from(value));
                index = next;
            }
            "--transport" => {
                let (value, next) = flag_value(args, index, "--transport")?;
                transport = value;
                index = next;
            }
            "--bind" => {
                let (value, next) = flag_value(args, index, "--bind")?;
                bind = Some(value);
                index = next;
            }
            "--token" => {
                let (value, next) = flag_value(args, index, "--token")?;
                token = Some(value);
                index = next;
            }
            "--token-file" => {
                let (value, next) = flag_value(args, index, "--token-file")?;
                token_file = Some(PathBuf::from(value));
                index = next;
            }
            "--approval" => {
                let (value, next) = flag_value(args, index, "--approval")?;
                approval_mode = Some(
                    rde_mcp::ApprovalMode::parse(&value).map_err(ServeFailure::usage)?,
                );
                index = next;
            }
            "--allow-tools" => {
                let (value, next) = flag_value(args, index, "--allow-tools")?;
                allow_tools = value
                    .split(',')
                    .map(str::trim)
                    .filter(|tool| !tool.is_empty())
                    .map(str::to_owned)
                    .collect();
                index = next;
            }
            "--allow-run" => {
                allow_run = true;
                index += 1;
            }
            "--allow-destructive" => {
                allow_destructive = true;
                index += 1;
            }
            // Совместимость с M1: `--policy read-only` = deny.
            "--policy" => {
                let (value, next) = flag_value(args, index, "--policy")?;
                if value != "read-only" {
                    return Err(ServeFailure::usage(format!(
                        "M2: используйте --approval=deny|auto|allowlist вместо --policy {value}"
                    )));
                }
                legacy_read_only = true;
                index = next;
            }
            other => return Err(ServeFailure::usage(format!("неизвестный аргумент `{other}`"))),
        }
    }
    if legacy_read_only && (approval_mode.is_some() || !allow_tools.is_empty() || allow_run || allow_destructive) {
        return Err(ServeFailure::usage("--policy read-only не сочетается с флагами approvals"));
    }
    let transport = transport.trim().to_ascii_lowercase();
    let http_options = match transport.as_str() {
        "stdio" => {
            if bind.is_some() || token.is_some() || token_file.is_some() {
                return Err(ServeFailure::usage(
                    "--bind/--token/--token-file применимы только с --transport http",
                ));
            }
            None
        }
        "http" => {
            if token.is_some() && token_file.is_some() {
                return Err(ServeFailure::usage("укажите что-то одно: --token или --token-file"));
            }
            let token = match (token, token_file) {
                (Some(token), _) => Some(token),
                (None, Some(path)) => {
                    let text = std::fs::read_to_string(&path).map_err(|error| {
                        ServeFailure::project(format!(
                            "не удалось прочитать --token-file {}: {error}",
                            path.display()
                        ))
                    })?;
                    let text = text.trim().to_owned();
                    if text.is_empty() {
                        return Err(ServeFailure::project(format!(
                            "файл токена {} пуст",
                            path.display()
                        )));
                    }
                    Some(text)
                }
                (None, None) => std::env::var("RDE_MCP_TOKEN").ok().filter(|token| !token.trim().is_empty()),
            };
            let bind = bind.unwrap_or_else(|| "127.0.0.1:8765".to_owned());
            // Конфигурационные ошибки — код 2 (до открытия проекта); сервер проверяет ещё раз.
            rde_mcp::validate_bind(&bind).map_err(ServeFailure::usage)?;
            if let Some(token) = &token {
                rde_mcp::validate_token(token).map_err(ServeFailure::usage)?;
            }
            Some(rde_mcp::HttpOptions { bind, token })
        }
        other => {
            return Err(ServeFailure::usage(format!(
                "неизвестный транспорт `{other}` (stdio|http)"
            )));
        }
    };
    let options = rde_mcp::ApprovalOptions {
        mode: Some(approval_mode.unwrap_or(rde_mcp::ApprovalMode::Deny)),
        allow_tools,
        allow_run,
        allow_destructive,
    };
    let (policy, approval_hook) =
        rde_mcp::build_policy_and_hook(&options).map_err(ServeFailure::usage)?;
    let root = project
        .or_else(|| std::env::var_os("RDE_PROJECT").map(PathBuf::from))
        .unwrap_or_else(rde_core::demo_project_dir);
    let host = rde_host::HeadlessHost::open(&root)
        .map_err(|error| ServeFailure::project(format!("не удалось открыть проект: {error}")))?;
    // Fail-closed по умолчанию: без флагов любая мутация отклоняется политикой и DenyAllHook.
    if policy.approval_mode == "deny" {
        eprintln!("RDE MCP: approvals=deny (fail-closed): мутации и запуск скриптов запрещены");
    } else {
        eprintln!("RDE MCP: ВНИМАНИЕ: approvals={}, мутации разрешаются без интерактивного подтверждения (headless); аудит — .rde/agent/audit.log", policy.approval_mode);
        if policy.approval_mode == "auto" && !rde_core::load(host.root()).map(|project| project.manifest.trusted).unwrap_or(false) {
            eprintln!("RDE MCP: проект не доверенный (trusted=false): artifact_run будет отклонён (R-RUN-06)");
        }
    }
    let hooks = rde_mcp::HostHooks::jsonl(host.root()).with_approvals(approval_hook);
    let host = std::sync::Arc::new(host);
    match http_options {
        None => rde_mcp::serve_stdio_blocking(host, hooks, policy)
            .map_err(|error| ServeFailure::server(error.to_string())),
        Some(http) => rde_mcp::serve_http_blocking(host, hooks, policy, http)
            .map_err(|error| ServeFailure::server(error.to_string())),
    }
}
