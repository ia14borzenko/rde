//! RDE — Research Development Environment (desktop, prototype).
//!
//! Запуск: `cargo run -p rde` (нужно графическое окружение X11/Wayland).
//! Headless MCP-сервер (v0.3/M1, read-only): `rde mcp serve --project <dir>` — транспорт stdio.
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

const MCP_USAGE: &str = "использование: rde mcp serve --project <dir> | RDE_PROJECT=<dir> rde mcp serve (--policy read-only)";

/// Ошибка M1-ветки: сообщение в stderr + код возврата (2 — проект/аргументы, 1 — сервер).
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

/// `rde mcp serve --project <dir>`: headless MCP-сервер (stdio, read-only, M1).
fn run_mcp_serve(args: &[String]) -> Result<(), ServeFailure> {
    if args.first().map(String::as_str) != Some("serve") {
        return Err(ServeFailure::usage("неизвестная команда"));
    }
    let mut project: Option<PathBuf> = None;
    let mut policy_name = "read-only".to_owned();
    let mut index = 1;
    while index < args.len() {
        match args[index].as_str() {
            "--project" => {
                let value = args.get(index + 1).ok_or_else(|| ServeFailure::usage("--project без значения"))?;
                project = Some(PathBuf::from(value));
                index += 2;
            }
            "--policy" => {
                policy_name = args.get(index + 1).cloned().ok_or_else(|| ServeFailure::usage("--policy без значения"))?;
                index += 2;
            }
            other => return Err(ServeFailure::usage(format!("неизвестный аргумент `{other}`"))),
        }
    }
    if policy_name != "read-only" {
        return Err(ServeFailure::project(format!(
            "M1 поддерживает только --policy read-only (получено `{policy_name}`)"
        )));
    }
    let root = project
        .or_else(|| std::env::var_os("RDE_PROJECT").map(PathBuf::from))
        .unwrap_or_else(rde_core::demo_project_dir);
    let host = rde_host::HeadlessHost::open(&root)
        .map_err(|error| ServeFailure::project(format!("не удалось открыть проект: {error}")))?;
    let hooks = rde_mcp::HostHooks::jsonl(host.root());
    let policy = rde_mcp::McpPolicy::read_only();
    rde_mcp::serve_stdio_blocking(std::sync::Arc::new(host), hooks, policy)
        .map_err(|error| ServeFailure::server(error.to_string()))
}
