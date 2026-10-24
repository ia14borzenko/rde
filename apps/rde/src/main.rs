//! RDE — Research Development Environment (desktop, prototype).
//!
//! Запуск: `cargo run -p rde` (нужно графическое окружение X11/Wayland).
//! Концепт-скриншот без окна: `cargo run -p rde-ui --example screenshot -- out.png`.

use eframe::egui;
use rde_ui::{APP_TITLE, RdeApp};

fn main() -> eframe::Result<()> {
    let project = rde_core::demo();
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
