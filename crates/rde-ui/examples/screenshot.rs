//! Headless-скриншот окна RDE: рендерится виртуально (wgpu offscreen), без видимых
//! окон и без X11/Wayland — «второй рабочий стол» не нужен.
//!
//! `cargo run -p rde-ui --example screenshot -- /path/out.png [tab] [объект]`
//! tab: document|structure|artifacts|source|code; объект: id или часть названия.
//! Проект: `RDE_PROJECT=<dir>` (иначе демо-проект) — удобно снимать реальные проекты.

use std::path::Path;

use rde_ui::RdeApp;

fn main() {
    let out = std::env::args().nth(1).unwrap_or_else(|| "/tmp/rde-concept.png".to_owned());
    let tab = std::env::args().nth(2);
    let select = std::env::args().nth(3);
    let mut state = RdeApp::new(match std::env::var_os("RDE_PROJECT") {
        Some(root) => rde_core::load_project(Path::new(&root)).unwrap_or_else(|error| {
            eprintln!("RDE_PROJECT={}: {error}", Path::new(&root).display());
            std::process::exit(2);
        }),
        None => rde_core::demo(),
    });
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
        // M4.2b: настройки с включёнными мастер-галочками (видны подгалочки run/git).
        Some("app-settings-agents") => state.demo_open_app_settings_agents(),
        // M4.3: настройки с блоком «Встроенный агент (ACP)».
        Some("app-settings-agent") => state.demo_open_app_settings_agent(),
        // M4.3: панель чата встроенного ACP-агента (демо-транскрипт) и карточка permission.
        Some("agent-chat") => state.demo_open_agent_chat(),
        Some("agent-chat-permission") => state.demo_open_agent_chat_permission(),
        // M4.4: чат с чипами контекст-ссылок (§8.7.2) и окно diff-UI правок агента (R-AGT-05).
        Some("agent-chat-context") => state.demo_open_agent_chat_with_context(),
        Some("agent-suggestions") => state.demo_open_suggestions(),
        Some("source-pdf-region") => state.demo_open_pdf_region(),
        // M4.2b: диалог approval внешнего агента (run/git) и режим грантов в панели «Агент».
        Some("agent-approval") => state.demo_open_agent_approval("artifact_run"),
        Some("agent-approval-git") => state.demo_open_agent_approval("git_commit"),
        Some("agent-grants") => state.demo_set_agent_grants(true, true),
        Some("artifact-inputs") => state.demo_set_artifact_section("inputs"),
        Some("artifact-generation") => state.demo_set_artifact_section("generation"),
        Some("artifact-result") => state.demo_set_artifact_section("result"),
        // GUI-02 (ADR-0017): диалоги путей с «Проводник…», недавними проектами и валидацией.
        Some("open-project-dialog") => {
            // История «Недавние проекты» — демо-записи (пути могут быть не на этой машине).
            state.demo_set_recent_projects(vec![
                rde_core::RecentProject {
                    path: "/home/user/Документы/rdeprojects/вкр-2026".to_owned(),
                    name: "ВКР — исследование РСА".to_owned(),
                    opened_unix: 1_790_000_000,
                },
                rde_core::RecentProject {
                    path: "/home/user/Документы/rdeprojects/статья-радиолокация".to_owned(),
                    name: "Статья: радиолокация".to_owned(),
                    opened_unix: 1_789_000_000,
                },
                rde_core::RecentProject {
                    path: "/home/user/Документы/rdeprojects/учебный-проект".to_owned(),
                    name: "Учебный проект".to_owned(),
                    opened_unix: 1_788_000_000,
                },
            ]);
            state.demo_open_open_project_dialog();
        }
        Some("import-docx-dialog") => {
            // Существующий .docx (фикстура RDE) — «зелёная» проверка пути.
            let fixture = std::env::current_dir()
                .map(|dir| dir.join("crates/rde-doc/tests/fixtures/base-report.docx"))
                .unwrap_or_default();
            state.demo_open_import_docx_dialog(&fixture.display().to_string());
        }
        Some("import-docx-dialog-invalid") => {
            // Опечатка в пути — понятная ошибка до действия (красная подпись, «Импортировать» disabled).
            state.demo_open_import_docx_dialog("/home/user/Документы/отчёт-2026.doc");
        }
        Some("new-project-dialog") => {
            let path = state.project.root.join("новый-проект");
            state.demo_open_new_project_dialog(&path.display().to_string());
        }
        Some("import-citations-dialog") => {
            // Демо-файл BibTeX во временном каталоге: показывает валидный путь.
            let path = std::env::temp_dir().join("rde-demo-citations.bib");
            let _ = std::fs::write(
                &path,
                "@book{cumming2005,\n  author = {Cumming, I. G. and Wong, F. H.},\n  title = {Digital Processing of Synthetic Aperture Radar Data},\n  year = {2005}\n}\n",
            );
            state.demo_open_import_citations_dialog(&path.display().to_string());
        }
        Some("import-review-dialog") => state.demo_open_import_review_dialog(""),
        Some("source-edit-dialog") => state.demo_open_source_edit(),
        Some("repository-edit-dialog") => state.demo_open_repository_edit(),
        _ => {}
    }

    // 8-й аргумент — режим области документа: docview | full | preview (для скриншотов).
    if let Some(mode) = std::env::args().nth(8) {
        state.demo_set_doc_mode(&mode);
    }
    // 9-й аргумент — вертикальный скролл предпросмотра страниц (например, 2-я страница).
    if let Some(offset) = std::env::args().nth(9).and_then(|value| value.parse::<f32>().ok()) {
        state.demo_set_preview_scroll(offset);
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
    let steps = std::env::var("RDE_SCREENSHOT_STEPS").ok().and_then(|v| v.parse().ok()).unwrap_or(12);
    for _ in 0..steps {
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
