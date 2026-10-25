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
use rde_core::{CreateRequest, Freshness, Level, NewObjectKind, ObjectData, ObjectKind, Project};
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
    let (doc, _from_package) = rde_doc::open_document(path)?;
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

/// Состояние диалога создания объекта.
struct CreateDialogState {
    kind: NewObjectKind,
    title: String,
}

/// Состояние диалога переименования.
struct RenameDialogState {
    id: String,
    title: String,
}

/// Информация о PDF-источнике (M5): страницы и текстовый слой.
#[derive(Clone, Debug)]
struct PdfSourceInfo {
    pages: u32,
    text: String,
}

/// Реальный блок документа (из WordCraft-модели, а не mock).
#[derive(Clone, Debug)]
struct DocBlock {
    number: usize,
    kind: &'static str,
    style: String,
    anchor: String,
    text: String,
    images: usize,
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
    /// Панель-инспектор выбранного объекта.
    show_inspector: bool,
    create_dialog: Option<CreateDialogState>,
    rename_dialog: Option<RenameDialogState>,
    delete_confirm: Option<String>,
    /// Кэш реальной структуры документа (WordCraft-модель).
    structure_cache: Option<Result<(Vec<DocBlock>, usize), String>>,
    /// Кэш PDF-источника: путь → страницы/текст.
    pdf_cache: Option<(String, Result<PdfSourceInfo, String>)>,
    /// Кэш числа комментариев Word: ключ (путь+время) → количество.
    comments_cache: Option<(String, usize)>,
}

impl RdeApp {
    pub fn new(project: Project) -> Self {
        let doc = DocPreview::new(project.document_path());
        let export_dir = project.root.join("exports");
        let selected = project.document_object().map(|object| object.id.clone());
        let mut app = Self {
            project,
            tab: Tab::Document,
            tree_filter: String::new(),
            selected,
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
            show_inspector: true,
            create_dialog: None,
            rename_dialog: None,
            delete_confirm: None,
            structure_cache: None,
            pdf_cache: None,
            comments_cache: None,
        };
        match Store::open(&app.project.root) {
            Ok(store) => {
                app.store = Some(store);
                app.refresh_dag_statuses();
            }
            Err(error) => app.push_log(Level::Warn, format!("Хранилище проекта недоступно: {error}")),
        }
        // M3: проверка «документ правили вне RDE?» по rde:doc/body_hash.
        if let Some(path) = app.project.document_path() {
            if rde_doc::is_rde_package(&path) {
                match rde_doc::check_external_edit(&path) {
                    Ok(check) if check.verdict != rde_doc::EditVerdict::NotModified => app.push_log(
                        Level::Warn,
                        match check.verdict {
                            rde_doc::EditVerdict::MissingRdeDoc => "Документ пересохранён вне RDE: RDE-часть (customXml) потеряна — нужен импорт/сверка".to_owned(),
                            _ => "Документ изменён вне RDE: body_hash не совпадает — проверьте правки".to_owned(),
                        },
                    ),
                    Ok(_) => app.push_log(Level::Info, "Документ .doc.rde: RDE-часть на месте, внешних правок нет"),
                    Err(error) => app.push_log(Level::Warn, format!("Проверка .doc.rde: {error}")),
                }
            }
        }
        app
    }

    /// Сохранить документ в проект (`.doc.rde`) — RDE владелец сохранения.
    pub fn save_project_document(&mut self) {
        self.ensure_editor();
        let Some(editor) = self.editor.as_ref() else {
            self.push_log(Level::Warn, "Сохранить проект: редактор не открыт");
            return;
        };
        let Some(document_object) = self.project.document_object().cloned() else {
            self.push_log(Level::Warn, "Сохранить проект: в проекте нет документа");
            return;
        };
        let ObjectData::Document(manifest) = &document_object.data else { return };
        let old_path = document_object.dir.join(&manifest.file);
        let package = if rde_doc::is_rde_package(&old_path) {
            old_path.clone()
        } else {
            document_object.dir.join("report.doc.rde")
        };
        let temp = std::env::temp_dir().join(format!("rde-save-{}.docx", std::process::id()));
        if let Err(error) = wordcraft_engine::io::save_path(&temp, &editor.app.session.doc) {
            self.push_log(Level::Warn, format!("Сохранить проект: не удалось сохранить OOXML: {error}"));
            return;
        }
        let template = manifest.template.clone();
        let doc_id = manifest.id.clone();
        match rde_doc::save_engine_document(&temp, &package, &doc_id, &template) {
            Ok(rde_doc) => {
                if package != old_path {
                    let manifest_path = document_object.dir.join("document.toml");
                    if let Ok(text) = std::fs::read_to_string(&manifest_path) {
                        let new_text = text.replace(&format!("file = \"{}\"", manifest.file), "file = \"report.doc.rde\"");
                        let _ = std::fs::write(&manifest_path, new_text);
                    }
                }
                self.structure_cache = None;
                self.reload_project();
                self.push_log(
                    Level::Ok,
                    format!(
                        "Проект сохранён: {} ({} блоков, {} вставок, doc_id {})",
                        package.display(),
                        rde_doc.blocks.len(),
                        rde_doc.inserts.len(),
                        rde_doc.doc_id
                    ),
                );
            }
            Err(error) => self.push_log(Level::Warn, format!("Сохранить проект: {error}")),
        }
        let _ = std::fs::remove_file(&temp);
    }

    /// Оформить документ шаблоном (ГОСТ 7.32-2017) и/или вставить библиографию проекта.
    /// Работает на извлечённой копии, результат сохраняется обратно в `.doc.rde`.
    pub fn update_document_package(&mut self, apply_gost: bool, with_bibliography: bool) {
        let Some(path) = self.project.document_path() else {
            self.push_log(Level::Warn, "Оформление: в проекте нет документа");
            return;
        };
        if !rde_doc::is_rde_package(&path) {
            self.push_log(Level::Warn, "Оформление: документ ещё не в .doc.rde (сначала «Сохранить проект»)");
            return;
        }
        if self.editor.is_some() {
            self.save_project_document();
        }
        let Some(document_object) = self.project.document_object().cloned() else {
            self.push_log(Level::Warn, "Оформление: объект документа не найден");
            return;
        };
        let ObjectData::Document(manifest) = &document_object.data else { return };
        let work = std::env::temp_dir().join(format!("rde-format-{}", std::process::id()));
        let _ = fs::remove_dir_all(&work);
        if let Err(error) = fs::create_dir_all(&work) {
            self.push_log(Level::Warn, format!("Оформление: {error}"));
            return;
        }
        let clean = work.join("clean.docx");
        if let Err(error) = rde_doc::extract_docx(&path, &clean) {
            self.push_log(Level::Warn, format!("Оформление: не удалось извлечь .docx: {error}"));
            return;
        }
        let mut doc = match rde_doc::open_document(&clean) {
            Ok((doc, _)) => doc,
            Err(error) => {
                self.push_log(Level::Warn, format!("Оформление: не удалось открыть документ: {error}"));
                return;
            }
        };
        let mut logs = Vec::new();
        let mut template_id = manifest.template.clone();
        if apply_gost {
            let Some(template) = rde_templates::template_by_id("gost-7.32-2017") else {
                self.push_log(Level::Warn, "Оформление: шаблон ГОСТ не найден");
                return;
            };
            let result = if with_bibliography {
                rde_templates::apply_with_bibliography(template, &mut doc, &self.project)
            } else {
                Ok(rde_templates::apply(template, &mut doc))
            };
            match result {
                Ok(applied) => {
                    logs = applied;
                    template_id = template.id.clone();
                }
                Err(error) => {
                    self.push_log(Level::Warn, format!("Оформление: {error}"));
                    return;
                }
            }
        } else if with_bibliography {
            let entries = rde_templates::bibliography_entries(&self.project);
            let Some(template) = rde_templates::template_by_id(if template_id.is_empty() { "gost-7.32-2017" } else { &template_id }) else {
                self.push_log(Level::Warn, "Оформление: шаблон не найден");
                return;
            };
            match rde_templates::insert_bibliography(template, &mut doc, &entries) {
                Ok(0) => logs.push("библиография уже присутствует в документе".to_owned()),
                Ok(inserted) => logs.push(format!("библиография: добавлено {inserted} абзацев ({} источников)", entries.len())),
                Err(error) => {
                    self.push_log(Level::Warn, format!("Оформление: {error}"));
                    return;
                }
            }
        }
        let formatted = work.join("formatted.docx");
        if let Err(error) = wordcraft_engine::io::save_path(&formatted, &doc) {
            self.push_log(Level::Warn, format!("Оформление: не удалось сохранить .docx: {error}"));
            return;
        }
        match rde_doc::save_engine_document(&formatted, &path, &manifest.id, &template_id) {
            Ok(rde_doc) => {
                let manifest_path = document_object.dir.join("document.toml");
                set_manifest_string(&manifest_path, "template", &template_id);
                self.editor = None;
                self.doc = DocPreview::new(self.project.document_path());
                self.structure_cache = None;
                self.reload_project();
                self.push_log(
                    Level::Ok,
                    format!(
                        "Документ оформлен: шаблон {}, блоков {}, вставок {}",
                        template_id,
                        rde_doc.blocks.len(),
                        rde_doc.inserts.len()
                    ),
                );
                for line in &logs {
                    self.push_log(Level::Info, line.clone());
                }
            }
            Err(error) => self.push_log(Level::Warn, format!("Оформление: {error}")),
        }
        let _ = fs::remove_dir_all(&work);
    }

    /// Открыть копию документа в Word: экспортировать чистый `.docx` в `exports/` и, если есть
    /// графическая сессия, попробовать открыть системным обработчиком (headless — только путь).
    pub fn export_word_copy(&mut self) {
        let Some(package) = self.project.document_path() else {
            self.push_log(Level::Warn, "Копия для Word: в проекте нет документа");
            return;
        };
        let stem = package.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_else(|| "document".to_owned());
        let out = self.export_dir.join(format!("{stem}-word.docx"));
        let result = if rde_doc::is_rde_package(&package) {
            rde_doc::extract_docx(&package, &out).map(|_| ())
        } else {
            std::fs::copy(&package, &out).map(|_| ()).map_err(|e| e.to_string())
        };
        match result {
            Ok(()) => {
                self.push_log(Level::Ok, format!("Копия для Word: {}", out.display()));
                #[cfg(unix)]
                if std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
                    let _ = std::process::Command::new("xdg-open").arg(&out).spawn();
                }
            }
            Err(error) => self.push_log(Level::Warn, format!("Копия для Word: {error}")),
        }
    }

    /// Импорт внешнего ревью: взять самый свежий `.docx` из `exports/` (после правок в Word),
    /// принять его как новую версию документа, сохранив RDE-метаданные.
    pub fn import_review_from_exports(&mut self) {
        let Some(package) = self.project.document_path() else {
            self.push_log(Level::Warn, "Импорт ревью: в проекте нет документа");
            return;
        };
        let mut candidates: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(&self.export_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let is_docx = path.extension().and_then(|ext| ext.to_str()) == Some("docx");
                if !is_docx {
                    return None;
                }
                let modified = entry.metadata().and_then(|m| m.modified()).ok()?;
                Some((modified, path))
            })
            .collect();
        candidates.sort_by(|a, b| b.0.cmp(&a.0));
        let Some((_, external)) = candidates.first() else {
            self.push_log(Level::Warn, format!("Импорт ревью: в {} нет .docx", self.export_dir.display()));
            return;
        };
        match rde_doc::import_external_review(&package, external) {
            Ok(report) => {
                self.comments_cache = None;
                self.reload_project();
                self.push_log(
                    Level::Ok,
                    format!(
                        "Импорт ревью: {} (комментариев {} → {}, doc_id сохранён)",
                        external.display(),
                        report.comments_before,
                        report.comments_after
                    ),
                );
            }
            Err(error) => self.push_log(Level::Warn, format!("Импорт ревью: {error}")),
        }
    }

    /// «Обновить документ»: перенумерация подписей + пересохранение `.doc.rde` под контролем RDE.
    pub fn update_project_document(&mut self) {
        let Some(path) = self.project.document_path() else {
            self.push_log(Level::Warn, "Обновить документ: в проекте нет документа");
            return;
        };
        if !rde_doc::is_rde_package(&path) {
            self.push_log(Level::Warn, "Обновить документ: документ ещё не в .doc.rde (запустите миграцию/сохранение)");
            return;
        }
        if self.editor.is_some() {
            self.save_project_document();
        }
        match rde_doc::update_document_package(&path) {
            Ok(report) => {
                self.editor = None;
                self.doc = DocPreview::new(self.project.document_path());
                self.structure_cache = None;
                self.reload_project();
                self.push_log(
                    Level::Ok,
                    format!(
                        "Документ обновлён: подписей изменено {}, вставок {}",
                        report.changes.len(),
                        report.rde_doc.as_ref().map(|doc| doc.inserts.len()).unwrap_or(0)
                    ),
                );
                for change in &report.changes {
                    self.push_log(Level::Info, format!("Подпись: «{}» → «{}»", change.old_text, change.new_text));
                }
            }
            Err(error) => self.push_log(Level::Warn, format!("Обновить документ: {error}")),
        }
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
                self.structure_cache = None;
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

    /// Для демо/скриншотов: выбрать объект по id или вхождению в название.
    pub fn demo_select_object(&mut self, query: &str) {
        if let Some(object) = self
            .project
            .objects
            .iter()
            .find(|object| object.id == query || object.title.contains(query))
            .cloned()
        {
            self.selected = Some(object.id.clone());
            self.tab = match object.kind {
                ObjectKind::Document => Tab::Document,
                ObjectKind::Artifact | ObjectKind::Constants => Tab::Artifacts,
                ObjectKind::Source | ObjectKind::Note => Tab::Source,
                ObjectKind::Repository => Tab::Artifacts,
            };
        }
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
        // RDE — владелец сохранения: редактор работает с временной чистой копией, а запись в
        // проект (.doc.rde) делает RDE по явной команде «Сохранить проект» (см. save_project_document).
        let copy = std::env::temp_dir().join(format!("rde-embedded-doc-{}.docx", std::process::id()));
        if rde_doc::is_rde_package(&doc_path) {
            if let Err(error) = rde_doc::extract_docx(&doc_path, &copy) {
                self.editor_error = Some(format!("не удалось извлечь .doc.rde: {error}"));
                return;
            }
        } else if let Ok(bytes) = std::fs::read(&doc_path) {
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
        if self.show_inspector {
            self.inspector_panel(ui);
        }
        self.central(ui);
        self.dialogs(ui.ctx());
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
                        ui.checkbox(&mut self.show_inspector, "Инспектор объекта");
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
        let mut save_project = false;
        let mut update_document = false;
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
                    if ui
                        .add(egui::Button::new(RichText::new("Сохранить проект").color(TEXT)).fill(PANEL_HI))
                        .on_hover_text("Записать документ в .doc.rde (RDE — владелец сохранения)")
                        .clicked()
                    {
                        save_project = true;
                    }
                    if ui
                        .add(egui::Button::new(RichText::new("Обновить документ").color(TEXT)).fill(PANEL_HI))
                        .on_hover_text("Перенумеровать подписи/обновить вставки и пересохранить .doc.rde")
                        .clicked()
                    {
                        update_document = true;
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
        if save_project {
            self.save_project_document();
        }
        if update_document {
            self.update_project_document();
        }
        if new_object {
            self.create_dialog = Some(CreateDialogState { kind: NewObjectKind::ComputeArtifact, title: String::new() });
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
    // ── инспектор объекта (M2.4) ────────────────────────────────────────────
    fn selected_object(&self) -> Option<rde_core::Object> {
        self.selected.as_ref().and_then(|id| self.project.object(id).cloned())
    }

    fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        let object = self.selected_object();
        let mut open_clicked = false;
        let mut rename_clicked = false;
        let mut delete_clicked = false;
        let mut update_clicked = false;
        let mut gost_clicked = false;
        let mut bibliography_clicked = false;
        let mut word_copy_clicked = false;
        let mut review_import_clicked = false;
        egui::Panel::right("rde_inspector")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 8)))
            .default_size(320.0)
            .resizable(false)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("rde_inspector_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    let Some(object) = &object else {
                        ui.add_space(8.0);
                        ui.label(RichText::new("Объект не выбран").color(MUTED));
                        ui.label(RichText::new("Выберите объект в дереве проекта").small().color(MUTED));
                        return;
                    };
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(object.kind.label_ru()).small().color(MUTED));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button("×").on_hover_text("Скрыть инспектор").clicked() {
                                self.show_inspector = false;
                            }
                        });
                    });
                    ui.label(RichText::new(&object.title).size(16.0).strong().color(TEXT));
                    ui.add_space(1.0);
                    ui.label(RichText::new(fresh_label(object.freshness)).small().color(freshness_color(object.freshness)));
                    ui.add_space(6.0);
                    field_row(ui, "id", &object.id, true);
                    field_row(ui, "каталог", &object.dir.display().to_string(), true);
                    field_row(ui, "детали", &object.detail, false);
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new("Открыть").size(12.0)).clicked() {
                            open_clicked = true;
                        }
                        if ui.button(RichText::new("Переименовать").size(12.0)).clicked() {
                            rename_clicked = true;
                        }
                        if ui.button(RichText::new("Удалить").size(12.0)).clicked() {
                            delete_clicked = true;
                        }
                    });
                    ui.separator();
                    match &object.data {
                        ObjectData::Artifact(artifact) => {
                            field_row(ui, "тип", &artifact.kind, false);
                            if let Some(runtime) = &artifact.runtime {
                                field_row(ui, "runtime", &format!("{} · {}", runtime.kind, runtime.entry), false);
                            }
                            if !artifact.outputs.is_empty() {
                                field_row(ui, "выходы", &artifact.outputs.join(", "), false);
                            }
                            if !artifact.params.is_null() {
                                field_row(ui, "params", &artifact.params.to_string(), true);
                            }
                            let mut hashed = 0;
                            let mut refed = 0;
                            for group in &artifact.groups {
                                if group.mode == "hashed" { hashed += group.inputs.len(); } else { refed += group.inputs.len(); }
                            }
                            field_row(ui, "входы", &format!("{hashed} hashed · {refed} ref"), false);
                            if let Some(status) = self.dag_statuses.get(&object.id) {
                                ui.add_space(6.0);
                                ui.label(RichText::new("DAG / активация").small().color(MUTED));
                                if let Some(act_key) = &status.act_key {
                                    field_row(ui, "act_key", &act_key.chars().take(20).collect::<String>(), true);
                                }
                                if let Some(out_hash) = &status.out_hash {
                                    field_row(ui, "out_hash", &out_hash.chars().take(20).collect::<String>(), true);
                                }
                                if let Some(run_id) = &status.run_id {
                                    field_row(ui, "последний run", run_id, true);
                                }
                                if let Some(updated) = status.updated_unix {
                                    field_row(ui, "обновлён", &format!("unix {updated}"), false);
                                }
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    if ui.button(RichText::new("Обновить артефакт").size(12.0)).clicked() {
                                        update_clicked = true;
                                    }
                                    if ui.small_button("копировать act_key").clicked() {
                                        if let Some(act_key) = &status.act_key {
                                            ui.ctx().copy_text(act_key.clone());
                                        }
                                    }
                                });
                            }
                        }
                        ObjectData::Source(source) => {
                            field_row(ui, "kind", &source.kind, false);
                            if let Some(file) = &source.file {
                                field_row(ui, "файл", file, false);
                            }
                            if let Some(url) = &source.url {
                                field_row(ui, "url", url, true);
                            }
                            if let Some(reference) = &source.reference {
                                if !reference.title.is_empty() {
                                    field_row(ui, "библиография", &reference.title, false);
                                }
                            }
                        }
                        ObjectData::Note(note) => {
                            field_row(ui, "файл", &note.file, false);
                            let path = object.dir.join(&note.file);
                            if let Ok(text) = std::fs::read_to_string(&path) {
                                ui.add_space(4.0);
                                ui.label(RichText::new("содержимое").small().color(MUTED));
                                let preview: String = text.chars().take(400).collect();
                                ui.label(RichText::new(preview).size(12.0).color(TEXT));
                            }
                        }
                        ObjectData::Repository(repo) => {
                            field_row(ui, "kind", &repo.kind, false);
                            field_row(ui, "path", &repo.path, true);
                        }
                        ObjectData::Document(document) => {
                            field_row(ui, "файл", &document.file, false);
                            field_row(ui, "шаблон", if document.template.is_empty() { "—" } else { &document.template }, false);
                            if rde_doc::is_rde_package(&object.dir.join(&document.file)) {
                                let package = object.dir.join(&document.file);
                                let comments_key = format!("{}:{}", package.display(), fs::metadata(&package).and_then(|m| m.modified()).map(|t| format!("{t:?}")).unwrap_or_default());
                                let fresh = self.comments_cache.as_ref().is_some_and(|(key, _)| key == &comments_key);
                                if !fresh {
                                    let count = rde_doc::comments(&package).map(|comments| comments.len()).unwrap_or(0);
                                    self.comments_cache = Some((comments_key, count));
                                }
                                if let Some((_, count)) = &self.comments_cache {
                                    field_row(ui, "комментариев Word", &count.to_string(), false);
                                }
                                let template = rde_templates::template_by_id(if document.template.is_empty() { "gost-7.32-2017" } else { &document.template });
                                if let Some(template) = template {
                                    field_row(ui, "описание", &template.description, false);
                                }
                                ui.add_space(4.0);
                                ui.horizontal(|ui| {
                                    if ui.button(RichText::new("Оформить по ГОСТ").size(12.0)).clicked() {
                                        gost_clicked = true;
                                    }
                                    if ui.button(RichText::new("Вставить библиографию").size(12.0)).clicked() {
                                        bibliography_clicked = true;
                                    }
                                });
                                ui.horizontal(|ui| {
                                    if ui.button(RichText::new("Открыть копию в Word").size(12.0)).clicked() {
                                        word_copy_clicked = true;
                                    }
                                    if ui.button(RichText::new("Импорт ревью из exports").size(12.0)).clicked() {
                                        review_import_clicked = true;
                                    }
                                });
                            }
                        }
                    }
                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("id").small().color(MUTED));
                        ui.label(RichText::new(&object.id).monospace().size(10.5).color(MUTED));
                        if ui.small_button("копировать").clicked() {
                            ui.ctx().copy_text(object.id.clone());
                        }
                    });
                });
            });
        if let Some(object) = &object {
            if open_clicked {
                self.tab = match object.kind {
                    ObjectKind::Document => Tab::Document,
                    ObjectKind::Artifact | ObjectKind::Constants => Tab::Artifacts,
                    ObjectKind::Source | ObjectKind::Note => Tab::Source,
                    ObjectKind::Repository => Tab::Artifacts,
                };
            }
            if rename_clicked {
                self.rename_dialog = Some(RenameDialogState { id: object.id.clone(), title: object.title.clone() });
            }
            if delete_clicked {
                self.delete_confirm = Some(object.id.clone());
            }
            if update_clicked {
                self.update_dependencies(Some(vec![object.id.clone()]));
            }
            if gost_clicked {
                self.update_document_package(true, true);
            }
            if bibliography_clicked {
                self.update_document_package(false, true);
            }
            if word_copy_clicked {
                self.export_word_copy();
            }
            if review_import_clicked {
                self.import_review_from_exports();
            }
        }
    }

    fn create_object_action(&mut self, kind: NewObjectKind, title: &str) -> Result<(), String> {
        let dir = rde_core::create_object(&self.project.root, &CreateRequest { kind, title: title.to_owned() })?;
        self.reload_project();
        self.selected = self.project.objects.iter().find(|object| object.dir == dir).map(|object| object.id.clone());
        self.push_log(Level::Ok, format!("Создан объект «{}» ({})", title.trim(), kind.label_ru()));
        Ok(())
    }

    fn rename_object_action(&mut self, id: &str, title: &str) -> Result<(), String> {
        rde_core::rename_object(&self.project, id, title)?;
        self.reload_project();
        self.selected = Some(id.to_owned());
        self.push_log(Level::Ok, format!("Объект переименован: «{}»", title.trim()));
        Ok(())
    }

    fn delete_object_action(&mut self, id: &str) -> Result<(), String> {
        let title = self.project.object(id).map(|object| object.title.clone()).unwrap_or_else(|| id.to_owned());
        let trash = rde_core::delete_object(&self.project, id)?;
        self.selected = None;
        self.reload_project();
        self.push_log(Level::Ok, format!("Объект «{title}» перемещён в корзину: {}", trash.display()));
        Ok(())
    }

    /// Окна диалогов создания/переименования/удаления.
    fn dialogs(&mut self, ctx: &egui::Context) {
        if let Some(mut dialog) = self.create_dialog.take() {
            let mut create = false;
            let mut cancel = false;
            egui::Window::new("Новый объект")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(420.0);
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Тип").color(MUTED));
                        egui::ComboBox::from_id_salt("rde_new_kind")
                            .selected_text(dialog.kind.label_ru())
                            .show_ui(ui, |ui| {
                                for kind in NewObjectKind::ALL {
                                    ui.selectable_value(&mut dialog.kind, kind, kind.label_ru());
                                }
                            });
                    });
                    ui.add_space(6.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.title)
                            .hint_text("Название объекта")
                            .desired_width(f32::INFINITY),
                    );
                    ui.label(RichText::new("Документы создаются в M3 (нужен .doc.rde)").small().color(MUTED));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Создать").clicked() {
                            create = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                });
            if create {
                let kind = dialog.kind;
                let title = dialog.title.trim().to_owned();
                if let Err(error) = self.create_object_action(kind, &title) {
                    self.push_log(Level::Warn, format!("Не удалось создать объект: {error}"));
                }
            } else if !cancel {
                self.create_dialog = Some(dialog);
            }
        }

        if let Some(mut dialog) = self.rename_dialog.take() {
            let mut rename = false;
            let mut cancel = false;
            egui::Window::new("Переименовать объект")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(420.0);
                    ui.add(
                        egui::TextEdit::singleline(&mut dialog.title)
                            .hint_text("Новое название")
                            .desired_width(f32::INFINITY),
                    );
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Переименовать").clicked() {
                            rename = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                });
            if rename {
                if let Err(error) = self.rename_object_action(&dialog.id, dialog.title.trim()) {
                    self.push_log(Level::Warn, format!("Не удалось переименовать: {error}"));
                }
            } else if !cancel {
                self.rename_dialog = Some(dialog);
            }
        }

        if let Some(id) = self.delete_confirm.clone() {
            let title = self.project.object(&id).map(|object| object.title.clone()).unwrap_or_else(|| id.clone());
            let mut confirm = false;
            let mut cancel = false;
            egui::Window::new("Удалить объект?")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(400.0);
                    ui.label(RichText::new(format!("«{title}»")).strong());
                    ui.label(RichText::new("Объект будет перемещён в .rde/trash (безвозвратного удаления нет).").small().color(MUTED));
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button("Удалить").clicked() {
                            confirm = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                });
            if confirm {
                if let Err(error) = self.delete_object_action(&id) {
                    self.push_log(Level::Warn, format!("Не удалось удалить объект: {error}"));
                }
            } else if cancel {
                self.delete_confirm = None;
            } else {
                self.delete_confirm = Some(id);
            }
        }
    }

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
        if self.structure_cache.is_none() {
            let path = self.project.document_path();
            self.structure_cache = Some(match path {
                Some(path) => extract_document_structure(&path).map_err(|error| format!("{}: {error}", path.display())),
                None => Err("в проекте нет документа".to_owned()),
            });
        }
        let cache = self.structure_cache.clone();
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Блоки документа").strong().color(TEXT));
            if let Some(Ok((blocks, figures))) = &cache {
                let headings = blocks.iter().filter(|block| matches!(block.kind, "H1" | "H2" | "H")).count();
                ui.label(
                    RichText::new(format!("{} блоков · {headings} заголовков · {figures} рисунков · из модели WordCraft", blocks.len()))
                        .small()
                        .color(MUTED),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("вставки rde:doc — M3").small().color(MUTED));
            });
        });
        ui.separator();
        egui::ScrollArea::both().id_salt("rde_struct_scroll").auto_shrink([false, false]).show(ui, |ui| {
            match cache {
                Some(Ok((blocks, _figures))) => {
                    egui::Grid::new("rde_blocks").num_columns(5).spacing([14.0, 5.0]).striped(true).show(ui, |ui| {
                        ui.label(RichText::new("№").small().color(MUTED));
                        ui.label(RichText::new("тип").small().color(MUTED));
                        ui.label(RichText::new("стиль").small().color(MUTED));
                        ui.label(RichText::new("путь (block)").small().color(MUTED));
                        ui.label(RichText::new("содержимое").small().color(MUTED));
                        ui.end_row();
                        for block in &blocks {
                            ui.label(RichText::new(block.number.to_string()).monospace().size(11.0).color(MUTED));
                            let color = match block.kind {
                                "H1" => ACCENT,
                                "H2" | "H" => Color32::from_rgb(0x7c, 0xa8, 0xff),
                                "FIG" => AMBER,
                                "CAP" => GREEN,
                                _ => MUTED,
                            };
                            let kind_label = if block.images > 0 {
                                format!("{}·{}", block.kind, block.images)
                            } else {
                                block.kind.to_owned()
                            };
                            ui.label(RichText::new(kind_label).monospace().size(11.0).color(color));
                            ui.label(RichText::new(if block.style.is_empty() { "—" } else { &block.style }).monospace().size(10.5).color(MUTED));
                            ui.label(RichText::new(&block.anchor).monospace().size(10.5).color(MUTED));
                            ui.label(RichText::new(&block.text).size(12.0).color(TEXT));
                            ui.end_row();
                        }
                    });
                }
                Some(Err(error)) => {
                    ui.add_space(12.0);
                    ui.label(RichText::new("Не удалось разобрать документ").strong().color(RED));
                    ui.label(RichText::new(error).small().color(MUTED));
                }
                None => {}
            }
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
                        "PDF: текстовый слой и рендер (poppler), OCR (tesseract).",
                        "Импорт цитат BibTeX/RIS/CSL-JSON — rde-sources.",
                        "Веб-архив (monolith) — при наличии инструмента.",
                        "",
                        "Источник доступен агенту как объект проекта.",
                    ] {
                        ui.label(RichText::new(line).size(12.0).color(TEXT));
                    }
                });
        });
        // M5: PDF-источник — страницы, текстовый слой и OCR (poppler/tesseract).
        if let Some(file) = src.file.clone() {
            let path = obj.dir.join(&file);
            let lower = file.to_ascii_lowercase();
            if lower.ends_with(".pdf") {
                let key = path.display().to_string();
                let fresh = self.pdf_cache.as_ref().is_some_and(|(cached, _)| cached == &key);
                if !fresh {
                    let info = (|| -> Result<PdfSourceInfo, String> {
                        let pages = rde_sources::pdf_page_count(&path)?;
                        let text = rde_sources::pdf_text(&path).unwrap_or_default();
                        Ok(PdfSourceInfo { pages, text })
                    })();
                    self.pdf_cache = Some((key, info));
                }
                if let Some((_, info)) = &self.pdf_cache {
                    ui.add_space(6.0);
                    match info {
                        Ok(info) => {
                            ui.label(RichText::new(format!("PDF · {} стр. · текстовый слой {} символов", info.pages, info.text.chars().count())).small().color(MUTED));
                            let excerpt: String = info.text.chars().take(600).collect();
                            egui::ScrollArea::vertical().id_salt("rde_pdf_text").max_height(160.0).show(ui, |ui| {
                                ui.label(RichText::new(excerpt).monospace().size(11.0).color(TEXT));
                            });
                            if ui.button(RichText::new("OCR страницы 1").size(12.0)).clicked() {
                                match rde_sources::ocr_pdf_page(&path, 1) {
                                    Ok(text) => {
                                        let _ = fs::write(obj.dir.join("ocr.txt"), &text);
                                        let preview: String = text.chars().take(400).collect();
                                        self.push_log(Level::Ok, format!("OCR PDF: распознано {} символов (ocr.txt)", text.chars().count()));
                                        self.push_log(Level::Info, preview);
                                    }
                                    Err(error) => self.push_log(Level::Warn, format!("OCR: {error}")),
                                }
                            }
                        }
                        Err(error) => {
                            ui.label(RichText::new(format!("PDF: {error}")).small().color(AMBER));
                        }
                    }
                }
            } else if [".png", ".jpg", ".jpeg", ".webp", ".tif", ".tiff"].iter().any(|ext| lower.ends_with(ext)) {
                ui.add_space(6.0);
                if ui.button(RichText::new("Распознать изображение (OCR)").size(12.0)).clicked() {
                    match rde_sources::ocr_image(&path) {
                        Ok(text) => {
                            let _ = fs::write(obj.dir.join("ocr.txt"), &text);
                            self.push_log(Level::Ok, format!("OCR изображения: {} символов (ocr.txt)", text.chars().count()));
                            self.push_log(Level::Info, text.chars().take(400).collect::<String>());
                        }
                        Err(error) => self.push_log(Level::Warn, format!("OCR: {error}")),
                    }
                }
            }
        }

    }


    fn code_tab(&mut self, ui: &mut egui::Ui) {
        let object = self.selected_object();
        let (header, path, mut text) = match &object {
            Some(object) => {
                let name = match &object.data {
                    ObjectData::Document(_) => "document.toml",
                    ObjectData::Artifact(_) => "artifact.toml",
                    ObjectData::Source(_) => "source.toml",
                    ObjectData::Note(_) => "note.toml",
                    ObjectData::Repository(_) => "repo.toml",
                };
                let path = object.dir.join(name);
                let text = std::fs::read_to_string(&path)
                    .unwrap_or_else(|error| format!("# не удалось прочитать {}: {error}\n", path.display()));
                (format!("Манифест объекта «{}»: {}", object.title, path.display()), path, text)
            }
            None => ("Объект не выбран".to_owned(), PathBuf::new(), "# выберите объект в дереве проекта\n".to_owned()),
        };
        if let Some(object) = &object {
            if let ObjectData::Document(document) = &object.data {
                let file = object.dir.join(&document.file);
                let size = fs::metadata(&file).map(|metadata| metadata.len()).unwrap_or(0);
                if rde_doc::is_rde_package(&file) {
                    // M3: показываем реальную RDE-часть пакета (.doc.rde) + манифест.
                    if let Ok(package_doc) = rde_doc::load_rde_doc(&file) {
                        let xml = rde_doc::render_rde_doc(&package_doc);
                        text = format!(
                            "# .doc.rde: {}\n# RDE-часть customXml/item1.xml ({} блоков, {} вставок)\n\n{}\n# ───── document.toml ─────\n{}",
                            file.display(),
                            package_doc.blocks.len(),
                            package_doc.inserts.len(),
                            xml,
                            text
                        );
                    } else {
                        text = format!("# .doc.rde: {} ({size} байт)\n\n{text}", file.display());
                    }
                } else {
                    text.push_str(&format!(
                        "\n# файл документа: {} ({size} байт)\n# пакет .doc.rde (customXml/rde:doc) — появится после сохранения проекта\n",
                        file.display()
                    ));
                }
            }
        }
        let path_label = if path.as_os_str().is_empty() { String::new() } else { path.display().to_string() };
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Манифест / код объекта").strong().color(TEXT));
            ui.label(RichText::new(path_label).small().color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("read-only · источник истины — файлы проекта").small().color(MUTED));
            });
        });
        ui.separator();
        egui::ScrollArea::both().id_salt("rde_code_scroll").auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(4.0);
            ui.add(
                egui::TextEdit::multiline(&mut text)
                    .font(egui::TextStyle::Monospace)
                    .desired_width(f32::INFINITY)
                    .desired_rows(28)
                    .interactive(false),
            );
            ui.add_space(4.0);
            ui.label(RichText::new(header).small().color(MUTED));
        });
    }

}

// ── M2.4: данные инспектора/структуры ────────────────────────────────────────

/// Записать/заменить строковое поле верхнего уровня в TOML-манифесте (без переформатирования файла).
fn set_manifest_string(path: &Path, key: &str, value: &str) {
    let Ok(text) = fs::read_to_string(path) else { return };
    let mut out = String::with_capacity(text.len() + value.len() + 8);
    let mut replaced = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if !replaced && trimmed.starts_with(&format!("{key} =")) && !line.starts_with(' ') {
            out.push_str(&format!("{key} = {value:?}\n"));
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !replaced {
        out.push_str(&format!("{key} = {value:?}\n"));
    }
    let _ = fs::write(path, out);
}

fn field_row(ui: &mut egui::Ui, label: &str, value: &str, monospace: bool) {
    ui.horizontal_wrapped(|ui| {
        ui.label(RichText::new(label).small().color(MUTED));
        if monospace {
            ui.label(RichText::new(value).monospace().size(10.5).color(TEXT));
        } else {
            ui.label(RichText::new(value).size(12.0).color(TEXT));
        }
    });
}

/// Разобрать реальную структуру документа через модель WordCraft (M2.4): абзацы, стили,
/// рисунки, подписи. Никаких mock-блоков — используется настоящий `.docx` проекта.
fn extract_document_structure(path: &Path) -> Result<(Vec<DocBlock>, usize), String> {
    let (doc, _from_package) = rde_doc::open_document(path)?;
    let story = wordcraft_engine::doc::StoryRef::Body;
    let paths = doc.para_paths(story);
    let mut blocks = Vec::new();
    let mut figures = 0usize;
    for (index, para_path) in paths.iter().enumerate() {
        let Some(paragraph) = doc.para(story, para_path) else { continue };
        let images = paragraph
            .objects
            .iter()
            .filter(|object| matches!(object, wordcraft_engine::doc::InlineObject::Image { .. }))
            .count();
        figures += images;
        let style = paragraph.props.style.clone().unwrap_or_default();
        let lower = style.to_ascii_lowercase();
        let kind: &'static str = if lower == "1" || lower == "heading1" || lower.starts_with("heading 1") {
            "H1"
        } else if lower == "2" || lower == "heading2" || lower.starts_with("heading 2") {
            "H2"
        } else if lower.starts_with("heading") {
            "H"
        } else if images > 0 {
            "FIG"
        } else if paragraph.text.trim_start().starts_with("Рисунок") || paragraph.text.trim_start().starts_with("Таблица") {
            "CAP"
        } else {
            "P"
        };
        if paragraph.text.trim().is_empty() && images == 0 {
            continue;
        }
        let mut text = paragraph.text.trim().replace('\n', " ");
        if text.chars().count() > 96 {
            text = format!("{}…", text.chars().take(96).collect::<String>());
        }
        blocks.push(DocBlock { number: index + 1, kind, style, anchor: para_path.to_string(), text, images });
    }
    Ok((blocks, figures))
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

    // ── M2.4: реальная структура документа и CRUD объектов ──────────────────
    #[test]
    fn structure_parser_reads_real_docx() {
        let project = rde_core::load_project(&rde_core::demo_project_dir()).unwrap();
        let path = project.document_path().expect("в демо-проекте есть документ");
        let (blocks, figures) = super::extract_document_structure(&path).expect("разобрать .docx");
        assert!(blocks.len() >= 8, "блоков: {}", blocks.len());
        assert!(figures >= 1, "рисунков: {figures}");
        assert!(blocks.iter().any(|block| block.kind == "FIG"), "должен быть блок-рисунок: {:?}", blocks.iter().map(|b| b.kind).collect::<Vec<_>>());
        assert!(blocks.iter().all(|block| block.number > 0));
    }

    #[test]
    fn save_project_writes_doc_rde_and_update_document_works() {
        let root = temp_demo_copy("doc-save");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        let package = app.project.document_path().expect("документ");
        assert_eq!(package.extension().and_then(|ext| ext.to_str()), Some("rde"));

        // Открыть редактор (он работает на извлечённой копии) и сохранить проект силами RDE.
        app.ensure_editor();
        assert!(app.editor.is_some(), "редактор должен открыть .doc.rde: {:?}", app.editor_error);
        app.save_project_document();
        assert!(
            app.project.log.iter().any(|line| line.text.contains("Проект сохранён")),
            "нет записи о сохранении: {:?}",
            app.project.log.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
        let check = rde_doc::check_external_edit(&package).unwrap();
        assert_eq!(check.verdict, rde_doc::EditVerdict::NotModified, "{check:?}");
        let rde_doc = rde_doc::load_rde_doc(&package).unwrap();
        assert!(rde_doc.blocks.len() >= 8);
        assert!(rde_doc.inserts.iter().any(|insert| insert.kind == "figure" && !insert.target.is_empty()));

        // «Обновить документ»: перенумерация подписей + пересохранение пакета под контролем RDE.
        app.update_project_document();
        assert!(
            app.project.log.iter().any(|line| line.text.contains("Документ обновлён")),
            "нет записи об обновлении: {:?}",
            app.project.log.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
        assert_eq!(rde_doc::check_external_edit(&package).unwrap().verdict, rde_doc::EditVerdict::NotModified);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gost_formatting_from_ui_persists_to_doc_rde() {
        let root = temp_demo_copy("gost");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        let package = app.project.document_path().expect("документ");
        app.update_document_package(true, true);
        assert!(
            app.project.log.iter().any(|line| line.text.contains("Документ оформлен")),
            "нет записи об оформлении: {:?}",
            app.project.log.iter().map(|l| &l.text).collect::<Vec<_>>()
        );
        // Манифест и пакет обновились согласованно.
        let manifest = std::fs::read_to_string(app.project.document_object().unwrap().dir.join("document.toml")).unwrap();
        assert!(manifest.contains("template = \"gost-7.32-2017\""), "{manifest}");
        assert_eq!(rde_doc::check_external_edit(&package).unwrap().verdict, rde_doc::EditVerdict::NotModified);
        let rde_doc = rde_doc::load_rde_doc(&package).unwrap();
        assert_eq!(rde_doc.template, "gost-7.32-2017");

        // Чистый OOXML: поля ГОСТ и список источников на месте, LO-гейт проходит.
        let work = std::env::temp_dir().join(format!("rde-ui-gost-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&work);
        std::fs::create_dir_all(&work).unwrap();
        let clean = work.join("clean.docx");
        rde_doc::extract_docx(&package, &clean).unwrap();
        let (doc, _) = rde_doc::open_document(&clean).unwrap();
        let text = doc.plain_text(wordcraft_engine::doc::StoryRef::Body);
        assert!(text.contains("СПИСОК ИСПОЛЬЗОВАННЫХ ИСТОЧНИКОВ"), "нет библиографии");
        let sections = doc.sections();
        let margin_left = sections.first().unwrap().1.margin_left;
        assert!((margin_left - 30.0 * 72.0 / 25.4).abs() < 0.5, "margin_left={margin_left}");
        let report = rde_docx::run_gate(&clean, &work.join("gate")).unwrap();
        assert!(report.critical_missing.is_empty(), "{:?}", report.critical_missing);
        if report.lo.available {
            assert!(report.passed(), "{report:?}");
        }
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&work);
    }

    #[test]
    fn pdf_source_info_is_available() {
        let project = rde_core::load_project(&rde_core::demo_project_dir()).unwrap();
        assert!(project.issues.is_empty(), "{:?}", project.issues);
        let pdf_source = project
            .sources()
            .find(|object| matches!(&object.data, rde_core::ObjectData::Source(source) if source.file.as_deref().is_some_and(|file| file.ends_with(".pdf"))))
            .expect("в демо-проекте должен быть PDF-источник");
        let rde_core::ObjectData::Source(source) = &pdf_source.data else { panic!() };
        let path = pdf_source.dir.join(source.file.as_ref().unwrap());
        assert!(path.is_file(), "PDF-файл источника: {}", path.display());
        let pages = rde_sources::pdf_page_count(&path).expect("pdfinfo");
        assert!(pages >= 1);
        let text = rde_sources::pdf_text(&path).expect("pdftotext");
        assert!(text.chars().count() > 100, "текстовый слой: {}", text.chars().count());
    }

    #[test]
    fn structure_parser_reads_doc_rde_package() {
        let project = rde_core::load_project(&rde_core::demo_project_dir()).unwrap();
        let path = project.document_path().expect("документ");
        assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("rde"));
        let (blocks, figures) = super::extract_document_structure(&path).expect("разобрать .doc.rde");
        assert!(blocks.len() >= 8, "блоков: {}", blocks.len());
        assert!(figures >= 1);
    }

    #[test]
    fn object_crud_from_ui_actions() {
        let root = std::env::temp_dir().join(format!("rde-ui-crud-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("rde.toml"),
            "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"CRUD-тест\"\n",
        )
        .unwrap();
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);

        app.create_object_action(rde_core::NewObjectKind::ComputeArtifact, "Новый расчёт").expect("создать");
        assert_eq!(app.project.object_count(), 1);
        let id = app.selected.clone().expect("новый объект выбран");
        assert_eq!(app.project.object(&id).unwrap().title, "Новый расчёт");
        assert!(app.project.object(&id).unwrap().dir.join("script.py").is_file());

        app.rename_object_action(&id, "Переименованный расчёт").expect("переименовать");
        assert_eq!(app.project.object(&id).unwrap().title, "Переименованный расчёт");

        app.delete_object_action(&id).expect("удалить");
        assert_eq!(app.project.object_count(), 0);
        assert!(app.selected.is_none());
        assert!(root.join(".rde/trash").is_dir());

        let _ = std::fs::remove_dir_all(&root);
    }
}
