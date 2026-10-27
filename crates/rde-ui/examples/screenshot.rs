//! Headless-скриншот окна RDE: рендерится виртуально (wgpu offscreen), без видимых
//! окон и без X11/Wayland — «второй рабочий стол» не нужен.
//!
//! `cargo run -p rde-ui --example screenshot -- /path/out.png [tab] [объект]`
//! tab: document|structure|artifacts|source|code; объект: id или часть названия.

use rde_core::demo;
use rde_ui::RdeApp;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/rde-concept.png".to_owned());
    let tab = std::env::args().nth(2);
    let select = std::env::args().nth(3);
    let mut state = RdeApp::new(demo());
    if let Some(tab) = tab {
        state.demo_select_tab(&tab);
    }
    if let Some(select) = select {
        state.demo_select_object(&select);
    }
    // 4-й аргумент — нижняя панель: 0 журнал, 1 задачи (транзакции/откат), 2 агент.
    if let Some(logs_tab) = std::env::args().nth(4).and_then(|value| value.parse::<usize>().ok()) {
        state.demo_set_logs_tab(logs_tab);
    }
    // 5-й аргумент — действие для снимка: update | insert-value | insert-figure | insert-formula …
    match std::env::args().nth(5).as_deref() {
        Some("update") => state.request_update(None, true),
        Some("insert-value") => state.demo_open_insert_dialog("value"),
        Some("insert-figure") => state.demo_open_insert_dialog("figure"),
        Some("insert-table") => state.demo_open_insert_dialog("table"),
        Some("insert-constant") => state.demo_open_insert_dialog("constant"),
        Some("insert-crossref") => state.demo_open_insert_dialog("crossref"),
        Some("insert-formula") => state.demo_open_insert_dialog("formula"),
        Some("formula-inspector") => {
            // P3: служебные объекты включены в дереве, выбран объект формулы — инспектор с
            // живым статусом и «где вставлено» (R-DOC-19); центр — вкладка «Документ».
            state.demo_show_service_objects();
            state.demo_select_object("Формула");
            state.demo_select_tab("document");
        }
        Some("app-settings") => state.demo_open_app_settings(),
        Some("artifact-inputs") => state.demo_set_artifact_section("inputs"),
        Some("artifact-generation") => state.demo_set_artifact_section("generation"),
        Some("artifact-result") => state.demo_set_artifact_section("result"),
        _ => {}
    }

    // 6-й аргумент — тема оболочки: dark | light | system (GUI фаза 2).
    if let Some(theme) = std::env::args().nth(6) {
        state.demo_set_theme(&theme);
    }

    // 7-й аргумент — высота окна: `tall` = 1400 (длинный инспектор), по умолчанию 1000.
    let height = if std::env::args().nth(7).as_deref() == Some("tall") { 1400.0 } else { 1000.0 };

    let mut harness = egui_kittest::Harness::builder()
        .with_size(egui::vec2(1600.0, height))
        .with_pixels_per_point(1.5)
        .with_max_steps(100_000)
        .wgpu()
        .build_ui_state(
            |ui, app: &mut RdeApp| {
                app.ui(ui);
            },
            state,
        );
    harness.input_mut().max_texture_side = Some(8192);
    for _ in 0..12 {
        harness.step();
    }

    match harness.render() {
        Ok(img) => {
            img.save(&out).expect("save png");
            println!("{out}");
        }
        Err(e) => {
            eprintln!("render failed: {e}");
            std::process::exit(1);
        }
    }
}
