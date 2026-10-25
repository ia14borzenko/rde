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

/// Тип объекта в диалоге создания (документ создаётся силами UI: нужен WordCraft-движок).
#[derive(Clone, Copy, PartialEq, Eq)]
enum UiObjectKind {
    Core(NewObjectKind),
    Document,
}

impl UiObjectKind {
    const ALL: [UiObjectKind; 7] = [
        UiObjectKind::Core(NewObjectKind::ComputeArtifact),
        UiObjectKind::Core(NewObjectKind::DataArtifact),
        UiObjectKind::Core(NewObjectKind::ConstantsArtifact),
        UiObjectKind::Document,
        UiObjectKind::Core(NewObjectKind::Source),
        UiObjectKind::Core(NewObjectKind::Note),
        UiObjectKind::Core(NewObjectKind::Repository),
    ];

    fn label_ru(self) -> &'static str {
        match self {
            UiObjectKind::Document => "Документ (.doc.rde)",
            UiObjectKind::Core(kind) => kind.label_ru(),
        }
    }
}

/// Действие из контекстного меню дерева объектов.
#[derive(Clone, Debug)]
enum TreeAction {
    Open(String),
    Rename(String),
    Delete(String),
    CopyId(String),
    Update(String),
    UpdateDependents(String),
    FormatGost(String),
    InsertBibliography(String),
    WordCopy,
    ImportReview,
    Ocr(String),
    Reveal(String),
    Inspect(String),
}

/// Состояние диалога создания объекта.
struct CreateDialogState {
    kind: UiObjectKind,
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
/// Диалог согласия на запуск скриптов (R-RUN-06): что запустить после подтверждения.
#[derive(Clone, Debug)]
struct TrustDialogState {
    targets: Option<Vec<String>>,
    dependents: bool,
}

/// Результат фонового обновления (возвращается из потока вместе с хранилищем).
struct UpdateJobResult {
    store: Option<Store>,
    report: Option<rde_dag::UpdateReport>,
    error: Option<String>,
}

/// Фоновое обновление артефактов: UI не блокируется, есть отмена (R-RUN-05).
struct UpdateJob {
    handle: std::thread::JoinHandle<UpdateJobResult>,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    started: std::time::Instant,
}

pub struct RdeApp {
    pub project: Project,
    tab: Tab,
    tree_filter: String,
    selected: Option<String>,
    doc: DocPreview,
    zoom: f32,
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
    /// Кэш числа вставок rde:doc для вкладки «Структура».
    inserts_cache: Option<(String, usize)>,
    /// Фокус на поиске объектов (кнопка «Поиск», меню «Найти», Ctrl+F).
    search_focus: bool,
    open_project_dialog: Option<String>,
    new_project_dialog: Option<(String, String)>,
    settings_dialog: Option<(String, String, String)>,
    check_dialog: Option<String>,
    audit_dialog: Option<Vec<String>>,
    about_open: bool,
    /// Диалог «проект доверенный» перед первым запуском скриптов.
    trust_dialog: Option<TrustDialogState>,
    /// Фоновое обновление артефактов (очередь/отмена).
    update_job: Option<UpdateJob>,
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
            inserts_cache: None,
            search_focus: false,
            open_project_dialog: None,
            new_project_dialog: None,
            settings_dialog: None,
            check_dialog: None,
            audit_dialog: None,
            about_open: false,
            trust_dialog: None,
            update_job: None,
        };
        // R-RUN-08: ретенция запусков (логи/провенанс не растут бесконечно).
        let _ = rde_run::prune_runs(&app.project.root, 200);
        match Store::open(&app.project.root) {
            Ok(store) => {
                app.store = Some(store);
                app.refresh_dag_statuses();
            }
            Err(error) => app.push_log(Level::Warn, format!("Хранилище проекта недоступно: {error}")),
        }
        app.check_external_edit_on_startup();
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
                live => format!("{} · {}", artifact.kind, live.reason().unwrap_or_else(|| live.label_ru().to_owned())),
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

    /// Запросить обновление: если проект не помечен доверенным — сначала диалог согласия
    /// (R-RUN-06); если обновление уже идёт — не запускать второе (очередь из одного задания).
    pub fn request_update(&mut self, targets: Option<Vec<String>>, dependents: bool) {
        if self.update_job.is_some() {
            self.push_log(Level::Info, "Обновление уже выполняется — дождитесь завершения или отмените");
            return;
        }
        if !self.project.manifest.trusted {
            self.trust_dialog = Some(TrustDialogState { targets, dependents });
            return;
        }
        self.start_update(targets, dependents);
    }

    /// Запустить обновление в фоне: UI остаётся отзывчивым, есть кнопка «Отменить» (R-RUN-05).
    pub fn start_update(&mut self, targets: Option<Vec<String>>, dependents: bool) {
        let Some(store) = self.store.take() else {
            self.push_log(Level::Warn, "Обновление невозможно: хранилище проекта не открыто");
            return;
        };
        let project = self.project.clone();
        let force = targets.is_some() && !dependents;
        let batch = targets.is_none() || dependents;
        let options = rde_dag::UpdateOptions { targets, include_dependents: dependents, force, batch, ..Default::default() };
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut options = options;
        options.cancel = Some(cancel.clone());
        let handle = std::thread::spawn(move || {
            let mut store = store;
            let (report, error) = match rde_dag::update(&project, &mut store, &options) {
                Ok(report) => (Some(report), None),
                Err(error) => (None, Some(error)),
            };
            UpdateJobResult { store: Some(store), report, error }
        });
        self.update_job = Some(UpdateJob { handle, cancel, started: std::time::Instant::now() });
        self.push_log(Level::Info, "Обновление артефактов запущено в фоне (кнопка «Отменить» в панели сверху)");
    }

    /// Отменить текущее фоновое обновление (SIGTERM → grace → SIGKILL для запущенного скрипта).
    pub fn cancel_update(&mut self) {
        if let Some(job) = &self.update_job {
            job.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            self.push_log(Level::Info, "Отмена обновления: текущий запуск будет остановлен");
        }
    }

    /// Забрать результат фонового задания, когда оно завершилось.
    fn poll_update_job(&mut self) {
        let Some(job) = self.update_job.take() else { return };
        if !job.handle.is_finished() {
            self.update_job = Some(job);
            return;
        }
        let was_cancelled = job.cancel.load(std::sync::atomic::Ordering::Relaxed);
        match job.handle.join() {
            Ok(result) => {
                if let Some(store) = result.store {
                    self.store = Some(store);
                }
                if let Some(report) = result.report {
                    self.reload_project();
                    self.log_update_report(&report);
                }
                if let Some(error) = result.error {
                    self.push_log(Level::Warn, format!("Обновление не удалось: {error}"));
                }
                if was_cancelled {
                    self.push_log(Level::Info, "Обновление отменено пользователем");
                }
            }
            Err(_) => self.push_log(Level::Warn, "Обновление: фоновый поток завершился аварийно"),
        }
    }

    /// Записать отчёт обновления в журнал (общий путь для синхронного и фонового запуска).
    fn log_update_report(&mut self, report: &rde_dag::UpdateReport) {
        for artifact in &report.artifacts {
            let text = match artifact.action {
                UpdateAction::Ran => format!(
                    "Артефакт «{}»: {} (act_key {})",
                    artifact.title,
                    artifact.message,
                    artifact.act_key.as_deref().map(|key| &key[..12.min(key.len())]).unwrap_or("—")
                ),
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
        for note in &report.notes {
            self.push_log(Level::Info, note.clone());
        }
        if report.failed() == 0 {
            for error in &report.errors {
                self.push_log(Level::Warn, error.clone());
            }
        }
        self.push_log(
            if report.failed() > 0 { Level::Warn } else { Level::Ok },
            format!("Обновление связанных: выполнено {}, пропущено {}, ошибок {}", report.ran(), report.skipped(), report.failed()),
        );
    }

    /// Реальное обновление артефактов через DAG/транзакции (`targets=None` — все устаревшие).
    ///
    /// `dependents = true` — «обновить связанные» (цели + транзитивно зависимые);
    /// `dependents = false` — «обновить артефакт» (только цель, принудительно даже если актуален).
    pub fn update_dependencies(&mut self, targets: Option<Vec<String>>, dependents: bool) -> Option<rde_dag::UpdateReport> {
        if self.store.is_none() {
            self.push_log(Level::Warn, "Обновление невозможно: хранилище проекта не открыто");
            return None;
        }
        let force = targets.is_some() && !dependents;
        // «Обновить всё»/«связанные» — одна транзакция на пачку (атомарный откат),
        // точечное «обновить артефакт» — своя транзакция (гранулярный откат).
        let batch = targets.is_none() || dependents;
        let options = rde_dag::UpdateOptions { targets, include_dependents: dependents, force, batch, ..Default::default() };
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
        self.log_update_report(&report);
        Some(report)
    }

    /// Откатить обновление артефактов (последнее или конкретную транзакцию): store переключает
    /// корни O(1), dag материализует прежние выходы из CAS и возвращает ledger-состояния.
    pub fn rollback_transaction(&mut self, txid: Option<String>) -> Option<rde_dag::RollbackOutcome> {
        let Some(store) = self.store.as_mut() else {
            self.push_log(Level::Warn, "Откат невозможен: хранилище проекта не открыто");
            return None;
        };
        match rde_dag::rollback(&self.project, store, txid.as_deref()) {
            Ok(Some(outcome)) => {
                let message = format!(
                    "Откат выполнен: восстановлено {}, сброшено {} (транзакция {})",
                    outcome.restored.len(),
                    outcome.reset.len(),
                    outcome.rolled_back_tx.chars().take(10).collect::<String>()
                );
                // reload_project пересоздаёт проект и его журнал — пишем строку после перезагрузки.
                self.reload_project();
                self.push_log(Level::Ok, message);
                Some(outcome)
            }
            Ok(None) => {
                self.push_log(Level::Info, "Нечего откатывать: нет транзакций обновлений со снапшотом");
                None
            }
            Err(error) => {
                self.push_log(Level::Warn, format!("Откат не выполнен: {error}"));
                None
            }
        }
    }

    /// Для демо/скриншотов: переключить нижнюю панель (0 — Журнал, 1 — Задачи, 2 — Агент).
    pub fn demo_set_logs_tab(&mut self, tab: usize) {
        self.logs_tab = tab.min(2);
    }

    /// Кандидаты на откат для панели «Задачи».
    pub fn rollback_candidates(&self) -> Vec<rde_store::RollbackCandidate> {
        self.store
            .as_ref()
            .and_then(|store| store.rollback_candidates().ok())
            .unwrap_or_default()
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
        self.poll_update_job();
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
        // Горячие клавиши (Ctrl+F — поиск, Ctrl+S — сохранить проект, Ctrl+Z/Y — отмена/повтор в редакторе).
        let (find, save, undo, redo) = ui.input(|input| {
            (
                input.modifiers.command && input.key_pressed(egui::Key::F),
                input.modifiers.command && input.key_pressed(egui::Key::S),
                input.modifiers.command && input.key_pressed(egui::Key::Z),
                input.modifiers.command && input.key_pressed(egui::Key::Y),
            )
        });
        if find {
            self.show_tree = true;
            self.search_focus = true;
        }
        if save {
            self.save_project_document();
        }
        if undo && self.editor.is_some() {
            self.undo_document(false);
        }
        if redo && self.editor.is_some() {
            self.undo_document(true);
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
        self.dialogs_extra(ui.ctx());
    }

    fn push_log(&mut self, level: Level, text: impl Into<String>) {
        self.project.log.push(rde_core::LogLine { time: "сейчас", level, text: text.into() });
    }

    // ── верхнее меню ────────────────────────────────────────────────────────
    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let project_name = self.project.name.clone();
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum MenuAction {
            NewProject,
            OpenProject,
            ExportDocx,
            ExportProject,
            Undo,
            Redo,
            Find,
            CheckProject,
            UpdateAll,
            Settings,
            About,
            Audit,
        }
        let mut action: Option<MenuAction> = None;
        egui::Panel::top("rde_menu")
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 2)))
            .show(ui, |ui| {
                egui::MenuBar::new().ui(ui, |ui| {
                    ui.menu_button("Файл", |ui| {
                        if ui.button("Новый проект…").clicked() {
                            action = Some(MenuAction::NewProject);
                            ui.close();
                        }
                        if ui.button("Открыть проект…").clicked() {
                            action = Some(MenuAction::OpenProject);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Экспорт .docx").clicked() {
                            action = Some(MenuAction::ExportDocx);
                            ui.close();
                        }
                        if ui.button("Экспорт проекта (.rde.zip)").clicked() {
                            action = Some(MenuAction::ExportProject);
                            ui.close();
                        }
                    });
                    ui.menu_button("Правка", |ui| {
                        let can_undo = self.editor.as_ref().is_some_and(|editor| editor.app.session.can_undo());
                        let can_redo = self.editor.as_ref().is_some_and(|editor| editor.app.session.can_redo());
                        if ui.add_enabled(can_undo, egui::Button::new("Отменить")).clicked() {
                            action = Some(MenuAction::Undo);
                            ui.close();
                        }
                        if ui.add_enabled(can_redo, egui::Button::new("Повторить")).clicked() {
                            action = Some(MenuAction::Redo);
                            ui.close();
                        }
                        ui.separator();
                        if ui.button("Найти в проекте…").clicked() {
                            action = Some(MenuAction::Find);
                            ui.close();
                        }
                    });
                    ui.menu_button("Проект", |ui| {
                        if ui.button("Проверить и починить…").clicked() {
                            action = Some(MenuAction::CheckProject);
                            ui.close();
                        }
                        if ui.button("Обновить все артефакты").clicked() {
                            action = Some(MenuAction::UpdateAll);
                            ui.close();
                        }
                        if ui.button("Настройки проекта…").clicked() {
                            action = Some(MenuAction::Settings);
                            ui.close();
                        }
                    });
                    ui.menu_button("Вид", |ui| {
                        ui.checkbox(&mut self.show_tree, "Дерево проекта");
                        ui.checkbox(&mut self.show_agent, "Панель агента");
                        ui.checkbox(&mut self.show_logs, "Журнал и задачи");
                        ui.checkbox(&mut self.show_inspector, "Инспектор объекта");
                    });
                    ui.menu_button("Агент", |ui| {
                        ui.label(RichText::new("MCP/ACP — v0.3 (не подключён)").small().color(MUTED));
                        if ui.button("Аудит действий…").clicked() {
                            action = Some(MenuAction::Audit);
                            ui.close();
                        }
                    });
                    ui.menu_button("Помощь", |ui| {
                        if ui.button("О прототипе").clicked() {
                            action = Some(MenuAction::About);
                            ui.close();
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("RDE · прототип v0.1").small().color(MUTED));
                        ui.add_space(6.0);
                        ui.label(RichText::new(&project_name).small().color(TEXT));
                    });
                });
            });
        match action {
            Some(MenuAction::NewProject) => {
                let base = self.project.root.parent().map(|parent| parent.to_path_buf()).unwrap_or_else(|| self.project.root.clone());
                self.new_project_dialog = Some((base.join("rde-project").display().to_string(), "Новый проект RDE".to_owned()));
            }
            Some(MenuAction::OpenProject) => self.open_project_dialog = Some(self.project.root.display().to_string()),
            Some(MenuAction::ExportDocx) => {
                self.ensure_editor();
                self.save_editor_copy();
            }
            Some(MenuAction::ExportProject) => self.export_project_archive(),
            Some(MenuAction::Undo) => self.undo_document(false),
            Some(MenuAction::Redo) => self.undo_document(true),
            Some(MenuAction::Find) => {
                self.show_tree = true;
                self.search_focus = true;
            }
            Some(MenuAction::CheckProject) => self.run_project_check(),
            Some(MenuAction::UpdateAll) => {
                self.request_update(None, true);
            }
            Some(MenuAction::Settings) => {
                self.settings_dialog = Some((
                    self.project.manifest.name.clone(),
                    self.project.manifest.default_template.clone(),
                    self.project.manifest.default_rebuild.clone(),
                ));
            }
            Some(MenuAction::About) => self.about_open = true,
            Some(MenuAction::Audit) => self.audit_dialog = Some(self.audit_lines()),
            None => {}
        }
    }

    // ── панель действий ─────────────────────────────────────────────────────
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let mut update_all = false;
        let mut export = false;
        let mut save_project = false;
        let mut update_document = false;
        let mut new_object = false;
        let mut search_clicked = false;
        let mut cancel_update = false;
        let running = self.update_job.is_some();
        let running_secs = self.update_job.as_ref().map(|job| job.started.elapsed().as_secs_f32()).unwrap_or(0.0);
        egui::Panel::top("rde_toolbar")
            .frame(egui::Frame::new().fill(BG).inner_margin(Margin::symmetric(10, 4)))
            .exact_size(36.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !running,
                            egui::Button::new(RichText::new("Обновить всё").color(Color32::WHITE)).fill(ACCENT),
                        )
                        .on_hover_text("Реально пересчитать устаревшие артефакты (rde-dag: act_key/out_hash, транзакции)")
                        .clicked()
                    {
                        update_all = true;
                    }
                    if running {
                        ui.label(RichText::new(format!("● обновление… {running_secs:.0} с")).small().color(AMBER));
                        if ui
                            .add(egui::Button::new(RichText::new("Отменить").color(TEXT)).fill(PANEL_HI))
                            .on_hover_text("Остановить текущий запуск (SIGTERM → grace → SIGKILL) и отменить обновление")
                            .clicked()
                        {
                            cancel_update = true;
                        }
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
                    if ui.button(RichText::new("Поиск").size(13.0)).on_hover_text("Поиск по объектам (Ctrl+F)").clicked() {
                        search_clicked = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new(format!("• {} устаревших объектов", self.project.stale_count())).small().color(AMBER));
                        ui.separator();
                        ui.label(RichText::new("MCP: off").small().color(MUTED));
                    });
                });
            });
        if cancel_update {
            self.cancel_update();
        }
        if update_all {
            self.request_update(None, true);
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
            self.create_dialog = Some(CreateDialogState { kind: UiObjectKind::Core(NewObjectKind::ComputeArtifact), title: String::new() });
        }
        if search_clicked {
            self.show_tree = true;
            self.search_focus = true;
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
                    let branch = if self.project.branch.trim().is_empty() { "—".to_owned() } else { self.project.branch.clone() };
                    ui.label(RichText::new(format!("git: {branch}")).small().color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("A4 · ru-RU").small().color(MUTED));
                        ui.separator();
                        ui.label(RichText::new(format!("{} объектов", self.project.object_count())).small().color(MUTED));
                        ui.separator();
                        ui.label(RichText::new(format!("• {} устаревших", self.project.stale_count())).small().color(AMBER));
                        ui.separator();
                        ui.label(RichText::new("Агент: не подключён (v0.3)").small().color(MUTED));
                    });
                });
            });
    }

    // ── журнал / задачи ─────────────────────────────────────────────────────
    fn logs_panel(&mut self, ui: &mut egui::Ui) {
        let mut tab = self.logs_tab;
        let mut hide_logs = false;
        let mut clear = false;
        let log = self.project.log.clone();
        let tasks = self.run_history_lines();
        let rollback_candidates = self.rollback_candidates();
        let mut rollback_clicked: Option<String> = None;
        egui::Panel::bottom("rde_logs")
            .resizable(true)
            .default_size(150.0)
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
                        if tab == 0 && ui.small_button("очистить").clicked() {
                            clear = true;
                        }
                        if tab == 1 {
                            ui.label(RichText::new(format!("{} записей", tasks.len())).small().color(MUTED));
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("rde_logs_scroll")
                    .stick_to_bottom(true)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        match tab {
                            1 => {
                                if tasks.is_empty() {
                                    ui.label(RichText::new("Пока нет запусков и транзакций — нажмите «Обновить всё»").small().color(MUTED));
                                }
                                for line in &tasks {
                                    ui.label(RichText::new(line).monospace().size(11.5).color(TEXT));
                                }
                                ui.add_space(8.0);
                                ui.label(RichText::new("ТРАНЗАКЦИИ ОБНОВЛЕНИЙ — ОТКАТ (O(1))").small().strong().color(MUTED));
                                if rollback_candidates.is_empty() {
                                    ui.label(RichText::new("Пока нет транзакций со снапшотом корней").small().color(MUTED));
                                }
                                for candidate in rollback_candidates.iter().take(8) {
                                    ui.horizontal(|ui| {
                                        let stamp = candidate
                                            .updated_unix
                                            .map(|unix| format!("unix {unix}"))
                                            .unwrap_or_else(|| "—".to_owned());
                                        ui.label(RichText::new(format!("{} · {stamp}", candidate.note)).size(11.5).color(TEXT));
                                        if candidate.rolled_back {
                                            ui.label(RichText::new("откачено").small().color(MUTED));
                                        } else if ui.small_button("Откатить").on_hover_text("Переключить корни на предыдущие (O(1)) и восстановить выходы из CAS").clicked() {
                                            rollback_clicked = Some(candidate.txid.clone());
                                        }
                                    });
                                }
                            }
                            2 => {
                                ui.label(RichText::new("Агент не подключён (MCP/ACP — v0.3).").strong().color(TEXT));
                                ui.label(RichText::new("Пока доступны быстрые действия без агента — они в правой панели «Агент».").small().color(MUTED));
                                ui.add_space(4.0);
                                ui.label(RichText::new("Когда появится: запуск артефактов и обновление вставок по запросу, проверка проекта, оформление по ГОСТ — с approvals и аудитом действий.").small().color(MUTED));
                            }
                            _ => {
                                for line in &log {
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
                            }
                        }
                    });
            });
        self.logs_tab = tab;
        if hide_logs {
            self.show_logs = false;
        }
        if clear {
            self.project.log.clear();
        }
        if let Some(txid) = rollback_clicked {
            self.rollback_transaction(Some(txid));
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
        let mut action: Option<TreeAction> = None;
        let mut new_object = false;
        let mut hide_tree = false;
        let focus_filter = std::mem::take(&mut self.search_focus);
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
                let filter_response = ui.add(
                    egui::TextEdit::singleline(&mut self.tree_filter)
                        .id(egui::Id::new("rde_tree_filter"))
                        .hint_text("Поиск объектов… (Ctrl+F)")
                        .desired_width(f32::INFINITY),
                );
                if focus_filter {
                    filter_response.request_focus();
                }
                if !self.tree_filter.is_empty() && ui.small_button("сбросить фильтр").clicked() {
                    self.tree_filter.clear();
                }
                ui.add_space(2.0);
                if ui.add(egui::Button::new(RichText::new("+ Новый объект").size(12.0)).fill(PANEL_HI)).clicked() {
                    new_object = true;
                }
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("rde_tree_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    for group in &groups {
                        let visible: Vec<&rde_core::Object> = group
                            .objects
                            .iter()
                            .filter_map(|&index| objects.get(index))
                            .filter(|object| filter.is_empty() || object.title.to_lowercase().contains(&filter))
                            .collect();
                        if visible.is_empty() {
                            continue;
                        }
                        let header = RichText::new(format!("{}  {}  · {}", group.kind.icon(), group.title, visible.len())).strong().size(12.5).color(TEXT);
                        egui::CollapsingHeader::new(header).default_open(true).show(ui, |ui| {
                            for obj in visible {
                                let is_sel = selected.as_deref() == Some(obj.id.as_str());
                                let resp = ui
                                    .horizontal(|ui| {
                                        ui.label(RichText::new("●").color(freshness_color(obj.freshness)).size(9.0));
                                        ui.label(RichText::new(obj.kind.icon()).color(MUTED).size(12.0));
                                        ui.selectable_label(is_sel, RichText::new(&obj.title).size(13.0))
                                    })
                                    .inner;
                                // M6: контекстное меню объекта (правая кнопка мыши).
                                resp.context_menu(|ui| {
                                    if ui.button("Открыть").clicked() {
                                        action = Some(TreeAction::Open(obj.id.clone()));
                                        ui.close();
                                    }
                                    if ui.button("Показать в инспекторе").clicked() {
                                        action = Some(TreeAction::Inspect(obj.id.clone()));
                                        ui.close();
                                    }
                                    ui.separator();
                                    match &obj.data {
                                        ObjectData::Artifact(artifact) if artifact.kind == "compute" => {
                                            if ui.button("Обновить артефакт").clicked() {
                                                action = Some(TreeAction::Update(obj.id.clone()));
                                                ui.close();
                                            }
                                            if ui.button("Обновить связанные").clicked() {
                                                action = Some(TreeAction::UpdateDependents(obj.id.clone()));
                                                ui.close();
                                            }
                                        }
                                        ObjectData::Document(_) => {
                                            if ui.button("Оформить по ГОСТ + библиография").clicked() {
                                                action = Some(TreeAction::FormatGost(obj.id.clone()));
                                                ui.close();
                                            }
                                            if ui.button("Вставить библиографию").clicked() {
                                                action = Some(TreeAction::InsertBibliography(obj.id.clone()));
                                                ui.close();
                                            }
                                            if ui.button("Копия для Word").clicked() {
                                                action = Some(TreeAction::WordCopy);
                                                ui.close();
                                            }
                                            if ui.button("Импорт ревью из exports").clicked() {
                                                action = Some(TreeAction::ImportReview);
                                                ui.close();
                                            }
                                        }
                                        ObjectData::Source(source) => {
                                            let is_pdf = source.file.as_deref().is_some_and(|file| file.to_ascii_lowercase().ends_with(".pdf"));
                                            if is_pdf && ui.button("OCR страницы 1").clicked() {
                                                action = Some(TreeAction::Ocr(obj.id.clone()));
                                                ui.close();
                                            }
                                        }
                                        _ => {}
                                    }
                                    ui.separator();
                                    if ui.button("Копировать id").clicked() {
                                        action = Some(TreeAction::CopyId(obj.id.clone()));
                                        ui.close();
                                    }
                                    if ui.button("Показать каталог").clicked() {
                                        action = Some(TreeAction::Reveal(obj.dir.display().to_string()));
                                        ui.close();
                                    }
                                    ui.separator();
                                    if ui.button("Переименовать…").clicked() {
                                        action = Some(TreeAction::Rename(obj.id.clone()));
                                        ui.close();
                                    }
                                    if ui.button("Удалить (в корзину)…").clicked() {
                                        action = Some(TreeAction::Delete(obj.id.clone()));
                                        ui.close();
                                    }
                                });
                                if resp.clicked() {
                                    clicked = Some(obj.id.clone());
                                }
                            }
                        });
                    }
                    ui.add_space(6.0);
                    if filter.is_empty() && groups.iter().all(|group| group.objects.is_empty()) {
                        ui.label(RichText::new("В проекте пока нет объектов").small().color(MUTED));
                    }
                    // Пустое место дерева: создание объекта.
                    let empty = ui.allocate_response(vec2(ui.available_width(), 18.0), Sense::click());
                    empty.context_menu(|ui| {
                        if ui.button("Создать объект…").clicked() {
                            new_object = true;
                            ui.close();
                        }
                    });
                });
            });
        if hide_tree {
            self.show_tree = false;
        }
        if new_object {
            self.create_dialog = Some(CreateDialogState { kind: UiObjectKind::Core(NewObjectKind::ComputeArtifact), title: String::new() });
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
        if let Some(action) = action {
            self.apply_tree_action(action);
        }
    }

    /// Выполнить действие из контекстного меню дерева.
    fn apply_tree_action(&mut self, action: TreeAction) {
        match action {
            TreeAction::Open(id) | TreeAction::Inspect(id) => {
                if let Some(object) = self.project.object(&id).cloned() {
                    self.selected = Some(id);
                    self.show_inspector = true;
                    self.tab = match object.kind {
                        ObjectKind::Document => Tab::Document,
                        ObjectKind::Artifact | ObjectKind::Constants => Tab::Artifacts,
                        ObjectKind::Source | ObjectKind::Note => Tab::Source,
                        ObjectKind::Repository => Tab::Artifacts,
                    };
                }
            }
            TreeAction::Rename(id) => {
                if let Some(object) = self.project.object(&id) {
                    self.rename_dialog = Some(RenameDialogState { id: object.id.clone(), title: object.title.clone() });
                }
            }
            TreeAction::Delete(id) => self.delete_confirm = Some(id),
            TreeAction::CopyId(id) => self.push_log(Level::Info, format!("id объекта: {id}")),
            TreeAction::Update(id) => {
                self.request_update(Some(vec![id]), false);
            }
            TreeAction::UpdateDependents(id) => {
                self.request_update(Some(vec![id]), true);
            }
            TreeAction::FormatGost(id) => {
                self.selected = Some(id);
                self.update_document_package(true, true);
            }
            TreeAction::InsertBibliography(id) => {
                self.selected = Some(id);
                self.update_document_package(false, true);
            }
            TreeAction::WordCopy => self.export_word_copy(),
            TreeAction::ImportReview => self.import_review_from_exports(),
            TreeAction::Ocr(id) => self.ocr_source(&id),
            TreeAction::Reveal(path) => {
                let path = PathBuf::from(path);
                #[cfg(unix)]
                if std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
                    let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                }
                self.push_log(Level::Info, format!("Каталог: {}", path.display()));
            }
        }
    }

    // ── панель агента ───────────────────────────────────────────────────────
    fn agent_panel(&mut self, ui: &mut egui::Ui) {
        let mut hide_agent = false;
        let mut check = false;
        let mut update = false;
        let mut gost = false;
        let mut review = false;
        egui::Panel::right("rde_agent")
            .resizable(true)
            .default_size(316.0)
            .min_size(260.0)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(10, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("АГЕНТ").small().strong().color(MUTED));
                    ui.label(RichText::new("• не подключён").small().color(AMBER));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Скрыть панель (вернуть — меню «Вид»)").clicked() {
                            hide_agent = true;
                        }
                        ui.label(RichText::new("v0.3 · MCP/ACP").small().color(MUTED));
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().id_salt("rde_agent_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Встроенный агент появится в v0.3").strong().color(TEXT));
                    ui.label(
                        RichText::new(
                            "Планируется: MCP-сервер (инструменты, ресурсы, approvals) и ACP-агент, \
                             который сможет запускать артефакты, обновлять вставки документа, проверять проект \
                             и оформлять по ГОСТ — с аудитом и откатом.",
                        )
                        .small()
                        .color(MUTED),
                    );
                    ui.add_space(10.0);
                    ui.label(RichText::new("БЫСТРЫЕ ДЕЙСТВИЯ (без агента)").small().strong().color(MUTED));
                    ui.add_space(4.0);
                    if ui.add(egui::Button::new("Проверить проект").fill(PANEL_HI)).clicked() {
                        check = true;
                    }
                    if ui.add(egui::Button::new("Обновить устаревшие артефакты").fill(PANEL_HI)).clicked() {
                        update = true;
                    }
                    if ui.add(egui::Button::new("Оформить документ по ГОСТ").fill(PANEL_HI)).clicked() {
                        gost = true;
                    }
                    if ui.add(egui::Button::new("Импорт ревью из exports").fill(PANEL_HI)).clicked() {
                        review = true;
                    }
                    ui.add_space(10.0);
                    ui.label(RichText::new("Журнал и история запусков — в панели снизу («Задачи»).").small().color(MUTED));
                });
                ui.add_space(4.0);
                ui.add_enabled(
                    false,
                    egui::TextEdit::singleline(&mut String::new()).hint_text("Чат с агентом — появится в v0.3").desired_width(f32::INFINITY),
                );
            });
        if hide_agent {
            self.show_agent = false;
        }
        if check {
            self.run_project_check();
        }
        if update {
            self.request_update(None, true);
        }
        if gost {
            self.update_document_package(true, true);
        }
        if review {
            self.import_review_from_exports();
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
                                field_row(ui, "окружение", &format!("режим {}", runtime.env), false);
                                if let Some(status) = self.dag_statuses.get(&object.id) {
                                    if let Some(digest) = &status.env_lock_digest {
                                        let recorded = rde_env::recorded_digest(&self.project.root, &runtime.kind);
                                        let drift = match &recorded {
                                            Some(recorded) if recorded != digest => " · изменилось",
                                            Some(_) => " · совпадает с rde.lock",
                                            None => " · rde.lock ещё нет",
                                        };
                                        field_row(ui, "env digest", &format!("{}{drift}", digest.chars().take(16).collect::<String>()), true);
                                    }
                                }
                            }
                            if !artifact.outputs.is_empty() {
                                field_row(ui, "выходы", &artifact.outputs.join(", "), false);
                            }
                            let network_allowed = artifact.network_allow.unwrap_or(self.project.manifest.network_allow);
                            field_row(
                                ui,
                                "сеть",
                                if network_allowed { "разрешена (network_allow)" } else { "запрещена (best-effort)" },
                                false,
                            );
                            field_row(
                                ui,
                                "запуск",
                                if self.project.manifest.trusted { "разрешён (trusted)" } else { "требует согласия (trusted = false)" },
                                false,
                            );
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
                                    let provenance = self.project.root.join(".rde/runs").join(run_id).join("provenance.json");
                                    if provenance.is_file() {
                                        ui.horizontal(|ui| {
                                            if ui.small_button("копировать путь provenance").clicked() {
                                                ui.ctx().copy_text(provenance.display().to_string());
                                            }
                                        });
                                    }
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
                // «Обновить артефакт» — только этот артефакт, принудительно.
                self.request_update(Some(vec![object.id.clone()]), false);
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

    fn create_object_action(&mut self, kind: UiObjectKind, title: &str) -> Result<(), String> {
        if kind == UiObjectKind::Document {
            return self.create_document_action(title);
        }
        let UiObjectKind::Core(kind) = kind else { unreachable!() };
        let dir = rde_core::create_object(&self.project.root, &CreateRequest { kind, title: title.to_owned() })?;
        self.reload_project();
        self.selected = self.project.objects.iter().find(|object| object.dir == dir).map(|object| object.id.clone());
        self.push_log(Level::Ok, format!("Создан объект «{}» ({})", title.trim(), kind.label_ru()));
        Ok(())
    }

    /// Создать новый документ: `document.toml` + пустой OOXML → `.doc.rde` (WordCraft + rde:doc).
    fn create_document_action(&mut self, title: &str) -> Result<(), String> {
        let dir = rde_core::create_document_stub(&self.project.root, title)?;
        let temp = std::env::temp_dir().join(format!("rde-new-doc-{}.docx", std::process::id()));
        let doc = wordcraft_engine::doc::Document::default();
        if let Err(error) = wordcraft_engine::io::save_path(&temp, &doc) {
            let _ = fs::remove_dir_all(&dir);
            return Err(format!("сохранить пустой .docx: {error}"));
        }
        let project = rde_core::load_project(&self.project.root)?;
        let Some(object) = project.objects.iter().find(|object| object.dir == dir).cloned() else {
            let _ = fs::remove_file(&temp);
            return Err("новый документ не загрузился".to_owned());
        };
        let ObjectData::Document(manifest) = &object.data else {
            let _ = fs::remove_file(&temp);
            return Err("новый объект не является документом".to_owned());
        };
        let package = dir.join("report.doc.rde");
        let result = rde_doc::build_rde_doc(&temp, &manifest.id, &manifest.template)
            .and_then(|rde_doc| rde_doc::write_package(&temp, &rde_doc, &package).map(|_| ()));
        let _ = fs::remove_file(&temp);
        if let Err(error) = result {
            let _ = fs::remove_dir_all(&dir);
            return Err(error);
        }
        self.reload_project();
        self.selected = self.project.objects.iter().find(|object| object.dir == dir).map(|object| object.id.clone());
        self.tab = Tab::Document;
        self.push_log(Level::Ok, format!("Создан документ «{}» (.doc.rde)", title.trim()));
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

    /// Проверка внешних правок документа при открытии проекта (M3/M5).
    fn check_external_edit_on_startup(&mut self) {
        if let Some(path) = self.project.document_path() {
            if rde_doc::is_rde_package(&path) {
                match rde_doc::check_external_edit(&path) {
                    Ok(check) if check.verdict != rde_doc::EditVerdict::NotModified => self.push_log(
                        Level::Warn,
                        match check.verdict {
                            rde_doc::EditVerdict::MissingRdeDoc => "Документ пересохранён вне RDE: RDE-часть (customXml) потеряна — нужен импорт/сверка".to_owned(),
                            _ => "Документ изменён вне RDE: body_hash не совпадает — проверьте правки".to_owned(),
                        },
                    ),
                    Ok(_) => self.push_log(Level::Info, "Документ .doc.rde: RDE-часть на месте, внешних правок нет"),
                    Err(error) => self.push_log(Level::Warn, format!("Проверка .doc.rde: {error}")),
                }
            }
        }
    }

    /// Открыть проект по пути: перезагрузить модель, хранилище, документ и статусы.
    pub fn open_project(&mut self, root: &Path) {
        match rde_core::load_project(root) {
            Ok(project) => {
                self.project = project;
                self.selected = self
                    .project
                    .document_object()
                    .map(|object| object.id.clone())
                    .or_else(|| self.project.objects.first().map(|object| object.id.clone()));
                self.doc = DocPreview::new(self.project.document_path());
                self.editor = None;
                self.editor_error = None;
                self.structure_cache = None;
                self.pdf_cache = None;
                self.comments_cache = None;
                self.inserts_cache = None;
                self.tab = Tab::Document;
                self.store = Store::open(&self.project.root).ok();
                self.refresh_dag_statuses();
                self.push_log(Level::Ok, format!("Проект открыт: {} ({} объектов)", self.project.name, self.project.object_count()));
                self.check_external_edit_on_startup();
            }
            Err(error) => self.push_log(Level::Warn, format!("Не удалось открыть проект {}: {error}", root.display())),
        }
    }

    /// Создать новый проект и открыть его.
    pub fn create_project_ui(&mut self, root: &Path, name: &str) -> Result<(), String> {
        rde_core::create_project(root, name)?;
        self.open_project(root);
        self.push_log(Level::Ok, format!("Создан проект «{}»: {}", name.trim(), root.display()));
        Ok(())
    }

    /// Сохранить настройки проекта (`rde.toml`) и перезагрузить модель.
    pub fn save_project_settings(&mut self, name: &str, template: &str, rebuild: &str) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("название проекта не может быть пустым".to_owned());
        }
        let path = self.project.root.join("rde.toml");
        set_manifest_string(&path, "name", name.trim());
        set_manifest_string(&path, "default_template", template.trim());
        set_manifest_string(&path, "default_rebuild", rebuild.trim());
        self.reload_project();
        self.push_log(Level::Ok, "Настройки проекта сохранены");
        Ok(())
    }

    /// «Проверить и починить»: целостность CAS/индекса, статусы артефактов, документ, issues.
    pub fn run_project_check(&mut self) {
        let mut lines: Vec<String> = Vec::new();
        if let Some(store) = self.store.as_mut() {
            match store.rebuild_index() {
                Ok(report) => {
                    lines.push(format!(
                        "Хранилище: {} блобов, {} деревьев, {} транзакций; повреждено {}",
                        report.blobs,
                        report.trees,
                        report.txs,
                        report.mismatched.len()
                    ));
                    if !report.mismatched.is_empty() {
                        lines.push(format!("Повреждённые части: {:?}", report.mismatched));
                    }
                }
                Err(error) => lines.push(format!("Индекс: ошибка — {error}")),
            }
            match rde_dag::evaluate(&self.project, store) {
                Ok(statuses) => {
                    let stale: Vec<&rde_dag::ArtifactStatus> = statuses
                        .values()
                        .filter(|status| !matches!(status.status, rde_dag::LiveStatus::Fresh))
                        .collect();
                    lines.push(format!("Артефакты: {} (неактуальных {})", statuses.len(), stale.len()));
                    for status in stale {
                        lines.push(format!(
                            "  {}: {}{}",
                            status.title,
                            status.status.label_ru(),
                            status.status.reason().map(|reason| format!(" — {reason}")).unwrap_or_default()
                        ));
                    }
                }
                Err(error) => lines.push(format!("Статусы: ошибка — {error}")),
            }
            match store.incomplete_txs() {
                Ok(ids) if !ids.is_empty() => lines.push(format!("Незавершённые транзакции: {}", ids.len())),
                Ok(_) => lines.push("Незавершённых транзакций нет".to_owned()),
                Err(error) => lines.push(format!("Журнал: ошибка — {error}")),
            }
        } else {
            lines.push("Хранилище проекта не открыто".to_owned());
        }
        if let Some(path) = self.project.document_path() {
            if rde_doc::is_rde_package(&path) {
                match rde_doc::check_external_edit(&path) {
                    Ok(check) => {
                        lines.push(format!(
                            "Документ .doc.rde: {}",
                            match check.verdict {
                                rde_doc::EditVerdict::NotModified => "внешних правок нет",
                                rde_doc::EditVerdict::ExternalEdit => "изменён вне RDE (body_hash не совпал)",
                                rde_doc::EditVerdict::MissingRdeDoc => "RDE-часть потеряна (пересохранён вне RDE)",
                            }
                        ));
                    }
                    Err(error) => lines.push(format!("Документ: ошибка — {error}")),
                }
                if let Ok(comments) = rde_doc::comments(&path) {
                    lines.push(format!("Комментариев Word: {}", comments.len()));
                }
            }
        }
        lines.push(format!("Проблемы загрузки проекта: {}", self.project.issues.len()));
        for issue in &self.project.issues {
            lines.push(format!("  {}: {}", issue.path.display(), issue.message));
        }
        self.check_dialog = Some(lines.join("\n"));
        self.push_log(Level::Ok, "Проверка проекта завершена — отчёт в диалоге");
    }

    /// Экспорт проекта в zip (`<имя>.rde.zip`): rde.toml + objects/** без сгенерированных данных.
    pub fn export_project_archive(&mut self) {
        let name = self.project.name.chars().map(|ch| if ch.is_alphanumeric() || ch == '-' || ch == '_' { ch } else { '-' }).collect::<String>();
        let out = self.export_dir.join(format!("{name}.rde.zip"));
        match write_project_zip(&self.project.root, &out) {
            Ok(files) => self.push_log(Level::Ok, format!("Экспорт проекта: {} ({files} файлов)", out.display())),
            Err(error) => self.push_log(Level::Warn, format!("Экспорт проекта: {error}")),
        }
    }

    /// Отмена/повтор в документе (история WordCraft-сессии).
    pub fn undo_document(&mut self, redo: bool) {
        self.ensure_editor();
        let Some(editor) = self.editor.as_mut() else {
            self.push_log(Level::Warn, "Отмена: редактор не открыт");
            return;
        };
        let label = if redo {
            editor.app.session.redo_label().map(|label| label.to_owned())
        } else {
            editor.app.session.undo_label().map(|label| label.to_owned())
        };
        let done = if redo { editor.app.session.redo() } else { editor.app.session.undo() };
        if done {
            self.push_log(Level::Info, format!("{}: {}", if redo { "Повтор" } else { "Отмена" }, label.unwrap_or_default()));
        } else {
            self.push_log(Level::Info, if redo { "Повторять нечего" } else { "Отменять нечего" });
        }
    }

    /// Аудит действий: журнал транзакций хранилища.
    fn audit_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        match &self.store {
            Some(store) => match store.list_txs() {
                Ok(txs) => {
                    lines.push(format!("Транзакции хранилища: {}", txs.len()));
                    for tx in txs.iter().take(60) {
                        lines.push(format!("{}  {:?}  {}", tx.started_unix, tx.status, tx.note));
                    }
                    if txs.is_empty() {
                        lines.push("Транзакций пока нет".to_owned());
                    }
                }
                Err(error) => lines.push(format!("Журнал: ошибка — {error}")),
            },
            None => lines.push("Хранилище проекта не открыто".to_owned()),
        }
        lines
    }

    /// История запусков/транзакций для вкладки «Задачи».
    fn run_history_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if let Some(store) = &self.store {
            if let Ok(txs) = store.list_txs() {
                for tx in txs.iter().rev().take(30) {
                    lines.push(format!("[{}] {:?} · {}", tx.id.chars().take(8).collect::<String>(), tx.status, tx.note));
                }
            }
        }
        let runs = self.project.root.join(".rde/runs");
        if let Ok(entries) = fs::read_dir(&runs) {
            let mut dirs: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()).collect();
            dirs.sort();
            dirs.reverse();
            for dir in dirs.into_iter().take(20) {
                if let Ok(text) = fs::read_to_string(dir.join("run.json")) {
                    if let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) {
                        lines.push(format!(
                            "run {} · {} · {} мс · выходов {}",
                            dir.file_name().map(|name| name.to_string_lossy().to_string()).unwrap_or_default(),
                            value.get("status").and_then(|value| value.as_str()).unwrap_or("?"),
                            value.get("duration_ms").and_then(|value| value.as_u64()).unwrap_or(0),
                            value.get("outputs").and_then(|value| value.as_array()).map(|items| items.len()).unwrap_or(0)
                        ));
                    }
                }
            }
        }
        if lines.is_empty() {
            lines.push("Пока нет запусков и транзакций — нажмите «Обновить всё»".to_owned());
        }
        lines
    }

    /// OCR источника (PDF-страница или изображение) с записью `ocr.txt` рядом с источником.
    pub fn ocr_source(&mut self, id: &str) {
        let Some(object) = self.project.object(id).cloned() else { return };
        let ObjectData::Source(source) = &object.data else {
            self.push_log(Level::Warn, "OCR: выбранный объект — не источник");
            return;
        };
        let Some(file) = source.file.clone() else {
            self.push_log(Level::Warn, "OCR: у источника нет файла (URL)");
            return;
        };
        let path = object.dir.join(&file);
        let lower = file.to_ascii_lowercase();
        let result = if lower.ends_with(".pdf") {
            rde_sources::ocr_pdf_page(&path, 1)
        } else {
            rde_sources::ocr_image(&path)
        };
        match result {
            Ok(text) => {
                let _ = fs::write(object.dir.join("ocr.txt"), &text);
                self.push_log(Level::Ok, format!("OCR «{}»: {} символов (ocr.txt)", object.title, text.chars().count()));
                self.push_log(Level::Info, text.chars().take(300).collect::<String>());
            }
            Err(error) => self.push_log(Level::Warn, format!("OCR: {error}")),
        }
    }

    /// Дополнительные диалоги: проекты, настройки, проверка, аудит, «О прототипе».
    fn dialogs_extra(&mut self, ctx: &egui::Context) {
        if let Some(mut path) = self.open_project_dialog.take() {
            let mut open = false;
            let mut cancel = false;
            egui::Window::new("Открыть проект")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(520.0);
                    ui.add(egui::TextEdit::singleline(&mut path).hint_text("Путь к каталогу с rde.toml").desired_width(f32::INFINITY));
                    ui.label(RichText::new("Нужен каталог, в котором есть rde.toml").small().color(MUTED));
                    ui.horizontal(|ui| {
                        if ui.button("Открыть").clicked() {
                            open = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                });
            if open {
                let root = PathBuf::from(path.trim());
                if root.join("rde.toml").is_file() {
                    self.open_project(&root);
                } else {
                    self.push_log(Level::Warn, format!("Открыть проект: в {} нет rde.toml", root.display()));
                }
            }
            if !open && !cancel {
                self.open_project_dialog = Some(path);
            }
        }

        if let Some((mut path, mut name)) = self.new_project_dialog.take() {
            let mut create = false;
            let mut cancel = false;
            egui::Window::new("Новый проект")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(520.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Каталог").color(MUTED));
                        ui.add(egui::TextEdit::singleline(&mut path).desired_width(f32::INFINITY));
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Название").color(MUTED));
                        ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY));
                    });
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
                let root = PathBuf::from(path.trim());
                if let Err(error) = self.create_project_ui(&root, &name) {
                    self.push_log(Level::Warn, format!("Новый проект: {error}"));
                }
            }
            if !create && !cancel {
                self.new_project_dialog = Some((path, name));
            }
        }

        if let Some((mut name, mut template, mut rebuild)) = self.settings_dialog.take() {
            let mut save = false;
            let mut cancel = false;
            egui::Window::new("Настройки проекта")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(460.0);
                    egui::Grid::new("rde_settings").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        ui.label(RichText::new("Название").color(MUTED));
                        ui.add(egui::TextEdit::singleline(&mut name).desired_width(300.0));
                        ui.end_row();
                        ui.label(RichText::new("Шаблон по умолчанию").color(MUTED));
                        egui::ComboBox::from_id_salt("rde_settings_template")
                            .selected_text(if template.is_empty() { "—" } else { &template })
                            .show_ui(ui, |ui| {
                                for builtin in rde_templates::builtin_templates() {
                                    ui.selectable_value(&mut template, builtin.id.clone(), builtin.title.clone());
                                }
                            });
                        ui.end_row();
                        ui.label(RichText::new("Пересборка").color(MUTED));
                        egui::ComboBox::from_id_salt("rde_settings_rebuild")
                            .selected_text(if rebuild.is_empty() { "—" } else { &rebuild })
                            .show_ui(ui, |ui| {
                                for value in ["manual", "auto"] {
                                    ui.selectable_value(&mut rebuild, value.to_owned(), value);
                                }
                            });
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Сохранить").clicked() {
                            save = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                });
            if save {
                if let Err(error) = self.save_project_settings(&name, &template, &rebuild) {
                    self.push_log(Level::Warn, format!("Настройки: {error}"));
                }
            }
            if !save && !cancel {
                self.settings_dialog = Some((name, template, rebuild));
            }
        }

        if let Some(text) = self.check_dialog.clone() {
            let mut close = false;
            egui::Window::new("Проверка проекта")
                .collapsible(false)
                .resizable(true)
                .default_width(620.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -20.0))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        ui.label(RichText::new(text).monospace().size(11.5).color(TEXT));
                    });
                    if ui.button("Закрыть").clicked() {
                        close = true;
                    }
                });
            if close {
                self.check_dialog = None;
            }
        }

        if let Some(lines) = self.audit_dialog.clone() {
            let mut close = false;
            egui::Window::new("Аудит действий")
                .collapsible(false)
                .resizable(true)
                .default_width(620.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
                        for line in &lines {
                            ui.label(RichText::new(line).monospace().size(11.5).color(TEXT));
                        }
                    });
                    if ui.button("Закрыть").clicked() {
                        close = true;
                    }
                });
            if close {
                self.audit_dialog = None;
            }
        }

        if self.about_open {
            let mut close = false;
            egui::Window::new("О прототипе RDE")
                .collapsible(false)
                .resizable(false)
                .default_width(520.0)
                .show(ctx, |ui| {
                    ui.label(RichText::new("RDE — Research Development Environment").strong().size(15.0));
                    ui.label(RichText::new(format!("прототип v0.1 · сборка {}", env!("CARGO_PKG_VERSION"))).small().color(MUTED));
                    ui.add_space(6.0);
                    ui.label(RichText::new("Документ: .doc.rde (OOXML + rde:doc) · WordCraft DocView · ГОСТ 7.32-2017 · артефакты с act_key/out_hash · PDF/OCR · ревью Word.").small().color(TEXT));
                    ui.add_space(8.0);
                    ui.label(RichText::new("Горячие клавиши").strong().small().color(MUTED));
                    for (keys, action) in [
                        ("Ctrl+F", "поиск по объектам"),
                        ("Ctrl+S", "сохранить проект (.doc.rde)"),
                        ("Ctrl+Z / Ctrl+Y", "отмена/повтор в редакторе"),
                        ("ПКМ по объекту", "контекстное меню действий"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(keys).monospace().size(11.5).color(ACCENT));
                            ui.label(RichText::new(action).small().color(TEXT));
                        });
                    }
                    ui.add_space(6.0);
                    if ui.button("Закрыть").clicked() {
                        close = true;
                    }
                });
            if close {
                self.about_open = false;
            }
        }

        // R-RUN-06: согласие на запуск скриптов проекта (первый запуск / trusted = false).
        if let Some(state) = self.trust_dialog.take() {
            let mut confirm = false;
            let mut cancel = false;
            let project_name = self.project.manifest.name.clone();
            let root = self.project.root.display().to_string();
            egui::Window::new("Запуск скриптов проекта")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(560.0);
                    ui.label(
                        RichText::new("Проект содержит скрипты, которые будут выполнены на вашей машине.")
                            .strong()
                            .color(TEXT),
                    );
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("Проект: «{project_name}»")).small().color(TEXT));
                    ui.label(RichText::new(format!("Каталог: {root}")).small().color(MUTED));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(
                            "RDE запускает код артефактов от вашего имени (без ОС-песочницы в v0.1): \
                             сеть по умолчанию запрещена best-effort, входы доступны только для чтения, \
                             а запуск и его provenance фиксируются в журнале. Доверяйте только своим проектам.",
                        )
                        .small()
                        .color(MUTED),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(RichText::new("Доверяю проекту и запустить").color(Color32::WHITE)).fill(ACCENT))
                            .clicked()
                        {
                            confirm = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                });
            if confirm {
                match rde_core::set_project_trusted(&self.project.root, true) {
                    Ok(()) => {
                        self.reload_project();
                        self.push_log(Level::Ok, "Проект помечен доверенным (trusted = true): запуск скриптов разрешён");
                        self.start_update(state.targets, state.dependents);
                    }
                    Err(error) => self.push_log(Level::Warn, format!("Не удалось записать trusted в rde.toml: {error}")),
                }
            } else if !cancel {
                self.trust_dialog = Some(state);
            }
        }
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
                                for kind in UiObjectKind::ALL {
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
                    ui.label(RichText::new("Документ создаётся пустым .doc.rde (WordCraft + rde:doc)").small().color(MUTED));
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
                    // WordCraft рисует canvas по available_rect, игнорируя clip_rect: ограничиваем
                    // его пересечением с реальной видимой областью, иначе он заезжает на панели.
                    let rect = ui.available_rect_before_wrap().intersect(ui.clip_rect());
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
        // Число вставок из rde:doc (кэш по пути пакета).
        let package_key = self.project.document_path().map(|path| path.display().to_string()).unwrap_or_default();
        if self.inserts_cache.as_ref().map(|(key, _)| key != &package_key).unwrap_or(true) {
            let count = self
                .project
                .document_path()
                .and_then(|path| rde_doc::load_rde_doc(&path).ok())
                .map(|rde_doc| rde_doc.inserts.len())
                .unwrap_or(0);
            self.inserts_cache = Some((package_key.clone(), count));
        }
        let inserts_count = self.inserts_cache.as_ref().map(|(_, count)| *count).unwrap_or(0);
        let mut clicked_block: Option<(usize, String)> = None;
        ui.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(RichText::new("Блоки документа").strong().color(TEXT));
            if let Some(Ok((blocks, figures))) = &cache {
                let headings = blocks.iter().filter(|block| matches!(block.kind, "H1" | "H2" | "H")).count();
                ui.label(
                    RichText::new(format!("{} блоков · {headings} заголовков · {figures} рисунков · {inserts_count} вставок rde:doc", blocks.len()))
                        .small()
                        .color(MUTED),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("клик по блоку — выбрать документ").small().color(MUTED));
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
                            let text_resp = ui.add(egui::Label::new(RichText::new(&block.text).size(12.0).color(TEXT)).sense(Sense::click()));
                            if text_resp.clicked() {
                                clicked_block = Some((block.number, block.text.clone()));
                            }
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
        if let Some((number, text)) = clicked_block {
            if let Some(document) = self.project.document_object() {
                self.selected = Some(document.id.clone());
            }
            self.push_log(Level::Info, format!("Блок {number}: {text}"));
        }
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
        let mut clicked_node: Option<String> = None;
        for (i, object) in artifacts.iter().take(n).enumerate() {
            let node_response = ui.interact(nodes[i], ui.id().with(("rde_dag_node", i)), Sense::click());
            if node_response.clicked() {
                clicked_node = Some(object.id.clone());
            }
            if node_response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            node_response.context_menu(|ui| {
                if ui.button("Открыть в инспекторе").clicked() {
                    clicked_node = Some(object.id.clone());
                    ui.close();
                }
            });
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
        if let Some(id) = clicked_node {
            self.selected = Some(id.clone());
            self.show_inspector = true;
            if let Some(object) = self.project.object(&id) {
                self.push_log(Level::Info, format!("Выбран артефакт: {} ({})", object.title, object.detail));
            }
        }
        if refresh {
            let targets = self.selected.as_ref().and_then(|id| {
                self.project
                    .artifact_object(id)
                    .filter(|object| matches!(&object.data, ObjectData::Artifact(artifact) if artifact.kind == "compute"))
                    .map(|object| vec![object.id.clone()])
            });
            self.request_update(targets, false);
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
        let source_file = src.file.clone();
        let source_url = src.url.clone();
        let source_id = obj.id.clone();
        let mut open_source = false;
        let mut copy_locator = false;
        let mut show_in_tree = false;
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
                        if ui.button(RichText::new("Открыть источник").size(12.0)).clicked() {
                            open_source = true;
                        }
                        if ui.button(RichText::new("Копировать локатор").size(12.0)).clicked() {
                            copy_locator = true;
                        }
                        if ui.button(RichText::new("Показать в дереве").size(12.0)).clicked() {
                            show_in_tree = true;
                        }
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

        // M6: действия вкладки «Источник» (реальные: файл/URL, выбор в дереве, копирование).
        if open_source {
            let target = source_file.as_ref().map(|file| obj.dir.join(file)).or_else(|| source_url.clone().map(PathBuf::from));
            match target {
                Some(path) if path.exists() => {
                    #[cfg(unix)]
                    if std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
                        let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
                    }
                    self.push_log(Level::Info, format!("Источник открыт: {}", path.display()));
                }
                Some(path) => self.push_log(Level::Info, format!("Источник: {}", path.display())),
                None => self.push_log(Level::Warn, "У источника нет файла/URL"),
            }
        }
        if copy_locator {
            let locator = source_file.clone().or(source_url.clone()).unwrap_or_default();
            ui.ctx().copy_text(locator.clone());
            self.push_log(Level::Info, format!("Локатор скопирован: {locator}"));
        }
        if show_in_tree {
            self.selected = Some(source_id);
            self.show_tree = true;
            self.push_log(Level::Info, "Источник выбран в дереве проекта");
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

/// Экспорт проекта в zip: rde.toml + objects/** (без .rde/exports/out/__pycache__).
fn write_project_zip(root: &Path, out: &Path) -> Result<usize, String> {
    use std::io::Write;
    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("создать {}: {e}", parent.display()))?;
    }
    let file = fs::File::create(out).map_err(|e| format!("создать {}: {e}", out.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut count = 0usize;
    fn walk(root: &Path, dir: &Path, zip: &mut zip::ZipWriter<std::fs::File>, options: zip::write::SimpleFileOptions, count: &mut usize) -> Result<(), String> {
        let Ok(entries) = fs::read_dir(dir) else { return Ok(()) };
        let mut entries: Vec<_> = entries.flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(name.as_str(), ".rde" | "exports" | "target" | "__pycache__" | "out") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, zip, options, count)?;
            } else if let Ok(relative) = path.strip_prefix(root) {
                let relative = relative.to_string_lossy().replace('\\', "/");
                zip.start_file(relative, options).map_err(|e| e.to_string())?;
                let bytes = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                zip.write_all(&bytes).map_err(|e| e.to_string())?;
                *count += 1;
            }
        }
        Ok(())
    }
    walk(root, root, &mut zip, options, &mut count)?;
    zip.finish().map_err(|e| e.to_string())?;
    Ok(count)
}

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
        // Копия демо: тест пишет документ и не конкурирует с другими тестами за demo-project.
        let root = temp_demo_copy("docview-input");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
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
        let _ = std::fs::remove_dir_all(&root);
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

        let report = app.update_dependencies(None, true).expect("DAG-обновление");
        assert_eq!(report.failed(), 0, "{:?}", report.errors);
        assert_eq!(report.ran(), 2, "stats и plot должны выполниться: {:?}", report.artifacts);
        let stats = app.project.artifact_object(STATS).unwrap();
        assert_eq!(stats.freshness, super::Freshness::UpToDate, "после запуска актуален: {}", stats.detail);
        assert!(app.project.root.join("exports").exists() || root.join(".rde/updates").is_dir());

        let second = app.update_dependencies(None, true).expect("повторное обновление");
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

        let third = app.update_dependencies(None, true).expect("обновление после правки входа");
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
    fn project_create_open_check_and_export_from_ui() {
        let parent = std::env::temp_dir().join(format!("rde-ui-project-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(&parent).unwrap();
        let root = parent.join("new-project");
        let demo_copy = temp_demo_copy("project-create");
        let mut app = RdeApp::new(rde_core::load_project(&demo_copy).unwrap());

        app.create_project_ui(&root, "UI-проект").expect("создать проект");
        assert_eq!(app.project.root, root);
        assert_eq!(app.project.object_count(), 0);
        assert!(app.store.is_some(), "хранилище нового проекта должно открыться");

        app.create_document_action("Новый отчёт").expect("создать документ");
        assert_eq!(app.project.object_count(), 1);
        let package = app.project.document_path().expect("документ");
        assert_eq!(package.extension().and_then(|ext| ext.to_str()), Some("rde"));
        let rde_doc = rde_doc::load_rde_doc(&package).expect("пакет .doc.rde");
        assert_eq!(rde_doc.doc_id, app.project.document().unwrap().id);

        app.save_project_settings("UI-проект 2", "gost-7.32-2017", "manual").expect("настройки");
        assert_eq!(app.project.manifest.name, "UI-проект 2");

        app.run_project_check();
        let report = app.check_dialog.clone().expect("отчёт проверки");
        assert!(report.contains("Хранилище"), "{report}");
        assert!(report.contains("Документ .doc.rde"), "{report}");

        app.export_dir = parent.join("exports");
        app.export_project_archive();
        let archive = app.export_dir.join("UI-проект-2.rde.zip");
        assert!(archive.is_file(), "архив проекта: {}", archive.display());
        let file = std::fs::File::open(&archive).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let names: Vec<String> = (0..zip.len()).map(|index| zip.by_index(index).unwrap().name().to_owned()).collect();
        assert!(names.contains(&"rde.toml".to_owned()), "{names:?}");
        assert!(names.iter().any(|name| name.ends_with("document.toml")), "{names:?}");
        let _ = std::fs::remove_dir_all(&parent);
        let _ = std::fs::remove_dir_all(&demo_copy);
    }

    #[test]
    fn tree_actions_and_undo_are_wired() {
        let root = temp_demo_copy("tree-actions");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        let note = app
            .project
            .objects
            .iter()
            .find(|object| matches!(&object.data, rde_core::ObjectData::Note(_)))
            .map(|object| object.id.clone())
            .expect("заметка в демо-проекте");
        // «Показать в инспекторе» выбирает объект и открывает панель.
        app.show_inspector = false;
        app.apply_tree_action(super::TreeAction::Inspect(note.clone()));
        assert_eq!(app.selected.as_deref(), Some(note.as_str()));
        assert!(app.show_inspector);
        // «Копировать id» и «Показать каталог» не паникуют и пишут в журнал.
        app.apply_tree_action(super::TreeAction::CopyId(note.clone()));
        let object = app.project.object(&note).unwrap().clone();
        app.apply_tree_action(super::TreeAction::Reveal(object.dir.display().to_string()));
        assert!(app.project.log.iter().any(|line| line.text.contains("id объекта")));
        // «Переименовать…» открывает диалог с текущим названием.
        app.apply_tree_action(super::TreeAction::Rename(note.clone()));
        assert_eq!(app.rename_dialog.as_ref().map(|dialog| dialog.id.clone()), Some(note));
        // Undo без открытого редактора не паникует.
        app.undo_document(false);
        let _ = std::fs::remove_dir_all(&root);
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

        app.create_object_action(super::UiObjectKind::Core(rde_core::NewObjectKind::ComputeArtifact), "Новый расчёт").expect("создать");
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

    #[test]
    fn rollback_from_ui_restores_previous_outputs() {
        if !python_available() {
            return;
        }
        let root = temp_demo_copy("rollback");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        let report = app.update_dependencies(None, true).expect("обновление");
        assert_eq!(report.failed(), 0, "{:?}", report.errors);
        let stats_out = app
            .project
            .artifact_object(STATS)
            .unwrap()
            .dir
            .join("out/stats.json");
        let v1 = std::fs::read_to_string(&stats_out).unwrap();

        // Меняем вход и обновляем — появляется новая версия.
        let input = app
            .project
            .artifact_object(REALIZATIONS)
            .unwrap()
            .dir
            .join("inputs/experiment.csv");
        let mut text = std::fs::read_to_string(&input).unwrap();
        text.push_str("50,50.0,50.0\n");
        std::fs::write(&input, text).unwrap();
        app.reload_project();
        app.update_dependencies(None, true).expect("повторное обновление");
        assert_ne!(std::fs::read_to_string(&stats_out).unwrap(), v1);

        // Кандидат на откат виден панели «Задачи», откат возвращает выходы и ledger.
        let candidates = app.rollback_candidates();
        assert!(!candidates.is_empty(), "должна быть транзакция для отката");
        let outcome = app.rollback_transaction(None).expect("откат из UI");
        assert!(!outcome.restored.is_empty(), "{outcome:?}");
        assert_eq!(std::fs::read_to_string(&stats_out).unwrap(), v1, "выходы восстановлены из CAS");
        assert!(
            app.project.log.iter().any(|line| line.text.contains("Откат выполнен")),
            "журнал: {:?}",
            app.project.log.iter().map(|line| &line.text).collect::<Vec<_>>()
        );
        // После отката панель показывает пометку «откачено».
        assert!(app.rollback_candidates().iter().any(|candidate| candidate.rolled_back));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn untrusted_project_asks_consent_before_running_scripts() {
        if !python_available() {
            return;
        }
        let root = temp_demo_copy("trust");
        rde_core::set_project_trusted(&root, false).unwrap();
        let project = rde_core::load_project(&root).unwrap();
        assert!(!project.manifest.trusted);
        let mut app = RdeApp::new(project);

        app.request_update(None, true);
        assert!(app.trust_dialog.is_some(), "без согласия показывается диалог");
        assert!(app.update_job.is_none(), "скрипты не запускаются до согласия");

        // Подтверждение согласия (как кнопка диалога): trusted=true, затем запуск.
        rde_core::set_project_trusted(&root, true).unwrap();
        app.reload_project();
        assert!(app.project.manifest.trusted);
        app.trust_dialog = None;
        app.start_update(None, true);
        for _ in 0..200 {
            app.poll_update_job();
            if app.update_job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(app.update_job.is_none(), "фоновое обновление завершилось");
        assert!(app.store.is_some(), "хранилище вернулось в UI после потока");
        assert!(
            app.project.log.iter().any(|line| line.text.contains("Обновление связанных")),
            "журнал: {:?}",
            app.project.log.iter().map(|line| &line.text).collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn background_update_can_be_cancelled() {
        if !python_available() {
            return;
        }
        let root = temp_demo_copy("cancel");
        // График «зависает»: даём время нажать «Отменить».
        std::fs::write(
            root.join("objects/artifacts/01M5VJ1JG0QQJM2S6R76FQCZ25/script.py"),
            "import time\nwhile True:\n    time.sleep(0.1)\n",
        )
        .unwrap();
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        app.start_update(None, true);
        assert!(app.update_job.is_some(), "обновление идёт в фоне");
        // Повторный запрос не создаёт второе задание.
        app.request_update(None, true);
        std::thread::sleep(std::time::Duration::from_millis(400));
        app.cancel_update();
        for _ in 0..200 {
            app.poll_update_job();
            if app.update_job.is_none() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert!(app.update_job.is_none(), "отменённое обновление завершилось");
        assert!(app.store.is_some());
        let log: Vec<String> = app.project.log.iter().map(|line| line.text.clone()).collect();
        assert!(
            log.iter().any(|line| line.contains("отменено пользователем") || line.contains("Обновление отменено")),
            "{log:?}"
        );
        assert!(
            log.iter().any(|line| line.contains("отменено пользователем") || line.contains("Обновление связанных")),
            "отчёт об отмене: {log:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
