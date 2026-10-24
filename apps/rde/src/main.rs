//! RDE — Research Development Environment (desktop, prototype).
//!
//! Запуск: `cargo run -p rde` (нужно графическое окружение X11/Wayland).
//! Концепт-скриншот без окна: `cargo run -p rde-ui --example screenshot -- out.png`.

use std::path::PathBuf;

use eframe::egui;
use rde_ui::{APP_TITLE, RdeApp};

fn main() -> eframe::Result<()> {
    // Проект: аргумент `--project <dir>` / первый путь / переменная RDE_PROJECT / демо-проект.
    let args: Vec<String> = std::env::args().skip(1).collect();
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
