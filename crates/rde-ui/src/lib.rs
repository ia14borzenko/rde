//! RDE prototype UI shell.
//!
//! Цель этой итерации — «общий вид концепции»: окно IDE-подобной среды, где
//! документ — один из объектов проекта, а рядом живут дерево объектов, DAG
//! артефактов, источники, журнал и панель агента. Документ показывается
//! **по-настоящему**: `.docx` читается движком WordCraft, размечается его
//! layout-движком и растеризуется в текстуры страниц.
//!
//! Что здесь демонстрационное (mock), а что настоящее:
//! - настоящее: рендер страниц документа, экспорт-путь WordCraft (позже), каркас панелей;
//! - mock: дерево объектов, DAG и статусы, журнал, чат агента, вкладки «Структура»/«Источник»/«Код».
//! Следующие итерации заменяют mock-данные на `rde-core`/`rde-store`/`rde-dag`
//! (см. `.agent/plan-v01.md`).

use std::fs;
use std::path::Path;
use std::sync::Arc;

use egui::{Align2, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};
use rde_core::{Freshness, Level, Project};

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
    path: std::path::PathBuf,
    state: DocState,
}

impl DocPreview {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path, state: DocState::Pending }
    }

    fn ensure(&mut self, ctx: &egui::Context) {
        if !matches!(self.state, DocState::Pending) {
            return;
        }
        match load_and_render(&self.path, ctx) {
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
    /// Полноценный редактор WordCraft, встроенный как панель без изменений исходников.
    Editor,
    /// Растеризованный предпросмотр страниц (лёгкий, работает без UI-стека WordCraft).
    Preview,
}

/// Встроенный редактор WordCraft: полноценный `WordApp` (ribbon + canvas + ввод).
struct EmbeddedEditor {
    app: wordcraft_ui_egui::WordApp,
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
}

impl RdeApp {
    pub fn new(project: Project) -> Self {
        let doc = DocPreview::new(project.demo_docx_path());
        Self {
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
            doc_mode: DocMode::Editor,
            editor: None,
            editor_error: None,
        }
    }

    /// Лениво создать встроенный редактор WordCraft с тем же документом, что и предпросмотр.
    fn ensure_editor(&mut self) {
        if self.editor.is_some() || self.editor_error.is_some() {
            return;
        }
        let region = wordcraft_engine::Region::from_tag("ru-RU");
        // Встроенный WordApp умеет сам сохранять документ (autosave/сохранение при выходе).
        // Пока у RDE нет своего слоя сохранения проекта (rde:doc/транзакции — следующая веха),
        // открываем временную копию: правки редактора не могут испортить файлы проекта.
        let copy = std::env::temp_dir().join("rde-embedded-doc.docx");
        if let Ok(bytes) = std::fs::read(&self.doc.path) {
            let _ = std::fs::write(&copy, bytes);
        }
        let edit_path = if copy.is_file() { copy } else { self.doc.path.clone() };
        match wordcraft_engine::io::open_path_in(&edit_path, None, &region) {
            Ok((doc, _encrypted)) => {
                let mut session = wordcraft_engine::Session::new(doc);
                session.path = Some(edit_path.clone());
                let mut app = wordcraft_ui_egui::WordApp::new(session, wordcraft_ui_egui::Services::default());
                app.integrated_titlebar = false;
                app.ui.theme = wordcraft_ui_egui::theme::Appearance::Dark;
                self.editor = Some(EmbeddedEditor { app });
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
                        .on_hover_text("Пересчитать устаревшие артефакты и обновить вставки документа")
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
                        ui.label(RichText::new(format!("• {} устаревших · документ: 2 вставки", self.project.stale)).small().color(AMBER));
                        ui.separator();
                        ui.label(RichText::new("MCP: off").small().color(MUTED));
                    });
                });
            });
        if update_all {
            self.push_log(Level::Ok, "Запрошено обновление всех артефактов (mock: транзакция В2–В4 не реализована)");
        }
        if export {
            self.push_log(Level::Info, "Экспорт .docx: пайплайн WordCraft готов, RDE-гейты — следующая итерация");
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
                    ui.label(RichText::new(&self.project.root).small().color(MUTED));
                    ui.separator();
                    ui.label(RichText::new(format!("git: {} · чистый", self.project.branch)).small().color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("A4 · ru-RU").small().color(MUTED));
                        ui.separator();
                        ui.label(RichText::new(format!("{} объектов", self.project.object_count())).small().color(MUTED));
                        ui.separator();
                        ui.label(RichText::new(format!("• {} устаревших", self.project.stale)).small().color(AMBER));
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
                            for obj in &group.objects {
                                if !filter.is_empty() && !obj.title.to_lowercase().contains(&filter) {
                                    continue;
                                }
                                let is_sel = selected.as_deref() == Some(obj.id.as_str());
                                let color = freshness_color(obj.freshness);
                                let resp = ui
                                    .horizontal(|ui| {
                                        ui.label(RichText::new("•").color(color).size(13.0));
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
            self.selected = Some(id);
        }
        if new_object {
            self.push_log(Level::Info, "Создание объекта: мастер появится вместе с rde-core");
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
                        if self.project.stale > 0 {
                            ui.label(RichText::new(format!("• {} устаревшие вставки", self.project.stale)).small().color(AMBER));
                        } else {
                            ui.label(RichText::new("• документ актуален").small().color(GREEN));
                        }
                        ui.label(RichText::new(format!("{} стр. · A4", self.project.document_pages)).small().color(MUTED));
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
        let mut update_inserts = false;
        let mut mode = self.doc_mode;
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            if ui.selectable_label(mode == DocMode::Editor, RichText::new("Редактор WordCraft").size(12.0)).clicked() {
                mode = DocMode::Editor;
            }
            if ui.selectable_label(mode == DocMode::Preview, RichText::new("Предпросмотр страниц").size(12.0)).clicked() {
                mode = DocMode::Preview;
            }
            ui.separator();
            match mode {
                DocMode::Editor => {
                    ui.label(
                        RichText::new("полноценный WordApp встроен в панель, исходники не изменялись")
                            .small()
                            .color(MUTED),
                    );
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
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("Документ: демо-отчёт · 2 страницы").small().color(MUTED));
            });
        });
        ui.separator();
        self.doc_mode = mode;
        match mode {
            DocMode::Editor => {
                self.ensure_editor();
                if self.editor.is_none() {
                    if let Some(err) = &self.editor_error {
                        ui.vertical_centered(|ui| {
                            ui.add_space(50.0);
                            ui.label(RichText::new("Не удалось открыть редактор WordCraft").color(RED).strong());
                            ui.label(RichText::new(err).color(MUTED));
                        });
                    }
                    return;
                }
                if let Some(ed) = &mut self.editor {
                    let ctx = ui.ctx().clone();
                    ed.app.logic(&ctx);
                    ed.app.ui(ui);
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
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("DAG артефактов").strong().color(TEXT));
            ui.label(RichText::new(format!("{} устаревших", self.project.stale)).small().color(AMBER));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(RichText::new("Обновить связанные").size(12.0)).clicked() {
                    refresh = true;
                }
            });
        });
        ui.separator();
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 250.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let nw = 178.0;
        let nh = 62.0;
        let margin = 26.0;
        let step = (rect.width() - 2.0 * margin - nw) / 3.0;
        let y = rect.top() + 64.0;
        let node = |i: usize| Rect::from_min_size(pos2(rect.left() + margin + step * i as f32, y), vec2(nw, nh));
        let n0 = node(0);
        let n1 = node(1);
        let n2 = node(2);
        let n3 = node(3);
        draw_node(&painter, n0, "realizations.csv", "data · 4 файла", GREEN, "актуален");
        draw_node(&painter, n1, "stats (compute)", "script.py · params", AMBER, "устарел");
        draw_node(&painter, n2, "plot.svg", "figure · out_hash 8f3a", AMBER, "устарел");
        draw_node(&painter, n3, "Отчёт по НИР", "вставка: рисунок 1", AMBER, "устарел");
        arrow(&painter, n0, n1, AMBER);
        arrow(&painter, n1, n2, AMBER);
        arrow(&painter, n2, n3, AMBER);
        painter.text(
            pos2(rect.left() + margin, rect.bottom() - 54.0),
            Align2::LEFT_TOP,
            "Изменён script.py → in_hash изменился → устарели stats.csv, plot.svg и 1 вставка в документе.",
            FontId::proportional(12.5),
            MUTED,
        );
        painter.text(
            pos2(rect.left() + margin, rect.bottom() - 32.0),
            Align2::LEFT_TOP,
            "Кнопка «Обновить связанные» пересчитает их по топологическому порядку и обновит документ одной транзакцией.",
            FontId::proportional(12.5),
            MUTED,
        );
        if refresh {
            self.push_log(Level::Info, "Обновление связанных (mock): DAG-транзакция — следующая итерация");
        }
    }

    fn source_tab(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.add_space(12.0);
            egui::Frame::new()
                .fill(PANEL_HI)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_max_width(560.0);
                    ui.label(RichText::new("▣  ГОСТ 7.32-2017").size(18.0).strong().color(TEXT));
                    ui.add_space(2.0);
                    ui.label(RichText::new("Источник проекта · локальная копия PDF").small().color(MUTED));
                    ui.add_space(10.0);
                    egui::Grid::new("rde_source_meta").num_columns(2).spacing([18.0, 6.0]).show(ui, |ui| {
                        for (k, v) in [
                            ("Обозначение", "ГОСТ 7.32-2017"),
                            ("Область", "отчёт о НИР: структура и правила оформления"),
                            ("Файл", "sources/gost-7.32-2017/files/gost.pdf"),
                            ("Хеш", "sha256:3f9a…c41d"),
                            ("Вставок в документ", "[1] — список источников"),
                            ("Используется шаблоном", "семейства: рисунки, таблицы, заголовки"),
                        ] {
                            ui.label(RichText::new(k).small().color(MUTED));
                            ui.label(RichText::new(v).size(12.5).color(TEXT));
                            ui.end_row();
                        }
                    });
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        let _ = ui.button(RichText::new("Открыть PDF").size(12.0));
                        let _ = ui.button(RichText::new("Ссылка в чат агента").size(12.0));
                        let _ = ui.button(RichText::new("Показать в дереве").size(12.0));
                    });
                });
            ui.add_space(12.0);
            egui::Frame::new().fill(PANEL).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(14)).show(ui, |ui| {
                ui.set_max_width(430.0);
                ui.label(RichText::new("Извлечённый текст (демо)").small().strong().color(MUTED));
                ui.add_space(6.0);
                for line in [
                    "Требования к структуре и оформлению отчёта о НИР:",
                    "  · титульный лист, список исполнителей, реферат;",
                    "  · содержание, введение, основная часть;",
                    "  · заключение, список использованных источников;",
                    "  · приложения (при наличии).",
                    "",
                    "Правила применяются шаблоном как данные (без текста стандарта).",
                ] {
                    ui.label(RichText::new(line).monospace().size(11.5).color(TEXT));
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

fn draw_node(painter: &egui::Painter, rect: Rect, title: &str, sub: &str, color: Color32, status: &str) {
    painter.rect_filled(rect, CornerRadius::same(8), PANEL_HI);
    painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0, BORDER), egui::StrokeKind::Inside);
    painter.circle_filled(rect.left_top() + vec2(12.0, 14.0), 4.0, color);
    painter.text(rect.left_top() + vec2(24.0, 7.0), Align2::LEFT_TOP, title, FontId::proportional(13.0), TEXT);
    painter.text(rect.left_top() + vec2(12.0, 30.0), Align2::LEFT_TOP, sub, FontId::proportional(11.0), MUTED);
    painter.text(rect.left_bottom() + vec2(12.0, -16.0), Align2::LEFT_TOP, status, FontId::proportional(10.5), color);
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
