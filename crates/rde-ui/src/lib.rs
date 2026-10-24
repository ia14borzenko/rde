//! RDE prototype UI shell.
//!
//! Цель этой итерации — «общий вид концепции»: окно IDE-подобной среды, где
//! документ — один из объектов проекта, а рядом живут дерево объектов, DAG
//! артефактов, источники, журнал и панель агента. Документ показывается
//! **по-настоящему**: `.docx` читается движком WordCraft, размечается его
//! layout-движком и растеризуется в текстуры страниц.
//!
//! Что здесь настоящее (M2.1), а что ещё mock:
//! - настоящее: дерево/статусы/журнал/источники загружаются из реального проекта на диске
//!   (`rde.toml` + `objects/**`); документ рендерится/редактируется движком WordCraft;
//!   DAG-панель рисуется по реальным `hashed`-зависимостям манифестов;
//! - mock: чат агента, вкладки «Структура»/«Код документа», «Обновить связанные» —
//!   заменяются в M2.2/M2.3 (store/CAS, hash/DAG, run).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};
use rde_core::{Freshness, Level, ObjectData, ObjectKind, Project};
use rde_dag::{LiveStatus, UpdateAction};
use rde_store::Store;

pub const APP_TITLE: &str = "RDE — Research Development Environment";

// ── палитра (тёмная тема, близкая к IDE) ─────────────────────────────────────
const BG: Color32 = Color32::from_rgb(0x16, 0x17, 0x1b);
const PANEL: Color32 = Color32::from_rgb(0x1d, 0x1f, 0x24);
const PANEL_HI: Color32 = Color32::from_rgb(0x25, 0x28, 0x30);
const BORDER: Color32 = Color32::from_rgb(0x32, 0x36, 0x40);
const TEXT: Color32 = Color32::from_rgb(0xd8, 0xdb, 0xe2);
const MUTED: Color32 = Color32::from_rgb(0x8d, 0x93, 0xa0);
const ACCENT: Color32 = Color32::from_rgb(0x51, 0x8c, 0xff);
const GREEN: Color32 = Color32::from_rgb(0x52, 0xc4, 0x7d);
const AMBER: Color32 = Color32::from_rgb(0xe3, 0xa7, 0x3e);
const RED: Color32 = Color32::from_rgb(0xe0, 0x60, 0x60);

/// Вкладки центральной области.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Document,
    Structure,
    Artifacts,
    Source,
    Code,
}

impl Tab {
    const ALL: [Tab; 5] = [Tab::Document, Tab::Structure, Tab::Artifacts, Tab::Source, Tab::Code];

    fn title(self) -> &'static str {
        match self {
            Tab::Document => "Документ",
            Tab::Structure => "Структура",
            Tab::Artifacts => "Артефакты",
            Tab::Source => "Источник",
            Tab::Code => "Код документа",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Document => "",
            Tab::Structure => "",
            Tab::Artifacts => "",
            Tab::Source => "",
            Tab::Code => "</>",
        }
    }
}

/// Состояние предпросмотра документа.
enum DocState {
    Pending,
    Ready(ReadyDoc),
    Failed(String),
}

struct ReadyDoc {
    pages: Vec<egui::TextureHandle>,
    page_pt: Vec2,
    count: usize,
}

/// Предпросмотр документа: ленивая загрузка и растеризация страниц движком WordCraft.
struct DocPreview {
    path: Option<std::path::PathBuf>,
    state: DocState,
}

impl DocPreview {
    fn new(path: Option<std::path::PathBuf>) -> Self {
        let state = if path.is_some() { DocState::Pending } else { DocState::Failed("в проекте нет документа".to_owned()) };
        Self { path, state }
    }

    fn ensure(&mut self, ctx: &egui::Context) {
        if !matches!(self.state, DocState::Pending) {
            return;
        }
        let Some(path) = self.path.clone() else {
            self.state = DocState::Failed("в проекте нет документа".to_owned());
            return;
        };
        match load_and_render(&path, ctx) {
            Ok((pages, page_pt, count)) => self.state = DocState::Ready(ReadyDoc { pages, page_pt, count }),
            Err(e) => self.state = DocState::Failed(e),
        }
    }
}

/// Открыть `.docx`, разметить и растеризовать первые страницы через WordCraft.
fn load_and_render(path: &Path, ctx: &egui::Context) -> Result<(Vec<egui::TextureHandle>, Vec2, usize), String> {
    let region = wordcraft_engine::Region::from_tag("ru-RU");
    let (doc, _encrypted) = wordcraft_engine::io::open_path_in(path, None, &region)?;
    let mut session = wordcraft_engine::Session::new(doc);
    let layout = session.layout();
    if layout.pages.is_empty() {
        return Err("в документе нет страниц".to_owned());
    }
    let page_pt = vec2(layout.pages[0].w, layout.pages[0].h);
    let opts = wordcraft_engine::render::RenderOptions::default();
    let scale = 1.25_f32;
    let mut pages = Vec::new();
    for (i, page) in layout.pages.iter().take(6).enumerate() {
        let img = wordcraft_engine::render::render_page(&session.doc, page, scale, &opts);
        if img.width == 0 || img.height == 0 {
            continue;
        }
        let color = egui::ColorImage::from_rgba_premultiplied([img.width as usize, img.height as usize], &img.pixels);
        pages.push(ctx.load_texture(format!("rde-doc-page-{i}"), color, egui::TextureOptions::LINEAR));
    }
    Ok((pages, page_pt, layout.pages.len()))
}

/// Режим области документа.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DocMode {
    /// Чистый редактор: canvas WordCraft без его ribbon/titlebar (RDE-патч `rde-docview`).
    DocView,
    /// Полный интерфейс WordCraft, встроенный как панель (для сравнения/отладки).
    Full,
    /// Растеризованный предпросмотр страниц (лёгкий, без UI-стека WordCraft).
    Preview,
}

impl DocMode {
    const ALL: [DocMode; 3] = [DocMode::DocView, DocMode::Full, DocMode::Preview];

    fn label(self) -> &'static str {
        match self {
            DocMode::DocView => "Редактор RDE (DocView)",
            DocMode::Full => "Полный WordCraft",
            DocMode::Preview => "Предпросмотр страниц",
        }
    }
}

/// Встроенный редактор WordCraft: полноценный `WordApp` (ribbon + canvas + ввод).
struct EmbeddedEditor {
    app: wordcraft_ui_egui::WordApp,
    pages: usize,
}

/// Приложение прототипа RDE.
pub struct RdeApp {
    pub project: Project,
    tab: Tab,
    tree_filter: String,
    selected: Option<String>,
    doc: DocPreview,
    zoom: f32,
    agent_input: String,
    suggestion_open: bool,
    logs_tab: usize,
    fonts_installed: bool,
    // видимость панелей (скрываются через меню «Вид» или кнопкой × в заголовке)
    show_tree: bool,
    show_agent: bool,
    show_logs: bool,
    doc_mode: DocMode,
    editor: Option<EmbeddedEditor>,
    editor_error: Option<String>,
    /// Куда RDE сохраняет явные экспорты (редактор пока работает с временной копией).
    export_dir: PathBuf,
    /// Хранилище проекта (CAS/журнал транзакций; открывается при старте).
    store: Option<Store>,
    /// Статусы артефактов из DAG (act_key/out_hash/выходы), наложенные на модель проекта.
    dag_statuses: BTreeMap<String, rde_dag::ArtifactStatus>,
}

impl RdeApp {
    pub fn new(project: Project) -> Self {
        let doc = DocPreview::new(project.document_path());
        let export_dir = project.root.join("exports");
        let mut app = Self {
            project,
            tab: Tab::Document,
            tree_filter: String::new(),
            selected: Some("doc-report".to_owned()),
            doc,
            zoom: 0.72,
            agent_input: String::new(),
            suggestion_open: true,
            logs_tab: 0,
            fonts_installed: false,
            show_tree: true,
            show_agent: true,
            show_logs: true,
            doc_mode: DocMode::DocView,
            editor: None,
            editor_error: None,
            export_dir,
            store: None,
            dag_statuses: BTreeMap::new(),
        };
        match Store::open(&app.project.root) {
            Ok(store) => {
                app.store = Some(store);
                app.refresh_dag_statuses();
            }
            Err(error) => app.push_log(Level::Warn, format!("Хранилище проекта недоступно: {error}")),
        }
        app
    }

    /// Пересчитать статусы артефактов из фактов (act_key/out_hash/выходы) и наложить на модель.
    pub fn refresh_dag_statuses(&mut self) {
        let Some(store) = self.store.as_ref() else { return };
        let statuses = match rde_dag::evaluate(&self.project, store) {
            Ok(statuses) => statuses,
            Err(error) => {
                self.push_log(Level::Warn, format!("DAG-статусы: {error}"));
                return;
            }
        };
        let mut overlays: Vec<(String, Freshness, String)> = Vec::new();
        for object in &self.project.objects {
            let ObjectData::Artifact(artifact) = &object.data else { continue };
            let Some(status) = statuses.get(&artifact.id) else { continue };
            let freshness = match status.status {
                LiveStatus::Fresh => Freshness::UpToDate,
                LiveStatus::Stale(_) | LiveStatus::NeverRun => Freshness::Stale,
                LiveStatus::NotComputable(_) => Freshness::Failed,
            };
            let detail = match &status.status {
                LiveStatus::Fresh => object.detail.clone(),
                live => format!("{} · {}", artifact.kind, live.reason().unwrap_or_else(|| live.label_ru())),
            };
            overlays.push((artifact.id.clone(), freshness, detail));
        }
        for (id, freshness, detail) in overlays {
            if let Some(object) = self.project.objects.iter_mut().find(|o| o.id == id) {
                object.freshness = freshness;
                object.detail = detail;
            }
        }
        self.dag_statuses = statuses;
    }

    /// Перезагрузить проект с диска и обновить статусы/документ.
    fn reload_project(&mut self) {
        match rde_core::load_project(&self.project.root) {
            Ok(project) => {
                let new_path = project.document_path();
                let path_changed = new_path != self.doc.path;
                self.project = project;
                if path_changed {
                    self.doc = DocPreview::new(new_path);
                }
                self.refresh_dag_statuses();
            }
            Err(error) => self.push_log(Level::Warn, format!("Не удалось перезагрузить проект: {error}")),
        }
    }

    /// Реальное обновление артефактов через DAG/транзакции (`targets=None` — все устаревшие).
    pub fn update_dependencies(&mut self, targets: Option<Vec<String>>) -> Option<rde_dag::UpdateReport> {
        if self.store.is_none() {
            self.push_log(Level::Warn, "Обновление невозможно: хранилище проекта не открыто");
            return None;
        }
        let options = rde_dag::UpdateOptions { targets, continue_on_error: false };
        let result = {
            let store = self.store.as_mut().expect("store проверен выше");
            rde_dag::update(&self.project, store, &options)
        };
        let report = match result {
            Ok(report) => report,
            Err(error) => {
                self.push_log(Level::Warn, format!("Обновление не удалось: {error}"));
                return None;
            }
        };
        self.reload_project();
        for artifact in &report.artifacts {
            let text = match artifact.action {
                UpdateAction::Ran => format!("Артефакт «{}»: {} (act_key {})", artifact.title, artifact.message, artifact.act_key.as_deref().map(|key| &key[..12.min(key.len())]).unwrap_or("—")),
                UpdateAction::Skipped => format!("Артефакт «{}»: актуален, запуск не нужен", artifact.title),
                UpdateAction::Failed => format!("Артефакт «{}»: ошибка — {}", artifact.title, artifact.message),
            };
            let level = match artifact.action {
                UpdateAction::Ran => Level::Ok,
                UpdateAction::Skipped => Level::Info,
                UpdateAction::Failed => Level::Warn,
            };
            self.push_log(level, text);
        }
        self.push_log(
            if report.failed() > 0 { Level::Warn } else { Level::Ok },
            format!("Обновление связанных: выполнено {}, пропущено {}, ошибок {}", report.ran(), report.skipped(), report.failed()),
        );
        Some(report)
    }

    /// Для демо/скриншотов: переключить центральную вкладку по имени
    /// (`document|structure|artifacts|source|code`).
    pub fn demo_select_tab(&mut self, name: &str) {
        self.tab = match name {
            "structure" => Tab::Structure,
            "artifacts" => Tab::Artifacts,
            "source" => Tab::Source,
            "code" => Tab::Code,
            _ => Tab::Document,
        };
    }

    /// Сколько страниц в документе (из редактора или предпросмотра), если известно.
    fn doc_pages(&self) -> Option<usize> {
        if let Some(ed) = &self.editor {
            return Some(ed.pages);
        }
        if let DocState::Ready(doc) = &self.doc.state {
            return Some(doc.count);
        }
        None
    }

    /// Сохранить текущее содержимое встроенного редактора средствами RDE (владелец сохранения — RDE).
    /// Пока это .docx-экспорт в `exports/`; сохранение в `.doc.rde`/проект появится с `rde:doc`.
    pub fn save_editor_copy(&mut self) {
        let msg = {
            let Some(ed) = &self.editor else {
                return;
            };
            let dir = self.export_dir.clone();
            match std::fs::create_dir_all(&dir) {
                Ok(()) => {
                    let path = dir.join("demo-report-edited.docx");
                    match wordcraft_engine::io::save_path(&path, &ed.app.session.doc) {
                        Ok(()) => (Level::Ok, format!("Экспорт .docx (RDE): {} (шрифты/оформление — дальше с шаблонами)", path.display())),
                        Err(e) => (Level::Warn, format!("Не удалось сохранить документ: {e}")),
                    }
                }
                Err(e) => (Level::Warn, format!("Не удалось создать каталог {}: {e}", dir.display())),
            }
        };
        self.push_log(msg.0, msg.1);
    }

    /// Лениво создать встроенный редактор WordCraft с тем же документом, что и предпросмотр.
    fn ensure_editor(&mut self) {
        if self.editor.is_some() || self.editor_error.is_some() {
            return;
        }
        let Some(doc_path) = self.doc.path.clone() else {
            self.editor_error = Some("в проекте нет документа".to_owned());
            return;
        };
        let region = wordcraft_engine::Region::from_tag("ru-RU");
        // Встроенный WordApp умеет сам сохранять документ (autosave/сохранение при выходе).
        // Пока у RDE нет своего слоя сохранения проекта (rde:doc/транзакции — следующая веха),
        // открываем временную копию: правки редактора не могут испортить файлы проекта.
        let copy = std::env::temp_dir().join("rde-embedded-doc.docx");
        if let Ok(bytes) = std::fs::read(&doc_path) {
            let _ = std::fs::write(&copy, bytes);
        }
        let edit_path = if copy.is_file() { copy } else { doc_path };
        match wordcraft_engine::io::open_path_in(&edit_path, None, &region) {
            Ok((doc, _encrypted)) => {
                let mut session = wordcraft_engine::Session::new(doc);
                session.path = Some(edit_path.clone());
                let mut app = wordcraft_ui_egui::WordApp::new(session, wordcraft_ui_egui::Services::default());
                app.integrated_titlebar = false;
                app.ui.theme = wordcraft_ui_egui::theme::Appearance::Dark;
                // RDE owns saving: no WordCraft autosave recovery writes behind its back.
                app.autosave = false;
                let pages = app.session.layout().pages.len();
                self.editor = Some(EmbeddedEditor { app, pages });
            }
            Err(e) => self.editor_error = Some(e),
        }
    }

    /// Нарисовать всё окно. Вызывается и из eframe, и из headless-харнесса скриншотов.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if !self.fonts_installed {
            install_fonts(ui.ctx());
            install_style(ui.ctx());
            self.fonts_installed = true;
        }
        if self.editor.is_some() {
            // WordCraft's own UI applies its theme/fonts globally in `logic`; restore the
            // RDE shell visuals each frame so the environment keeps a stable look. WordCraft
            // draws itself from the effective dark palette (`Tokens::get`), so both match.
            install_style(ui.ctx());
        }
        self.menu_bar(ui);
        self.toolbar(ui);
        self.status_bar(ui);
        if self.show_logs {
            self.logs_panel(ui);
        }
        if self.show_tree {
            self.tree_panel(ui);
        }
        if self.show_agent {
            self.agent_panel(ui);
        }
        self.central(ui);
    }

    fn push_log(&mut self, level: Level, text: impl Into<String>) {
        self.project.log.push(rde_core::LogLine { time: "сейчас", level, text: text.into() });
    }

    // ── верхнее меню ────────────────────────────────────────────────────────
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let project_name = self.project.name.clone();
        egui::Panel::top("rde_menu")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 2)))
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("Файл", |ui| {
                        let _ = ui.button("Новый проект…");
                        let _ = ui.button("Открыть проект…");
                        ui.separator();
                        let _ = ui.button("Экспорт .docx");
                        let _ = ui.button("Экспорт проекта (.rde)");
                    });
                    ui.menu_button("Правка", |ui| {
                        let _ = ui.button("Отменить");
                        let _ = ui.button("Повторить");
                        ui.separator();
                        let _ = ui.button("Найти в проекте…");
                    });
                    ui.menu_button("Проект", |ui| {
                        let _ = ui.button("Проверить и починить…");
                        let _ = ui.button("Обновить все артефакты");
                        let _ = ui.button("Настройки проекта…");
                    });
                    ui.menu_button("Вид", |ui| {
                        ui.checkbox(&mut self.show_tree, "Дерево проекта");
                        ui.checkbox(&mut self.show_agent, "Панель агента");
                        ui.checkbox(&mut self.show_logs, "Журнал и задачи");
                    });
                    ui.menu_button("Агент", |ui| {
                        let _ = ui.button("Новая сессия");
                        let _ = ui.button("Проверить подключение (ACP)");
                        let _ = ui.button("Аудит действий…");
                    });
                    ui.menu_button("Помощь", |ui| {
                        let _ = ui.button("О прототипе");
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("RDE · прототип v0.1").small().color(MUTED));
                        ui.add_space(6.0);
                        ui.label(RichText::new(&project_name).small().color(TEXT));
                    });
                });
            });
    }

    // ── панель действий ─────────────────────────────────────────────────────
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let mut update_all = false;
        let mut export = false;
        let mut new_object = false;
        egui::Panel::top("rde_toolbar")
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::symmetric(10, 4)))
            .exact_size(36.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(egui::Button::new(RichText::new("Обновить всё").color(Color32::WHITE)).fill(ACCENT))
                        .on_hover_text("Реально пересчитать устаревшие артефакты (rde-dag: act_key/out_hash, транзакции)")
                        .clicked()
                    {
                        update_all = true;
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("Экспорт .docx").color(TEXT)).fill(PANEL_HI))
                        .on_hover_text("Собрать чистый .docx из .doc.rde")
                        .clicked()
                    {
                        export = true;
                    }
                    ui.separator();
                    if ui.button(RichText::new("+ Объект").size(13.0)).clicked() {
                        new_object = true;
                    }
                    let _ = ui.button(RichText::new("Поиск").size(13.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format!("• {} устаревших объектов", self.project.stale_count())).small().color(AMBER));
                        ui.separator();
                        ui.label(RichText::new("MCP: off").small().color(MUTED));
                    });
                });
            });
        if update_all {
            self.update_dependencies(None);
        }
        if export {
            self.ensure_editor();
            self.save_editor_copy();
        }
        if new_object {
            self.push_log(Level::Info, "Создание объекта: мастер появится вместе с rde-core");
        }
    }

    // ── статус-бар ──────────────────────────────────────────────────────────
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("rde_status")
            .frame(egui::Frame::new().fill(PANEL_HI).inner_margin(Margin::symmetric(10, 3)))
            .exact_size(24.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("•  RDE").color(ACCENT).size(11.0));
                    ui.label(RichText::new("прототип v0.1").small().color(MUTED));
                    ui.separator();
                    ui.label(RichText::new(self.project.root.display().to_string()).small().color(MUTED));
                    ui.separator();
                    ui.label(RichText::new(format!("git: {}", self.project.branch)).small().color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("A4 · ru-RU").small().color(MUTED));
                        ui.separator();
                        ui.label(RichText::new(format!("{} объектов", self.project.object_count())).small().color(MUTED));
                        ui.separator();
                        ui.label(RichText::new(format!("• {} устаревших", self.project.stale_count())).small().color(AMBER));
                        ui.separator();
                        ui.label(RichText::new("Агент: ожидает").small().color(GREEN));
                    });
                });
            });
    }

    // ── журнал / задачи ─────────────────────────────────────────────────────
    fn logs_panel(&mut self, ui: &mut egui::Ui) {
        let mut tab = self.logs_tab;
        let mut hide_logs = false;
        let log = &self.project.log;
        egui::Panel::bottom("rde_logs")
            .resizable(true)
            .default_size(142.0)
            .min_size(56.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 6)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("ПАНЕЛЬ").small().strong().color(MUTED));
                    ui.add_space(4.0);
                    for (i, name) in ["Журнал", "Задачи", "Агент"].iter().enumerate() {
                        if ui.selectable_label(tab == i, RichText::new(*name).size(12.0)).clicked() {
                            tab = i;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Скрыть панель (вернуть — меню «Вид»)").clicked() {
                            hide_logs = true;
                        }
                        let _ = ui.small_button("очистить");
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("rde_logs_scroll")
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for line in log {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(line.time).monospace().size(10.5).color(MUTED));
                                let color = match line.level {
                                    Level::Info => MUTED,
                                    Level::Ok => GREEN,
                                    Level::Warn => AMBER,
                                };
                                ui.label(RichText::new("•").color(color).size(11.0));
                                ui.label(RichText::new(&line.text).size(12.0).color(TEXT));
                            });
                        }
                    });
            });
        self.logs_tab = tab;
        if hide_logs {
            self.show_logs = false;
        }
    }

    // ── дерево проекта ──────────────────────────────────────────────────────
    fn tree_panel(&mut self, ui: &mut egui::Ui) {
        let groups = self.project.groups.clone();
        let objects = self.project.objects.clone();
        let object_count = self.project.object_count();
        let filter = self.tree_filter.to_lowercase();
        let selected = self.selected.clone();
        let mut clicked: Option<String> = None;
        let mut new_object = false;
        let mut hide_tree = false;
        egui::Panel::left("rde_tree")
            .resizable(true)
            .default_size(268.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(8, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("ПРОЕКТ").small().strong().color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Скрыть панель (вернуть — меню «Вид»)").clicked() {
                            hide_tree = true;
                        }
                        ui.label(RichText::new(format!("{object_count} объектов")).small().color(MUTED));
                    });
                });
                ui.add(
                    egui::TextEdit::singleline(&mut self.tree_filter)
                        .hint_text("Поиск объектов…")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(2.0);
                if ui.add(egui::Button::new(RichText::new("+ Новый объект").size(12.0)).fill(PANEL_HI)).clicked() {
                    new_object = true;
                }
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("rde_tree_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    for group in &groups {
                        let header = RichText::new(format!("{}  {}", group.kind.icon(), group.title)).strong().size(12.5).color(TEXT);
                        egui::CollapsingHeader::new(header).default_open(true).show(ui, |ui| {
                            for &idx in &group.objects {
                                let Some(obj) = objects.get(idx) else {
                                    continue;
                                };
                                if !filter.is_empty() && !obj.title.to_lowercase().contains(&filter) {
                                    continue;
                                }
                                let is_sel = selected.as_deref() == Some(obj.id.as_str());
                                let resp = ui
                                    .horizontal(|ui| {
                                        ui.label(RichText::new("•").color(freshness_color(obj.freshness)).size(13.0));
                                        ui.label(RichText::new(obj.kind.icon()).color(MUTED).size(12.0));
                                        ui.selectable_label(is_sel, RichText::new(&obj.title).size(13.0))
                                    })
                                    .inner;
                                ui.label(RichText::new(&obj.detail).size(10.5).color(MUTED));
                                ui.add_space(2.0);
                                if resp.clicked() {
                                    clicked = Some(obj.id.clone());
                                }
                            }
                        });
                    }
                });
            });
        if hide_tree {
            self.show_tree = false;
        }
        if let Some(id) = clicked {
            if let Some(object) = self.project.object(&id) {
                self.tab = match object.kind {
                    ObjectKind::Document => Tab::Document,
                    ObjectKind::Artifact | ObjectKind::Constants => Tab::Artifacts,
                    ObjectKind::Source | ObjectKind::Note => Tab::Source,
                    ObjectKind::Repository => Tab::Artifacts,
                };
            }
            self.selected = Some(id);
        }
        if new_object {
            self.push_log(Level::Info, "Создание объекта: мастер появится вместе с rde-store (M2.2)");
        }
    }

    // ── панель агента ───────────────────────────────────────────────────────
    fn agent_panel(&mut self, ui: &mut egui::Ui) {
        let mut send = false;
        let mut accept = false;
        let mut reject = false;
        let mut hide_agent = false;
        egui::Panel::right("rde_agent")
            .resizable(true)
            .default_size(316.0)
            .min_size(260.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("АГЕНТ").small().strong().color(MUTED));
                    ui.label(RichText::new("• DSH · deepseek-v4-flash").small().color(GREEN));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Скрыть панель (вернуть — меню «Вид»)").clicked() {
                            hide_agent = true;
                        }
                        let _ = ui.small_button("…");
                        ui.label(RichText::new("ACP").small().color(MUTED));
                    });
                });
                ui.separator();
                let input_height = 38.0;
                egui::ScrollArea::vertical()
                    .id_salt("rde_agent_scroll")
                    .auto_shrink([false, false])
                    .max_height(ui.available_height() - input_height)
                    .show(ui, |ui| {
                        agent_bubble(ui, "Вы", "Проверь рисунок 1 и обнови подпись: у нас новый plot.svg", false);
                        ui.label(RichText::new("rde://doc/01JB7Z6Q/rev/42#block/b12").monospace().size(10.0).color(ACCENT));
                        ui.add_space(6.0);
                        agent_bubble(ui, "RDE Agent", "Проверил вставку рисунка: act_key артефакта plot изменился, документ помечен устаревшим. Подпись обновлю после пересчёта.", true);
                        ui.add_space(6.0);
                        tool_card(ui, "artifact_run", "plot (compute) · OK · 1.2 s · out_hash 8f3a…");
                        ui.add_space(6.0);
                        if self.suggestion_open {
                            suggestion_card(ui, &mut accept, &mut reject);
                        }
                        ui.add_space(6.0);
                        agent_bubble(ui, "RDE Agent", "Готово: «Рисунок 1 — Статистическая обработка реализаций» (2 вставки обновлены). Добавить ссылку на источник [1] в абзац?", true);
                    });
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let width = ui.available_width() - 58.0;
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut self.agent_input)
                            .hint_text("Спросить агента…")
                            .desired_width(width),
                    );
                    send = ui.add(egui::Button::new(RichText::new("Отпр.").size(11.5).color(Color32::WHITE)).fill(ACCENT)).clicked();
                    if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        send = true;
                    }
                });
            });
        if hide_agent {
            self.show_agent = false;
        }
        if send {
            let text = std::mem::take(&mut self.agent_input);
            let text = if text.trim().is_empty() { "Продолжай".to_owned() } else { text };
            self.push_log(Level::Info, format!("Агент (mock): запрос — {text}"));
        }
        if accept || reject {
            self.suggestion_open = false;
            self.push_log(
                Level::Ok,
                if accept { "Предложение агента принято (mock): вставка рисунка 1 обновлена" } else { "Предложение агента отклонено (mock)" },
            );
        }
    }

    // ── центральная область ─────────────────────────────────────────────────
    fn central(&mut self, ui: &mut egui::Ui) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::symmetric(0, 0)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    for tab in Tab::ALL {
                        let sel = self.tab == tab;
                        let text = RichText::new(format!("{} {}", tab.icon(), tab.title())).size(13.0).color(if sel { TEXT } else { MUTED });
                        if ui.add(egui::Button::new(text).fill(if sel { PANEL_HI } else { Color32::TRANSPARENT })).clicked() {
                            self.tab = tab;
                        }
                        ui.add_space(2.0);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let stale = self.project.stale_count();
                        if stale > 0 {
                            ui.label(RichText::new(format!("• {stale} устаревших объектов")).small().color(AMBER));
                        } else {
                            ui.label(RichText::new("• проект актуален").small().color(GREEN));
                        }
                        if let Some(pages) = self.doc_pages() {
                            ui.label(RichText::new(format!("{pages} стр. · A4")).small().color(MUTED));
                        }
                    });
                });
                ui.separator();
                match self.tab {
                    Tab::Document => self.doc_tab(ui),
                    Tab::Structure => self.structure_tab(ui),
                    Tab::Artifacts => self.artifacts_tab(ui),
                    Tab::Source => self.source_tab(ui),
                    Tab::Code => self.code_tab(ui),
                }
            });
    }

    fn doc_tab(&mut self, ui: &mut egui::Ui) {
        let mut mode = self.doc_mode;
        let mut update_inserts = false;
        let mut zoom_action: Option<&'static str> = None;
        if mode != DocMode::Preview {
            self.ensure_editor();
        }
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            for m in DocMode::ALL {
                if ui.selectable_label(mode == m, RichText::new(m.label()).size(12.0)).clicked() {
                    mode = m;
                }
            }
            ui.separator();
            match mode {
                DocMode::DocView | DocMode::Full => {
                    if let Some(ed) = &mut self.editor {
                        if ui.small_button("-").on_hover_text("Уменьшить масштаб").clicked() {
                            zoom_action = Some("view.zoomOut");
                        }
                        ui.label(
                            RichText::new(format!("{:.0}%", ed.app.session.view.zoom * 100.0)).monospace().size(11.5).color(MUTED),
                        );
                        if ui.small_button("+").on_hover_text("Увеличить масштаб").clicked() {
                            zoom_action = Some("view.zoomIn");
                        }
                        if ui.small_button("Сброс").on_hover_text("Масштаб 100%").clicked() {
                            zoom_action = Some("view.zoom100");
                        }
                        if ui.small_button("По ширине").clicked() {
                            zoom_action = Some("view.pageWidth");
                        }
                        ui.separator();
                    }
                }
                DocMode::Preview => {
                    if ui.button(RichText::new("Обновить вставки").size(12.0)).clicked() {
                        update_inserts = true;
                    }
                    ui.separator();
                    ui.label(RichText::new("Масштаб").small().color(MUTED));
                    ui.add(egui::Slider::new(&mut self.zoom, 0.35..=1.5).show_value(false));
                    ui.separator();
                    let _ = ui.button(RichText::new("Открыть копию в Word").size(12.0));
                }
            }
        });
        ui.separator();
        self.doc_mode = mode;
        if let (Some(id), Some(ed)) = (zoom_action, &mut self.editor) {
            let _ = ed.app.run(id, serde_json::json!({}));
        }
        match mode {
            DocMode::DocView => {
                if let Some(ed) = &mut self.editor {
                    let ctx = ui.ctx().clone();
                    ed.app.logic(&ctx);
                    // The WordCraft canvas paints across its whole area; give it a clipped
                    // child Ui so it can never draw over the RDE side panels.
                    let rect = ui.available_rect_before_wrap();
                    let mut child = ui.new_child(
                        egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)),
                    );
                    child.set_clip_rect(rect);
                    ed.app.doc_view_ui(&mut child);
                } else {
                    self.editor_error_ui(ui);
                }
            }
            DocMode::Full => {
                if let Some(ed) = &mut self.editor {
                    let ctx = ui.ctx().clone();
                    ed.app.logic(&ctx);
                    ed.app.ui(ui);
                } else {
                    self.editor_error_ui(ui);
                }
            }
            DocMode::Preview => {
                self.doc.ensure(ui.ctx());
                let zoom = self.zoom;
                match &self.doc.state {
                    DocState::Pending => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(60.0);
                            ui.spinner();
                            ui.label(RichText::new("Рендеринг документа движком WordCraft…").color(MUTED));
                        });
                    }
                    DocState::Failed(e) => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(60.0);
                            ui.label(RichText::new("Не удалось открыть документ").color(RED).strong());
                            ui.label(RichText::new(e).color(MUTED));
                        });
                    }
                    DocState::Ready(doc) => {
                        egui::ScrollArea::both().id_salt("rde_doc_scroll").auto_shrink([false, false]).show(ui, |ui| {
                            ui.add_space(14.0);
                            ui.vertical_centered(|ui| {
                                for (i, tex) in doc.pages.iter().enumerate() {
                                    let size = doc.page_pt * zoom;
                                    ui.image((tex.id(), size));
                                    ui.add_space(4.0);
                                    ui.label(RichText::new(format!("Страница {} из {}", i + 1, doc.count)).small().color(MUTED));
                                    ui.add_space(12.0);
                                }
                            });
                        });
                    }
                }
            }
        }
        if update_inserts {
            self.push_log(Level::Info, "Обновление вставок документа: транзакция появится в следующей итерации (сейчас mock)");
        }
    }

    /// Сообщение, если встроенный редактор не удалось открыть.
    fn editor_error_ui(&self, ui: &mut egui::Ui) {
        if let Some(err) = &self.editor_error {
            ui.vertical_centered(|ui| {
                ui.add_space(50.0);
                ui.label(RichText::new("Не удалось открыть редактор WordCraft").color(RED).strong());
                ui.label(RichText::new(err).color(MUTED));
            });
        }
    }

    fn structure_tab(&mut self, ui: &mut egui::Ui) {
        const BLOCKS: [(&str, &str, &str, &str); 13] = [
            ("b1", "H1", "", "Среда разработки отчёта RDE"),
            ("b2", "P", "", "Демонстрационный документ проекта RDE · прототип v0.1 · 2026"),
            ("b3", "H1", "", "1 Введение"),
            ("b4", "P", "", "Проект RDE (Research Development Environment) — среда, в которой отчёт собирается…"),
            ("b5", "P", "", "Документ в RDE остаётся документом: его можно экспортировать в полноценный .docx…"),
            ("b6", "H1", "", "2 Экспериментальная часть"),
            ("b7", "P", "", "Ниже приведён пример автоматически сгенерированного графика…"),
            ("b8", "FIG", "8f3a…", "Рисунок 1 — Статистическая обработка реализаций"),
            ("b9", "CAP", "b8", "подпись: INSERT(rde://artifact/plot@8f3a/out/plot.svg)"),
            ("b10", "P", "", "График построен по выходным данным артефакта: его хеш зафиксирован…"),
            ("b11", "H1", "", "3 Заключение"),
            ("b12", "P", "", "Прототип показывает основной цикл: данные, артефакт, рисунок, документ…"),
            ("b13", "P", "", ""),
        ];
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Блоки документа").strong().color(TEXT));
            ui.label(RichText::new("(mock — реальный разбор rde:doc будет в следующей итерации)").small().color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("13 блоков · 2 вставки").small().color(MUTED));
            });
        });
        ui.separator();
        egui::ScrollArea::vertical().id_salt("rde_struct_scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(4.0);
            egui::Grid::new("rde_blocks").num_columns(4).spacing([14.0, 6.0]).striped(true).show(ui, |ui| {
                ui.label(RichText::new("блок").small().color(MUTED));
                ui.label(RichText::new("тип").small().color(MUTED));
                ui.label(RichText::new("paraId / цель").small().color(MUTED));
                ui.label(RichText::new("содержимое").small().color(MUTED));
                ui.end_row();
                for (id, kind, anchor, text) in BLOCKS {
                    ui.label(RichText::new(id).monospace().size(11.0).color(ACCENT));
                    let kind_color = match kind {
                        "H1" => ACCENT,
                        "FIG" => AMBER,
                        "CAP" => GREEN,
                        _ => MUTED,
                    };
                    ui.label(RichText::new(kind).monospace().size(11.0).color(kind_color));
                    ui.label(RichText::new(anchor).monospace().size(10.5).color(MUTED));
                    ui.label(RichText::new(text).size(12.0).color(TEXT));
                    ui.end_row();
                }
            });
        });
    }

    fn artifacts_tab(&mut self, ui: &mut egui::Ui) {
        let mut refresh = false;
        let artifacts: Vec<rde_core::Object> = self
            .project
            .objects
            .iter()
            .filter(|o| matches!(&o.data, ObjectData::Artifact(a) if a.kind != "constants"))
            .cloned()
            .collect();
        let edges = self.project.dag_edges();
        let stale: Vec<(String, String)> = artifacts
            .iter()
            .filter(|o| matches!(o.freshness, Freshness::Stale | Freshness::Failed))
            .map(|o| (o.title.clone(), o.detail.clone()))
            .collect();
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("DAG артефактов").strong().color(TEXT));
            ui.label(RichText::new(format!("{} устаревших", stale.len())).small().color(AMBER));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new("Обновить связанные").size(12.0)).clicked() {
                    refresh = true;
                }
            });
        });
        ui.separator();
        if artifacts.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(RichText::new("В проекте нет артефактов").color(MUTED));
            });
            return;
        }
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 240.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let n = artifacts.len().min(4);
        let nw = 178.0;
        let nh = 62.0;
        let margin = 26.0;
        let step = if n > 1 { (rect.width() - 2.0 * margin - nw) / (n as f32 - 1.0) } else { 0.0 };
        let y = rect.top() + 46.0;
        let nodes: Vec<Rect> = (0..n)
            .map(|i| Rect::from_min_size(pos2(rect.left() + margin + step * i as f32, y), vec2(nw, nh)))
            .collect();
        let index_of = |id: &str| artifacts.iter().position(|o| o.id == id);
        let selected_id = self.selected.clone();
        for (i, object) in artifacts.iter().take(n).enumerate() {
            let color = freshness_color(object.freshness);
            let selected = selected_id.as_deref() == Some(object.id.as_str());
            draw_node(&painter, nodes[i], &object.title, &object.detail, color, object.freshness.label_ru(), selected);
        }
        for (from, to) in &edges {
            if let (Some(a), Some(b)) = (index_of(from), index_of(to)) {
                if a < n && b < n {
                    arrow(&painter, nodes[a], nodes[b], AMBER);
                }
            }
        }
        let mut line_y = rect.bottom() - 70.0;
        for (title, detail) in &stale {
            painter.text(
                pos2(rect.left() + margin, line_y),
                Align2::LEFT_TOP,
                format!("• {title}: {detail}"),
                FontId::proportional(12.0),
                AMBER,
            );
            line_y += 17.0;
        }
        if artifacts.len() > n {
            painter.text(
                pos2(rect.left() + margin, rect.bottom() - 22.0),
                Align2::LEFT_TOP,
                format!("…и ещё {} артефакт(ов)", artifacts.len() - n),
                FontId::proportional(11.5),
                MUTED,
            );
        }
        if refresh {
            let targets = self.selected.as_ref().and_then(|id| {
                self.project
                    .artifact_object(id)
                    .filter(|object| matches!(&object.data, ObjectData::Artifact(artifact) if artifact.kind == "compute"))
                    .map(|object| vec![object.id.clone()])
            });
            self.update_dependencies(targets);
        }
    }

    fn source_tab(&mut self, ui: &mut egui::Ui) {
        let selected = self.selected.clone();
        let is_source = |o: &&rde_core::Object| matches!(o.data, ObjectData::Source(_));
        let source = self
            .project
            .objects
            .iter()
            .find(|o| is_source(o) && selected.as_deref() == Some(o.id.as_str()))
            .or_else(|| self.project.objects.iter().find(is_source))
            .cloned();
        ui.add_space(10.0);
        let Some(obj) = source else {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(RichText::new("В проекте нет источников").color(MUTED));
            });
            return;
        };
        let ObjectData::Source(src) = &obj.data else {
            return;
        };
        let locator = src.file.clone().or_else(|| src.url.clone()).unwrap_or_default();
        let retrieved = src.retrieved_at.clone().unwrap_or_else(|| "—".to_owned());
        let reference = src.reference.clone();
        let dir = obj.dir.display().to_string();
        ui.vertical(|ui| {
            ui.add_space(0.0);
            egui::Frame::new()
                .fill(PANEL_HI)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_max_width(680.0);
                    ui.label(RichText::new(format!("▣  {}", obj.title)).size(18.0).strong().color(TEXT));
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(format!("Источник проекта · {}", fresh_label(obj.freshness))).small().color(MUTED),
                    );
                    ui.add_space(10.0);
                    let mut rows: Vec<(&str, String)> = vec![
                        ("Тип", src.kind.clone()),
                        ("Локатор", locator.clone()),
                        ("Получен", retrieved.clone()),
                        ("Каталог", dir.clone()),
                    ];
                    if let Some(r) = &reference {
                        if !r.designation.is_empty() {
                            rows.push(("Обозначение", r.designation.clone()));
                        }
                        if !r.title.is_empty() {
                            rows.push(("Название", r.title.clone()));
                        }
                        if !r.authors.is_empty() {
                            rows.push(("Авторы", r.authors.join("; ")));
                        }
                        if let Some(year) = r.year {
                            rows.push(("Год", year.to_string()));
                        }
                    }
                    egui::Grid::new("rde_source_meta").num_columns(2).spacing([18.0, 6.0]).show(ui, |ui| {
                        for (k, v) in rows {
                            ui.label(RichText::new(k).small().color(MUTED));
                            ui.label(RichText::new(v).size(12.5).color(TEXT));
                            ui.end_row();
                        }
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let _ = ui.button(RichText::new("Открыть источник").size(12.0));
                        let _ = ui.button(RichText::new("Ссылка в чат агента").size(12.0));
                        let _ = ui.button(RichText::new("Показать в дереве").size(12.0));
                    });
                });
            ui.add_space(12.0);
            egui::Frame::new()
                .fill(PANEL)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_max_width(680.0);
                    ui.label(RichText::new("Текст источника").small().strong().color(MUTED));
                    ui.add_space(6.0);
                    for line in [
                        "Метаданные загружены из source.toml.",
                        "Извлечение текста (PDF/HTML), веб-архив и цитаты —",
                        "следующая веха rde-sources (v0.2).",
                        "",
                        "Пока источник виден в дереве и доступен агенту",
                        "как объект проекта.",
                    ] {
                        ui.label(RichText::new(line).size(12.0).color(TEXT));
                    }
                });
        });
    }

    fn code_tab(&mut self, ui: &mut egui::Ui) {
        const CODE: &[&str] = &[
            r#"<rde:doc xmlns:rde="https://rde.dev/ns/doc" schema_version="1""#,
            r#"         doc_id="01JB7Z6Q4W8R0N2S5T9V3K1M6P" body_hash="sha256:91c7…">"#,
            r#"  <rde:template name="gost-7.32-2017" fig="gost-7.32-2017" table="gost-2.105-2019"/>"#,
            r#"  <rde:blocks>"#,
            r#"    <rde:block id="b1"  paraId="1A2B3C41" kind="heading1"/>"#,
            r#"    <rde:block id="b8"  paraId="1A2B3C48" kind="figure"/>"#,
            r#"    <rde:block id="b12" paraId="1A2B3C4C" kind="paragraph"/>"#,
            r#"  </rde:blocks>"#,
            r#"  <rde:inserts>"#,
            r#"    <rde:insert id="i1" block="b8""#,
            r#"        target="rde://artifact/01JB80F1K2M4N6P8Q0R2S4T6V@sha256:8f3a…/out/plot.svg""#,
            r#"        kind="figure" policy="update-with-document"/>"#,
            r#"    <rde:insert id="i2" block="b9" kind="caption">"#,
            r#"      <rde:field name="SEQ" value="1"/> — <rde:text>Статистическая обработка…</rde:text>"#,
            r#"    </rde:insert>"#,
            r#"  </rde:inserts>"#,
            r#"  <rde:review open="2" resolved="5"/>"#,
            r#"</rde:doc>"#,
        ];
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("rde:doc — RDE-часть пакета .doc.rde (customXml/item1.xml)").small().color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("read-only · схема появится вместе с rde-doc").small().color(MUTED));
            });
        });
        ui.separator();
        egui::ScrollArea::both().id_salt("rde_code_scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(6.0);
            for (i, line) in CODE.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("{:>3}", i + 1)).monospace().size(11.0).color(MUTED));
                    ui.label(RichText::new(*line).monospace().size(12.0).color(TEXT));
                });
            }
        });
    }
}

// ── вспомогательная отрисовка ────────────────────────────────────────────────

/// Обрезать строку до `max` символов, добавив многоточие.
fn ellipsize(s: &str, max: usize) -> String {
    let mut chars = s.char_indices();
    match chars.nth(max) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_owned(),
    }
}

fn fresh_label(f: Freshness) -> &'static str {
    f.label_ru()
}

fn freshness_color(f: Freshness) -> Color32 {
    match f {
        Freshness::UpToDate => GREEN,
        Freshness::Stale => AMBER,
        Freshness::Failed => RED,
        Freshness::External => MUTED,
    }
}

fn agent_bubble(ui: &mut egui::Ui, from: &str, text: &str, is_agent: bool) {
    egui::Frame::new()
        .fill(if is_agent { PANEL_HI } else { Color32::from_rgb(0x24, 0x30, 0x49) })
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::same(9))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.label(RichText::new(from).small().strong().color(if is_agent { GREEN } else { ACCENT }));
            ui.label(RichText::new(text).size(12.5).color(TEXT));
        });
    ui.add_space(2.0);
}

fn tool_card(ui: &mut egui::Ui, tool: &str, detail: &str) {
    egui::Frame::new()
        .fill(BG)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(9))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(">").color(ACCENT).monospace());
                ui.label(RichText::new(tool).monospace().size(12.0).strong().color(TEXT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("approval: auto").small().color(MUTED));
                });
            });
            ui.label(RichText::new(detail).monospace().size(11.0).color(MUTED));
        });
}

fn suggestion_card(ui: &mut egui::Ui, accept: &mut bool, reject: &mut bool) {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x25, 0x2c, 0x22))
        .stroke(Stroke::new(1.0, Color32::from_rgb(0x46, 0x5a, 0x3c)))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(9))
        .show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            ui.label(RichText::new("ПРЕДЛОЖЕНИЕ АГЕНТА").small().strong().color(GREEN));
            ui.label(RichText::new("Заменить подпись рисунка 1 на актуальную из кэша вставки").size(12.0).color(TEXT));
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new("Принять").size(12.0).color(Color32::WHITE)).fill(Color32::from_rgb(0x3c, 0x7a, 0x4f))).clicked() {
                    *accept = true;
                }
                if ui.button(RichText::new("Отклонить").size(12.0)).clicked() {
                    *reject = true;
                }
            });
        });
}

fn draw_node(
    painter: &egui::Painter,
    rect: Rect,
    title: &str,
    sub: &str,
    color: Color32,
    status: &str,
    selected: bool,
) {
    painter.rect_filled(rect, CornerRadius::same(8), PANEL_HI);
    let border = if selected { ACCENT } else { BORDER };
    painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(if selected { 1.6 } else { 1.0 }, border), egui::StrokeKind::Inside);
    painter.circle_filled(rect.left_top() + vec2(12.0, 14.0), 4.0, color);
    painter.text(
        rect.left_top() + vec2(24.0, 7.0),
        Align2::LEFT_TOP,
        ellipsize(title, 21),
        FontId::proportional(13.0),
        TEXT,
    );
    painter.text(
        rect.left_top() + vec2(12.0, 30.0),
        Align2::LEFT_TOP,
        ellipsize(sub, 26),
        FontId::proportional(11.0),
        MUTED,
    );
    painter.text(
        rect.left_bottom() + vec2(12.0, -16.0),
        Align2::LEFT_TOP,
        status,
        FontId::proportional(10.5),
        color,
    );
}

fn arrow(painter: &egui::Painter, from: Rect, to: Rect, color: Color32) {
    let start = pos2(from.right(), from.center().y);
    let end = pos2(to.left() - 2.0, to.center().y);
    painter.line_segment([start, end], Stroke::new(1.4, color));
    let tip = pos2(end.x - 7.0, end.y);
    painter.add(egui::Shape::convex_polygon(
        vec![end, pos2(tip.x, tip.y - 4.0), pos2(tip.x, tip.y + 4.0)],
        color,
        Stroke::NONE,
    ));
}

// ── тема и шрифты ───────────────────────────────────────────────────────────

/// Подключить системный шрифт с кириллицей (egui-дефолт может не покрывать всё).
pub fn install_fonts(ctx: &egui::Context) {
    const CANDIDATES: [&str; 5] = [
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/liberation2/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    ];
    for path in CANDIDATES {
        if let Ok(bytes) = fs::read(path) {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert("rde-sans".to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
            fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "rde-sans".to_owned());
            ctx.set_fonts(fonts);
            break;
        }
    }
}

fn install_style(ctx: &egui::Context) {
    let mut dark = egui::Visuals::dark();
    dark.panel_fill = BG;
    dark.window_fill = BG;
    dark.extreme_bg_color = Color32::from_rgb(0x12, 0x13, 0x16);
    dark.faint_bg_color = PANEL_HI;
    dark.widgets.noninteractive.bg_fill = PANEL;
    dark.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    dark.widgets.inactive.bg_fill = PANEL_HI;
    dark.widgets.inactive.weak_bg_fill = PANEL_HI;
    dark.widgets.hovered.bg_fill = Color32::from_rgb(0x2f, 0x34, 0x3e);
    dark.widgets.active.bg_fill = Color32::from_rgb(0x36, 0x3c, 0x48);
    dark.selection.bg_fill = Color32::from_rgb(0x2a, 0x3d, 0x66);
    dark.selection.stroke = Stroke::new(1.0, ACCENT);
    dark.override_text_color = Some(TEXT);
    dark.window_corner_radius = CornerRadius::same(8);
    ctx.set_visuals_of(egui::Theme::Dark, dark);
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(9.0, 3.0);
        style.spacing.interact_size.y = 24.0;
    });
}

#[cfg(test)]
mod docview_tests {
    use super::{DocMode, RdeApp};

    /// S5-полировка: клавиатурный ввод доходит до встроенного DocView, а сохраняет документ RDE.
    #[test]
    fn docview_receives_keyboard_input_and_rde_saves() {
        let mut app = RdeApp::new(rde_core::demo());
        app.doc_mode = DocMode::DocView;
        app.export_dir = std::env::temp_dir().join(format!("rde-ui-test-{}", std::process::id()));
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(
                |ui, app: &mut RdeApp| {
                    app.ui(ui);
                },
                app,
            );
        for _ in 0..8 {
            harness.step();
        }
        // Клик по странице: canvas получает клавиатурный фокус.
        let pos = egui::pos2(800.0, 500.0);
        harness.event(egui::Event::PointerMoved(pos));
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::default(),
        });
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::default(),
        });
        for _ in 0..4 {
            harness.step();
        }
        harness.event(egui::Event::Text("RDE".to_owned()));
        for _ in 0..4 {
            harness.step();
        }
        let text = harness
            .state()
            .editor
            .as_ref()
            .expect("встроенный редактор должен открыться")
            .app
            .session
            .doc
            .plain_text(wordcraft_engine::doc::StoryRef::Body);
        assert!(text.contains("RDE"), "набранный текст не дошёл до документа: {text:.120}");

        // Сохранение под контролем RDE: явный .docx-экспорт в export_dir.
        harness.state_mut().save_editor_copy();
        let out = harness.state().export_dir.join("demo-report-edited.docx");
        assert!(out.is_file(), "RDE-сохранение не создало {}", out.display());
        let _ = std::fs::remove_dir_all(&harness.state().export_dir);
    }

    // ── M2.3: статусы из хешей и реальное обновление связанных артефактов ─────
    const STATS: &str = "01M5VJ1JG039MMV97SZGY6V9EQ";
    const REALIZATIONS: &str = "01M5VJ1JG0HD99FDTGJ8YEPFZX";

    fn python_available() -> bool {
        std::process::Command::new("python3").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(&*name, ".rde" | "exports" | "__pycache__" | "out") {
                continue;
            }
            let path = entry.path();
            let target = dst.join(&*name);
            if path.is_dir() {
                copy_tree(&path, &target);
            } else {
                std::fs::copy(&path, &target).unwrap();
            }
        }
    }

    fn temp_demo_copy(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("rde-ui-dag-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        copy_tree(&rde_core::demo_project_dir(), &root);
        root
    }

    /// UI-слой: статусы артефактов берутся из DAG (никогда не запускался), «Обновить всё»
    /// действительно запускает скрипты и меняет статусы на «актуален»; повтор — без запусков.
    #[test]
    fn dag_update_from_ui_runs_real_artifacts() {
        if !python_available() {
            return;
        }
        let root = temp_demo_copy("update");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        let stats = app.project.artifact_object(STATS).unwrap();
        assert_eq!(stats.freshness, super::Freshness::Stale, "до запуска артефакт должен быть устаревшим");
        assert!(stats.detail.contains("не запускался"), "detail: {}", stats.detail);

        let report = app.update_dependencies(None).expect("DAG-обновление");
        assert_eq!(report.failed(), 0, "{:?}", report.errors);
        assert_eq!(report.ran(), 2, "stats и plot должны выполниться: {:?}", report.artifacts);
        let stats = app.project.artifact_object(STATS).unwrap();
        assert_eq!(stats.freshness, super::Freshness::UpToDate, "после запуска актуален: {}", stats.detail);
        assert!(app.project.root.join("exports").exists() || root.join(".rde/updates").is_dir());

        let second = app.update_dependencies(None).expect("повторное обновление");
        assert_eq!(second.ran(), 0, "повторный запуск не нужен: {:?}", second.artifacts);
        assert_eq!(second.skipped(), 2);

        // Изменяем вход data-артефакта → статус из хешей становится «устарел» с причиной.
        let input = app.project.artifact_object(REALIZATIONS).unwrap().dir.join("inputs/experiment.csv");
        let mut text = std::fs::read_to_string(&input).unwrap();
        text.push_str("50,50.0,50.0\n");
        std::fs::write(&input, text).unwrap();
        app.reload_project();
        let stats = app.project.artifact_object(STATS).unwrap();
        assert_eq!(stats.freshness, super::Freshness::Stale);
        assert!(stats.detail.contains("изменились"), "detail: {}", stats.detail);

        let third = app.update_dependencies(None).expect("обновление после правки входа");
        assert_eq!(third.ran(), 2, "stats и plot пересчитываются: {:?}", third.artifacts);
        assert_eq!(app.project.artifact_object(STATS).unwrap().freshness, super::Freshness::UpToDate);
        let _ = std::fs::remove_dir_all(&root);
    }
}
