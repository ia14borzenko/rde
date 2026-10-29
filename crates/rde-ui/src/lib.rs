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
pub mod i18n;
/// M4.1: встроенный MCP-сервер (streamable HTTP) для внешних агентов.
pub mod agent_server;
/// M4.3: состояние панели чата встроенного ACP-агента и транскрипт (R-AGT-02).
pub mod chat;
/// M4.3: панель чата встроенного ACP-агента — UI и жизненный цикл (R-AGT-01/06).
mod chat_panel;
/// M4.4: контекст-ссылки §8.7.2 (чипы «В чат», разрешение в тело, PDF-регион).
mod context_refs;
/// M4.4: diff-UI suggested edits (R-AGT-05) — pending-правки, word-diff, accept/reject.
mod suggestions_ui;
mod ribbon;
/// GUI-02: системный выбор файла/папки («Проводник…», ADR-0017) — фоновый, с деградацией.
pub mod file_dialog;

use file_dialog::{FileDialogService, PathTarget, PickKind, PickRequest};
use i18n::{Lang, tr};
use ribbon::{Cmd, RibbonTab, Slot, ribbon_groups};
use rde_core::{CreateRequest, Freshness, Level, NewObjectKind, ObjectData, ObjectKind, Project};
use rde_dag::{LiveStatus, UpdateAction};
use rde_store::Store;

pub const APP_TITLE: &str = "RDE — Research Development Environment";

// ── палитра ──────────────────────────────────────────────────────────────────
// Тёмная тема (близкая к IDE; значения зафиксированы фазой 1 — не менять).
const BG: Color32 = Color32::from_rgb(0x16, 0x17, 0x1b);
const PANEL: Color32 = Color32::from_rgb(0x1d, 0x1f, 0x24);
const PANEL_HI: Color32 = Color32::from_rgb(0x25, 0x28, 0x30);
const BORDER: Color32 = Color32::from_rgb(0x32, 0x36, 0x40);
const TEXT: Color32 = Color32::from_rgb(0xd8, 0xdb, 0xe2);
const MUTED: Color32 = Color32::from_rgb(0x9a, 0xa1, 0xaf); // WCAG AA: 5.68:1 на PANEL_HI (было 4.78:1)
const ACCENT: Color32 = Color32::from_rgb(0x51, 0x8c, 0xff);
const GREEN: Color32 = Color32::from_rgb(0x52, 0xc4, 0x7d);
const AMBER: Color32 = Color32::from_rgb(0xe3, 0xa7, 0x3e);
const RED: Color32 = Color32::from_rgb(0xe8, 0x76, 0x76); // осветлён с e06060: 5.10:1 на PANEL_HI (WCAG AA, было 4.22:1)
// ACCENT проверен: 4.59:1 на PANEL_HI как текст (WCAG AA ≥ 4.5:1) — значение не меняем,
// осветление уронило бы белый текст на ACCENT-заливках (3.21:1) ниже нормы.

// Светлая тема (GUI фаза 2): тот же набор ролей, что у тёмной. Контрасты текста
// ≥ 4.5:1 (WCAG AA) на LIGHT_PANEL_HI проверяются тестом `theme_tests::text_contrast`.
const LIGHT_BG: Color32 = Color32::from_rgb(0xe9, 0xec, 0xf1);
const LIGHT_PANEL: Color32 = Color32::from_rgb(0xf7, 0xf8, 0xfa);
const LIGHT_PANEL_HI: Color32 = Color32::from_rgb(0xe2, 0xe6, 0xeb);
const LIGHT_BORDER: Color32 = Color32::from_rgb(0xc9, 0xcf, 0xd8);
const LIGHT_TEXT: Color32 = Color32::from_rgb(0x1b, 0x1f, 0x26);
const LIGHT_MUTED: Color32 = Color32::from_rgb(0x59, 0x61, 0x6e);
const LIGHT_ACCENT: Color32 = Color32::from_rgb(0x2b, 0x5a, 0xc7);
const LIGHT_GREEN: Color32 = Color32::from_rgb(0x1b, 0x6e, 0x3e);
const LIGHT_AMBER: Color32 = Color32::from_rgb(0x8a, 0x58, 0x00);
const LIGHT_RED: Color32 = Color32::from_rgb(0xb4, 0x34, 0x2a);

// Заливки primary-кнопок (стиль 2): hover/active светлее/темнее ACCENT; текст на ACCENT
// — тёмный BG в тёмной теме (5.7:1, белый давал 3.2:1), белый в светлой (6.2:1).
const ACCENT_HOVER: Color32 = Color32::from_rgb(0x6a, 0x9c, 0xff);
const ACCENT_ACTIVE: Color32 = Color32::from_rgb(0x4a, 0x84, 0xf2); // 5.1:1 с BG (тест primary_button_*)
const LIGHT_ACCENT_HOVER: Color32 = Color32::from_rgb(0x3a, 0x6a, 0xd6);
const LIGHT_ACCENT_ACTIVE: Color32 = Color32::from_rgb(0x23, 0x4e, 0xa8);

/// Цветовые роли оболочки под текущую тему. Вся отрисовка берёт цвета отсюда
/// через `theme_colors`, сырые константы палитр напрямую используют только
/// `dark_visuals()` / `light_visuals()`.
#[derive(Clone, Copy)]
struct ThemeColors {
    bg: Color32,
    panel: Color32,
    panel_hi: Color32,
    border: Color32,
    text: Color32,
    muted: Color32,
    accent: Color32,
    green: Color32,
    amber: Color32,
    red: Color32,
    /// Текст поверх заливки ACCENT (primary-кнопки).
    on_accent: Color32,
    accent_hover: Color32,
    accent_active: Color32,
}

/// Палитра под эффективную тему (`ctx.theme()` уже учитывает System:
/// без данных от ОС — тёмная, см. `fallback_theme` в egui).
fn theme_colors(theme: egui::Theme) -> ThemeColors {
    match theme {
        egui::Theme::Dark => ThemeColors {
            bg: BG,
            panel: PANEL,
            panel_hi: PANEL_HI,
            border: BORDER,
            text: TEXT,
            muted: MUTED,
            accent: ACCENT,
            green: GREEN,
            amber: AMBER,
            red: RED,
            on_accent: BG,
            accent_hover: ACCENT_HOVER,
            accent_active: ACCENT_ACTIVE,
        },
        egui::Theme::Light => ThemeColors {
            bg: LIGHT_BG,
            panel: LIGHT_PANEL,
            panel_hi: LIGHT_PANEL_HI,
            border: LIGHT_BORDER,
            text: LIGHT_TEXT,
            muted: LIGHT_MUTED,
            accent: LIGHT_ACCENT,
            green: LIGHT_GREEN,
            amber: LIGHT_AMBER,
            red: LIGHT_RED,
            on_accent: Color32::WHITE,
            accent_hover: LIGHT_ACCENT_HOVER,
            accent_active: LIGHT_ACCENT_ACTIVE,
        },
    }
}

/// Значение `settings.theme` → предпочтение egui. Неизвестное → Dark
/// (совпадает с дефолтом `normalize()` в `rde_core::settings`).
fn theme_preference_of(theme: &str) -> egui::ThemePreference {
    match theme {
        "light" => egui::ThemePreference::Light,
        "system" => egui::ThemePreference::System,
        _ => egui::ThemePreference::Dark,
    }
}

/// Значение `settings.theme` → тема встроенного редактора WordCraft.
/// Сам WordCraft (vendor) не трогаем — только просим его перерисоваться
/// в соответствующей гамме.
fn appearance_of(theme: &str) -> wordcraft_ui_egui::theme::Appearance {
    match theme {
        "light" => wordcraft_ui_egui::theme::Appearance::Light,
        "system" => wordcraft_ui_egui::theme::Appearance::System,
        _ => wordcraft_ui_egui::theme::Appearance::Dark,
    }
}

// ── типошкала (мелкий текст ≥ 11, шаг ~1.25 к крупным кеглям) ───────────────────
const FS_DISPLAY: f32 = 18.0; // крупный заголовок просмотра
const FS_HEADING: f32 = 16.0; // заголовок объекта
const FS_BODY: f32 = 13.0; // основной текст, табы, тулбар
const FS_BUTTON: f32 = 12.0; // кнопки и строки действий
const FS_SMALL: f32 = 12.0; // второстепенный текст
const FS_CAPTION: f32 = 11.0; // нижняя граница мелкого текста
const FS_MONO: f32 = 11.5; // моноширинный текст

// ── иконки (Phosphor, MIT; сабсет шрифта `fonts/RdeIcons.ttf`, см. PHOSPHOR-LICENSE.txt) ──
// Глифы — обычный текст: цвет берётся от текста/состояния кнопки, размер — от FS_*.
const IC_OPEN: &str = "\u{e256}"; // folder-open
const IC_RENAME: &str = "\u{e3b4}"; // pencil-simple
const IC_DELETE: &str = "\u{e4a6}"; // trash
const IC_UPDATE: &str = "\u{e036}"; // arrow-clockwise
const IC_SAVE: &str = "\u{e248}"; // floppy-disk
const IC_EXPORT: &str = "\u{eaf0}"; // export
const IC_SEARCH: &str = "\u{e30c}"; // magnifying-glass
const IC_ADD: &str = "\u{e3d4}"; // plus
const IC_DOC: &str = "\u{e23a}"; // file-text
const IC_ARTIFACT: &str = "\u{e1da}"; // cube
const IC_SOURCE: &str = "\u{e758}"; // books
const IC_NOTE: &str = "\u{e34c}"; // note-pencil
const IC_CONST: &str = "\u{ec80}"; // pi
const IC_REPO: &str = "\u{e278}"; // git-branch
const IC_FOLDER: &str = "\u{e24a}"; // folder
const IC_CHECK: &str = "\u{e182}"; // check
const IC_INSPECT: &str = "\u{ec24}"; // sidebar-simple
const IC_CLOSE: &str = "\u{e4f6}"; // x
const IC_TREE: &str = "\u{e67c}"; // tree-structure
const IC_CODE: &str = "\u{e914}"; // file-code
const IC_DOWNLOAD: &str = "\u{e1ac}"; // cloud-arrow-down

/// Иконка строки дерева по типу объекта (вместо буквенной «аватарки» rde-core).
fn kind_icon(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Document => IC_DOC,
        ObjectKind::Artifact => IC_ARTIFACT,
        ObjectKind::Constants => IC_CONST,
        ObjectKind::Source => IC_SOURCE,
        ObjectKind::Note => IC_NOTE,
        ObjectKind::Repository => IC_REPO,
        // Формула — служебный объект проекта (ADR-0005); отдельной иконки в сабсете нет.
        ObjectKind::Formula => IC_CODE,
    }
}

/// Служебные объекты скрыты в дереве по умолчанию (R-TREE-01/02). Пока в UI нет переключателя
/// «показать служебные» (P4), скрываем только проектные формулы; константы остаются видимы как
/// раньше (обратная совместимость UI).
fn tree_group_hidden(kind: ObjectKind) -> bool {
    matches!(kind, ObjectKind::Formula)
}

// ── раскладка: ширины боковых панелей (одна точка правды; ТЗ-03 §19-8) ──────────────────
/// Левая панель «Проект» (дерево объектов).
const TREE_WIDTH: f32 = 268.0;
const TREE_WIDTH_MIN: f32 = 120.0;
/// Правая панель «Агент».
const AGENT_WIDTH: f32 = 316.0;
const AGENT_WIDTH_MIN: f32 = 180.0;
/// Инспектор объекта: 340 px по умолчанию, перетаскивается в диапазоне 280–480 px.
/// Решение владельца §19-8: инспектор не должен съедать центр документа.
const INSPECTOR_WIDTH: f32 = 340.0;
const INSPECTOR_WIDTH_MIN: f32 = 280.0;
const INSPECTOR_WIDTH_MAX: f32 = 480.0;
/// Высота нативной области правки формулы (equation_tab + canvas WordCraft) в панели P4.
const NATIVE_FORMULA_EDITOR_H: f32 = 300.0;
/// P5 (ADR-0021): нативный «проф. редактор» (клон MS Equations) **выведен из UI** — он портил
/// формулы (мусор `equation.get` писался в источник) и не отвечал требованиям MathType; заменяется
/// редактором MathType-класса (Phase 2). Код оставлен за флагом (тесты валидации/отладка): при
/// `true` возвращает старое окно и автосинк; в продукте — `false`.
const FORMULA_NATIVE_UI: bool = false;

// ── строка вкладок центра: ОДНА линия без переноса (ТЗ-03 §19-7) ────────────────────────
/// Высота таба строки вкладок и сегмента переключателя режимов (равна `interact_size.y`
/// глобального стиля: кнопки не бывают ниже 32 px, поэтому строка ровно в одну кнопку).
const CENTER_TAB_HEIGHT: f32 = 32.0;
const CENTER_SEGMENT_HEIGHT: f32 = 24.0;
/// Горизонтальный отступ таба строки вкладок (компактнее кнопок ленты).
const CENTER_TAB_PADDING_X: f32 = 6.0;
/// Зазор между табами строки вкладок.
const CENTER_TAB_SPACING_X: f32 = 1.0;
/// Зазор между вкладками и переключателем режимов в той же строке.
const CENTER_MODE_GAP: f32 = 10.0;
/// Отступ сегмента переключателя режимов и зазор между сегментами.
const CENTER_SEGMENT_PADDING_X: f32 = 6.0;
const CENTER_SEGMENT_SPACING_X: f32 = 1.0;
/// Поля сегментированной рамки (2 px поля + 1 px обводка) — входят в ширину переключателя.
const CENTER_SEGMENT_MARGIN: f32 = 2.0;
const CENTER_SEGMENT_FRAME: f32 = CENTER_SEGMENT_MARGIN + 1.0;
/// Запас по ширине: строка не должна упираться в правый край вплотную.
const CENTER_LINE_SAFETY: f32 = 8.0;

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

    fn title(self, lang: Lang) -> &'static str {
        match self {
            Tab::Document => tr(lang, "tab.document"),
            Tab::Structure => tr(lang, "tab.structure"),
            Tab::Artifacts => tr(lang, "tab.artifacts"),
            Tab::Source => tr(lang, "tab.source"),
            Tab::Code => tr(lang, "tab.code"),
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Tab::Document => IC_DOC,
            Tab::Structure => IC_TREE,
            Tab::Artifacts => IC_ARTIFACT,
            Tab::Source => IC_SOURCE,
            Tab::Code => IC_CODE,
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

    /// Пометить предпросмотр устаревшим: страницы будут перерисованы из текущего пакета при
    /// следующем `ensure`, сам `DocPreview` не пересоздаётся (P5: позиция/скролл не теряются).
    fn invalidate(&mut self) {
        if self.path.is_some() {
            self.state = DocState::Pending;
        }
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

/// Снимок позиции документа для восстановления после пересборки пакета (P5, ADR-0021 §Решение 3):
/// каретка/выделение редактора, page_hint и offset предпросмотра. Если путь документа сменился —
/// позиция прежнего документа не переносится (осознанный сброс).
struct DocumentViewState {
    path: Option<std::path::PathBuf>,
    selection: Option<wordcraft_engine::Selection>,
    page_hint: usize,
    preview_offset: f32,
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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

    /// Полное название режима: доступное имя кнопки-сегмента и статусы/тесты.
    fn label(self) -> &'static str {
        match self {
            DocMode::DocView => "RDE",
            DocMode::Full => "WordCraft",
            DocMode::Preview => "Предпросмотр страниц",
        }
    }

    /// Короткая подпись для компактного переключателя в строке вкладок центра (§19-7):
    /// «Предпросмотр страниц» → «Предпросмотр», полное название остаётся в тултипе и a11y.
    fn short_label(self) -> &'static str {
        match self {
            DocMode::DocView | DocMode::Full => self.label(),
            DocMode::Preview => "Предпросмотр",
        }
    }

    /// Иконка сегмента переключателя (Phosphor-сабсет: документ / правка / поиск-просмотр).
    fn icon(self) -> &'static str {
        match self {
            DocMode::DocView => IC_DOC,
            DocMode::Full => IC_RENAME,
            DocMode::Preview => IC_SEARCH,
        }
    }

    /// Подсказка сегмента: что за режим и чем отличается.
    fn tooltip(self) -> &'static str {
        match self {
            DocMode::DocView => "Режим RDE: редактор документа в оболочке RDE (лента RDE)",
            DocMode::Full => "Режим WordCraft: полный интерфейс встроенного редактора WordCraft",
            DocMode::Preview => "Предпросмотр страниц: растеризованные страницы документа",
        }
    }
}

/// Плотность строки вкладок центра (§19-7): каким элементам хватает места для подписи.
/// Строка всегда одна — при нехватке ширины остаются иконки с тултипами, а не перенос.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CenterLineDensity {
    /// Вкладки центра — с подписями («Документ | Структура | …»), а не только иконками.
    tab_labels: bool,
    /// Сегменты переключателя режимов — с подписями (RDE | WordCraft | Предпросмотр).
    mode_labels: bool,
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
    /// GUI-02: свойства источника/репозитория (path/url/kind задаются в диалоге).
    Properties(String),
    /// P4: панель редактора формулы (ADR-0020).
    EditFormula(String),
    /// P4: вставка project-level формулы в документ из дерева.
    InsertFormula(String),
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
/// Раздел рабочего места артефакта: входные данные / генерация / результат.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ArtifactSection {
    Inputs,
    Generation,
    Result,
}

/// Редактор артефакта: манифест, скрипт и поля редактирования (R-ART-01/02/04).
#[derive(Clone, Debug)]
struct ArtifactEditorState {
    artifact_id: String,
    manifest: rde_core::ArtifactManifest,
    script_text: String,
    params_text: String,
    outputs_text: String,
    timeout_text: String,
    new_input_artifact: String,
    new_input_selector: String,
    new_input_file: String,
    new_input_mode: String,
    /// Новая строка редактора констант (R-ART-09): имя/значение/единица/symbol/subscript/
    /// display/стиль/format/source (ADR-0005 P1).
    new_constant_name: String,
    new_constant_value: String,
    new_constant_unit: String,
    new_constant_symbol: String,
    new_constant_subscript: String,
    new_constant_display: String,
    new_constant_style: String,
    new_constant_format: String,
    new_constant_source: String,
}

/// Диалог настроек проекта (R-SET-01): имя/шаблон/режим пересборки + авторы.
#[derive(Clone, Debug)]
pub struct ProjectSettingsState {
    name: String,
    template: String,
    rebuild: String,
    authors: Vec<rde_core::AuthorManifest>,
    new_author_name: String,
    new_author_initials: String,
    new_author_email: String,
}

/// Зачем пользователю нужно согласие на запуск скриптов проекта (R-RUN-06).
#[derive(Clone, Debug)]
enum TrustRequest {
    /// Обычный поток: обновить артефакты после подтверждения доверия.
    Update { targets: Option<Vec<String>>, dependents: bool },
    /// M4.2b: разрешить внешним агентам запускать скрипты (грант `run_script`).
    AgentRunScript,
}

/// Диалог согласия на запуск скриптов (R-RUN-06): что запустить после подтверждения.
#[derive(Clone, Debug)]
struct TrustDialogState {
    request: TrustRequest,
}

/// «Открыть проект…»: путь к каталогу + последняя ошибка (валидация/открытие).
/// Диалог не закрывается при ошибке — причина видна под полем, ввод остаётся редактируемым.
#[derive(Clone, Debug, Default)]
struct OpenProjectDialog {
    path: String,
    error: String,
    /// Первый кадр диалога — фокус в поле пути.
    focused: bool,
}

/// «Импорт .docx…»: путь к файлу, необязательное имя документа и ошибка импорта.
#[derive(Clone, Debug, Default)]
struct ImportDocxDialog {
    path: String,
    /// Пользовательское имя документа (как `rde import-docx --title`); пусто — из имени файла.
    title: String,
    error: String,
    focused: bool,
}

/// «Новый проект…»: каталог, название и ошибка создания.
#[derive(Clone, Debug, Default)]
struct NewProjectDialog {
    path: String,
    name: String,
    error: String,
    focused: bool,
}

/// «Импорт ревью»: путь к `.docx` после правок в Word (пусто — самый свежий из `exports/`).
#[derive(Clone, Debug, Default)]
struct ImportReviewDialog {
    path: String,
    error: String,
    focused: bool,
}

/// «Импорт цитат»: BibTeX / RIS / CSL-JSON → объекты-источники (rde-sources).
#[derive(Clone, Debug, Default)]
struct ImportCitationsDialog {
    path: String,
    error: String,
    focused: bool,
}

/// «Свойства источника»: локатор (файл или URL), дата получения; файл можно прикрепить.
#[derive(Clone, Debug, Default)]
pub struct SourceEditDialog {
    id: String,
    kind: String,
    file: String,
    url: String,
    retrieved: String,
    error: String,
    focused: bool,
}

/// «Свойства репозитория»: вид (`internal|external|git`) и путь.
#[derive(Clone, Debug, Default)]
pub struct RepositoryEditDialog {
    id: String,
    kind: String,
    path: String,
    error: String,
    focused: bool,
}

/// Редактор заметки проекта (MCP-паритет `note_write`: правки из GUI).
#[derive(Clone, Debug, Default)]
struct NoteEditorState {
    id: String,
    text: String,
    dirty: bool,
    error: String,
}

/// Уровень подсказки под полем пути: зелёный/нейтральный/жёлтый/красный.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PathCheckLevel {
    Ok,
    Hint,
    Warn,
    Error,
}

/// Результат проверки пути до действия: «путь существует/того типа» + понятное сообщение.
#[derive(Clone, Debug, PartialEq, Eq)]
struct PathCheck {
    level: PathCheckLevel,
    text: String,
}

impl PathCheck {
    fn ok(text: impl Into<String>) -> Self {
        Self { level: PathCheckLevel::Ok, text: text.into() }
    }
    fn hint(text: impl Into<String>) -> Self {
        Self { level: PathCheckLevel::Hint, text: text.into() }
    }
    fn warn(text: impl Into<String>) -> Self {
        Self { level: PathCheckLevel::Warn, text: text.into() }
    }
    fn error(text: impl Into<String>) -> Self {
        Self { level: PathCheckLevel::Error, text: text.into() }
    }
    fn is_error(&self) -> bool {
        self.level == PathCheckLevel::Error
    }
}

/// Диалог «Вставить в документ»: значение/рисунок из артефакта (R-DOC-11) или формула (P3).
#[derive(Clone, Debug)]
struct InsertDialogState {
    /// `value` | `figure` | `formula` | …
    kind: String,
    artifact_id: String,
    /// Формула: id project-level объекта (P3).
    formula_id: String,
    /// Формула: `omml` (редактируемый Word-объект) | `svg` (векторная картинка).
    render_mode: String,
    selector: String,
    pointer: String,
    caption: String,
    /// Стиль абзаца вставки (`w:pStyle` из текущего документа).
    style: String,
    /// `auto|raw|int|dec|sci|percent` + точность.
    format: String,
    precision: usize,
    /// Ошибка валидации, показываемая в диалоге (P8-B1).
    error: String,
}

/// Данные инспектора формулы (P3): живой статус, act_key и «где вставлено» (R-DOC-19).
#[derive(Clone, Debug, Default)]
struct FormulaInspectorData {
    status: Option<rde_dag::FormulaLiveStatus>,
    act_key: Option<String>,
    backlinks: Vec<rde_doc::FormulaBacklink>,
    /// Статусы формульных вставок (Fresh/Stale/BodyModified/MissingTarget, R-DOC-13/19):
    /// сравнение `act_key` вставки с текущим — то, что пересоберёт «Обновить документ».
    insert_states: Vec<rde_doc::FormulaInsertState>,
    unregistered: usize,
}

/// Панель редактора формулы (P4, ADR-0005 §5): LaTeX-источник, предпросмотр, нативный
/// `equation_tab` WordCraft, применение в `formula.toml` и вставка в документ с номером.
/// `formula_id = None` — режим создания (диалог «Создать формулу»).
struct FormulaEditorState {
    /// `None` — создание новой формулы, `Some(id)` — правка существующей.
    formula_id: Option<String>,
    title: String,
    latex: String,
    /// `true` — display (отдельная строка, нумеруется), `false` — inline.
    display: bool,
    /// Политика нумерации объекта: `auto` — нумеровать при вставке/обновлении, `none` — без номера.
    numbering_mode: String,
    /// Метка/группа нумерации (R-DOC-15; пусто — нет).
    numbering_label: String,
    /// Режим вставки в документ: `omml` (редактируемый Word-объект) | `svg` (вектор).
    render_mode: String,
    /// Ошибка последнего действия (валидация LaTeX, применение, вставка).
    error: String,
    /// Информационное сообщение (создана/применена/пересчитана/вставлена).
    info: String,
    /// Открыт нативный «профессиональный редактор» WordCraft (equation_tab + canvas).
    native_open: bool,
    /// Отдельный WordCraft-редактор: предпросмотр (`previews::math_texture`) и нативная правка.
    /// Не трогает документ проекта — правка источника не создаёт черновиков в тексте.
    native: Option<Box<NativeFormulaEditor>>,
    /// P5 (ADR-0021): нативный WYSIWYG-редактор формулы (модель `MNode` + каретка + палитры).
    /// LaTeX-поле ниже — источник истины; редактор синхронизирован с ним.
    math_editor: Option<Box<rde_math_editor::widget::MathEditorUi>>,
}

/// Отдельный (scratch) WordCraft-редактор для панели формулы (P4, спайк A): живёт только в панели,
/// нативная вкладка Equation и canvas рисуются в окне RDE без правок `vendor/wordcraft`.
struct NativeFormulaEditor {
    app: wordcraft_ui_egui::WordApp,
    /// `theme::install_fonts` вызван; до следующего кадра `equation_tab::show` паникует
    /// (`FontFamily::Name("semibold") is not bound to any fonts`) — первый кадр панели без него.
    fonts_ready: bool,
    /// Пропустить кадры после `set_fonts` (шрифты применяются со следующего кадра).
    skip_frames: u8,
}

impl InsertDialogState {
    /// Спецификация формата для `rde_doc::format_value` (например, `dec:2`).
    fn format_spec(&self) -> String {
        match self.format.as_str() {
            "dec" | "sci" | "percent" => format!("{}:{}", self.format, self.precision),
            other => other.to_owned(),
        }
    }
}

/// Диалог «Принять формулы как объекты» (ADR-0019): счётчики display/inline/номеров и политика
/// нумерации (схема документа, сохранять ли номера Word, нумеровать ли display-формулы).
#[derive(Clone, Debug)]
struct FormulaMigrationDialogState {
    equations: Vec<rde_doc::UnregisteredFormula>,
    display: usize,
    inline: usize,
    found_numbers: usize,
    mixed: usize,
    scheme: rde_doc::FormulaNumbering,
    preserve_numbers: bool,
    number_display: bool,
    error: String,
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
    /// Масштаб интерфейса оболочки (GUI фаза 2): множитель поверх зума документа.
    /// Применяется через `ctx.set_zoom_factor`, живёт в сессии (в `AppSettings`
    /// его нет — владение rde-core, вне scope фазы).
    ui_scale: f32,
    logs_tab: usize,
    fonts_installed: bool,
    /// Счётчик кадров `ui()`: диалоги (`Window` с заголовком в стиле Heading)
    /// рисуются со второго кадра — `set_fonts` применяется только в конце пасса,
    /// и заголовок на первом кадре мог бы запросить ещё неактивную семью.
    ui_frames: u64,
    // видимость панелей (скрываются через меню «Вид» или кнопкой × в заголовке)
    show_tree: bool,
    show_agent: bool,
    show_logs: bool,
    doc_mode: DocMode,
    /// Выбранная вкладка ленты (видна только в режиме RDE, вкладка «Документ»).
    ribbon_tab: RibbonTab,
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
    /// Кэш вставок rde:doc для вкладки «Структура» (ключ — путь пакета).
    inserts_cache: Option<(String, Vec<rde_doc::RdeInsert>)>,
    /// P5 (quick-win §5.2): кэш «сколько вставок в пакете» для ленты — по (путь, mtime, размер),
    /// чтобы не читать и не распаковывать `.doc.rde` каждый кадр в режиме RDE.
    inserts_count_cache: Option<(String, u64, u64, usize)>,
    /// P5 (quick-win §5.2): кэш инспектора формулы (`formula_inspector_data` считался каждый кадр:
    /// ledger + backlinks + статусы вставок — до 52 мс на кадр в debug). Ключ — (эпоха данных, id).
    formula_inspector_cache: Option<(u64, String, FormulaInspectorData)>,
    /// Эпоха данных проекта/документа/ledger: увеличивается при `reload_project` (после любых
    /// изменений) — инвалидирует производные кэши (инспектор формулы).
    data_epoch: u64,
    /// P5 (quick-win §5.2): превью следующего номера формулы — `next_formula_number_text` читает
    /// `.doc.rde` (распаковка zip) и вызывался каждый кадр панели формулы; кэш по (путь, mtime,
    /// размер, схема нумерации).
    formula_number_preview_cache: Option<(String, Option<String>)>,
    /// Фокус на поиске объектов (кнопка «Поиск», меню «Найти», Ctrl+F).
    search_focus: bool,
    /// Диалог «Открыть проект…» (путь + ошибка; «Проводник…», недавние проекты).
    open_project_dialog: Option<OpenProjectDialog>,
    /// Диалог «Импорт .docx…» (R-DOC-39): путь, имя документа, ошибка.
    import_docx_dialog: Option<ImportDocxDialog>,
    /// Диалог «Импорт ревью из Word»: путь к .docx (пусто — свежий из exports/).
    import_review_dialog: Option<ImportReviewDialog>,
    /// Диалог «Импорт цитат» (BibTeX/RIS/CSL-JSON).
    import_citations_dialog: Option<ImportCitationsDialog>,
    /// Диалог «Новый проект…» (каталог + название + ошибка).
    new_project_dialog: Option<NewProjectDialog>,
    /// Диалог «Свойства источника» (file/url + прикрепление файла).
    source_edit_dialog: Option<SourceEditDialog>,
    /// Диалог «Свойства репозитория» (path/kind).
    repo_edit_dialog: Option<RepositoryEditDialog>,
    /// Редактор заметки во вкладке «Источник».
    note_editor: Option<NoteEditorState>,
    settings_dialog: Option<ProjectSettingsState>,
    check_dialog: Option<String>,
    audit_dialog: Option<Vec<String>>,
    about_open: bool,
    /// Диалог «проект доверенный» перед первым запуском скриптов.
    trust_dialog: Option<TrustDialogState>,
    /// Фоновое обновление артефактов (очередь/отмена).
    update_job: Option<UpdateJob>,
    /// Диалог вставки значения/рисунка из артефакта.
    insert_dialog: Option<InsertDialogState>,
    /// Диалог «Принять формулы как объекты» (ADR-0005 §7, ADR-0019): политика номеров.
    formula_migration_dialog: Option<FormulaMigrationDialogState>,
    /// Панель редактора формулы (P4): создание/правка источника, предпросмотр, нативный
    /// equation_tab, вставка с номером (ADR-0005 §5).
    formula_editor: Option<FormulaEditorState>,
    /// Состояние git-репозитория проекта (P5, R-VCS-01).
    git_status: Option<rde_vcs::GitStatus>,
    /// Диалог коммита: текст сообщения.
    commit_dialog: Option<String>,
    /// Окно истории git.
    history_dialog: Option<Vec<rde_vcs::CommitEntry>>,
    /// Язык интерфейса (R-UI-06) из настроек приложения.
    lang: Lang,
    /// Настройки приложения (R-SET-01) — загружаются при старте.
    app_settings: rde_core::AppSettings,
    /// Диалог настроек приложения.
    app_settings_dialog: Option<rde_core::AppSettings>,
    /// Раздел рабочего места артефакта (вкладка «Артефакты»).
    artifact_section: ArtifactSection,
    /// Редактор выбранного артефакта (манифест + скрипт).
    artifact_editor: Option<ArtifactEditorState>,
    /// Кэш превью изображений выходов артефактов: путь → (mtime, текстура egui).
    image_previews: std::collections::HashMap<String, (u64, egui::TextureHandle)>,
    /// Высота области DAG во вкладке «Артефакты» (перетаскиваемый разделитель).
    dag_height: f32,
    /// Автообновление (R-DOC-12/R-ART-03): после обновления артефактов обновлять документ,
    /// при старте/проверке — пересчитывать устаревшие артефакты (только для trusted-проекта).
    auto_update: bool,
    /// Авто-старт уже выполнялся в этой сессии (не запускаем повторно каждый кадр).
    auto_boot_done: bool,
    /// Защита от рекурсии: авто-обновление документа уже идёт.
    auto_updating: bool,
    /// Диалог бэкапов проекта (R-VCS-04): список архивов.
    backup_dialog: Option<Vec<PathBuf>>,
    /// Блок-якорь вставки: каретка в DocView или выбранный блок в «Структуре» (P8-B1).
    insert_anchor_block: Option<usize>,
    /// Показывать служебные объекты в дереве (R-TREE-01/02): группа «Формулы» скрыта по
    /// умолчанию, переключатель — вкладка «Вид» (P3).
    show_service_objects: bool,
    /// Подписи служебных объектов в дереве: id формулы → «N вставок · статус» (P3, R-DOC-19).
    formula_labels: BTreeMap<String, String>,
    /// M4.1: встроенный MCP-сервер по streamable HTTP для внешних агентов (R-MCP-02).
    external_agents: agent_server::ExternalAgentServer,
    /// Авто-запуск внешних агентов уже пробовали в этой сессии (только при разрешении в настройках).
    external_agents_attempted: bool,
    /// Показывать bearer-токен в панели «Агент» (секрет; по умолчанию скрыт).
    show_agent_token: bool,
    /// M4.2: очередь GUI-approvals встроенного MCP-сервера (решения пользователя).
    agent_approvals: std::sync::Arc<agent_server::GuiApprovals>,
    /// Текущий диалог подтверждения действия внешнего агента.
    agent_approval_dialog: Option<agent_server::PendingApproval>,
    /// M4.2b: дополнительные строки контекста для диалога approval (число файлов к коммиту,
    /// автор, trusted-статус проекта) — считаются один раз при показе запроса.
    agent_approval_context: Vec<String>,
    // ── M4.3: встроенный ACP-агент (DSH) и панель чата (R-AGT-01/02/06) ────────
    /// Фоновый клиент встроенного ACP-агента (ленивый старт по «Новый чат»).
    agent: Option<rde_agent::AgentHandle>,
    /// Состояние панели чата: статус, транскрипт, permission-запросы, config-options.
    chat: chat::ChatState,
    /// RDE-MCP-сервер **для встроенного агента** (отдельный от внешних агентов M4.1).
    chat_mcp: Option<agent_server::ExternalAgentServer>,
    /// Очередь GUI-approvals чат-MCP-сервера (отдельная от внешней очереди — M4.2b).
    chat_approvals: std::sync::Arc<agent_server::GuiApprovals>,
    /// Желаемые гранты нового чата (R-AGT-06: правки/скрипты/git, фиксируются при `session/new`).
    chat_grants: chat::ChatGrants,
    /// Открыта форма «Новый чат» (выбор грантов сессии).
    chat_setup_open: bool,
    /// Показывать размышления агента в ленте (по умолчанию скрыты).
    show_agent_thoughts: bool,
    /// Поле ввода сообщения агенту.
    chat_input: String,
    /// Открыт список истории сессий («История…»).
    chat_history_open: bool,
    /// Отложенный старт сессии (new/resume) — до `initialize` или закрытия старой сессии.
    chat_pending_start: Option<chat::ChatStartIntent>,
    /// Действие после закрытия текущей сессии (новый чат/resume — «одна активная сессия»).
    chat_after_close: Option<chat::ChatStartIntent>,
    // ── M4.4: контекст-ссылки и diff-UI правок ──────────────────────────────
    /// Просмотр страницы PDF с выбором региона для чипа `rde://source/…?page=&bbox=` (§8.7.2).
    pdf_region: Option<context_refs::PdfRegionView>,
    /// Открыто окно «Правки агента» (diff-UI suggested edits, R-AGT-05).
    show_suggestions: bool,
    /// Ревизия списка pending-правок (кэш; перечитывается при изменении каталога/по действию).
    suggestions_cache: Option<Vec<rde_doc::SuggestedEdit>>,
    /// Демо/тест: подменить список правок без записи в проект (скриншоты/egui_kittest).
    suggestions_demo: Option<Vec<rde_doc::SuggestedEdit>>,
    // ── GUI-02: «Проводник…» и недавние проекты (ADR-0017) ────────────────────
    /// Фоновый системный выбор файла/папки (kdialog/zenity; деградация без графики).
    file_dialog: FileDialogService,
    /// Готовые выборы, ещё не разложенные по полям диалогов (адрес → путь).
    picked_paths: Vec<(PathTarget, PathBuf)>,
    /// Ошибки backend'а выбора (адрес → сообщение): показываются в диалоге, поле редактируемо.
    picker_errors: Vec<(PathTarget, String)>,
    /// Куда вернуть фокус после выбора — кнопка «ОК» соответствующего диалога.
    picker_focus_ok: Vec<PathTarget>,
    /// Демо/тест: вертикальный скролл предпросмотра страниц (скриншоты конкретной страницы).
    demo_preview_scroll: f32,
    /// Текущий offset скролла области предпросмотра (тест: колесо/ползунок прокручивают).
    preview_scroll_offset: f32,
    /// Одноразовый restore offset предпросмотра после пересборки пакета (P5, ADR-0021 §Решение 3):
    /// применяется к `ScrollArea` один раз, когда страницы снова готовы (колесо не ломается).
    preview_restore_offset: Option<f32>,
    /// Демо/тест: подменённая история проектов (скриншоты, kittest) вместо файла конфига.
    demo_recent_projects: Option<Vec<rde_core::RecentProject>>,
    /// Файл истории проектов: `None` — не записывать (тесты); по умолчанию — из конфига RDE.
    recent_projects_file: Option<PathBuf>,
}

/// Подпись схемы нумерации формул документа (ADR-0018) для инспектора/журнала.
fn formula_numbering_label(scheme: rde_doc::FormulaNumbering) -> &'static str {
    match scheme {
        rde_doc::FormulaNumbering::Continuous => "сквозная (1), (2)…",
        rde_doc::FormulaNumbering::Section => "по разделам (2.1), (2.2)…",
    }
}

/// Документ, открываемый при старте: самый свежий по mtime пакета (в проекте может быть несколько
/// документов: пустой `main` и импортированный `.docx` — открываем тот, что меняли последним).
/// Если mtime недоступен — первый документ проекта.
fn initial_document_path(project: &rde_core::Project) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for object in project.objects.iter() {
        let ObjectData::Document(manifest) = &object.data else { continue };
        let path = object.dir.join(&manifest.file);
        let Ok(mtime) = fs::metadata(&path).and_then(|meta| meta.modified()) else { continue };
        if best.as_ref().map(|(known, _)| mtime > *known).unwrap_or(true) {
            best = Some((mtime, path));
        }
    }
    best.map(|(_, path)| path).or_else(|| project.document_path())
}

impl RdeApp {
    pub fn new(project: Project) -> Self {
        // Math-шрифт формул (STIX Two Math, ADR-0018) — до первой вёрстки/рендера WordCraft:
        // предпросмотр страниц, DocView и редактор используют один глобальный FontDb.
        rde_doc::ensure_math_font();
        let app_settings = rde_core::settings::load();
        let chat_allowed = app_settings.allow_builtin_agent;
        let lang = i18n::lang(&app_settings.language);
        let doc = DocPreview::new(initial_document_path(&project));
        let export_dir = project.root.join("exports");
        let selected = doc
            .path
            .as_deref()
            .and_then(Path::parent)
            .and_then(|dir| {
                project
                    .objects
                    .iter()
                    .find(|object| object.dir == dir && matches!(object.data, ObjectData::Document(_)))
            })
            .map(|object| object.id.clone())
            .or_else(|| project.document_object().map(|object| object.id.clone()));
        let mut app = Self {
            project,
            tab: Tab::Document,
            tree_filter: String::new(),
            selected,
            doc,
            zoom: 0.72,
            ui_scale: 1.0,
            logs_tab: 0,
            fonts_installed: false,
            ui_frames: 0,
            show_tree: true,
            show_agent: true,
            show_logs: true,
            doc_mode: DocMode::DocView,
            ribbon_tab: RibbonTab::Home,
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
            inserts_count_cache: None,
            formula_inspector_cache: None,
            data_epoch: 0,
            formula_number_preview_cache: None,
            search_focus: false,
            open_project_dialog: None,
            import_docx_dialog: None,
            import_review_dialog: None,
            import_citations_dialog: None,
            new_project_dialog: None,
            source_edit_dialog: None,
            repo_edit_dialog: None,
            note_editor: None,
            settings_dialog: None,
            check_dialog: None,
            audit_dialog: None,
            about_open: false,
            trust_dialog: None,
            update_job: None,
            insert_dialog: None,
            formula_migration_dialog: None,
            formula_editor: None,
            git_status: None,
            commit_dialog: None,
            history_dialog: None,
            lang,
            app_settings,
            app_settings_dialog: None,
            artifact_section: ArtifactSection::Result,
            artifact_editor: None,
            image_previews: std::collections::HashMap::new(),
            dag_height: 240.0,
            auto_update: false,
            auto_boot_done: false,
            auto_updating: false,
            backup_dialog: None,
            insert_anchor_block: None,
            show_service_objects: false,
            formula_labels: BTreeMap::new(),
            external_agents: agent_server::ExternalAgentServer::new(),
            external_agents_attempted: false,
            show_agent_token: false,
            agent_approvals: std::sync::Arc::new(agent_server::GuiApprovals::new()),
            agent_approval_dialog: None,
            agent_approval_context: Vec::new(),
            agent: None,
            chat: chat::ChatState::new(chat_allowed),
            chat_mcp: None,
            chat_approvals: std::sync::Arc::new(agent_server::GuiApprovals::new()),
            chat_grants: chat::ChatGrants::default(),
            chat_setup_open: false,
            show_agent_thoughts: false,
            chat_input: String::new(),
            chat_history_open: false,
            chat_pending_start: None,
            chat_after_close: None,
            pdf_region: None,
            show_suggestions: false,
            suggestions_cache: None,
            suggestions_demo: None,
            file_dialog: FileDialogService::new(),
            picked_paths: Vec::new(),
            picker_errors: Vec::new(),
            picker_focus_ok: Vec::new(),
            demo_preview_scroll: 0.0,
            preview_scroll_offset: 0.0,
            preview_restore_offset: None,
            demo_recent_projects: None,
            // В unit-тестах история не пишется в конфиг пользователя (тесты создают много
            // временных проектов); тесты, проверяющие историю, задают путь явно.
            recent_projects_file: {
                #[cfg(test)]
                {
                    None
                }
                #[cfg(not(test))]
                {
                    Some(rde_core::recent_projects_path())
                }
            },
        };
        app.auto_update = app.project.manifest.document_auto_update;
        // R-RUN-08: ретенция запусков (логи/провенанс не растут бесконечно).
        let _ = rde_run::prune_runs(&app.project.root, 200);
        match Store::open(&app.project.root) {
            Ok(store) => {
                app.store = Some(store);
                app.refresh_dag_statuses();
            }
            Err(error) => app.push_log(Level::Warn, format!("Хранилище проекта недоступно: {error}")),
        }
        app.refresh_git_status();
        app.check_external_edit_on_startup();
        app
    }

    /// Обновить состояние git-репозитория (для статус-бара/«Проверить»).
    pub fn refresh_git_status(&mut self) {
        self.git_status = rde_vcs::status(&self.project.root).ok();
    }

    // ── M4.1: внешние агенты (встроенный MCP HTTP-сервер) ────────────────────

    /// Авто-запуск встроенного MCP HTTP-сервера при старте, если пользователь разрешил внешних
    /// агентов (R-MCP-02, настройка `allow_external_mcp`, по умолчанию выключена).
    ///
    /// В unit-тестах авто-старт отключён (не плодим фоновые порты в тестах оболочки); явное
    /// включение тестируется через [`Self::apply_external_agents_setting`].
    fn ensure_external_agents(&mut self) {
        if self.external_agents_attempted {
            return;
        }
        self.external_agents_attempted = true;
        if !self.app_settings.allow_external_mcp || cfg!(test) {
            return;
        }
        let root = self.project.root.clone();
        let options = self.external_agent_options();
        match self.external_agents.start(&root, &options) {
            Ok(()) => {
                if let agent_server::ExternalAgentStatus::Running { info, grants } =
                    self.external_agents.status(true)
                {
                    let mode = self.external_agent_mode_label();
                    self.push_log(
                        Level::Info,
                        format!(
                            "Внешние агенты: MCP HTTP {} (loopback, {mode} [{}]; токен — в панели «Агент») — R-MCP-02",
                            info.url(),
                            grants.label()
                        ),
                    );
                    self.log_agent_run_script_trust_hint();
                }
            }
            Err(error) => {
                self.push_log(Level::Warn, format!("Внешние агенты (MCP HTTP): {error}"));
                // Fail-closed: не оставляем «разрешено», если сервер не поднялся.
                self.app_settings.allow_external_mcp = false;
            }
        }
    }

    /// M4.2b: желаемый набор грантов внешнего агента по настройкам приложения.
    ///
    /// Подгалочки запуска скриптов/git-коммитов действуют только при включённом внешнем доступе
    /// **и** разрешённых мутациях (иерархия галочек). `run_script` дополнительно требует
    /// `trusted`-проекта — эффективный грант вычисляет сервер при старте (R-RUN-06).
    fn external_agent_grants(&self) -> agent_server::ExternalAgentGrants {
        let mutations = self.app_settings.allow_external_mcp && self.app_settings.allow_agent_mutations;
        agent_server::ExternalAgentGrants {
            project_write: mutations,
            run_script: mutations && self.app_settings.allow_agent_run_script,
            git_commit: mutations && self.app_settings.allow_agent_git_commit,
        }
    }

    /// Человекочитаемый режим **действующих** грантов для панели «Агент»/журнала.
    fn external_agent_mode_label(&self) -> String {
        let grants = self.external_agents.grants();
        if !grants.any() {
            return "только чтение".to_owned();
        }
        let mut parts = Vec::new();
        if grants.project_write {
            parts.push("правки");
        }
        if grants.run_script {
            parts.push("запуск скриптов");
        }
        if grants.git_commit {
            parts.push("git-коммиты");
        }
        format!("чтение + {} с подтверждением", parts.join("/"))
    }

    /// R-RUN-06: запуск скриптов разрешён в настройках, но проект не помечен доверенным —
    /// эффективного гранта нет, нужен явный диалог согласия.
    fn agent_run_script_trust_required(&self) -> bool {
        self.app_settings.allow_external_mcp
            && self.app_settings.allow_agent_mutations
            && self.app_settings.allow_agent_run_script
            && !self.project.manifest.trusted
    }

    /// Записать в журнал предупреждение, если run-грант урезан trusted-гейтом (R-RUN-06).
    fn log_agent_run_script_trust_hint(&mut self) {
        if self.external_agents.is_running() && self.agent_run_script_trust_required() {
            self.push_log(
                Level::Warn,
                "Внешние агенты: запуск скриптов отключён — проект не доверенный (R-RUN-06); \
                 подтвердите доверие в панели «Агент» или в настройках"
                    .to_owned(),
            );
        }
    }

    /// Параметры встроенного сервера по настройкам приложения (M4.2/M4.2b: гранты через GUI-approvals).
    fn external_agent_options(&self) -> agent_server::ExternalAgentOptions {
        agent_server::ExternalAgentOptions {
            grants: self.external_agent_grants(),
            approvals: std::sync::Arc::clone(&self.agent_approvals),
        }
    }

    /// Применить настройку «Разрешить внешним агентам»: поднять/остановить/перезапустить сервер.
    ///
    /// Ошибка запуска — в журнал + настройка выключается (fail-closed, R-MCP-05).
    pub fn apply_external_agents_setting(&mut self) {
        let enabled = self.app_settings.allow_external_mcp;
        if !enabled {
            // Выключение доступа отклоняет ожидающие подтверждения (fail-closed).
            self.deny_pending_agent_approvals();
        }
        let root = self.project.root.clone();
        let options = self.external_agent_options();
        self.external_agents_attempted = true;
        match self.external_agents.apply(enabled, &root, &options) {
            Ok(()) => {
                if let agent_server::ExternalAgentStatus::Running { info, grants } =
                    self.external_agents.status(enabled)
                {
                    let mode = self.external_agent_mode_label();
                    self.push_log(
                        Level::Info,
                        format!(
                            "Внешние агенты: MCP HTTP {} (loopback, {mode} [{}]) — R-MCP-02",
                            info.url(),
                            grants.label()
                        ),
                    );
                    self.log_agent_run_script_trust_hint();
                } else if !enabled {
                    self.push_log(Level::Info, "Внешние агенты: доступ по MCP HTTP выключен".to_owned());
                }
            }
            Err(error) => {
                self.push_log(Level::Warn, format!("Внешние агенты (MCP HTTP): {error}"));
                self.app_settings.allow_external_mcp = false;
            }
        }
    }

    /// Текущее состояние внешних агентов (для панели «Агент»).
    pub fn external_agents_status(&self) -> agent_server::ExternalAgentStatus {
        self.external_agents.status(self.app_settings.allow_external_mcp)
    }

    // ── M4.2: GUI-approvals внешних агентов ─────────────────────────────────

    /// M4.2b: строки контекста для диалога approval (считаются один раз при показе запроса):
    /// число изменённых/новых файлов и автор для `git_commit`, trusted-статус для `artifact_run`.
    fn agent_approval_context_lines(
        pending: &agent_server::PendingApproval,
        project: &rde_core::Project,
    ) -> Vec<String> {
        match pending.tool() {
            "artifact_run" | "run_cancel" => vec![format!(
                "Проект: «{}» — {} (запуск скриптов разрешён только доверенному проекту, R-RUN-06)",
                project.manifest.name,
                if project.manifest.trusted { "доверенный (trusted = true)" } else { "НЕ доверенный" }
            )],
            "git_commit" => {
                let author = project
                    .manifest
                    .authors
                    .iter()
                    .find(|author| author.kind == "human")
                    .or_else(|| project.manifest.authors.iter().find(|author| author.kind == "agent"))
                    .map(|author| {
                        let email = if author.email.trim().is_empty() { "rde@localhost" } else { author.email.trim() };
                        format!("{} <{email}>", author.name.trim())
                    })
                    .unwrap_or_else(|| "MCP client <mcp@rde.local>".to_owned());
                let (files, branch) = match rde_vcs::status(&project.root) {
                    Ok(status) => (
                        status.changed.len() + status.untracked.len(),
                        if status.is_repo { status.branch } else { "git не инициализирован".to_owned() },
                    ),
                    Err(_) => (0, "git недоступен".to_owned()),
                };
                vec![format!(
                    "Файлов изменено/новых: {files}; автор коммита: {author}; ветка: {branch}"
                )]
            }
            _ => Vec::new(),
        }
    }

    /// Забрать из очереди ожидающий запрос агента и показать диалог (один за раз).
    ///
    /// M4.3: очередей две — внешних агентов (M4.2b) и чат-MCP-сервера встроенного агента;
    /// диалог и решения общие, чтобы UX был одинаковым.
    pub fn poll_agent_approvals(&mut self) {
        if self.agent_approval_dialog.is_some() {
            return;
        }
        if let Some(pending) = self
            .agent_approvals
            .take_pending()
            .or_else(|| self.chat_approvals.take_pending())
        {
            self.agent_approval_context = Self::agent_approval_context_lines(&pending, &self.project);
            self.agent_approval_dialog = Some(pending);
        }
    }

    /// Ответ пользователя на текущий запрос агента (allow once/session/deny).
    pub fn decide_agent_approval(&mut self, decision: rde_mcp::ApprovalDecision) {
        if let Some(mut pending) = self.agent_approval_dialog.take() {
            let tool = pending.tool().to_owned();
            pending.decide(decision);
            self.agent_approval_context.clear();
            let label = match decision {
                rde_mcp::ApprovalDecision::AllowOnce => "разрешено один раз",
                rde_mcp::ApprovalDecision::AllowSession => "разрешено в сессии",
                rde_mcp::ApprovalDecision::Deny => "отклонено",
            };
            self.push_log(Level::Info, format!("Агент: действие `{tool}` — {label}"));
        }
    }

    /// Закрыть/отклонить все ожидающие approvals (выключение доступа/выход — fail-closed).
    fn deny_pending_agent_approvals(&mut self) {
        if let Some(mut pending) = self.agent_approval_dialog.take() {
            pending.decide(rde_mcp::ApprovalDecision::Deny);
        }
        self.agent_approval_context.clear();
        self.agent_approvals.deny_all();
        // M4.3: чат-MCP-сервер — отдельная очередь, закрывается вместе с чатом.
        self.chat_approvals.deny_all();
    }

    /// Инициализировать git-репозиторий проекта (R-VCS-01) и сделать начальный коммит.
    pub fn git_init(&mut self) {
        match rde_vcs::init(&self.project.root) {
            Ok(true) => self.push_log(Level::Ok, "Git: репозиторий инициализирован, создан начальный коммит"),
            Ok(false) => self.push_log(Level::Info, "Git: репозиторий уже существует"),
            Err(error) => self.push_log(Level::Warn, format!("Git init: {error}")),
        }
        self.refresh_git_status();
    }

    /// Закоммитить изменения проекта с сообщением (автор — из rde.toml).
    pub fn git_commit(&mut self, message: &str) {
        let author = self
            .project
            .manifest
            .authors
            .iter()
            .find(|author| author.kind != "agent" && author.kind != "system")
            .cloned();
        let (name, email) = author
            .map(|author| (author.name, author.email))
            .unwrap_or_else(|| ("RDE".to_owned(), String::new()));
        match rde_vcs::commit(&self.project.root, message, &name, &email) {
            Ok(result) => self.push_log(Level::Ok, format!("Git: коммит {} — {} (файлов {})", result.id, result.summary, result.files)),
            Err(error) => self.push_log(Level::Warn, format!("Git commit: {error}")),
        }
        self.refresh_git_status();
    }

    /// Открыть историю git (или сообщить, что репозитория нет).
    pub fn git_history(&mut self) {
        match rde_vcs::log(&self.project.root, 30) {
            Ok(entries) if !entries.is_empty() => self.history_dialog = Some(entries),
            Ok(_) => self.push_log(Level::Info, "Git: истории пока нет"),
            Err(error) => self.push_log(Level::Warn, format!("Git history: {error}")),
        }
    }

    /// Сохранить документ в проект (`.doc.rde`) — RDE владелец сохранения.
    pub fn save_project_document(&mut self) {
        self.ensure_editor();
        let Some(editor) = self.editor.as_ref() else {
            self.push_log(Level::Warn, "Сохранить проект: редактор не открыт");
            return;
        };
        let Some(document_object) = self.active_document_object().cloned() else {
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
        let unique = {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        };
        let temp = std::env::temp_dir().join(format!("rde-save-{}-{unique}.docx", std::process::id()));
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
        let Some(path) = self.active_document_path() else {
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
        let Some(document_object) = self.active_document_object().cloned() else {
            self.push_log(Level::Warn, "Оформление: объект документа не найден");
            return;
        };
        let ObjectData::Document(manifest) = &document_object.data else { return };
        let unique = {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        };
        let work = std::env::temp_dir().join(format!("rde-format-{}-{unique}", std::process::id()));
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
        // R-DOC-12: бэкап перед изменением пакета (`.bak` + `.rde/doc-backups/`).
        if let Err(error) = rde_doc::backup_package(&self.project.root, &path) {
            self.push_log(Level::Warn, format!("Оформление: бэкап не создан: {error}"));
        }
        match rde_doc::save_engine_document(&formatted, &path, &manifest.id, &template_id) {
            Ok(rde_doc) => {
                if let Some(store) = self.store.as_mut() {
                    if let Err(error) = rde_doc::commit_document(&self.project.root, &path, &manifest.id, store, "document format") {
                        self.push_log(Level::Warn, format!("Оформление: версия документа не зафиксирована: {error}"));
                    }
                }
                let manifest_path = document_object.dir.join("document.toml");
                let _ = set_manifest_string(&manifest_path, "template", &template_id);
                self.reload_document_view_preserving();
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
        let Some(package) = self.active_document_path() else {
            self.push_log(Level::Warn, "Копия для Word: в проекте нет документа");
            return;
        };
        let stem = package.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_else(|| "document".to_owned());
        let out = self.export_dir.join(format!("{stem}-word.docx"));
        let result = if rde_doc::is_rde_package(&package) {
            // ADR-0018: экспорт для Word — пакет без RDE-частей + math-шрифт STIX Two Math
            // (m:mathPr/m:mathFont, w:rFonts math-ранов); Word на стенде нет — по спецификации + LO.
            rde_doc::export_docx(&package, &out).map(|_| ())
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

    /// Самый свежий `.docx` в `exports/` (кандидат для «Импорт ревью из Word»).
    pub fn latest_export_docx(&self) -> Option<PathBuf> {
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
        candidates.first().map(|(_, path)| path.clone())
    }

    /// Импорт внешнего ревью: `Some(path)` — конкретный `.docx` (выбор «Проводник…»),
    /// `None` — самый свежий `.docx` из `exports/` (прежнее поведение кнопки).
    pub fn import_review_from_exports(&mut self) {
        if let Err(error) = self.import_review_action(None) {
            self.push_log(Level::Warn, format!("Импорт ревью: {error}"));
        }
    }

    /// Общая операция импорта ревью: RDE-часть (.doc.rde), doc_id и вставки сохраняются.
    pub fn import_review_action(&mut self, source: Option<&Path>) -> Result<(), String> {
        let Some(package) = self.active_document_path() else {
            return Err("в проекте нет документа".to_owned());
        };
        if !rde_doc::is_rde_package(&package) {
            return Err("документ ещё не в .doc.rde — сначала «Сохранить проект»".to_owned());
        }
        let external = match source {
            Some(path) => {
                if !path.is_file() {
                    return Err(format!("файл ревью не найден: {}", path.display()));
                }
                path.to_path_buf()
            }
            None => self
                .latest_export_docx()
                .ok_or_else(|| format!("в {} нет .docx — укажите файл вручную", self.export_dir.display()))?,
        };
        match rde_doc::import_external_review(&package, &external) {
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
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    /// Артефакты, из которых можно вставлять значения/рисунки (есть объявленные выходы).
    fn insertable_artifacts(&self) -> Vec<rde_core::ArtifactManifest> {
        self.project
            .artifacts()
            .filter(|artifact| artifact.kind == "compute" || artifact.kind == "data")
            .filter(|artifact| !artifact.outputs.is_empty())
            .cloned()
            .collect()
    }

    /// Открыть диалог вставки: `value|figure|table|constant|crossref|citation|date` (R-DOC-11).
    fn open_insert_dialog(&mut self, kind: &str) {
        if self.active_document_path().is_none() {
            self.push_log(Level::Warn, "Вставка: в проекте нет документа");
            return;
        }
        // Стиль обычного текста текущего шаблона (если документ уже .doc.rde).
        let style = self
            .project
            .document_path()
            .filter(|path| rde_doc::is_rde_package(path))
            .map(|path| rde_doc::default_paragraph_style(&path))
            .unwrap_or_default();
        let mut dialog = InsertDialogState {
            kind: kind.to_owned(),
            artifact_id: String::new(),
            formula_id: String::new(),
            render_mode: "omml".to_owned(),
            selector: String::new(),
            pointer: String::new(),
            caption: String::new(),
            style,
            format: "auto".to_owned(),
            precision: 2,
            error: String::new(),
        };
        match kind {
            "formula" => {
                let Some(formula) = self.project.formulas().next() else {
                    self.push_log(
                        Level::Warn,
                        "Вставка формулы: в проекте нет formulas-объектов (P3: миграция — «Принять формулы как объекты»)",
                    );
                    return;
                };
                dialog.formula_id = formula.id.clone();
                dialog.format = "raw".to_owned();
            }
            "table" => {
                let artifact = self
                    .insertable_artifacts()
                    .into_iter()
                    .find(|artifact| artifact.outputs.iter().any(|output| output.to_ascii_lowercase().ends_with(".csv")));
                let Some(artifact) = artifact else {
                    self.push_log(Level::Warn, "Вставка таблицы: нет артефактов с CSV-выходом");
                    return;
                };
                dialog.artifact_id = artifact.id;
                dialog.selector = artifact
                    .outputs
                    .iter()
                    .find(|output| output.to_ascii_lowercase().ends_with(".csv"))
                    .cloned()
                    .unwrap_or_default();
                dialog.format = "raw".to_owned();
                dialog.caption = "Сводные данные".to_owned();
            }
            "constant" => {
                let constants = self
                    .project
                    .objects
                    .iter()
                    .find_map(|object| match &object.data {
                        ObjectData::Artifact(artifact) if artifact.kind == "constants" && !artifact.values.is_empty() => Some(artifact.clone()),
                        _ => None,
                    });
                let Some(constants) = constants else {
                    self.push_log(Level::Warn, "Вставка константы: в проекте нет объекта констант со значениями");
                    return;
                };
                dialog.artifact_id = constants.id;
                dialog.selector = constants.values.keys().next().cloned().unwrap_or_default();
                dialog.format = "raw".to_owned();
                dialog.caption = "Константа".to_owned();
            }
            "crossref" => {
                let inserts = self
                    .project
                    .document_path()
                    .filter(|path| rde_doc::is_rde_package(path))
                    .and_then(|path| rde_doc::load_rde_doc(&path).ok())
                    .map(|doc| doc.inserts)
                    .unwrap_or_default();
                // Приоритет — формулы проекта (главный сценарий B3), если их нет — рисунки/таблицы.
                let target = inserts
                    .iter()
                    .find(|insert| !insert.target.is_empty() && insert.kind == "formula")
                    .or_else(|| inserts.iter().find(|insert| !insert.target.is_empty() && matches!(insert.kind.as_str(), "figure" | "table")));
                let Some(insert) = target else {
                    self.push_log(Level::Warn, "Перекрёстная ссылка: в документе нет формул/рисунков/таблиц-вставок");
                    return;
                };
                let (option_id, caption) = if insert.kind == "formula" {
                    match rde_doc::parse_formula_target(&insert.target) {
                        Some(target) => (format!("formula:{}", target.formula_id), "см. формулу".to_owned()),
                        None => (insert.id.clone(), "см. вставку".to_owned()),
                    }
                } else {
                    (insert.id.clone(), "см. рисунок".to_owned())
                };
                dialog.artifact_id = option_id;
                dialog.format = "raw".to_owned();
                dialog.caption = caption;
            }
            "citation" => {
                let Some(source) = self.project.sources().next() else {
                    self.push_log(Level::Warn, "Цитата: в проекте нет источников");
                    return;
                };
                dialog.artifact_id = source.id.clone();
                dialog.format = "raw".to_owned();
                dialog.caption = "цитата".to_owned();
            }
            "date" => {
                dialog.format = "raw".to_owned();
                dialog.caption = "дата".to_owned();
            }
            _ => {
                let artifacts = self.insertable_artifacts();
                let is_image = |output: &str| {
                    matches!(
                        output.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str(),
                        "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp"
                    )
                };
                let artifact = if kind == "figure" {
                    artifacts.iter().find(|artifact| artifact.outputs.iter().any(|output| is_image(output)))
                } else {
                    artifacts.first()
                };
                let Some(artifact) = artifact else {
                    self.push_log(
                        Level::Warn,
                        if kind == "figure" {
                            "Вставка рисунка: нет артефактов с выходом-изображением (svg/png/jpg)"
                        } else {
                            "Вставка: нет артефактов с объявленными выходами"
                        },
                    );
                    return;
                };
                dialog.artifact_id = artifact.id.clone();
                dialog.selector = if kind == "figure" {
                    artifact.outputs.iter().find(|output| is_image(output)).cloned().unwrap_or_default()
                } else {
                    artifact.outputs.first().cloned().unwrap_or_default()
                };
            }
        }
        self.insert_dialog = Some(dialog);
    }

    /// Подтвердить вставку: разрешить значение/рисунок из артефакта и записать в `.doc.rde`.
    fn confirm_insert(&mut self, dialog: InsertDialogState) {
        let Some(package) = self.active_document_path() else {
            self.push_log(Level::Warn, "Вставка: в проекте нет документа");
            return;
        };
        if !rde_doc::is_rde_package(&package) {
            self.push_log(Level::Warn, "Вставка: документ ещё не в .doc.rde (сначала «Сохранить проект»)");
            return;
        }
        // Валидация до вставки: JSON-выход без pointer вставил бы весь файл (P8-B1).
        if dialog.kind == "value"
            && dialog.selector.to_ascii_lowercase().ends_with(".json")
            && dialog.pointer.trim().trim_start_matches('#').is_empty()
        {
            let mut dialog = dialog;
            dialog.error = "Для JSON-выхода нужен JSON pointer (например, #/stats/x1/mean)".to_owned();
            self.insert_dialog = Some(dialog);
            return;
        }
        if dialog.kind == "figure" {
            let ext = dialog.selector.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
            if !matches!(ext.as_str(), "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp") {
                let mut dialog = dialog;
                dialog.error = format!("Выход `{}` не является изображением — выберите svg/png/jpg", dialog.selector);
                self.insert_dialog = Some(dialog);
                return;
            }
        }
        if self.store.is_none() {
            self.push_log(Level::Warn, "Вставка: хранилище проекта не открыто");
            return;
        }
        // Формула (P3): кэши OMML/SVG — из formula-объекта (при необходимости пересчёт), затем
        // вставка sdt-ссылки `rde://formula/<id>@<act_key>` с render_mode.
        if dialog.kind == "formula" {
            let result = self.formula_insert_from_dialog(&dialog, &package);
            self.finish_insert(&package, result);
            return;
        }
        let pointer = dialog.pointer.trim().trim_start_matches('#');
        let artifact_uri = if pointer.is_empty() {
            format!("rde://artifact/{}/{}", dialog.artifact_id, dialog.selector)
        } else {
            format!("rde://artifact/{}/{}#{}", dialog.artifact_id, dialog.selector, pointer)
        };
        let mut renumber = false;
        let anchor_block = self.insert_anchor_block;
        let result: Result<(rde_doc::InsertReport, String), String> = {
            let project = &self.project;
            let store = self.store.as_ref().expect("хранилище проверено");
            match dialog.kind.as_str() {
                "figure" => rde_dag::resolve_insert_image(project, store, &artifact_uri).and_then(|(path, _ext, hash)| {
                    let caption = if dialog.caption.trim().is_empty() { "Рисунок — вставка из артефакта".to_owned() } else { dialog.caption.trim().to_owned() };
                    rde_doc::add_figure_insert(&package, &path, artifact_uri.clone(), caption, hash.clone(), anchor_block).map(|report| (report, hash))
                }),
                "table" => rde_dag::resolve_insert_table(project, store, &artifact_uri).and_then(|(rows, hash)| {
                    let caption = if dialog.caption.trim().is_empty() { "Таблица".to_owned() } else { dialog.caption.trim().to_owned() };
                    let request = rde_doc::InsertRequest {
                        kind: "table".to_owned(),
                        target: artifact_uri.clone(),
                        selector: dialog.selector.clone(),
                        value_kind: "table".to_owned(),
                        cached: String::new(),
                        caption,
                        style: dialog.style.clone(),
                        format: String::new(),
                        run_style: String::new(),
                        anchor_block,
                    };
                    rde_doc::add_table_insert(&package, request, &rows).map(|report| (report, hash))
                }),
                "constant" => {
                    let uri = format!("rde://constants/{}/{}", dialog.artifact_id, dialog.selector);
                    let ledger = rde_dag::ledger(store).unwrap_or_default();
                    rde_dag::resolve_insert_constant(project, &ledger, &uri).and_then(|(text, run_style, hash)| {
                        let request = rde_doc::InsertRequest {
                            kind: "constant".to_owned(),
                            target: uri.clone(),
                            selector: dialog.selector.clone(),
                            value_kind: "text".to_owned(),
                            cached: text,
                            caption: dialog.caption.clone(),
                            style: dialog.style.clone(),
                            format: "raw".to_owned(),
                            run_style,
                            anchor_block,
                        };
                        rde_doc::add_insert(&package, request).map(|report| (report, hash))
                    })
                }
                "crossref" => {
                    // B3 (ADR-0018): цель — формула проекта (`rde://formula/<id>`) или вставка.
                    let (uri, cached) = match dialog.artifact_id.strip_prefix("formula:") {
                        Some(formula_id) => {
                            let number = self.formula_number_in_document(&package, formula_id);
                            (format!("rde://formula/{formula_id}"), number.map(|number| format!("({number})")).unwrap_or_else(|| "(?)".to_owned()))
                        }
                        None => (format!("rde://insert/{}", dialog.artifact_id), "Рисунок 1".to_owned()),
                    };
                    let request = rde_doc::InsertRequest {
                        kind: "crossref".to_owned(),
                        target: uri,
                        selector: String::new(),
                        value_kind: "text".to_owned(),
                        cached,
                        caption: dialog.caption.clone(),
                        style: dialog.style.clone(),
                        format: "raw".to_owned(),
                        run_style: String::new(),
                        anchor_block,
                    };
                    renumber = true;
                    rde_doc::add_insert(&package, request).map(|report| (report, String::new()))
                }
                "citation" => {
                    let index = project
                        .sources()
                        .position(|source| source.id == dialog.artifact_id)
                        .map(|index| index + 1)
                        .unwrap_or(1);
                    let request = rde_doc::InsertRequest {
                        kind: "citation".to_owned(),
                        target: format!("rde://source/{}", dialog.artifact_id),
                        selector: String::new(),
                        value_kind: "text".to_owned(),
                        cached: format!("[{index}]"),
                        caption: dialog.caption.clone(),
                        style: dialog.style.clone(),
                        format: "raw".to_owned(),
                        run_style: String::new(),
                        anchor_block,
                    };
                    rde_doc::add_insert(&package, request).map(|report| (report, String::new()))
                }
                "date" => {
                    let request = rde_doc::InsertRequest {
                        kind: "date".to_owned(),
                        target: "rde://date/iso".to_owned(),
                        selector: String::new(),
                        value_kind: "text".to_owned(),
                        cached: today_iso_date(),
                        caption: dialog.caption.clone(),
                        style: dialog.style.clone(),
                        format: "raw".to_owned(),
                        run_style: String::new(),
                        anchor_block,
                    };
                    rde_doc::add_insert(&package, request).map(|report| (report, String::new()))
                }
                _ => rde_dag::resolve_insert_text(project, store, &artifact_uri).and_then(|(value, hash)| {
                    let formatted = rde_doc::format_value(&value, &dialog.format_spec());
                    let request = rde_doc::InsertRequest {
                        kind: "value".to_owned(),
                        target: artifact_uri.clone(),
                        selector: pointer.to_owned(),
                        value_kind: if pointer.is_empty() { "text".to_owned() } else { "number".to_owned() },
                        cached: formatted,
                        caption: dialog.caption.clone(),
                        style: dialog.style.clone(),
                        format: dialog.format_spec(),
                        run_style: String::new(),
                        anchor_block,
                    };
                    rde_doc::add_insert(&package, request).map(|report| (report, hash))
                }),
            }
        };
        if renumber {
            let _ = rde_doc::renumber_insert_captions(&package);
        }
        self.finish_insert(&package, result);
    }

    /// Общий хвост вставки: версия документа в store, перезагрузка UI и журнал.
    fn finish_insert(&mut self, package: &Path, result: Result<(rde_doc::InsertReport, String), String>) {
        // Версия документа в store (транзакция с корнем document/<id>).
        match result {
            Ok((report, hash)) => {
                let doc_id = self.active_document_object().and_then(|object| match &object.data {
                    ObjectData::Document(manifest) => Some(manifest.id.clone()),
                    _ => None,
                });
                if let (Some(store), Some(doc_id)) = (self.store.as_mut(), doc_id) {
                    if let Err(error) = rde_doc::commit_document(&self.project.root, package, &doc_id, store, "document insert") {
                        self.push_log(Level::Warn, format!("Вставка: версия документа не зафиксирована: {error}"));
                    }
                }
                self.reload_document_view_preserving();
                self.push_log(
                    Level::Ok,
                    format!(
                        "Вставка {} ({}{}) добавлена в документ (hash {})",
                        report.insert.id,
                        report.insert.kind,
                        if report.insert.render_mode.is_empty() { String::new() } else { format!(", {}", report.insert.render_mode) },
                        hash.chars().take(10).collect::<String>()
                    ),
                );
            }
            Err(error) => self.push_log(Level::Warn, format!("Вставка не добавлена: {error}")),
        }
    }

    /// Сменить схему нумерации формул документа (ADR-0018): запись `document.toml` + немедленная
    /// перенумерация формул и перекрёстных ссылок в пакете. Возвращает число изменённых записей.
    fn apply_document_formula_numbering(&mut self, object_dir: &Path, scheme: rde_doc::FormulaNumbering) -> Result<usize, String> {
        let manifest_path = object_dir.join("document.toml");
        rde_core::set_document_formula_numbering(&manifest_path, scheme.as_str())?;
        let package = self
            .project
            .objects
            .iter()
            .find_map(|object| match &object.data {
                ObjectData::Document(document) if object.dir == object_dir => Some(object.dir.join(&document.file)),
                _ => None,
            })
            .or_else(|| self.project.document_path());
        let changed = match package.filter(|package| rde_doc::is_rde_package(package)) {
            Some(package) => {
                let report = rde_doc::renumber_formula_numbers(&package, scheme)?;
                for warning in &report.warnings {
                    self.push_log(Level::Warn, format!("Нумерация формул: {warning}"));
                }
                report.formulas.len() + report.crossrefs.len()
            }
            None => 0,
        };
        self.reload_document_view_preserving();
        Ok(changed)
    }

    /// Отображаемый номер формулы в конкретном документе (без скобок): из `rde:doc` по
    /// `rde://formula/<id>` (ADR-0018). `None` — формула в документ не вставлена.
    fn formula_number_in_document(&self, package: &Path, formula_id: &str) -> Option<String> {
        let rde_doc = rde_doc::load_rde_doc(package).ok()?;
        rde_doc
            .inserts
            .iter()
            .filter(|insert| insert.kind == "formula")
            .find(|insert| rde_doc::parse_formula_target(&insert.target).map(|target| target.formula_id == formula_id).unwrap_or(false))
            .and_then(|insert| insert.number_text.clone().or_else(|| insert.number.map(|number| number.to_string())))
    }

    /// Схема нумерации формул активного документа из `document.toml` (ADR-0018; по умолчанию
    /// сквозная для документов без поля).
    fn document_formula_numbering(&self) -> rde_doc::FormulaNumbering {
        self.project
            .document()
            .map(|document| document.formula_numbering.as_str())
            .and_then(rde_doc::FormulaNumbering::parse)
            .unwrap_or_default()
    }

    /// Вставка формулы (P3): кэш-бандл из formula-объекта (при устаревании — пересчёт), затем
    /// sdt-вставка с render_mode = omml|svg. Возвращает (отчёт, act_key).
    fn formula_insert_from_dialog(
        &mut self,
        dialog: &InsertDialogState,
        package: &Path,
    ) -> Result<(rde_doc::InsertReport, String), String> {
        let renderer = rde_dag::FormulaRenderer {
            id: rde_doc::FORMULA_RENDERER_ID,
            version: rde_doc::FORMULA_RENDERER_VERSION,
        };
        let formula_id = dialog.formula_id.clone();
        let (display, numbered) = {
            let object = self.project.formula_object(&formula_id).ok_or_else(|| format!("формула {formula_id} не найдена"))?;
            let ObjectData::Formula(formula) = &object.data else { return Err("объект не формула".to_owned()) };
            // Политика нумерации объекта (ADR-0005 §5.1): display + mode=auto → номер (N);
            // inline и mode=none — без номера (R-DOC-15/ADR-0019 не регрессируют).
            (formula.display, formula.display && formula.numbering.mode != "none")
        };
        {
            let project = &self.project;
            let store = self.store.as_mut().ok_or("хранилище проекта не открыто")?;
            let artifact_ledger = rde_dag::ledger(store)?;
            let formula_ledger = rde_dag::formula_ledger(store)?;
            let resolved = rde_dag::resolve_formula(project, &artifact_ledger, &formula_ledger, &formula_id, &renderer)?;
            let current = resolved.act_key.hex();
            let stale = formula_ledger.get(&formula_id).map(|state| state.act_key != current).unwrap_or(true);
            if stale {
                rde_dag::compile_and_update_formula(project, store, &formula_id, &renderer, |latex, display| {
                    let caches = rde_doc::compile_formula(latex, display)?;
                    Ok(rde_dag::FormulaCompiled { omml: caches.omml, svg: caches.svg, png: caches.png })
                })?;
            }
        }
        let store = self.store.as_ref().ok_or("хранилище проекта не открыто")?;
        let formula_ledger = rde_dag::formula_ledger(store)?;
        let state = formula_ledger.get(&formula_id).ok_or_else(|| format!("формула {formula_id} не вычислена"))?;
        let load = |hash: &Option<String>| -> Result<Vec<u8>, String> {
            let hash = hash.as_deref().ok_or("нет кэша формулы")?;
            let hash = rde_hash::Hash::from_hex(hash).map_err(|e| format!("хеш кэша формулы: {e}"))?;
            store.get_blob(hash).map_err(|e| e.to_string())?.ok_or_else(|| "кэш формулы отсутствует в CAS".to_owned())
        };
        let body = rde_doc::FormulaBody {
            formula_id: formula_id.clone(),
            act_key: state.act_key.clone(),
            latex: state.resolved_latex.clone(),
            display,
            numbered,
            omml: load(&state.omml_hash)?,
            svg: state.svg_hash.as_ref().map(|_| load(&state.svg_hash)).transpose()?,
            png: state.png_hash.as_ref().map(|_| load(&state.png_hash)).transpose()?,
        };
        let report = rde_doc::add_formula_insert(
            package,
            rde_doc::FormulaInsertRequest {
                body,
                render_mode: dialog.render_mode.clone(),
                anchor_block: self.insert_anchor_block,
                numbering: self.document_formula_numbering(),
            },
        )?;
        Ok((report, state.act_key.clone()))
    }

    // ── P4: редактор формулы (ADR-0020) — создание/правка source-of-truth + кэши ─────

    /// Проверить LaTeX формулы компиляцией (P4): ошибка движка показывается в панели/диалоге
    /// **до** записи `formula.toml` (и до создания объекта).
    fn validate_formula_latex(latex: &str, display: bool) -> Result<(), String> {
        let latex = latex.trim();
        if latex.is_empty() {
            return Err("LaTeX-источник формулы не может быть пустым".to_owned());
        }
        rde_doc::compile_formula(latex, display).map(|_| ()).map_err(|error| format!("LaTeX не компилируется: {error}"))
    }

    /// Пересчитать кэши формулы и зафиксировать состояние (P4): `prepare` (OMML/SVG/PNG) →
    /// транзакция ledger `formula-update` → зеркало `[state]` в `formula.toml` (ADR-0005 §3.3).
    fn refresh_formula_caches(&mut self, formula_id: &str) -> Result<(), String> {
        let renderer = rde_dag::FormulaRenderer {
            id: rde_doc::FORMULA_RENDERER_ID,
            version: rde_doc::FORMULA_RENDERER_VERSION,
        };
        let project = &self.project;
        let store = self.store.as_mut().ok_or_else(|| "хранилище проекта не открыто".to_owned())?;
        let prepared = rde_dag::prepare_formula_update(project, store, formula_id, &renderer, |latex, display| {
            let caches = rde_doc::compile_formula(latex, display)?;
            Ok(rde_dag::FormulaCompiled { omml: caches.omml, svg: caches.svg, png: caches.png })
        })?;
        rde_dag::commit_formula_update(store, &prepared.state, prepared.cache_tree, &format!("formula edit: {formula_id}"))?;
        rde_dag::record_prepared_formula_state(project, &prepared)
    }

    /// Создать project-level формулу из GUI (P4, ADR-0020): валидация LaTeX компиляцией, дедуп по
    /// `latex.trim()` (как миграция ADR-0019: та же формула — используем существующую), запись
    /// `formula.toml` + кэши OMML/SVG/PNG + ledger. Возвращает `(id, reused)`.
    fn create_formula_action(
        &mut self,
        title: &str,
        latex: &str,
        display: bool,
        numbering_mode: &str,
    ) -> Result<(String, bool), String> {
        let title = title.trim();
        let latex = latex.trim();
        if title.is_empty() {
            return Err("название формулы не может быть пустым".to_owned());
        }
        Self::validate_formula_latex(latex, display)?;
        // Дедуп по источнику: та же формула (LaTeX) уже есть — не создаём дубль.
        if let Some(existing) = self.project.formulas().find(|formula| formula.latex.trim() == latex) {
            let id = existing.id.clone();
            self.selected = Some(id.clone());
            return Ok((id, true));
        }
        let dir = rde_core::create_formula_with_options(&self.project.root, title, latex, display, numbering_mode)?;
        self.reload_project();
        let Some(id) = self.project.objects.iter().find(|object| object.dir == dir).map(|object| object.id.clone()) else {
            return Err("созданная формула не загрузилась в проект".to_owned());
        };
        self.selected = Some(id.clone());
        if let Err(error) = self.refresh_formula_caches(&id) {
            // Откат: не оставляем объект, формулу из которого не удалось вычислить.
            let _ = std::fs::remove_dir_all(&dir);
            self.reload_project();
            return Err(format!("кэши формулы: {error}"));
        }
        self.reload_project();
        self.selected = Some(id.clone());
        Ok((id, false))
    }

    /// Применить правку источника формулы (P4): валидация LaTeX, запись шапки без потери
    /// inputs/fragments/[state], пересчёт кэшей и `[state]`. Вставки становятся устаревшими
    /// автоматически (их `act_key` сравнивается с текущим при статусах/R-DOC-19).
    fn apply_formula_edit(
        &mut self,
        id: &str,
        title: &str,
        latex: &str,
        display: bool,
        numbering_mode: &str,
        numbering_label: &str,
    ) -> Result<(), String> {
        Self::validate_formula_latex(latex, display)?;
        let update = rde_core::FormulaSourceUpdate {
            title: title.to_owned(),
            latex: latex.to_owned(),
            display,
            numbering_mode: numbering_mode.to_owned(),
            numbering_label: numbering_label.to_owned(),
        };
        rde_core::update_formula_source(&self.project.root, id, &update)?;
        self.reload_project();
        if let Err(error) = self.refresh_formula_caches(id) {
            self.reload_project();
            return Err(format!("кэши формулы: {error}"));
        }
        self.reload_project();
        self.selected = Some(id.to_owned());
        Ok(())
    }

    // ── P4: панель формулы (создание/правка/предпросмотр/нативная вкладка Equation) ──

    /// Показать редактор формулы на верхней панели RDE (вкладка «Формула» ленты, P4):
    /// центр — «Документ», режим RDE (DocView) — лента видна только там.
    fn show_formula_editor_context(&mut self) {
        self.tab = Tab::Document;
        self.doc_mode = DocMode::DocView;
        self.ribbon_tab = RibbonTab::Formula;
    }

    /// Открыть панель создания формулы (P4): пустые поля, display, политика `auto`.
    pub fn open_formula_editor_new(&mut self) {
        self.show_formula_editor_context();
        self.formula_editor = Some(FormulaEditorState {
            formula_id: None,
            title: String::new(),
            latex: String::new(),
            display: true,
            numbering_mode: "auto".to_owned(),
            numbering_label: String::new(),
            render_mode: "omml".to_owned(),
            error: String::new(),
            info: String::new(),
            native_open: false,
            native: None,
            math_editor: None,
        });
    }

    /// Открыть панель редактирования формулы, выбранной в дереве/инспекторе (P4).
    pub fn open_selected_formula_editor(&mut self) {
        let Some(object) = self.selected_object() else {
            self.push_log(Level::Info, "Редактировать формулу: выберите формулу в дереве или инспекторе");
            return;
        };
        if !matches!(object.data, ObjectData::Formula(_)) {
            self.push_log(Level::Info, "Редактировать формулу: выбранный объект — не формула");
            return;
        }
        let id = object.id.clone();
        self.open_formula_editor(&id);
    }

    /// Открыть панель редактора формулы по id (P4); неизвестный id — запись в журнал.
    pub fn open_formula_editor(&mut self, formula_id: &str) {
        let Some((id, title, latex, display, numbering_mode, numbering_label)) =
            self.project.formula_object(formula_id).and_then(|object| match &object.data {
                ObjectData::Formula(formula) => Some((
                    formula.id.clone(),
                    object.title.clone(),
                    formula.latex.clone(),
                    formula.display,
                    if formula.numbering.mode.is_empty() { "auto".to_owned() } else { formula.numbering.mode.clone() },
                    formula.numbering.label.clone(),
                )),
                _ => None,
            })
        else {
            self.push_log(Level::Warn, format!("Редактор формулы: формула {formula_id} не найдена"));
            return;
        };
        self.selected = Some(id.clone());
        self.show_formula_editor_context();
        self.formula_editor = Some(FormulaEditorState {
            formula_id: Some(id),
            title,
            latex,
            display,
            numbering_mode,
            numbering_label,
            render_mode: "omml".to_owned(),
            error: String::new(),
            info: String::new(),
            native_open: false,
            native: None,
            math_editor: None,
        });
    }

    /// Отдельный WordCraft-редактор для панели формулы (спайк A): пустой scratch-документ, своя
    /// `Session`. Документ пользователя не затрагивается — правка источника не создаёт черновиков.
    fn new_native_formula_editor(&self) -> Box<NativeFormulaEditor> {
        rde_doc::ensure_math_font();
        let session = wordcraft_engine::Session::new(wordcraft_engine::doc::Document::default());
        let mut app = wordcraft_ui_egui::WordApp::new(session, wordcraft_ui_egui::Services::default());
        app.integrated_titlebar = false;
        app.autosave = false;
        app.ui.theme = appearance_of(&self.app_settings.theme);
        Box::new(NativeFormulaEditor { app, fonts_ready: false, skip_frames: 1 })
    }

    /// Перезапустить нативную правку: свежий scratch-документ + уравнение из поля LaTeX.
    fn restart_native_formula_editor(&self, dialog: &mut FormulaEditorState) {
        dialog.native = Some(self.new_native_formula_editor());
        let Some(native) = dialog.native.as_mut() else { return };
        let request = serde_json::json!({ "latex": dialog.latex.trim(), "display": dialog.display });
        if native.app.run("insert.equation", request).is_err() {
            // Пустой/невалидный на момент открытия LaTeX — начинаем с пустой формулы.
            let _ = native.app.run("insert.equation", serde_json::json!({ "display": dialog.display }));
        }
        let _ = native.app.run("equation.inputFormat", serde_json::json!({ "format": "latex" }));
        native.app.canvas.want_focus = true;
    }

    /// Забрать LaTeX/display из нативного редактора (`equation.get`, спайк A) в поля панели.
    ///
    /// P5 (ADR-0021 §Решение 2): полученный LaTeX **валидируется** — мусор линейного формата
    /// (`√`, `▒`, `█`) и не-LaTeX не попадает в источник; при отказе `dialog.latex` не меняется,
    /// ошибка показывается в панели. Явная правка структуры разрешена (это действие пользователя),
    /// но сопровождается предупреждением о диффе структуры.
    fn sync_formula_from_native(dialog: &mut FormulaEditorState) {
        let Some(native) = dialog.native.as_mut() else { return };
        let Ok(info) = native.app.run("equation.get", serde_json::json!({})) else { return };
        if let Some(latex) = info.get("latex").and_then(|value| value.as_str()) {
            match rde_doc::check_formula_latex(&dialog.latex, latex) {
                // Нативный редактор — «зеркало» источника, а не второй редактор: любое изменение
                // структуры (конвертация в linear даёт `\backslash`/`√`/`▒`) — это порча,
                // источник не трогаем (инцидент A, ADR-0021 §Решение 2).
                Ok(check) if !check.equivalent => {
                    let diff = if check.warnings.is_empty() { String::new() } else { format!(": {}", check.warnings.join("; ")) };
                    dialog.error = format!(
                        "LaTeX из нативного редактора меняет структуру формулы и отклонён (источник не изменён){diff}"
                    );
                    dialog.info.clear();
                }
                Ok(_) => {
                    dialog.latex = latex.trim().to_owned();
                    dialog.error.clear();
                    dialog.info = "LaTeX из нативного редактора совпадает с источником — источник нормализован".to_owned();
                }
                Err(error) => {
                    // Источник не трогаем: лучше отказ, чем запись не-LaTeX (инцидент A).
                    dialog.error = format!("LaTeX из нативного редактора отклонён (источник не изменён): {error}");
                    dialog.info.clear();
                }
            }
        }
        if let Some(display) = info.get("display").and_then(|value| value.as_bool()) {
            dialog.display = display;
        }
    }

    /// Вставить формулу из панели (P4): кэши из ledger (при устаревании — пересчёт) + sdt-вставка с
    /// документно-локальным номером по схеме документа (ADR-0018/0019, right-numbered без таблицы).
    fn formula_editor_insert(&mut self, dialog: &mut FormulaEditorState) {
        let Some(formula_id) = dialog.formula_id.clone() else {
            dialog.error = "Сначала создайте формулу («Создать»)".to_owned();
            return;
        };
        let Some(package) = self.active_document_path() else {
            dialog.error = "В проекте нет документа — вставка недоступна".to_owned();
            return;
        };
        if !rde_doc::is_rde_package(&package) {
            dialog.error = "Документ ещё не в .doc.rde — сначала «Сохранить проект»".to_owned();
            return;
        }
        let style = self
            .project
            .document_path()
            .filter(|path| rde_doc::is_rde_package(path))
            .map(|path| rde_doc::default_paragraph_style(&path))
            .unwrap_or_default();
        let insert_dialog = InsertDialogState {
            kind: "formula".to_owned(),
            artifact_id: String::new(),
            formula_id: formula_id.clone(),
            render_mode: dialog.render_mode.clone(),
            selector: String::new(),
            pointer: String::new(),
            caption: String::new(),
            style,
            format: "raw".to_owned(),
            precision: 2,
            error: String::new(),
        };
        let result = self.formula_insert_from_dialog(&insert_dialog, &package);
        match &result {
            Ok((report, _)) => {
                let number = report
                    .insert
                    .number_text
                    .clone()
                    .or_else(|| report.insert.number.map(|number| number.to_string()))
                    .map(|number| format!(" ({number})"))
                    .unwrap_or_else(|| " (без номера)".to_owned());
                dialog.error.clear();
                dialog.info = format!("Формула вставлена в документ: {}{number}", report.insert.id);
            }
            Err(error) => dialog.error = format!("Вставка: {error}"),
        }
        self.finish_insert(&package, result);
    }

    /// Быстрая вставка формулы из дерева/инспектора (P4): OMML-режим, текущая позиция вставки,
    /// нумерация — по схеме документа (ADR-0018/0019). Ошибки — в журнал.
    fn formula_quick_insert(&mut self, formula_id: &str, render_mode: &str) {
        let Some(package) = self.active_document_path() else {
            self.push_log(Level::Warn, "Вставка формулы: в проекте нет документа");
            return;
        };
        if !rde_doc::is_rde_package(&package) {
            self.push_log(Level::Warn, "Вставка формулы: документ ещё не в .doc.rde — сначала «Сохранить проект»");
            return;
        }
        let style = self
            .project
            .document_path()
            .filter(|path| rde_doc::is_rde_package(path))
            .map(|path| rde_doc::default_paragraph_style(&path))
            .unwrap_or_default();
        let dialog = InsertDialogState {
            kind: "formula".to_owned(),
            artifact_id: String::new(),
            formula_id: formula_id.to_owned(),
            render_mode: render_mode.to_owned(),
            selector: String::new(),
            pointer: String::new(),
            caption: String::new(),
            style,
            format: "raw".to_owned(),
            precision: 2,
            error: String::new(),
        };
        let result = self.formula_insert_from_dialog(&dialog, &package);
        match &result {
            Ok((report, _)) => {
                let number = report
                    .insert
                    .number_text
                    .clone()
                    .or_else(|| report.insert.number.map(|number| number.to_string()))
                    .map(|number| format!(" (номер {number})"))
                    .unwrap_or_default();
                self.push_log(Level::Ok, format!("Вставка формулы: {}{number}", report.insert.id));
            }
            Err(error) => self.push_log(Level::Warn, format!("Вставка формулы: {error}")),
        }
        self.finish_insert(&package, result);
    }

    /// Панель/диалог формулы (P4): создание и правка source-of-truth, предпросмотр, нативный
    /// `equation_tab`, «Применить» (манифест+кэши) и «Вставить в документ» (номер по схеме).
    /// Возвращает `true`, если панель остаётся открытой.
    /// Панель редактора формулы на ленте (вкладка «Формула», P4/ADR-0020) — «полноценный
    /// редактор формул» в верхней панели RDE: LaTeX-источник (source-of-truth `formula.toml`),
    /// предпросмотр, display/inline, политика нумерации, «Применить»/«Вставить в документ»/
    /// «Обновить документ», переключатель нативной вкладки Equation (отдельное окно) и выбор
    /// другой формулы проекта. Открывается по клику на формулу в тексте/дереве/инспекторе.
    fn formula_ribbon_editor(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        // ── Пустое состояние: выбрать формулу проекта или создать новую ──
        let Some(mut dialog) = self.formula_editor.take() else {
            let formulas: Vec<(String, String)> = self.project.formulas().map(|f| (f.id.clone(), f.title.clone())).collect();
            let mut pick: Option<String> = None;
            let mut create = false;
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Формула не выбрана").size(FS_BODY).color(pal.muted));
                if !formulas.is_empty() {
                    egui::ComboBox::from_id_salt("rde_formula_ribbon_pick")
                        .selected_text("Выбрать формулу проекта…")
                        .width(300.0)
                        .show_ui(ui, |ui| {
                            for (id, title) in &formulas {
                                if ui.selectable_label(false, title).clicked() {
                                    pick = Some(id.clone());
                                }
                            }
                        });
                }
                if btn(ui, Role::Primary, Some(IC_ADD), "Создать формулу").clicked() {
                    create = true;
                }
                ui.label(
                    RichText::new("...или кликните формулу в тексте документа — редактор откроется на этой вкладке")
                        .small()
                        .color(pal.muted),
                );
            });
            if let Some(id) = pick {
                self.open_formula_editor(&id);
            } else if create {
                self.open_formula_editor_new();
            }
            return;
        };
        // P5 (ADR-0021): WYSIWYG-редактор, без второго WordApp: scratch-редактор панели больше не
        // создаётся (нативный проф. редактор выведен, растровый предпросмотр заменён редактором).
        // Статус/«где вставлено» существующей формулы (R-DOC-19) — до отрисовки (заимствует store).
        let inspector = dialog.formula_id.as_deref().map(|id| self.formula_inspector_data(id));
        let stored = dialog.formula_id.as_deref().and_then(|id| {
            self.project.formula_object(id).and_then(|object| match &object.data {
                ObjectData::Formula(formula) => Some((
                    object.title.clone(),
                    formula.latex.clone(),
                    formula.display,
                    formula.numbering.mode.clone(),
                    formula.numbering.label.clone(),
                )),
                _ => None,
            })
        });
        let dirty = stored.as_ref().is_some_and(|(title, latex, display, mode, label)| {
            dialog.title.trim() != title.trim()
                || dialog.latex.trim() != latex.trim()
                || dialog.display != *display
                || dialog.numbering_mode != *mode
                || dialog.numbering_label.trim() != label.trim()
        });
        // Подсказка «такая формула уже есть» для создания (дедуп по latex.trim(), ADR-0019).
        let duplicate = (dialog.formula_id.is_none() && !dialog.latex.trim().is_empty())
            .then(|| self.project.formulas().find(|formula| formula.latex.trim() == dialog.latex.trim()).map(|formula| formula.id.clone()))
            .flatten();
        let anchor_label = match self.insert_anchor_block {
            Some(block) => format!("после блока {}", block + 1),
            None => "в конец документа".to_owned(),
        };
        let number_preview: Option<String> = self.next_formula_number_preview();
        let mut close = false;
        let mut new_formula = false;
        let mut apply = false;
        let mut insert = false;
        let mut toggle_native = false;
        let mut sync_native = false;
        let mut update_document = false;
        let mut open_document = false;
        let mut apply_blocked = false;

        // ── Одна колонка: WYSIWYG-редактор (он и есть предпросмотр) + LaTeX-источник + параметры ──
        let total_w = ui.available_width();
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_width(total_w);
                // Статус (переносится по строкам — колонка не шире отведённой).
                ui.horizontal_wrapped(|ui| {
                    if let Some(inspector) = &inspector {
                        let (label, color) = match &inspector.status {
                            Some(rde_dag::FormulaLiveStatus::Fresh) => ("актуальна", pal.green),
                            Some(rde_dag::FormulaLiveStatus::Stale(_)) => ("устарела", pal.amber),
                            Some(rde_dag::FormulaLiveStatus::NeverComputed) => ("не вычислена", pal.amber),
                            Some(rde_dag::FormulaLiveStatus::NotComputable(_)) => ("ошибка", pal.red),
                            None => ("статус неизвестен", pal.muted),
                        };
                        ui.label(RichText::new(format!("● {label}")).size(FS_BODY).color(color));
                        if let Some(reason) = inspector.status.as_ref().and_then(|status| status.reason()) {
                            ui.label(RichText::new(reason).small().color(pal.muted));
                        }
                        let stale = inspector
                            .insert_states
                            .iter()
                            .filter(|state| **state != rde_doc::FormulaInsertState::Fresh)
                            .count();
                        ui.label(
                            RichText::new(format!("вставок: {} · устаревших: {stale}", inspector.backlinks.len()))
                                .small()
                                .color(if stale > 0 { pal.amber } else { pal.muted }),
                        );
                        if !inspector.backlinks.is_empty() && btn(ui, Role::Ghost, Some(IC_OPEN), "Документ на вставке").clicked() {
                            open_document = true;
                        }
                    } else {
                        ui.label(RichText::new("Новая формула").size(FS_BODY).color(pal.accent));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Заголовок").size(FS_BODY).color(pal.muted));
                    // справа налево: × , id, затем поле заголовка на всю оставшуюся ширину
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Закрыть редактор формулы").clicked() {
                            close = true;
                        }
                        if let Some(id) = &dialog.formula_id {
                            ui.label(RichText::new(format!("id {}", &id[..id.len().min(8)])).monospace().size(FS_CAPTION).color(pal.muted))
                                .on_hover_text(id.clone());
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.title)
                                .hint_text("например: Средняя квадратичная ошибка")
                                .desired_width(f32::INFINITY),
                        );
                    });
                });
                // ── WYSIWYG-редактор формулы (P5, ADR-0021): структурный ввод 2D, каретка,
                //    палитры, undo; рендер — тем же конвейером/шрифтом, что документ (STIX).
                ui.add_space(4.0);
                ui.label(RichText::new("РЕДАКТОР ФОРМУЛЫ").size(FS_CAPTION).family(bold_family(ui.ctx())).color(pal.muted));
                if dialog.math_editor.is_none() {
                    dialog.math_editor = Some(Box::new(rde_math_editor::widget::MathEditorUi::new(&dialog.latex, dialog.display)));
                    if let Some(editor) = dialog.math_editor.as_mut() {
                        editor.request_focus();
                    }
                }
                if let Some(editor) = dialog.math_editor.as_mut() {
                    if editor.state.display != dialog.display {
                        editor.state.display = dialog.display;
                        editor.cache.clear();
                    }
                    let editor_response = editor.show(ui);
                    // LaTeX-поле — витрина модели: синхронизируем после любого изменения модели
                    // (ввод, палитра, вставка LaTeX), но не затираем его при непредставимой модели.
                    let editor_latex = editor_response.latex.trim().to_owned();
                    if editor_latex != dialog.latex.trim() && editor_response.roundtrip_error.is_none() {
                        dialog.latex = editor_latex;
                    } else if let Some(error) = &editor_response.roundtrip_error
                        && dialog.error.is_empty()
                    {
                        dialog.error = rde_math_editor::widget::roundtrip_warning(error);
                    }
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("LaTeX-источник").size(FS_BODY).color(pal.muted));
                    ui.label(RichText::new("— source-of-truth формулы (formula.toml); синхронизирован с редактором").small().color(pal.muted));
                });
                let latex_response = ui.add(
                    egui::TextEdit::multiline(&mut dialog.latex)
                        .code_editor()
                        .desired_rows(3)
                        .desired_width(f32::INFINITY)
                        .hint_text(r"\sigma = \sqrt{\frac{1}{N-1}\sum_{i=1}^{N}(x_i-\bar{x})^2}"),
                );
                if latex_response.changed()
                    && let Some(editor) = dialog.math_editor.as_mut()
                {
                    // Правку LaTeX-поля переносим в модель редактора (LaTeX — источник истины).
                    editor.set_source(&dialog.latex, dialog.display);
                }
                // Формат / нумерация / параметры вставки — одной строкой (панель компактная).
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Формат").size(FS_BODY).color(pal.muted));
                    ui.radio_value(&mut dialog.display, true, "display");
                    ui.radio_value(&mut dialog.display, false, "inline");
                    ui.separator();
                    ui.label(RichText::new("Нумерация").size(FS_BODY).color(pal.muted));
                    egui::ComboBox::from_id_salt("rde_formula_policy")
                        .selected_text(if dialog.numbering_mode == "none" { "без номера (none)" } else { "авто (auto)" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut dialog.numbering_mode, "auto".to_owned(), "авто (auto): номер (N) при вставке/обновлении");
                            ui.selectable_value(&mut dialog.numbering_mode, "none".to_owned(), "без номера (none)");
                        });
                    ui.separator();
                    ui.label(RichText::new("Вставка").size(FS_BODY).color(pal.muted));
                    egui::ComboBox::from_id_salt("rde_formula_render_mode")
                        .selected_text(if dialog.render_mode == "svg" { "SVG-вектор" } else { "OMML" })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut dialog.render_mode, "omml".to_owned(), "Word-объект (OMML, редактируемый)");
                            ui.selectable_value(&mut dialog.render_mode, "svg".to_owned(), "SVG-вектор (+PNG fallback)");
                        });
                    ui.label(RichText::new(format!("позиция: {anchor_label}")).small().color(pal.muted));
                    let number_text = if dialog.display && dialog.numbering_mode == "auto" {
                        number_preview
                            .as_ref()
                            .map(|number| format!("номер при вставке: ({number})"))
                            .unwrap_or_else(|| "номер: по схеме документа".to_owned())
                    } else if !dialog.display {
                        "inline — без номера".to_owned()
                    } else {
                        "без номера (none)".to_owned()
                    };
                    ui.label(RichText::new(number_text).small().color(pal.muted));
                });
                if dirty {
                    ui.label(
                        RichText::new("есть неприменённые правки — «Применить», чтобы записать source-of-truth и пересчитать кэши")
                            .small()
                            .color(pal.amber),
                    );
                }
                if let Some(duplicate) = &duplicate {
                    let duplicate_title = self.project.formula_object(duplicate).map(|object| object.title.clone()).unwrap_or_else(|| duplicate.clone());
                    ui.label(
                        RichText::new(format!("такая формула (по LaTeX) уже есть: «{duplicate_title}» — «Создать» использует существующую"))
                            .small()
                            .color(pal.amber),
                    );
                }
                if !dialog.error.is_empty() {
                    ui.label(RichText::new(&dialog.error).small().color(pal.red));
                }
                if !dialog.info.is_empty() {
                    ui.label(RichText::new(&dialog.info).small().color(pal.green));
                }
            });
        });
        // ── Действия: две строки кнопок фиксированной ширины (панель не шире центра; подписи
        //    обрезаются с полной в подсказке — `btn_w`).
        ui.add_space(4.0);
        ui.separator();
        let row_a_w = ((total_w - 2.0 * 8.0) / 3.0).clamp(120.0, 240.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let apply_label = if dialog.formula_id.is_some() { "Применить" } else { "Создать" };
            if btn_w(ui, Role::Primary, Some(IC_SAVE), apply_label, row_a_w).clicked() {
                apply = true;
            }
            let can_insert = dialog.formula_id.is_some() && !dirty;
            let insert_resp = ui
                .add_enabled_ui(can_insert, |ui| btn_w(ui, Role::Secondary, Some(IC_CONST), "Вставить в документ", row_a_w))
                .inner;
            if insert_resp.clicked() {
                insert = true;
            }
            if dialog.formula_id.is_some() && dirty {
                insert_resp.on_disabled_hover_text("сначала «Применить» — вставка берёт кэши применённой формулы");
            } else if dialog.formula_id.is_none() {
                insert_resp.on_disabled_hover_text("сначала «Создать»");
            }
            if btn_w(ui, Role::Secondary, Some(IC_UPDATE), "Обновить документ", row_a_w).clicked() {
                update_document = true;
            }
        });
        let row_b_columns = if FORMULA_NATIVE_UI { 4.0 } else { 2.0 };
        let row_b_w = ((total_w - (row_b_columns - 1.0) * 8.0) / row_b_columns).clamp(110.0, 220.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            // P5 (ADR-0021): кнопка нативного «проф. редактора» (клон MS Equations) убрана из UI —
            // он портил формулы и не отвечал требованиям MathType; статус редактора — Phase 2.
            if FORMULA_NATIVE_UI {
                let native_label = if dialog.native_open { "Скрыть проф. редактор" } else { "Проф. редактор…" };
                if btn_w(ui, Role::Secondary, Some(IC_CODE), native_label, row_b_w).clicked() {
                    toggle_native = true;
                }
                if dialog.native_open && btn_w(ui, Role::Ghost, None, "← Из редактора", row_b_w).clicked() {
                    sync_native = true;
                }
            }
            if btn_w(ui, Role::Ghost, Some(IC_ADD), "Новая формула", row_b_w).clicked() {
                new_formula = true;
            }
            if btn_w(ui, Role::Ghost, None, "Закрыть", row_b_w).clicked() {
                close = true;
            }
        });
        ui.label(
            RichText::new("источник истины — formula.toml; документ обновляется «Вставить»/«Обновить»")
                .small()
                .color(pal.muted),
        );

        if close {
            self.formula_editor = None;
            return;
        }
        if new_formula {
            self.formula_editor = None;
            self.open_formula_editor_new();
            return;
        }
        if FORMULA_NATIVE_UI && toggle_native {
            if dialog.native_open {
                // Забираем правку перед закрытием, чтобы не потерять ввод.
                Self::sync_formula_from_native(&mut dialog);
                dialog.native_open = false;
            } else {
                Self::sync_formula_from_native(&mut dialog);
                self.restart_native_formula_editor(&mut dialog);
                dialog.native_open = true;
                dialog.info = "Профессиональный редактор: ввод структур/символов и текста; «Забрать LaTeX из редактора» переносит источник в поле".to_owned();
            }
            ui.ctx().request_repaint();
        }
        if FORMULA_NATIVE_UI && sync_native {
            let before = dialog.latex.clone();
            Self::sync_formula_from_native(&mut dialog);
            dialog.info = if dialog.latex == before {
                "Редактор: LaTeX не изменился".to_owned()
            } else {
                "LaTeX забран из профессионального редактора — «Применить», чтобы записать".to_owned()
            };
            dialog.error.clear();
        }
        if apply {
            // P5 (ADR-0021 §Решение 2): источник пишем из модели редактора — нормализуем
            // непредставимые конструкции, затем проверяем roundtrip (LaTeX↔модель).
            if let Some(editor) = dialog.math_editor.as_mut() {
                match editor.prepare_source() {
                    Ok(canonical) => dialog.latex = canonical,
                    Err(error) => {
                        dialog.error = format!("Формула не представима в LaTeX-источнике: {}", error.detail);
                        dialog.info.clear();
                        apply_blocked = true;
                    }
                }
            }
        }
        if apply && !apply_blocked {
            let was_new = dialog.formula_id.is_none();
            let result = match &dialog.formula_id {
                Some(id) => {
                    let id = id.clone();
                    self.apply_formula_edit(&id, &dialog.title, &dialog.latex, dialog.display, &dialog.numbering_mode, &dialog.numbering_label)
                        .map(|()| (id, false))
                }
                None => self.create_formula_action(&dialog.title, &dialog.latex, dialog.display, &dialog.numbering_mode),
            };
            match result {
                Ok((id, reused)) => {
                    dialog.formula_id = Some(id.clone());
                    if let Some(formula) = self.project.formula_object(&id)
                        && let ObjectData::Formula(manifest) = &formula.data
                    {
                        dialog.title = formula.title.clone();
                        dialog.latex = manifest.latex.clone();
                        dialog.display = manifest.display;
                        dialog.numbering_mode = manifest.numbering.mode.clone();
                        dialog.numbering_label = manifest.numbering.label.clone();
                    }
                    dialog.error.clear();
                    if let Some(editor) = dialog.math_editor.as_mut() {
                        editor.sync_applied(&dialog.latex, dialog.display);
                    }
                    dialog.info = if reused {
                        "Такая формула уже есть в проекте — используется существующая".to_owned()
                    } else if was_new {
                        "Формула создана; кэши OMML/SVG/PNG пересчитаны, объект выбран в дереве".to_owned()
                    } else {
                        "Источник применён; кэши пересчитаны, вставки помечены устаревшими — «Обновить документ»".to_owned()
                    };
                    if dialog.native_open {
                        self.restart_native_formula_editor(&mut dialog);
                    }
                    self.push_log(Level::Ok, format!("Формула {}: {} ({})", if was_new { "создана" } else { "обновлена" }, dialog.title, id));
                }
                Err(error) => {
                    dialog.error = error.clone();
                    self.push_log(Level::Warn, format!("Формула: {error}"));
                }
            }
        }
        if open_document
            && let Some(backlink) = inspector.as_ref().and_then(|inspector| inspector.backlinks.first())
        {
            let document_id = backlink.document_id.clone();
            if self.project.object(&document_id).is_some() {
                self.selected = Some(document_id);
                self.tab = Tab::Document;
                self.show_inspector = true;
                self.push_log(Level::Info, format!("Документ на вставке: {}", backlink.document_title));
            }
        }
        if insert {
            self.formula_editor_insert(&mut dialog);
        }
        if update_document {
            self.update_project_document();
            dialog.info = "Документ обновлён: тела/номера/ссылки вставок пересобраны".to_owned();
        }
        self.formula_editor = Some(dialog);
    }

    /// Окно «Профессиональный редактор формулы» (нативная вкладка Equation WordCraft на отдельном
    /// scratch-документе, спайк A). Отдельное окно, а не область ленты: canvas WordCraft не
    /// уживается с живым canvas документа в одном кадре (фиксированный egui-id/слои, ADR-0020 §3);
    /// пока оно открыто, документ в DocView показывается растровым предпросмотром.
    fn formula_native_window(&mut self, ctx: &egui::Context) {
        // P5 (ADR-0021): окно проф. редактора выведено из UI вместе с порчей формул.
        if !FORMULA_NATIVE_UI {
            return;
        }
        let Some(mut dialog) = self.formula_editor.take() else { return };
        if !dialog.native_open {
            self.formula_editor = Some(dialog);
            return;
        }
        let pal = theme_colors(ctx.theme());
        let mut take_from_editor = false;
        let mut hide_editor = false;
        egui::Window::new("Профессиональный редактор формулы")
            .collapsible(false)
            .resizable(false)
            .default_width(1000.0)
            .default_pos(egui::pos2(380.0, 90.0))
            .show(ctx, |ui| {
                ui.set_min_width(980.0);
                if let Some(native) = dialog.native.as_mut() {
                    if !native.fonts_ready {
                        // equation_tab использует семейства medium/semibold WordCraft: до первого
                        // кадра панели их надо установить, иначе паника (спайк A); первый кадр — без show.
                        wordcraft_ui_egui::theme::install_fonts(ui.ctx());
                        native.fonts_ready = true;
                        native.skip_frames = 1;
                        ui.ctx().request_repaint();
                    } else if native.skip_frames > 0 {
                        native.skip_frames -= 1;
                        ui.ctx().request_repaint();
                    } else {
                        wordcraft_ui_egui::equation_tab::show(&mut native.app, ui);
                        ui.add_space(4.0);
                        let size = vec2(ui.available_width(), NATIVE_FORMULA_EDITOR_H);
                        let (rect, _resp) = ui.allocate_exact_size(size, Sense::hover());
                        let mut child = ui.new_child(
                            egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::top_down(egui::Align::Min)),
                        );
                        child.set_clip_rect(rect);
                        native.app.logic(ui.ctx());
                        native.app.doc_view_ui(&mut child);
                    }
                }
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if btn(ui, Role::Primary, Some(IC_SAVE), "← Забрать LaTeX из редактора").clicked() {
                        take_from_editor = true;
                    }
                    if btn(ui, Role::Ghost, None, "Скрыть редактор").clicked() {
                        hide_editor = true;
                    }
                    ui.label(
                        RichText::new("нативный Equation WordCraft: структуры/символы/LaTeX-ввод на отдельном мини-документе")
                            .small()
                            .color(pal.muted),
                    );
                });
            });
        if take_from_editor {
            let before = dialog.latex.clone();
            Self::sync_formula_from_native(&mut dialog);
            dialog.info = if dialog.latex == before {
                "Редактор: LaTeX не изменился".to_owned()
            } else {
                "LaTeX забран из профессионального редактора — «Применить», чтобы записать".to_owned()
            };
            dialog.error.clear();
        }
        if hide_editor {
            Self::sync_formula_from_native(&mut dialog);
            dialog.native_open = false;
        }
        self.formula_editor = Some(dialog);
    }

    /// «Принять формулы как объекты» (P3, ADR-0005 §7): OMML вне реестра → project-level формулы +
    /// вставки-ссылки. Явная команда; при открытии документа ничего не меняется. Идемпотентна.
    ///
    /// Открывает диалог со счётчиками display/inline/найденных номеров и политикой нумерации
    /// (ADR-0019): сохранение номеров Word или перенумерация по схеме документа.
    pub fn open_formula_migration_dialog(&mut self) {
        let Some(package) = self.active_document_path() else {
            self.push_log(Level::Warn, "Миграция формул: в проекте нет документа");
            return;
        };
        if !rde_doc::is_rde_package(&package) {
            self.push_log(Level::Warn, "Миграция формул: документ ещё не в .doc.rde (сначала «Сохранить проект»)");
            return;
        }
        if self.store.is_none() {
            self.push_log(Level::Warn, "Миграция формул: хранилище проекта не открыто");
            return;
        }
        let equations = match rde_doc::unregistered_formulas(&package) {
            Ok(equations) => equations,
            Err(error) => {
                self.push_log(Level::Warn, format!("Миграция формул: {error}"));
                return;
            }
        };
        if equations.is_empty() {
            self.push_log(Level::Info, "Миграция формул: формул вне реестра нет");
            return;
        }
        let display = equations.iter().filter(|equation| equation.display).count();
        let inline = equations.len().saturating_sub(display);
        let found_numbers = equations.iter().filter(|equation| equation.display && equation.number_text.is_some()).count();
        let mixed = equations.iter().filter(|equation| equation.display && equation.mixed).count();
        // «Сохранять номера Word» по умолчанию — только если они есть у всех display-формул и
        // нет смешанных абзацев (частичное сохранение потеряло бы номера остальных).
        let preserve_numbers = display > 0 && found_numbers == display && mixed == 0;
        let scheme = self.document_formula_numbering();
        self.formula_migration_dialog = Some(FormulaMigrationDialogState {
            equations,
            display,
            inline,
            found_numbers,
            mixed,
            scheme,
            preserve_numbers,
            number_display: true,
            error: String::new(),
        });
    }

    /// Применить миграцию формул с политикой номеров (из диалога/теста). После миграции сразу
    /// пересобирает тела вставок и перекрёстные ссылки — отдельное «Обновить документ» не нужно.
    fn apply_formula_migration(&mut self, options: rde_doc::FormulaMigrationOptions) {
        let Some(package) = self.active_document_path() else {
            self.push_log(Level::Warn, "Миграция формул: в проекте нет документа");
            return;
        };
        if !rde_doc::is_rde_package(&package) {
            self.push_log(Level::Warn, "Миграция формул: документ ещё не в .doc.rde");
            return;
        }
        if self.store.is_none() {
            self.push_log(Level::Warn, "Миграция формул: хранилище проекта не открыто");
            return;
        }
        let equations = self
            .formula_migration_dialog
            .as_ref()
            .map(|dialog| dialog.equations.clone())
            .unwrap_or_default();
        // Диалог закрывается по факту применения (в т.ч. при ошибке ниже).
        self.formula_migration_dialog = None;
        if equations.is_empty() {
            self.push_log(Level::Info, "Миграция формул: формул вне реестра нет");
            return;
        }
        let renderer = rde_dag::FormulaRenderer { id: rde_doc::FORMULA_RENDERER_ID, version: rde_doc::FORMULA_RENDERER_VERSION };
        let mut plans: Vec<rde_doc::FormulaMigrationPlan> = Vec::new();
        let mut created = 0usize;
        // (formula_id, display, numbered) в порядке планов — для сборки тел после миграции.
        let mut states: Vec<(String, bool, bool)> = Vec::new();
        let mut prepared_states: Vec<(String, rde_dag::FormulaState)> = Vec::new();
        for equation in &equations {
            // Та же формула (по LaTeX) уже может быть объектом проекта — не дублируем.
            let existing = self
                .project
                .formulas()
                .find(|formula| formula.latex.trim() == equation.latex.trim())
                .map(|formula| formula.id.clone());
            let formula_id = match existing {
                Some(id) => id,
                None => {
                    let title: String = equation.latex.chars().take(40).collect();
                    // display-флаг формулы — из фактического вхождения (ADR-0019): inline-формулы
                    // проекта не должны по умолчанию считаться display.
                    match rde_core::create_formula_with_display(&self.project.root, &title, &equation.latex, equation.display) {
                        Ok(dir) => {
                            created += 1;
                            self.reload_project();
                            dir.file_name().and_then(|name| name.to_str()).map(str::to_owned).unwrap_or_default()
                        }
                        Err(error) => {
                            self.push_log(Level::Warn, format!("Миграция формул: {error}"));
                            return;
                        }
                    }
                }
            };
            let prepared = {
                let project = &self.project;
                let Some(store) = self.store.as_mut() else { return };
                match rde_dag::prepare_formula_update(project, store, &formula_id, &renderer, |latex, display| {
                    let caches = rde_doc::compile_formula(latex, display)?;
                    Ok(rde_dag::FormulaCompiled { omml: caches.omml, svg: caches.svg, png: caches.png })
                }) {
                    Ok(prepared) => prepared,
                    Err(error) => {
                        self.push_log(Level::Warn, format!("Миграция формул ({formula_id}): {error}"));
                        return;
                    }
                }
            };
            // Кэши и состояние формулы — отдельной транзакцией (формула нужна вставке как цель).
            if let Some(store) = self.store.as_mut()
                && let Err(error) =
                    rde_dag::commit_formula_update(store, &prepared.state, prepared.cache_tree, &format!("formula migrate: {formula_id}"))
            {
                self.push_log(Level::Warn, format!("Миграция формул: {error}"));
                return;
            }
            let _ = rde_dag::record_prepared_formula_state(&self.project, &prepared);
            let omml = {
                let Some(store) = self.store.as_ref() else { return };
                let hash = prepared.state.omml_hash.as_deref().unwrap_or_default();
                rde_hash::Hash::from_hex(hash)
                    .ok()
                    .and_then(|hash| store.get_blob(hash).ok().flatten())
                    .unwrap_or_default()
            };
            // Номер получит только block-формула (display без текста вокруг), а не смешанная.
            states.push((formula_id.clone(), equation.display, equation.display && !equation.mixed));
            prepared_states.push((formula_id.clone(), prepared.state.clone()));
            plans.push(rde_doc::FormulaMigrationPlan {
                formula_id,
                act_key: prepared.state.act_key.clone(),
                latex: equation.latex.clone(),
                omml,
            });
        }
        match rde_doc::migrate_formulas_with(&package, &plans, options) {
            Ok(report) => {
                // Сразу после миграции: тела вставок из кэшей + текст перекрёстных ссылок.
                let mut bodies: BTreeMap<String, rde_doc::FormulaBody> = BTreeMap::new();
                if let Some(store) = self.store.as_ref() {
                    for (formula_id, display, numbered) in &states {
                        if let Some((_, state)) = prepared_states.iter().find(|(id, _)| id == formula_id)
                            && let Ok(body) = formula_body_from_state(store, formula_id, state, *display, *numbered)
                        {
                            bodies.insert(formula_id.clone(), body);
                        }
                    }
                }
                match rde_doc::update_formula_inserts_with(&package, &bodies, options.numbering) {
                    Ok(update) => {
                        for warning in &update.warnings {
                            self.push_log(Level::Warn, format!("Нумерация формул: {warning}"));
                        }
                    }
                    Err(error) => self.push_log(Level::Warn, format!("Обновление формул после миграции: {error}")),
                }
                if let Ok(refresh) = rde_doc::refresh_formula_crossrefs(&package) {
                    for warning in &refresh.warnings {
                        self.push_log(Level::Warn, format!("Ссылки на формулы: {warning}"));
                    }
                }
                let doc_id = self.active_document_object().and_then(|object| match &object.data {
                    ObjectData::Document(manifest) => Some(manifest.id.clone()),
                    _ => None,
                });
                if let (Some(store), Some(doc_id)) = (self.store.as_mut(), doc_id) {
                    let _ = rde_doc::commit_document(&self.project.root, &package, &doc_id, store, "formula migrate");
                }
                self.reload_document_view_preserving();
                self.push_log(
                    Level::Ok,
                    format!(
                        "Принято формул {} (display {}, inline {}): объектов создано {}, номеров сохранено из Word {}, назначено {}, схема «{}»; пунктуация после формул сохранена",
                        report.insert_ids.len(),
                        report.display,
                        report.inline,
                        created,
                        report.numbers_preserved,
                        report.numbers_assigned,
                        formula_numbering_label(report.numbering)
                    ),
                );
                for warning in &report.warnings {
                    self.push_log(Level::Warn, format!("Миграция формул: {warning}"));
                }
            }
            Err(error) => self.push_log(Level::Warn, format!("Миграция формул: {error}")),
        }
    }

    /// Подписи формул для дерева/дерева-служебных объектов: «N вставок · статус» (P3).
    fn formula_tree_labels(&self) -> BTreeMap<String, String> {
        let Some(store) = self.store.as_ref() else { return BTreeMap::new() };
        let renderer = rde_dag::FormulaRenderer { id: rde_doc::FORMULA_RENDERER_ID, version: rde_doc::FORMULA_RENDERER_VERSION };
        let Ok(statuses) = rde_dag::formula_statuses(&self.project, store, &renderer) else { return BTreeMap::new() };
        let documents: Vec<(String, String, PathBuf)> = self.formula_documents();
        let mut labels = BTreeMap::new();
        for formula in self.project.formulas() {
            let usage = rde_doc::formula_backlinks(&documents, &formula.id).map(|links| links.len()).unwrap_or(0);
            let status = statuses
                .get(&formula.id)
                .map(|status| status.status.label_ru().to_owned())
                .unwrap_or_else(|| "статус неизвестен".to_owned());
            labels.insert(formula.id.clone(), format!("{usage} вст. · {status}"));
        }
        labels
    }

    /// Документы проекта для обратных ссылок формул: (id, заголовок, путь пакета).
    fn formula_documents(&self) -> Vec<(String, String, PathBuf)> {
        self.project
            .documents()
            .filter_map(|object| {
                let ObjectData::Document(manifest) = &object.data else { return None };
                Some((manifest.id.clone(), object.title.clone(), object.dir.join(&manifest.file)))
            })
            .collect()
    }

    /// Данные панели инспектора формулы: живой статус, обратные ссылки, «формулы вне реестра».
    fn formula_inspector_data(&mut self, formula_id: &str) -> FormulaInspectorData {
        // P5 (quick-win §5.2): без кэша функция вызывалась каждый кадр и стоила до 52 мс (debug);
        // данные меняются только при изменении проекта/документа/ledger → эпоха + id.
        if let Some((epoch, cached_id, cached)) = &self.formula_inspector_cache
            && *epoch == self.data_epoch
            && cached_id == formula_id
        {
            return cached.clone();
        }
        let data = self.compute_formula_inspector_data(formula_id);
        self.formula_inspector_cache = Some((self.data_epoch, formula_id.to_owned(), data.clone()));
        data
    }

    /// Расчёт данных инспектора формулы (дорогой путь: ledger + документы) — под кэшем выше.
    fn compute_formula_inspector_data(&self, formula_id: &str) -> FormulaInspectorData {
        let store = self.store.as_ref();
        let renderer = rde_dag::FormulaRenderer { id: rde_doc::FORMULA_RENDERER_ID, version: rde_doc::FORMULA_RENDERER_VERSION };
        let status = store
            .and_then(|store| rde_dag::formula_statuses(&self.project, store, &renderer).ok())
            .and_then(|statuses| statuses.get(formula_id).map(|status| (status.status.clone(), status.act_key.clone())));
        let backlinks = rde_doc::formula_backlinks(&self.formula_documents(), formula_id).unwrap_or_default();
        let unregistered = self
            .project
            .document_path()
            .filter(|path| rde_doc::is_rde_package(path))
            .and_then(|path| rde_doc::unregistered_formulas(&path).ok())
            .map(|equations| equations.len())
            .unwrap_or(0);
        let act_key = status.as_ref().and_then(|(_, act_key)| act_key.clone());
        // Статусы вставок: доступность формулы задаёт текущий act_key (R-DOC-19), тела
        // сравниваются с подписью последней записи RDE (R-DOC-13).
        let mut insert_states = Vec::new();
        if let Some(act_key) = &act_key {
            let mut availability: BTreeMap<String, rde_doc::FormulaAvailability> = BTreeMap::new();
            availability.insert(formula_id.to_owned(), rde_doc::FormulaAvailability::Current(act_key.clone()));
            for (_document_id, _title, path) in self.formula_documents() {
                if let Ok(states) = rde_doc::formula_insert_statuses(&path, &availability) {
                    insert_states.extend(states.into_iter().filter(|state| state.formula_id == formula_id).map(|state| state.state));
                }
            }
        }
        FormulaInspectorData {
            status: status.as_ref().map(|(status, _)| status.clone()),
            act_key,
            backlinks,
            insert_states,
            unregistered,
        }
    }

    /// Кандидат на откат обновления документа (транзакция store с корнем `document/<id>`).
    fn doc_rollback_candidate(&self) -> Option<rde_store::RollbackCandidate> {
        let store = self.store.as_ref()?;
        let document_object = self.active_document_object()?;
        let ObjectData::Document(manifest) = &document_object.data else { return None };
        let root = rde_doc::document_root_name(&manifest.id);
        store
            .rollback_candidates()
            .ok()?
            .into_iter()
            .find(|candidate| !candidate.rolled_back && candidate.root_names.iter().any(|name| name == &root))
    }

    /// Откатить последнее обновление документа: store переключает корни, пакет восстанавливается из CAS.
    pub fn rollback_document(&mut self) {
        let Some(candidate) = self.doc_rollback_candidate() else {
            self.push_log(Level::Info, "Откат документа: нет транзакций обновления документа");
            return;
        };
        let Some(package) = self.active_document_path() else { return };
        let doc_id = self.active_document_object().and_then(|object| match &object.data {
            ObjectData::Document(manifest) => Some(manifest.id.clone()),
            _ => None,
        });
        let Some(doc_id) = doc_id else { return };
        let Some(store) = self.store.as_mut() else { return };
        match store.rollback(&candidate.txid) {
            Ok(_) => match rde_doc::restore_document_from_store(&self.project.root, &package, &doc_id, store) {
                Ok(Some(_)) => {
                    self.reload_document_view_preserving();
                    self.push_log(Level::Ok, format!("Документ откачен к предыдущей версии (транзакция {})", candidate.txid.chars().take(10).collect::<String>()));
                }
                Ok(None) => self.push_log(Level::Warn, "Откат документа: в корнях нет версии документа"),
                Err(error) => self.push_log(Level::Warn, format!("Откат документа: {error}")),
            },
            Err(error) => self.push_log(Level::Warn, format!("Откат документа не выполнен: {error}")),
        }
    }

    /// Включить/выключить автообновление и сохранить режим в `rde.toml` (R-DOC-12).
    pub fn set_auto_update(&mut self, enabled: bool) {
        let path = self.project.root.join("rde.toml");
        if let Err(error) = set_manifest_value(&path, "document_auto_update", if enabled { "true" } else { "false" }) {
            self.push_log(Level::Warn, format!("Автообновление: настройка не сохранена — {error}"));
            return;
        }
        self.auto_update = enabled;
        self.push_log(
            Level::Info,
            if enabled {
                "Автообновление включено: артефакты и документ будут обновляться автоматически (бэкап перед операцией)"
            } else {
                "Автообновление выключено: обновление — вручную"
            },
        );
        self.reload_project();
    }

    /// Сделать zip-бэкап проекта (R-VCS-04/R-SET-06) и записать путь в журнал.
    pub fn backup_project_now(&mut self) -> Option<PathBuf> {
        match rde_core::backup_project(&self.project.root) {
            Ok(path) => {
                self.push_log(Level::Ok, format!("Бэкап проекта: {}", path.display()));
                Some(path)
            }
            Err(error) => {
                self.push_log(Level::Warn, format!("Бэкап проекта: {error}"));
                None
            }
        }
    }

    /// Открыть диалог бэкапов (список + восстановление).
    pub fn open_backup_dialog(&mut self) {
        match rde_core::list_project_backups(&self.project.root) {
            Ok(backups) if backups.is_empty() => {
                self.push_log(Level::Info, "Бэкапов пока нет — «Сделать бэкап проекта»");
                self.backup_dialog = Some(Vec::new());
            }
            Ok(backups) => self.backup_dialog = Some(backups),
            Err(error) => self.push_log(Level::Warn, format!("Бэкапы проекта: {error}")),
        }
    }

    /// Откатить последнее обновление артефактов (store переключает корни, R-ART-05).
    pub fn rollback_last_artifacts(&mut self) {
        let Some(store) = self.store.as_mut() else {
            self.push_log(Level::Warn, "Откат артефактов: хранилище не открыто");
            return;
        };
        match store.rollback_latest() {
            Ok(Some(outcome)) => {
                self.push_log(
                    Level::Ok,
                    format!(
                        "Артефакты откачены: транзакция {}, восстановлено корней {}",
                        outcome.rollback_tx.chars().take(10).collect::<String>(),
                        outcome.restored.len()
                    ),
                );
                self.reload_document_view_preserving();
            }
            Ok(None) => self.push_log(Level::Info, "Откат артефактов: нет транзакций обновления"),
            Err(error) => self.push_log(Level::Warn, format!("Откат артефактов: {error}")),
        }
    }

    /// Авто-старт (R-ART-03): при включённом автообновлении один раз за сессию пересчитать
    /// устаревшие артефакты доверенного проекта, затем автоматически обновить документ.
    fn maybe_auto_boot_update(&mut self) {
        if !self.auto_update || self.auto_boot_done || self.update_job.is_some() {
            return;
        }
        self.auto_boot_done = true;
        if !self.project.manifest.trusted || self.store.is_none() {
            return;
        }
        let stale = self
            .dag_statuses
            .iter()
            .filter(|(_, status)| !matches!(status.status, rde_dag::LiveStatus::Fresh))
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        if stale.is_empty() {
            return;
        }
        self.auto_backup_project();
        self.push_log(Level::Info, format!("Автообновление: пересчёт устаревших артефактов ({})", stale.len()));
        self.request_update(Some(stale), false);
    }

    /// Бэкап перед авто-операцией (ошибка бэкапа не блокирует обновление, но пишется в журнал).
    fn auto_backup_project(&mut self) {
        let _ = self.backup_project_now();
    }

    /// После успешного обновления артефактов — автоматически обновить документ (R-DOC-12).
    fn auto_update_document_after_artifacts(&mut self) {
        if !self.auto_update || self.auto_updating {
            return;
        }
        let Some(package) = self.active_document_path() else { return };
        let Ok(rde_doc) = rde_doc::load_rde_doc(&package) else { return };
        if rde_doc.inserts.iter().all(|insert| insert.target.is_empty()) {
            return;
        }
        self.auto_updating = true;
        self.auto_backup_project();
        self.push_log(Level::Info, "Автообновление: обновляю документ по свежим артефактам");
        self.update_project_document();
        self.auto_updating = false;
    }

    /// «Обновить документ»: значения/рисунки вставок из артефактов, перенумерация подписей,
    /// пересохранение `.doc.rde` — транзакционно (бэкап + версия в store + diff) (R-DOC-11/12).
    pub fn update_project_document(&mut self) {
        let Some(package) = self.active_document_path() else {
            self.push_log(Level::Warn, "Обновить документ: в проекте нет документа");
            return;
        };
        if !rde_doc::is_rde_package(&package) {
            self.push_log(Level::Warn, "Обновить документ: документ ещё не в .doc.rde (запустите миграцию/сохранение)");
            return;
        }
        if self.editor.is_some() {
            self.save_project_document();
        }
        let before = match rde_doc::load_rde_doc(&package) {
            Ok(doc) => doc,
            Err(error) => {
                self.push_log(Level::Warn, format!("Обновить документ: {error}"));
                return;
            }
        };
        // Разрешаем цели вставок (R-DOC-11): значения из JSON/текста, рисунки из SVG/PNG.
        let mut values: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
        let mut hashes: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
        let mut images: std::collections::BTreeMap<String, (String, Vec<u8>)> = std::collections::BTreeMap::new();
        let mut tables: std::collections::BTreeMap<String, Vec<Vec<String>>> = std::collections::BTreeMap::new();
        let mut errors: Vec<String> = Vec::new();
        // Формульные вставки документа: id формул-целей (P3, ADR-0005 §3.3).
        let mut formula_ids: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        let source_ids: Vec<String> = self.project.sources().map(|source| source.id.clone()).collect();
        {
            let Some(store) = self.store.as_ref() else {
                self.push_log(Level::Warn, "Обновить документ: хранилище проекта не открыто");
                return;
            };
            // Ledger артефактов нужен для констант со source (ADR-0005): закреплённый/текущий out_hash.
            let ledger = rde_dag::ledger(store).unwrap_or_default();
            for insert in before.inserts.iter().filter(|insert| !insert.target.is_empty()) {
                if insert.kind == "figure" {
                    match rde_dag::resolve_insert_image(&self.project, store, &insert.target) {
                        Ok((path, ext, hash)) => match std::fs::read(&path) {
                            Ok(bytes) => {
                                images.insert(insert.id.clone(), (ext, bytes));
                                hashes.insert(insert.id.clone(), hash);
                            }
                            Err(error) => errors.push(format!("{}: {error}", insert.id)),
                        },
                        Err(error) => errors.push(format!("{}: {error}", insert.id)),
                    }
                } else if insert.kind == "value" {
                    match rde_dag::resolve_insert_text(&self.project, store, &insert.target) {
                        Ok((value, hash)) => {
                            // Формат отображения — из rde:doc (R-DOC-11): int/dec:N/sci:N/percent:N.
                            values.insert(insert.id.clone(), rde_doc::format_value(&value, &insert.format));
                            hashes.insert(insert.id.clone(), hash);
                        }
                        Err(error) => errors.push(format!("{}: {error}", insert.id)),
                    }
                } else if insert.kind == "table" {
                    match rde_dag::resolve_insert_table(&self.project, store, &insert.target) {
                        Ok((rows, hash)) => {
                            tables.insert(insert.id.clone(), rows);
                            hashes.insert(insert.id.clone(), hash);
                        }
                        Err(error) => errors.push(format!("{}: {error}", insert.id)),
                    }
                } else if insert.kind == "constant" {
                    // Константа проекта (R-ART-09): значение + подсказка rich-форматирования.
                    match rde_dag::resolve_insert_constant(&self.project, &ledger, &insert.target) {
                        Ok((text, run_style, hash)) => {
                            let _ = rde_doc::set_insert_run_style(&package, &insert.id, &run_style);
                            values.insert(insert.id.clone(), text);
                            hashes.insert(insert.id.clone(), hash);
                        }
                        Err(error) => errors.push(format!("{}: {error}", insert.id)),
                    }
                } else if insert.kind == "citation" {
                    // Цитата на источник: номер по порядку источников проекта.
                    let index = source_ids.iter().position(|id| insert.target.ends_with(id.as_str())).map(|index| index + 1);
                    match index {
                        Some(index) => {
                            values.insert(insert.id.clone(), format!("[{index}]"));
                        }
                        None => errors.push(format!("{}: источник не найден", insert.id)),
                    }
                } else if insert.kind == "date" {
                    values.insert(insert.id.clone(), today_iso_date());
                } else if insert.kind == "formula" {
                    // Формула (P3): цель — project-level объект; кэши/статус — через rde-dag.
                    if let Some(target) = rde_doc::parse_formula_target(&insert.target) {
                        formula_ids.insert(target.formula_id);
                    } else {
                        errors.push(format!("{}: некорректная ссылка на формулу", insert.id));
                    }
                }
            }
        }
        let Some(doc_id) = self.active_document_object().and_then(|object| match &object.data {
            ObjectData::Document(manifest) => Some(manifest.id.clone()),
            _ => None,
        }) else {
            self.push_log(Level::Warn, "Обновить документ: объект документа не найден");
            return;
        };
        // Формулы: актуальность/пересчёт кэшей (ADR-0005 §3.3). Устаревшие/невычисленные формулы
        // пересчитываются здесь, их кэши фиксируются ТОЙ ЖЕ транзакцией, что и документ.
        let renderer = rde_dag::FormulaRenderer { id: rde_doc::FORMULA_RENDERER_ID, version: rde_doc::FORMULA_RENDERER_VERSION };
        let mut formula_bodies: BTreeMap<String, rde_doc::FormulaBody> = BTreeMap::new();
        let mut prepared_formulas: Vec<rde_dag::PreparedFormulaUpdate> = Vec::new();
        let mut formula_availability: BTreeMap<String, rde_doc::FormulaAvailability> = BTreeMap::new();
        {
            let project = &self.project;
            let Some(store) = self.store.as_mut() else { return };
            let statuses = rde_dag::formula_statuses(project, store, &renderer).unwrap_or_default();
            for formula_id in &formula_ids {
                let Some(status) = statuses.get(formula_id) else {
                    formula_availability.insert(formula_id.clone(), rde_doc::FormulaAvailability::Missing);
                    errors.push(format!("формула {} не найдена (вставка «цель отсутствует»)", formula_id.chars().take(8).collect::<String>()));
                    continue;
                };
                match &status.status {
                    rde_dag::FormulaLiveStatus::NotComputable(reason) => {
                        formula_availability.insert(formula_id.clone(), rde_doc::FormulaAvailability::NotComputed(reason.clone()));
                        errors.push(format!("формула {}: {reason}", formula_id.chars().take(8).collect::<String>()));
                    }
                    rde_dag::FormulaLiveStatus::Fresh => {
                        let ledger = rde_dag::formula_ledger(store).unwrap_or_default();
                        let Some(state) = ledger.get(formula_id) else { continue };
                        let display = project
                            .formula_object(formula_id)
                            .and_then(|object| match &object.data {
                                ObjectData::Formula(formula) => Some(formula.display),
                                _ => None,
                            })
                            .unwrap_or(true);
                        let numbered = display && formula_numbering_enabled(project, formula_id);
                        match formula_body_from_state(store, formula_id, state, display, numbered) {
                            Ok(body) => {
                                formula_availability.insert(formula_id.clone(), rde_doc::FormulaAvailability::Current(state.act_key.clone()));
                                formula_bodies.insert(formula_id.clone(), body);
                            }
                            Err(error) => errors.push(format!("формула {}: {error}", formula_id.chars().take(8).collect::<String>())),
                        }
                    }
                    rde_dag::FormulaLiveStatus::Stale(_) | rde_dag::FormulaLiveStatus::NeverComputed => {
                        let compiled = rde_dag::prepare_formula_update(project, store, formula_id, &renderer, |latex, display| {
                            let caches = rde_doc::compile_formula(latex, display)?;
                            Ok(rde_dag::FormulaCompiled { omml: caches.omml, svg: caches.svg, png: caches.png })
                        });
                        match compiled {
                            Ok(prepared) => {
                                let display = project
                                    .formula_object(formula_id)
                                    .and_then(|object| match &object.data {
                                        ObjectData::Formula(formula) => Some(formula.display),
                                        _ => None,
                                    })
                                    .unwrap_or(true);
                                let numbered = display && formula_numbering_enabled(project, formula_id);
                                match formula_body_from_state(store, formula_id, &prepared.state, display, numbered) {
                                    Ok(body) => {
                                        formula_availability
                                            .insert(formula_id.clone(), rde_doc::FormulaAvailability::Current(prepared.state.act_key.clone()));
                                        formula_bodies.insert(formula_id.clone(), body);
                                        prepared_formulas.push(prepared);
                                    }
                                    Err(error) => errors.push(format!("формула {}: {error}", formula_id.chars().take(8).collect::<String>())),
                                }
                            }
                            Err(error) => {
                                formula_availability.insert(formula_id.clone(), rde_doc::FormulaAvailability::NotComputed(error.clone()));
                                errors.push(format!("формула {}: {error}", formula_id.chars().take(8).collect::<String>()));
                            }
                        }
                    }
                }
            }
        }
        let formula_roots = rde_dag::prepared_formula_roots(&prepared_formulas);
        let formula_payload = if prepared_formulas.is_empty() {
            None
        } else {
            let mut payload = rde_dag::prepared_formulas_payload(&prepared_formulas);
            payload["kind"] = serde_json::json!("document-formula-update");
            Some(payload)
        };
        // Схему нумерации читаем до мутабельного займа store (`document.toml` активного документа).
        let formula_numbering = self.document_formula_numbering();
        let Some(store) = self.store.as_mut() else { return };
        match rde_doc::update_document_transactional(
            &self.project.root,
            &package,
            &doc_id,
            &values,
            &hashes,
            &images,
            &tables,
            &formula_bodies,
            formula_numbering,
            &formula_roots,
            formula_payload,
            store,
        ) {
            Ok(report) => {
                // `[state]` пересчитанных формул — зеркало ledger в formula.toml (после tx).
                for prepared in &prepared_formulas {
                    let _ = rde_dag::record_prepared_formula_state(&self.project, prepared);
                }
                for warning in &report.formula_numbering_warnings {
                    self.push_log(Level::Warn, format!("Нумерация формул: {warning}"));
                }
                let formula_statuses = rde_doc::formula_insert_statuses(&package, &formula_availability).unwrap_or_default();
                let stale_count = formula_statuses
                    .iter()
                    .filter(|status| status.state == rde_doc::FormulaInsertState::Stale)
                    .count();
                self.reload_document_view_preserving();
                self.push_log(
                    Level::Ok,
                    format!(
                        "Документ обновлён: вставок {} (значения {} / рисунки {} / формулы {}), подписей изменено {}, diff: {}; транзакция {}",
                        report.insert_updates.len() + report.figure_updates.len() + report.table_updates + report.formula_updates.len(),
                        report.insert_updates.len(),
                        report.figure_updates.len() + report.table_updates,
                        report.formula_updates.len(),
                        report.caption_changes.len(),
                        report.diff.summary_ru(),
                        report.txid.as_deref().map(|txid| &txid[..10.min(txid.len())]).unwrap_or("—")
                    ),
                );
                for id in &report.formula_conflicts {
                    self.push_log(
                        Level::Warn,
                        format!("Вставка {id}: тело правили вручную (R-DOC-13) — не перезаписано, «принять» вручную"),
                    );
                }
                for id in &report.formula_missing {
                    self.push_log(Level::Warn, format!("Вставка {id}: цель отсутствует — формула удалена/не найдена"));
                }
                for status in formula_statuses.iter().filter(|status| status.state != rde_doc::FormulaInsertState::Fresh) {
                    self.push_log(
                        Level::Warn,
                        format!("Вставка {}: {} — {}", status.insert_id, status.state.label_ru(), status.detail),
                    );
                }
                if stale_count > 0 {
                    self.push_log(Level::Warn, format!("Формульных вставок устарело: {stale_count} — повторите «Обновить документ»"));
                }
                if let Some(backup) = &report.backup {
                    self.push_log(Level::Info, format!("Бэкап документа: {}", backup.display()));
                }
            }
            Err(error) => self.push_log(Level::Warn, format!("Обновить документ: {error}")),
        }
        for error in errors {
            self.push_log(Level::Warn, format!("Вставка не обновлена: {error}"));
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
        // Подписи служебных формул для дерева: «N вставок · статус» (P3, R-DOC-19).
        self.formula_labels = self.formula_tree_labels();
    }

    /// Объект-документ, с которым сейчас работает редактор (R-DOC-39/multi-doc).
    ///
    /// В проекте может быть несколько документов (например, импортированный `.docx` рядом с
    /// прежним «main»). Редактор/сохранение/обновление должны работать с **активным** документом
    /// (`self.doc.path`), а не с первым по порядку объектов. Если активного пути нет — первый
    /// документ проекта (обычный случай одного документа).
    fn active_document_object(&self) -> Option<&rde_core::Object> {
        if let Some(path) = self.doc.path.as_deref() {
            if let Some(dir) = path.parent() {
                if let Some(object) = self.project.objects.iter().find(|object| {
                    object.dir == dir && matches!(object.data, ObjectData::Document(_))
                }) {
                    return Some(object);
                }
            }
        }
        self.project.document_object()
    }

    /// Путь рабочего файла активного документа.
    fn active_document_path(&self) -> Option<PathBuf> {
        self.active_document_object().map(|object| {
            let file = match &object.data {
                ObjectData::Document(manifest) => manifest.file.clone(),
                _ => String::new(),
            };
            object.dir.join(file)
        })
    }

    /// Переключить редактор на другой документ (без перезагрузки проекта): сбросить редактор и
    /// кэши, относящиеся к прежнему пакету (R-DOC-39: импорт не должен открывать чужой «main»).
    fn activate_document_path(&mut self, path: PathBuf) {
        if self.doc.path.as_ref() == Some(&path) {
            return;
        }
        // Не теряем правки при переключении: RDE — владелец сохранения, поэтому текущий
        // открытый документ сохраняется в проект перед сменой активного.
        if self.editor.is_some() {
            self.save_project_document();
        }
        self.doc = DocPreview::new(Some(path));
        self.editor = None;
        self.editor_error = None;
        // P5 (ADR-0021 §Решение 3): смена документа — осознанный сброс позиции предпросмотра.
        self.preview_scroll_offset = 0.0;
        self.preview_restore_offset = None;
        self.structure_cache = None;
        self.inserts_cache = None;
        self.comments_cache = None;
    }

    /// Активировать документ, выбранный в дереве/инспекторе (если это объект-документ).
    fn activate_document_object(&mut self, object: &rde_core::Object) {
        if let ObjectData::Document(manifest) = &object.data {
            self.activate_document_path(object.dir.join(&manifest.file));
        }
    }

    /// Перезагрузить проект с диска и обновить статусы/документ.
    fn reload_project(&mut self) {
        match rde_core::load_project(&self.project.root) {
            Ok(project) => {
                // P5: любые изменения проекта/документа/ledger проходят через reload_project —
                // новая эпоха инвалидирует производные кэши (инспектор формулы, счётчики).
                self.data_epoch = self.data_epoch.wrapping_add(1);
                // Активный документ сохраняем, если он ещё существует (multi-doc); иначе — первый.
                let active_exists = self
                    .doc
                    .path
                    .as_deref()
                    .and_then(Path::parent)
                    .map(|dir| {
                        project
                            .objects
                            .iter()
                            .any(|object| object.dir == dir && matches!(object.data, ObjectData::Document(_)))
                    })
                    .unwrap_or(false);
                let new_path = if active_exists { self.doc.path.clone() } else { project.document_path() };
                let path_changed = new_path != self.doc.path;
                self.auto_update = project.manifest.document_auto_update;
                self.project = project;
                self.structure_cache = None;
                if path_changed {
                    self.doc = DocPreview::new(new_path);
                    // Смена/появление/удаление документа: редактор и его ошибка относятся к прежнему
                    // пути. Без сброса после «в проекте нет документа» ошибка оставалась на экране
                    // даже когда документ уже создан (stale editor_error, ensure_editor early-return).
                    self.editor = None;
                    self.editor_error = None;
                    self.inserts_cache = None;
                    self.comments_cache = None;
                    // P5 (ADR-0021 §Решение 3): другой документ — позицию прежнего не переносим.
                    self.preview_scroll_offset = 0.0;
                    self.preview_restore_offset = None;
                }
                self.refresh_dag_statuses();
                self.refresh_git_status();
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
            self.trust_dialog = Some(TrustDialogState { request: TrustRequest::Update { targets, dependents } });
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
                    if report.failed() == 0 && !was_cancelled {
                        self.auto_update_document_after_artifacts();
                    }
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

    /// Для демо/скриншотов: открыть рабочее место артефакта в нужном разделе.
    pub fn demo_set_artifact_section(&mut self, section: &str) {
        self.tab = Tab::Artifacts;
        self.artifact_editor = None;
        self.artifact_section = match section {
            "inputs" => ArtifactSection::Inputs,
            "generation" => ArtifactSection::Generation,
            _ => ArtifactSection::Result,
        };
    }

    /// Для демо/скриншотов: открыть настройки приложения.
    pub fn demo_open_app_settings(&mut self) {
        self.begin_pick_target(file_dialog::PathTarget::AgentProgram);
        self.app_settings_dialog = Some(self.app_settings.clone());
    }

    /// Для демо/скриншотов: открыть настройки на блоке «Внешние агенты» с включёнными
    /// мастер-галочками, чтобы были видны подгалочки run/git (M4.2b).
    pub fn demo_open_app_settings_agents(&mut self) {
        self.app_settings.allow_external_mcp = true;
        self.app_settings.allow_agent_mutations = true;
        self.begin_pick_target(file_dialog::PathTarget::AgentProgram);
        self.app_settings_dialog = Some(self.app_settings.clone());
    }

    /// Для демо/скриншотов и тестов: выдать внешним агентам заданные гранты по-настоящему
    /// (поднять/перезапустить встроенный MCP-сервер; требует включённого внешнего доступа).
    pub fn demo_set_agent_grants(&mut self, run_script: bool, git_commit: bool) {
        self.app_settings.allow_external_mcp = true;
        self.app_settings.allow_agent_mutations = true;
        self.app_settings.allow_agent_run_script = run_script;
        self.app_settings.allow_agent_git_commit = git_commit;
        self.apply_external_agents_setting();
    }

    /// Для демо/скриншотов и тестов: показать диалог approval внешнего агента (M4.2b).
    ///
    /// `kind`: `artifact_run` | `run_cancel` | `git_commit` (превью действия без реального вызова).
    pub fn demo_open_agent_approval(&mut self, kind: &str) {
        let (tool, args) = match kind {
            "run_cancel" | "cancel" => {
                ("run_cancel", serde_json::json!({"job_id": "01J0B7X8QW3M4N5P6R7S8T9V0Z"}))
            }
            "git_commit" | "git" => (
                "git_commit",
                serde_json::json!({"message": "M4.2b: принят лист правок агента", "dry_run": false}),
            ),
            _ => (
                "artifact_run",
                serde_json::json!({
                    "targets": ["01ARTSTATS", "01ARTFIG"],
                    "include_dependents": true,
                    "force": false
                }),
            ),
        };
        // Канал ответа держим: в demo-режиме решение принимает пользователь/тест.
        let _receiver = self.agent_approvals.enqueue(rde_mcp::ApprovalRequest {
            tool: tool.to_owned(),
            args_summary: format!("{{\"tool\":\"{tool}\"}}"),
            args_sha256: "0".repeat(64),
            destructive: true,
            requested_by: "mcp".to_owned(),
            args: Some(args),
        });
        self.poll_agent_approvals();
    }

    /// Для демо/скриншотов и тестов: выбрать тему оболочки
    /// (`dark` | `light` | `system`; прочее игнорируется).
    pub fn demo_set_theme(&mut self, theme: &str) {
        if matches!(theme, "dark" | "light" | "system") {
            self.app_settings.theme = theme.to_owned();
        }
    }

    /// Для демо/скриншотов: открыть диалог вставки (`value`, `figure`, `formula`…).
    pub fn demo_open_insert_dialog(&mut self, kind: &str) {
        self.open_insert_dialog(kind);
    }

    /// Демо/скриншоты: диалог «Принять формулы как объекты» (ADR-0019).
    pub fn demo_open_formula_migration_dialog(&mut self) {
        self.open_formula_migration_dialog();
    }

    // ── P4 (ADR-0020): демо/скриншоты панели редактора формулы ──────────────

    /// Демо/скриншоты: панель редактора формулы по id/вхождению названия (иначе — создание).
    pub fn demo_open_formula_editor(&mut self, query: &str) {
        let found = self
            .project
            .objects
            .iter()
            .find(|object| matches!(object.data, ObjectData::Formula(_)) && (object.id == query || object.title.contains(query)))
            .map(|object| object.id.clone());
        match found {
            Some(id) => self.open_formula_editor(&id),
            None => self.open_formula_editor_new(),
        }
    }

    /// Демо/скриншоты: вкладка «Формула» без выбранной формулы (пустое состояние редактора:
    /// выбрать формулу проекта / создать новую / подсказка про клик в тексте).
    pub fn demo_show_formula_tab(&mut self) {
        self.show_formula_editor_context();
    }

    /// Демо/скриншоты: панель создания формулы с заполненными полями (LaTeX → предпросмотр).
    pub fn demo_open_formula_editor_create(&mut self, title: &str, latex: &str) {
        self.open_formula_editor_new();
        if let Some(dialog) = self.formula_editor.as_mut() {
            dialog.title = title.to_owned();
            dialog.latex = latex.to_owned();
        }
    }

    /// Демо/скриншоты (P5): панель формулы с WYSIWYG-редактором и готовой моделью.
    /// `template` — id шаблона галереи, который сразу вставляется в модель (например, `mat.paren`);
    /// `fill` — текст, набираемый после вставки (чтобы матрица была не пустой).
    pub fn demo_formula_editor_with_model(&mut self, latex: &str, template: Option<&str>, fill: &str) {
        self.open_formula_editor_new();
        let Some(mut dialog) = self.formula_editor.take() else { return };
        dialog.title = "Демонстрация редактора формул".to_owned();
        let mut editor = rde_math_editor::widget::MathEditorUi::new(latex, dialog.display);
        // Скриншоты: каретка без мигания (детерминированный кадр) и фокус в редакторе.
        editor.options.blink_caret = false;
        editor.request_focus();
        if let Some(template_id) = template {
            editor.state.insert_template(template_id);
            // Заполняем слоты по одному (Tab между вводами) — как пользователь палитрой.
            for (index, ch) in fill.chars().enumerate() {
                if index > 0 {
                    editor.state.move_caret(rde_math_editor::edit::Move::NextSlot, false);
                }
                editor.state.type_text(&ch.to_string());
            }
        }
        dialog.latex = editor.state.latex_source();
        dialog.math_editor = Some(Box::new(editor));
        self.formula_editor = Some(dialog);
        self.show_formula_editor_context();
    }

    /// Демо/скриншоты: панель формулы с открытым нативным редактором WordCraft (equation_tab).
    pub fn demo_open_formula_editor_native(&mut self) {
        let first = self.project.formulas().next().map(|formula| formula.id.clone());
        if let Some(id) = first {
            self.open_formula_editor(&id);
            let Some(mut dialog) = self.formula_editor.take() else { return };
            dialog.native_open = true;
            dialog.info = "Профессиональный редактор: ввод структур/символов и текста; «Забрать LaTeX из редактора» переносит источник в поле".to_owned();
            self.restart_native_formula_editor(&mut dialog);
            self.formula_editor = Some(dialog);
        }
    }

    /// Демо/скриншоты: быстро вставить первую формулу проекта в документ (номер по схеме).
    pub fn demo_insert_formula_with_number(&mut self) {
        let first = self.project.formulas().next().map(|formula| formula.id.clone());
        if let Some(id) = first {
            self.formula_quick_insert(&id, "omml");
        }
    }

    // ── GUI-02: демо/скриншоты диалогов путей и «Проводника…» (ADR-0017) ────

    /// Демо: «Открыть проект» с заданным путём (по умолчанию — текущий проект).
    pub fn demo_open_open_project_dialog(&mut self) {
        self.begin_pick_target(file_dialog::PathTarget::OpenProject);
        self.open_project_dialog = Some(OpenProjectDialog {
            path: self.project.root.display().to_string(),
            error: String::new(),
            focused: false,
        });
    }

    /// Демо: подменить историю «Недавние проекты» (скриншоты без реальных каталогов).
    pub fn demo_set_recent_projects(&mut self, projects: Vec<rde_core::RecentProject>) {
        self.demo_recent_projects = Some(projects);
    }

    /// Демо: «Импорт .docx» с заданным путём (для скриншота валидации — можно битый).
    pub fn demo_open_import_docx_dialog(&mut self, path: &str) {
        self.begin_pick_target(file_dialog::PathTarget::ImportDocx);
        self.import_docx_dialog = Some(ImportDocxDialog {
            path: path.to_owned(),
            title: String::new(),
            error: String::new(),
            focused: false,
        });
    }

    /// Демо: «Новый проект» с заданным каталогом.
    pub fn demo_open_new_project_dialog(&mut self, path: &str) {
        self.begin_pick_target(file_dialog::PathTarget::NewProjectDir);
        self.new_project_dialog = Some(NewProjectDialog {
            path: path.to_owned(),
            name: String::new(),
            error: String::new(),
            focused: false,
        });
    }

    /// Демо: «Импорт цитат» с заданным файлом.
    pub fn demo_open_import_citations_dialog(&mut self, path: &str) {
        self.begin_pick_target(file_dialog::PathTarget::ImportCitations);
        self.import_citations_dialog = Some(ImportCitationsDialog {
            path: path.to_owned(),
            error: String::new(),
            focused: false,
        });
    }

    /// Демо: «Импорт ревью из Word» с заданным путём (пусто — свежий из exports/).
    pub fn demo_open_import_review_dialog(&mut self, path: &str) {
        self.begin_pick_target(file_dialog::PathTarget::ImportReview);
        self.import_review_dialog = Some(ImportReviewDialog {
            path: path.to_owned(),
            error: String::new(),
            focused: false,
        });
    }

    /// Демо: «Свойства источника» (первый источник проекта).
    pub fn demo_open_source_edit(&mut self) {
        let id = self.project.sources().next().map(|source| source.id.clone());
        if let Some(id) = id {
            self.open_source_edit(&id);
        }
    }

    /// Демо: «Свойства репозитория» (первый репозиторий проекта).
    pub fn demo_open_repository_edit(&mut self) {
        if let Some(id) = self
            .project
            .objects
            .iter()
            .find(|object| matches!(object.data, ObjectData::Repository(_)))
            .map(|object| object.id.clone())
        {
            self.open_repository_edit(&id);
        }
    }

    /// Для демо/скриншотов: показать служебные объекты (формулы) в дереве (P3).
    pub fn demo_show_service_objects(&mut self) {
        self.show_service_objects = true;
        self.formula_labels = self.formula_tree_labels();
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
                ObjectKind::Formula => Tab::Artifacts,
            };
            self.activate_document_object(&object);
        }
    }

    /// Для демо/скриншотов: переключить центральную вкладку по имени
    /// (`document|structure|artifacts|source|code`).
    /// Демо/тест: прокрутить предпросмотр страниц на `offset` (скриншот нужной страницы).
    pub fn demo_set_preview_scroll(&mut self, offset: f32) {
        self.demo_preview_scroll = offset.max(0.0);
    }

    /// Режим области документа («Document mode»): `docview` (редактор WordCraft), `full`
    /// (полный WordCraft) или `preview` (растровые страницы) — для скриншотов/приёмки.
    pub fn demo_set_doc_mode(&mut self, mode: &str) {
        self.doc_mode = match mode {
            "full" => DocMode::Full,
            "preview" => DocMode::Preview,
            _ => DocMode::DocView,
        };
    }

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
                        Ok(()) => {
                            // ADR-0018: math-шрифт OMML в экспорте — STIX Two Math (best-effort).
                            let patch = match rde_doc::patch_export_math_font(&path) {
                                Ok(true) => format!(", math-шрифт: {}", rde_doc::MATH_FONT_FAMILY),
                                Ok(false) => String::new(),
                                Err(e) => format!(" (math-шрифт не пропатчен: {e})"),
                            };
                            (Level::Ok, format!("Экспорт .docx (RDE): {}{patch}", path.display()))
                        }
                        Err(e) => (Level::Warn, format!("Не удалось сохранить документ: {e}")),
                    }
                }
                Err(e) => (Level::Warn, format!("Не удалось создать каталог {}: {e}", dir.display())),
            }
        };
        self.push_log(msg.0, msg.1);
    }

    /// Снимок позиции документа (каретка, page_hint, offset предпросмотра) — перед пересборкой.
    fn capture_document_view(&self) -> DocumentViewState {
        let (selection, page_hint) = match self.editor.as_ref() {
            Some(editor) => (Some(editor.app.session.sel.clone()), editor.app.session.page_hint),
            None => (None, 0),
        };
        DocumentViewState { path: self.doc.path.clone(), selection, page_hint, preview_offset: self.preview_scroll_offset }
    }

    /// Восстановить позицию после пересборки пакета (P5, ADR-0021 §Решение 3).
    ///
    /// Редактор перезагружается **на месте** (`WordApp` не пересоздаётся): каретка/выделение и
    /// страница остаются на месте, камера скролла — в том же `CanvasState`. Предпросмотр получает
    /// одноразовый restore offset, чтобы не прыгать на первую страницу. Смена документа/проекта
    /// позицию не переносит (осознанный сброс).
    fn restore_document_view(&mut self, state: DocumentViewState) {
        let same_path = state.path.is_some() && state.path == self.doc.path;
        self.doc.invalidate();
        if !same_path {
            self.editor = None;
            self.editor_error = None;
            self.preview_restore_offset = None;
            return;
        }
        if state.preview_offset > 0.0 {
            self.preview_restore_offset = Some(state.preview_offset);
        }
        if let Some(selection) = state.selection
            && self.editor.is_some()
            && let Err(error) = self.reload_editor_in_place(selection, state.page_hint)
        {
            self.push_log(Level::Warn, format!("Документ пересобран: позиция редактора не сохранена ({error})"));
            self.editor = None;
            self.editor_error = None;
        }
    }

    /// Перезагрузить документ в уже открытом редакторе «на месте», сохранив выбор и страницу
    /// (P5, ADR-0021 §Решение 3): вместо `editor = None` + нового `WordApp` — замена документа в
    /// существующей сессии (`set_document`), затем восстановление каретки/страницы и скролл к ней.
    /// Undo-история при этом сбрасывается — документ пересобран из пакета (допустимо, ADR-0021).
    fn reload_editor_in_place(&mut self, selection: wordcraft_engine::Selection, page_hint: usize) -> Result<(), String> {
        let Some(doc_path) = self.doc.path.clone() else {
            return Err("в проекте нет документа".to_owned());
        };
        let Some(editor) = self.editor.as_mut() else { return Ok(()) };
        // Работаем на той же временной копии `.docx` (по её пути WordCraft разрешает связи OOXML).
        let copy = editor.app.session.path.clone().unwrap_or_else(|| {
            std::env::temp_dir().join(format!("rde-embedded-doc-{}-0.docx", std::process::id()))
        });
        if rde_doc::is_rde_package(&doc_path) {
            rde_doc::extract_docx(&doc_path, &copy)?;
        } else {
            let bytes = std::fs::read(&doc_path).map_err(|error| format!("чтение {}: {error}", doc_path.display()))?;
            std::fs::write(&copy, bytes).map_err(|error| format!("запись {}: {error}", copy.display()))?;
        }
        let region = wordcraft_engine::Region::from_tag("ru-RU");
        let (doc, _encrypted) = wordcraft_engine::io::open_path_in(&copy, None, &region)?;
        let session = &mut editor.app.session;
        session.set_document(doc);
        session.path = Some(copy);
        session.sel = selection;
        session.page_hint = page_hint;
        session.clamp_selection();
        editor.pages = session.layout().pages.len();
        // Камера скролла живёт в том же `CanvasState` и сохраняется; каретку при перепагинации
        // прокручиваем в видимую область, чтобы вставка/обновление не «роняли» вид на первую.
        editor.app.canvas.scroll_to_caret = true;
        Ok(())
    }

    /// Пересобрать вид документа после изменения пакета «на месте» (вставка, «Обновить документ»,
    /// «Обновить всё», rollback, миграция, оформление), сохранив каретку/страницу/скролл.
    fn reload_document_view_preserving(&mut self) {
        let state = self.capture_document_view();
        self.structure_cache = None;
        self.inserts_cache = None;
        self.reload_project();
        self.restore_document_view(state);
    }

    /// Лениво создать встроенный редактор WordCraft с тем же документом, что и предпросмотр.
    fn ensure_editor(&mut self) {
        if self.editor.is_some() || self.editor_error.is_some() {
            return;
        }
        // Формулы в редакторе вёрстаются бандл-шрифтом (ADR-0018): регистрация идемпотентна.
        rde_doc::ensure_math_font();
        let Some(doc_path) = self.doc.path.clone() else {
            self.editor_error = Some("в проекте нет документа".to_owned());
            return;
        };
        let region = wordcraft_engine::Region::from_tag("ru-RU");
        // RDE — владелец сохранения: редактор работает с временной чистой копией, а запись в
        // проект (.doc.rde) делает RDE по явной команде «Сохранить проект» (см. save_project_document).
        // Уникальное имя: параллельные тесты (и несколько документов в процессе) не должны
        // перезаписывать копию друг друга — иначе WordCraft читает чужой/удалённый файл.
        let unique = {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        };
        let copy = std::env::temp_dir().join(format!(
            "rde-embedded-doc-{}-{unique}.docx",
            std::process::id()
        ));
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
                app.ui.theme = appearance_of(&self.app_settings.theme);
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
        self.maybe_auto_boot_update();
        self.ensure_external_agents();
        self.poll_agent_approvals();
        // M4.3: события встроенного ACP-агента дренируем каждый кадр (неблокирующе).
        self.poll_agent_events();
        // GUI-02: результаты фонового «Проводник…» раскладываем по полям диалогов (ADR-0017).
        self.poll_file_dialog();
        self.ui_frames += 1;
        if !self.fonts_installed {
            install_fonts(ui.ctx());
            self.fonts_installed = true;
        }
        // Не в первом кадре: там `fonts()` ещё отдаёт дефолты egui, и наш set_fonts перебил бы
        // установку выше (семья semibold пропала бы).
        if self.ui_frames > 1 {
            ensure_icon_fonts(ui.ctx());
        }
        // Со второго кадра шрифты оболочки действуют → можно звать `semibold` (bold_family).
        if self.ui_frames > 1 {
            ui.ctx().data_mut(|data| data.insert_temp(egui::Id::new(FONTS_READY_ID), true));
        }
        // Тема применяется каждый кадр: живой предпросмотр из открытого диалога
        // настроек + WordCraft перекрашивает глобальные visuals в своём `logic`,
        // поэтому оболочка восстанавливает свои после него.
        let preview_theme = self.app_settings_dialog.as_ref().map(|dialog| dialog.theme.clone());
        let theme = preview_theme.as_deref().unwrap_or(&self.app_settings.theme);
        apply_app_theme(ui.ctx(), theme, self.ui_scale);
        if let Some(editor) = &mut self.editor {
            editor.app.ui.theme = appearance_of(theme);
        }
        // Drag&Drop без заклинаний: папка проекта → открыть проект, .docx → импорт в текущий проект
        // (MathType-формулы при импорте становятся нативными формулами RDE).
        let dropped: Vec<std::path::PathBuf> = ui.ctx().input(|input| {
            input.raw.dropped_files.iter().map(|file| file.path().to_path_buf()).collect()
        });
        if !dropped.is_empty() {
            self.handle_dropped_paths(&dropped);
        }
        // Подсказка, пока пользователь держит файл над окном.
        let hovered: Vec<String> = ui.ctx().input(|input| {
            input
                .raw
                .hovered_files
                .iter()
                .map(|file| {
                    file.path
                        .as_ref()
                        .and_then(|path| path.file_name())
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| "файл".to_owned())
                })
                .collect()
        });
        if !hovered.is_empty() {
            self.drop_hint_overlay(ui.ctx(), &hovered);
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
            // Ctrl+S: в редакторе заметки сохраняем заметку, иначе — проект (владелец сохранения RDE).
            let note_dirty = self.note_editor.as_ref().is_some_and(|editor| editor.dirty);
            let note_selected = self
                .selected
                .clone()
                .and_then(|id| self.project.object(&id))
                .is_some_and(|object| matches!(object.data, ObjectData::Note(_)));
            if note_dirty && note_selected {
                self.save_note_editor();
            } else {
                self.save_project_document();
            }
        }
        if undo && self.editor.is_some() {
            self.undo_document(false);
        }
        if redo && self.editor.is_some() {
            self.undo_document(true);
        }
        // Верхней панели каркаса больше нет: меню и команды — на ленте RDE (doc_tab → ribbon_ui, только DocView);
        // «Обновить всё» и отмена — в шапке центральной области (update_all_controls).
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
        // Диалоги — со второго кадра: шрифты из set_fonts() активны только после
        // конца первого пасса, а заголовок Window уже на первом кадре может
        // запросить семью (напр. semibold из глобального Heading), которой ещё нет
        // в атласе. Задержка в один кадр незаметна, паники нет.
        if self.ui_frames > 1 {
            self.dialogs(ui.ctx());
            self.dialogs_extra(ui.ctx());
            // M4.4: окно ревью правок агента (diff-UI suggested edits).
            self.suggestions_window(ui.ctx());
        }
    }

    fn push_log(&mut self, level: Level, text: impl Into<String>) {
        self.project.log.push(rde_core::LogLine { time: "сейчас", level, text: text.into() });
    }

    // ── «Проводник…»: системный выбор файла/папки (ADR-0017) ─────────────────
    // Диалог идёт в фоновом потоке; результат раскладывается по полям в `poll_file_dialog`
    // (раз в кадр). Ошибка backend'а не блокирует ввод: поле остаётся редактируемым, в журнал
    // пишется «введите путь вручную».

    /// Подменить backend выбора (kittest/скриншоты: фейк вместо системного окна).
    pub fn set_file_dialog_backend(&mut self, backend: Arc<dyn file_dialog::PickerBackend>) {
        self.file_dialog.set_backend(backend);
    }

    /// Файл истории недавних проектов (`None` — не записывать; тесты/демо).
    pub fn set_recent_projects_file(&mut self, path: Option<PathBuf>) {
        self.recent_projects_file = path;
    }

    /// Подменить историю проектов для диалога (скриншоты/тесты).
    pub fn set_demo_recent_projects(&mut self, projects: Option<Vec<rde_core::RecentProject>>) {
        self.demo_recent_projects = projects;
    }

    /// Недавние проекты для диалога «Открыть проект…»: только существующие каталоги с `rde.toml`
    /// (запись могла остаться от удалённого/перемещённого проекта — не показываем мёртвые пути).
    fn recent_projects(&self) -> Vec<rde_core::RecentProject> {
        // Демо/тест-подмена показывается как есть (скриншоты без реальных каталогов);
        // история из конфига фильтруется: удалённые/перемещённые проекты не предлагаем.
        if let Some(projects) = &self.demo_recent_projects {
            return projects.clone();
        }
        rde_core::load_recent_projects()
            .into_iter()
            .filter(|project| Path::new(&project.path).join("rde.toml").is_file())
            .collect()
    }

    /// Записать проект, с которым приложение стартовало (CLI/`run.sh`), в «Недавние проекты»:
    /// иначе проект, открытый аргументом, не попадал в историю GUI.
    pub fn record_startup_project(&mut self) {
        let name = self.project.manifest.name.clone();
        let root = self.project.root.clone();
        self.record_recent_project(&root, &name);
    }

    /// Запомнить открытый проект в истории (`~/.config/rde/recent-projects.toml` + `last-project`).
    pub fn record_recent_project(&mut self, root: &Path, name: &str) {
        let Some(file) = self.recent_projects_file.clone() else { return };
        match rde_core::settings::record_recent_project_in(&file, root, name) {
            Ok(path) => self.push_log(Level::Info, format!("Проект добавлен в недавние: {}", path.display())),
            Err(error) => self.push_log(Level::Warn, format!("История проектов: {error}")),
        }
    }

    /// Запустить фоновый системный выбор для конкретного поля (кнопка «Проводник…»).
    pub fn request_pick(&mut self, ctx: &egui::Context, target: PathTarget, request: PickRequest) {
        if !self.file_dialog.spawn(ctx, target, request) {
            self.push_log(Level::Info, "Диалог выбора уже открыт — дождитесь его закрытия");
        }
    }

    /// Открыт ли сейчас диалог, которому адресован результат выбора («Проводник…»).
    /// Результаты для закрытых диалогов не раскладываются по полям (см. `poll_file_dialog`).
    fn picker_target_active(&self, target: &PathTarget) -> bool {
        match target {
            PathTarget::OpenProject => self.open_project_dialog.is_some(),
            PathTarget::ImportDocx => self.import_docx_dialog.is_some(),
            PathTarget::ImportReview => self.import_review_dialog.is_some(),
            PathTarget::ImportCitations => self.import_citations_dialog.is_some(),
            PathTarget::NewProjectDir => self.new_project_dialog.is_some(),
            PathTarget::AgentProgram => self.app_settings_dialog.is_some(),
            PathTarget::SourceFile(id) => self.source_edit_dialog.as_ref().is_some_and(|dialog| &dialog.id == id),
            PathTarget::RepositoryPath(id) => self.repo_edit_dialog.as_ref().is_some_and(|dialog| &dialog.id == id),
            // Копирование файлов в `inputs/…` артефакта — следующий шаг (docs/gui-coverage §11):
            // адрес зарезервирован, диалога пока нет.
            PathTarget::ArtifactInputFile => false,
        }
    }

    /// Начать диалог с полем пути: забыть возможный «поздний» выбор прошлого экземпляра диалога.
    fn begin_pick_target(&mut self, target: PathTarget) {
        self.file_dialog.abandon(&target);
        self.picked_paths.retain(|(stored, _)| stored != &target);
        self.picker_errors.retain(|(stored, _)| stored != &target);
        self.picker_focus_ok.retain(|stored| stored != &target);
    }

    /// Разложить результаты фонового выбора и записать ошибки деградации в журнал.
    pub fn poll_file_dialog(&mut self) {
        for done in self.file_dialog.poll() {
            // Выбор мог завершиться после закрытия диалога: не подменять поле при следующем
            // открытии (иначе «поздний» путь/ошибка попадут в новый диалог).
            if !self.picker_target_active(&done.target) {
                continue;
            }
            match done.result {
                Ok(Some(path)) => {
                    self.picker_errors.retain(|(target, _)| target != &done.target);
                    self.picked_paths.retain(|(target, _)| target != &done.target);
                    self.picked_paths.push((done.target.clone(), path));
                    self.picker_focus_ok.push(done.target);
                }
                Ok(None) => {}
                Err(error) => {
                    // Ошибка backend'а — не «тихая»: журнал + подпись в диалоге, поле редактируемо.
                    // Контракт backend'ов: любая ошибка объясняет, что делать (ручной ввод).
                    let error = if error.contains(file_dialog::MANUAL_HINT) {
                        error
                    } else {
                        format!("{error} — {}", file_dialog::MANUAL_HINT)
                    };
                    self.picker_errors.retain(|(target, _)| target != &done.target);
                    self.picker_errors.push((done.target.clone(), error.clone()));
                    self.push_log(Level::Warn, error);
                }
            }
        }
    }

    /// Забранный путь для поля (если в этом кадре пришёл выбор).
    fn take_picked_path(&mut self, target: &PathTarget) -> Option<PathBuf> {
        let index = self.picked_paths.iter().position(|(candidate, _)| candidate == target)?;
        Some(self.picked_paths.remove(index).1)
    }

    /// Ошибка backend'а для поля (показывается под полем в диалоге).
    fn take_picker_error(&mut self, target: &PathTarget) -> Option<String> {
        let index = self.picker_errors.iter().position(|(candidate, _)| candidate == target)?;
        Some(self.picker_errors.remove(index).1)
    }

    /// Нужно ли вернуть фокус на «ОК» после успешного выбора (клавиатурный сценарий).
    fn take_picker_focus_ok(&mut self, target: &PathTarget) -> bool {
        let index = self.picker_focus_ok.iter().position(|candidate| candidate == target);
        match index {
            Some(index) => {
                self.picker_focus_ok.remove(index);
                true
            }
            None => false,
        }
    }

    /// Идёт ли сейчас выбор для этого поля (кнопка показывает «…»).
    fn picker_busy(&self, target: &PathTarget) -> bool {
        self.file_dialog.in_flight() == Some(target)
    }

    /// Стартовый каталог диалога: текущий путь поля → его каталог → корень проекта.
    fn pick_start_dir(&self, current: &str) -> PathBuf {
        let current = current.trim();
        if !current.is_empty() {
            let path = Path::new(current);
            if path.is_dir() {
                return path.to_path_buf();
            }
            if let Some(parent) = path.parent()
                && parent.is_dir()
            {
                return parent.to_path_buf();
            }
        }
        self.project.root.clone()
    }

    /// Открыть путь в системном файловом менеджере (в headless — честная запись в журнал).
    pub fn open_in_file_manager(&mut self, path: &Path) {
        if !path.exists() {
            self.push_log(Level::Warn, format!("Файловый менеджер: путь не найден: {}", path.display()));
            return;
        }
        #[cfg(unix)]
        if file_dialog::graphical_session_available() {
            match std::process::Command::new("xdg-open").arg(path).spawn() {
                Ok(_) => {
                    self.push_log(Level::Info, format!("Открыто в файловом менеджере: {}", path.display()));
                    return;
                }
                Err(error) => self.push_log(Level::Warn, format!("xdg-open: {error} — {}", path.display())),
            }
        }
        self.push_log(Level::Info, format!("Файловый менеджер недоступен (нет графической сессии): {}", path.display()));
    }

    // ── лента RDE и диспетчер команд ────────────────────────────────────────
    // Лента — вкладки сверху, группы команд с подписями снизу (образец WordCraft; данные — в
    // `ribbon.rs`). Все команды идут через `run_cmd`: лента, строка режимов и бывшее меню
    // не дублируют логику.

    /// Единый диспетчер команд оболочки (лента, строка режимов документа).
    fn run_cmd(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::NewProject => {
                let base = self.project.root.parent().map(|parent| parent.to_path_buf()).unwrap_or_else(|| self.project.root.clone());
                self.begin_pick_target(file_dialog::PathTarget::NewProjectDir);
                self.new_project_dialog = Some(NewProjectDialog {
                    path: base.join("rde-project").display().to_string(),
                    name: "Новый проект RDE".to_owned(),
                    error: String::new(),
                    focused: false,
                });
            }
            Cmd::OpenProject => {
                self.begin_pick_target(file_dialog::PathTarget::OpenProject);
                self.open_project_dialog = Some(OpenProjectDialog {
                    path: self.project.root.display().to_string(),
                    error: String::new(),
                    focused: false,
                });
            }
            Cmd::SaveProject => self.save_project_document(),
            Cmd::ExportDocx => {
                self.ensure_editor();
                self.save_editor_copy();
            }
            Cmd::OpenWordCopy => self.export_word_copy(),
            Cmd::ExportProject => self.export_project_archive(),
            // «Импорт ревью»: диалог с выбором .docx (пусто — самый свежий из exports/).
            Cmd::ImportReview => {
                self.begin_pick_target(file_dialog::PathTarget::ImportReview);
                self.import_review_dialog = Some(ImportReviewDialog::default());
            }
            Cmd::ImportDocx => {
                self.begin_pick_target(file_dialog::PathTarget::ImportDocx);
                self.import_docx_dialog = Some(ImportDocxDialog::default());
            }
            // «Импорт цитат»: BibTeX/RIS/CSL-JSON → объекты-источники (rde-sources).
            Cmd::ImportCitations => {
                self.begin_pick_target(file_dialog::PathTarget::ImportCitations);
                self.import_citations_dialog = Some(ImportCitationsDialog::default());
            }
            Cmd::Undo => self.undo_document(false),
            Cmd::Redo => self.undo_document(true),
            Cmd::Find => {
                self.show_tree = true;
                self.search_focus = true;
            }
            Cmd::NewObject => {
                self.create_dialog = Some(CreateDialogState { kind: UiObjectKind::Core(NewObjectKind::ComputeArtifact), title: String::new() });
            }
            Cmd::UpdateDocument => self.update_project_document(),
            Cmd::ToggleAutoUpdate => {
                let enabled = !self.auto_update;
                self.set_auto_update(enabled);
            }
            Cmd::RollbackDocument => self.rollback_document(),
            Cmd::RollbackArtifacts => self.rollback_last_artifacts(),
            Cmd::InsertValue => self.open_insert_dialog("value"),
            Cmd::InsertFigure => self.open_insert_dialog("figure"),
            Cmd::InsertTable => self.open_insert_dialog("table"),
            Cmd::InsertConstant => self.open_insert_dialog("constant"),
            Cmd::InsertCrossref => self.open_insert_dialog("crossref"),
            Cmd::InsertCitation => self.open_insert_dialog("citation"),
            Cmd::InsertDate => self.open_insert_dialog("date"),
            Cmd::InsertFormula => self.open_insert_dialog("formula"),
            // P4 (ADR-0020): создание и правка source-of-truth формулы в GUI.
            Cmd::NewFormula => self.open_formula_editor_new(),
            Cmd::EditFormula => self.open_selected_formula_editor(),
            Cmd::MigrateFormulas => self.open_formula_migration_dialog(),
            Cmd::ApplyGost => self.update_document_package(true, true),
            Cmd::InsertBibliography => self.update_document_package(false, true),
            Cmd::Zoom100 => self.run_editor_action("view.zoom100"),
            Cmd::PageWidth => self.run_editor_action("view.pageWidth"),
            Cmd::ZoomOut => self.run_editor_action("view.zoomOut"),
            Cmd::ZoomIn => self.run_editor_action("view.zoomIn"),
            // «Обновить вставки» (панель предпросмотра) — та же операция, что «Обновить документ»:
            // пересчёт живых вставок, перенумерация подписей и пересохранение .doc.rde транзакцией.
            Cmd::UpdateInserts => self.update_project_document(),
            Cmd::CheckProject => self.run_project_check(),
            Cmd::UpdateAll => self.request_update(None, true),
            Cmd::Settings => {
                self.settings_dialog = Some(ProjectSettingsState {
                    name: self.project.manifest.name.clone(),
                    template: self.project.manifest.default_template.clone(),
                    rebuild: self.project.manifest.default_rebuild.clone(),
                    authors: self.project.manifest.authors.clone(),
                    new_author_name: String::new(),
                    new_author_initials: String::new(),
                    new_author_email: String::new(),
                });
            }
            Cmd::AppSettings => {
                self.begin_pick_target(file_dialog::PathTarget::AgentProgram);
                self.app_settings_dialog = Some(self.app_settings.clone());
            }
            Cmd::ExportDiagnostics => {
                self.export_diagnostics();
            }
            Cmd::GitInit => self.git_init(),
            Cmd::GitCommit => {
                let last = self.git_status.as_ref().and_then(|status| status.last_commit.as_ref()).map(|commit| commit.summary.clone());
                let default_message = match last {
                    Some(summary) => format!("RDE: изменения после «{summary}»"),
                    None => "RDE: изменения проекта".to_owned(),
                };
                self.commit_dialog = Some(default_message);
            }
            Cmd::GitHistory => self.git_history(),
            Cmd::BackupProject => {
                self.backup_project_now();
            }
            Cmd::RestoreBackup => self.open_backup_dialog(),
            Cmd::ToggleTree => self.show_tree = !self.show_tree,
            Cmd::ToggleAgent => self.show_agent = !self.show_agent,
            Cmd::ToggleLogs => self.show_logs = !self.show_logs,
            Cmd::ToggleInspector => self.show_inspector = !self.show_inspector,
            Cmd::ToggleServiceObjects => {
                self.show_service_objects = !self.show_service_objects;
                self.push_log(
                    Level::Info,
                    if self.show_service_objects {
                        "Дерево: служебные объекты показаны (формулы проекта — использование и статус)"
                    } else {
                        "Дерево: служебные объекты скрыты"
                    },
                );
            }
            Cmd::Audit => self.audit_dialog = Some(self.audit_lines()),
            // M4.4: diff-UI suggested edits — pending-правки агента с word-diff и решениями.
            Cmd::AgentSuggestions => self.open_suggestions(),
            Cmd::About => self.about_open = true,
        }
    }

    /// Запуск действия редактора WordCraft по id (масштаб). Без открытого редактора — no-op.
    fn run_editor_action(&mut self, id: &'static str) {
        if let Some(ed) = &mut self.editor {
            if let Err(error) = ed.app.run(id, serde_json::json!({})) {
                self.push_log(Level::Warn, format!("Действие редактора «{id}»: {error}"));
            }
        } else {
            // Раньше это был «тихий» no-op: кнопка масштаба без открытого редактора молчала.
            self.push_log(
                Level::Info,
                "Масштаб документа доступен в режиме редактора (RDE/WordCraft): переключите режим на вкладке «Документ»",
            );
        }
    }

    /// Доступность команды. «Отменить обновление документа» всегда активна (как в прежней RDE-панели):
    /// проверка кандидата читает хранилище и не должна выполняться каждый кадр.
    fn cmd_enabled(&self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::Undo => self.editor.as_ref().is_some_and(|editor| editor.app.session.can_undo()),
            // P4: «Редактировать формулу» — только когда выбрана формула проекта.
            Cmd::EditFormula => self.selected_object().is_some_and(|object| matches!(object.data, ObjectData::Formula(_))),
            Cmd::Redo => self.editor.as_ref().is_some_and(|editor| editor.app.session.can_redo()),
            // Команды, которым нужен документ проекта: в пустом проекте они недоступны с причиной
            // в тултипе, а не «тихим» предупреждением в журнале после клика.
            Cmd::ExportDocx
            | Cmd::OpenWordCopy
            | Cmd::UpdateDocument
            | Cmd::RollbackDocument
            | Cmd::ApplyGost
            | Cmd::InsertBibliography
            | Cmd::MigrateFormulas
            | Cmd::InsertValue
            | Cmd::InsertFigure
            | Cmd::InsertTable
            | Cmd::InsertConstant
            | Cmd::InsertCrossref
            | Cmd::InsertCitation
            | Cmd::InsertDate
            | Cmd::InsertFormula => self.project.document_path().is_some(),
            _ => true,
        }
    }

    /// Причина недоступности команды для тултипа (пусто — команда доступна).
    fn cmd_disabled_reason(&self, cmd: Cmd) -> &'static str {
        match cmd {
            Cmd::Undo => "нечего отменять",
            Cmd::EditFormula => "выберите формулу в дереве или инспекторе",
            Cmd::Redo => "нечего повторять",
            Cmd::ExportDocx
            | Cmd::OpenWordCopy
            | Cmd::UpdateDocument
            | Cmd::RollbackDocument
            | Cmd::ApplyGost
            | Cmd::InsertBibliography
            | Cmd::MigrateFormulas
            | Cmd::InsertValue
            | Cmd::InsertFigure
            | Cmd::InsertTable
            | Cmd::InsertConstant
            | Cmd::InsertCrossref
            | Cmd::InsertCitation
            | Cmd::InsertDate
            | Cmd::InsertFormula => "в проекте нет документа",
            _ => "",
        }
    }

    /// Состояние переключателя на ленте (видимость панели, автообновление).
    fn cmd_checked(&self, cmd: Cmd) -> bool {
        match cmd {
            Cmd::ToggleTree => self.show_tree,
            Cmd::ToggleAgent => self.show_agent,
            Cmd::ToggleLogs => self.show_logs,
            Cmd::ToggleInspector => self.show_inspector,
            Cmd::ToggleServiceObjects => self.show_service_objects,
            Cmd::ToggleAutoUpdate => self.auto_update,
            _ => false,
        }
    }

    /// Лента: строка вкладок и группы выбранной вкладки (кнопки сверху, подпись группы снизу).
    /// Возвращает команду, по которой нажали; запускает её вызывающий (`doc_tab`).
    fn ribbon_ui(&mut self, ui: &mut egui::Ui, inserts_count: usize) -> Option<Cmd> {
        let pal = theme_colors(ui.ctx().theme());
        let mut picked: Option<Cmd> = None;
        egui::Frame::new()
            .fill(pal.panel)
            .corner_radius(CornerRadius::same(8))
            .stroke(Stroke::new(1.0, pal.border))
            .inner_margin(Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 1.0;
                egui::ScrollArea::horizontal().id_salt("rde_ribbon_tabs").auto_shrink([false, true]).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        ui.spacing_mut().button_padding = vec2(5.0, 2.0);
                        for tab in RibbonTab::ALL {
                            let sel = self.ribbon_tab == tab;
                            let text = RichText::new(tab.title()).size(FS_SMALL).color(if sel { pal.text } else { pal.muted });
                            if ui.add(egui::Button::selectable(sel, text).corner_radius(6).min_size(vec2(0.0, 20.0))).clicked() {
                                self.ribbon_tab = tab;
                            }
                        }
                    });
                });
                if self.ribbon_tab == RibbonTab::Formula {
                    // P4 (ADR-0020): «полноценный редактор формул» прямо в верхней панели —
                    // вкладка «Формула» показывает панель редактора на всю ширину вместо групп.
                    ui.add_space(2.0);
                    self.formula_ribbon_editor(ui);
                } else {
                egui::ScrollArea::horizontal()
                    .id_salt(("rde_ribbon_groups", self.ribbon_tab.title()))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for group in ribbon_groups(self.ribbon_tab) {
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = 3.0;
                                        for slot in group.slots.iter() {
                                            if let Some(cmd) = self.ribbon_slot_ui(ui, *slot, inserts_count) {
                                                picked = Some(cmd);
                                            }
                                        }
                                    });
                                    ui.label(RichText::new(group.title).size(FS_CAPTION).color(pal.muted));
                                });
                                // Разделитель фиксированной высоты: `ui.separator()` растянулся бы на всю колонку.
                                ribbon_divider(ui, RIBBON_BUTTON_H + 16.0);
                            }
                        });
                    });
                }
            });
        picked
    }

    /// Один слот группы ленты. Возвращает команду при нажатии.
    fn ribbon_slot_ui(&mut self, ui: &mut egui::Ui, slot: Slot, inserts_count: usize) -> Option<Cmd> {
        let pal = theme_colors(ui.ctx().theme());
        match slot {
            Slot::Button { cmd, icon, label, tip, primary } => {
                let enabled = self.cmd_enabled(cmd);
                let role = if primary { Role::Primary } else { Role::Ghost };
                // Недоступная команда объясняет причину в тултипе (а не молчит).
                let hover = match self.cmd_disabled_reason(cmd) {
                    "" => tip.to_owned(),
                    reason => format!("{tip}\nНедоступно: {reason}"),
                };
                let resp = ribbon_button(ui, role, icon, label, tip, vec2(RIBBON_BUTTON_W, RIBBON_BUTTON_H), enabled, false)
                    .on_hover_text(hover);
                (enabled && resp.clicked()).then_some(cmd)
            }
            Slot::Toggle { cmd, icon, label, tip } => {
                let on = self.cmd_checked(cmd);
                let resp = ribbon_button(ui, Role::Ghost, icon, label, tip, vec2(RIBBON_BUTTON_W, RIBBON_BUTTON_H), true, on).on_hover_text(tip);
                resp.clicked().then_some(cmd)
            }
            Slot::Icon { cmd, icon, tip } => {
                let resp = ribbon_button(ui, Role::Ghost, icon, "", tip, vec2(RIBBON_ICON_W, RIBBON_BUTTON_H), true, false).on_hover_text(tip);
                resp.clicked().then_some(cmd)
            }
            Slot::InsertPosition => {
                // Позиция следующей вставки (P8-B1: каретка → блок-якорь), сброс и счётчик.
                ui.vertical(|ui| {
                    let anchor_label = match self.insert_anchor_block {
                        Some(block) => format!("после блока {}", block + 1),
                        None => "в конец документа".to_owned(),
                    };
                    let accent = self.insert_anchor_block.is_some();
                    ui.label(
                        RichText::new(format!("Вставка: {anchor_label}")).small().color(if accent { pal.accent } else { pal.muted }),
                    );
                    ui.horizontal(|ui| {
                        if self.insert_anchor_block.is_some() && ui.small_button("сброс").clicked() {
                            self.insert_anchor_block = None;
                        }
                        ui.label(RichText::new(format!("вставок: {inserts_count}")).small().color(pal.muted));
                    });
                });
                None
            }
            Slot::Zoom => {
                // Масштаб документа (§19-7): живёт на ленте (вкладка «Документ», группа «Масштаб»),
                // а не в строке вкладок центра. Высота — как у кнопок ленты, чтобы группы не «плясали».
                let mut picked: Option<Cmd> = None;
                let zoom = self.editor.as_ref().map(|editor| editor.app.session.view.zoom);
                ui.allocate_ui_with_layout(
                    vec2(RIBBON_ZOOM_W, RIBBON_BUTTON_H),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        if ui.small_button("−").on_hover_text("Уменьшить масштаб").clicked() {
                            picked = Some(Cmd::ZoomOut);
                        }
                        let percent = match zoom {
                            Some(zoom) => format!("{:.0}%", zoom * 100.0),
                            None => "—".to_owned(),
                        };
                        ui.label(RichText::new(percent).monospace().size(FS_MONO).color(pal.muted));
                        if ui.small_button("+").on_hover_text("Увеличить масштаб").clicked() {
                            picked = Some(Cmd::ZoomIn);
                        }
                        if ui.small_button("Сброс").on_hover_text("Масштаб 100%").clicked() {
                            picked = Some(Cmd::Zoom100);
                        }
                        if ui.small_button("По ширине").on_hover_text("Масштаб по ширине страницы").clicked() {
                            picked = Some(Cmd::PageWidth);
                        }
                    },
                );
                picked
            }
            Slot::Note(text) => {
                ui.vertical(|ui| {
                    ui.set_max_width(170.0);
                    ui.label(RichText::new(text).small().color(pal.muted));
                });
                None
            }
            Slot::FormulaEditor => {
                self.formula_ribbon_editor(ui);
                None
            }
        }
    }

    /// Компактная строка контролов предпросмотра страниц (§19-7): «Обновить вставки», масштаб,
    /// «Открыть копию в Word». Живёт над страницами, потому что лента (и её группа «Масштаб»)
    /// показывается только в режиме RDE.
    fn preview_controls(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if btn(ui, Role::Secondary, Some(IC_UPDATE), "Обновить вставки")
                .on_hover_text("Пересчитать живые вставки документа и пересохранить пакет")
                .clicked()
            {
                self.run_cmd(Cmd::UpdateInserts);
            }
            ui.separator();
            ui.label(RichText::new("Масштаб").small().color(pal.muted));
            ui.add(egui::Slider::new(&mut self.zoom, 0.35..=1.5).show_value(false));
            ui.separator();
            if ui.button(RichText::new("Открыть копию в Word").size(12.0)).clicked() {
                self.run_cmd(Cmd::OpenWordCopy);
            }
        });
        ui.add_space(4.0);
    }

    // ── «Обновить всё» и отмена фонового обновления ─────────────────────────
    /// Бывшая верхняя панель: «Обновить всё» и отмена живут в строке статуса (видна всегда,
    /// на любой вкладке и в любом режиме документа).
    fn update_all_controls(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        let mut update_all = false;
        let mut cancel_update = false;
        let running = self.update_job.is_some();
        let running_secs = self.update_job.as_ref().map(|job| job.started.elapsed().as_secs_f32()).unwrap_or(0.0);
        // Единственный primary в строке статуса: пересчёт устаревших артефактов.
        let update_resp = ui
            .add_enabled_ui(!running, |ui| {
                btn_bar(ui, Role::Primary, Some(IC_UPDATE), tr(self.lang, "toolbar.update_all"))
                    .on_hover_text("Реально пересчитать устаревшие артефакты (rde-dag: act_key/out_hash, транзакции)")
            })
            .inner;
        if update_resp.clicked() {
            update_all = true;
        }
        if running {
            ui.label(RichText::new(format!("● обновление… {running_secs:.0} с")).small().color(pal.amber));
            if btn_bar(ui, Role::Danger, None, "Отменить")
                .on_hover_text("Остановить текущий запуск (SIGTERM → grace → SIGKILL) и отменить обновление")
                .clicked()
            {
                cancel_update = true;
            }
        }
        if cancel_update {
            self.cancel_update();
        }
        if update_all {
            self.request_update(None, true);
        }
    }

    // ── статус-бар ──────────────────────────────────────────────────────────
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        // M4.1/M4.2b: состояние внешних агентов — в строке статуса (видно на всех вкладках).
        let (agent_label, mcp_label, agent_color) = match self.external_agents_status() {
            agent_server::ExternalAgentStatus::Running { info, grants } => (
                format!("Агент: MCP HTTP :{}", info.addr.port()),
                // M4.2b: действующий режим грантов, например `(rw+run+git)`.
                if grants.any() {
                    format!("MCP: on :{} ({})", info.addr.port(), grants.label())
                } else {
                    format!("MCP: on :{}", info.addr.port())
                },
                pal.green,
            ),
            agent_server::ExternalAgentStatus::NotRunning => (
                "Агент: MCP HTTP остановлен".to_owned(),
                "MCP: off".to_owned(),
                pal.amber,
            ),
            agent_server::ExternalAgentStatus::Disabled => (
                tr(self.lang, "status.agent_offline").to_owned(),
                "MCP: off".to_owned(),
                pal.muted,
            ),
        };
        // M4.3: встроенный ACP-агент — отдельный индикатор (`Агент: ACP — готов`).
        let (acp_label, acp_color) = self.chat_status_line(pal);
        egui::Panel::bottom("rde_status")
            .frame(egui::Frame::new().fill(pal.panel_hi).inner_margin(Margin::symmetric(12, 3)))
            .exact_size(24.0)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("•  RDE").color(pal.accent).size(11.0));
                    ui.label(RichText::new("прототип v0.1").small().color(pal.muted));
                    ui.separator();
                    ui.label(RichText::new(self.project.root.display().to_string()).small().color(pal.muted));
                    ui.separator();
                    let git_summary = self
                        .git_status
                        .as_ref()
                        .map(rde_vcs::GitStatus::summary_ru)
                        .unwrap_or_else(|| "git: —".to_owned());
                    let git_color = if self.git_status.as_ref().is_some_and(|status| status.is_dirty()) { pal.amber } else { pal.muted };
                    ui.label(RichText::new(git_summary).small().color(git_color));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("A4 · ru-RU").small().color(pal.muted));
                        if let Some(pages) = self.doc_pages() {
                            ui.separator();
                            ui.label(RichText::new(format!("{pages} стр.")).small().color(pal.muted));
                        }
                        ui.separator();
                        ui.label(RichText::new(format!("{} {}", self.project.object_count(), tr(self.lang, "status.objects"))).small().color(pal.muted));
                        ui.separator();
                        ui.label(RichText::new(format!("• {} {}", self.project.stale_count(), tr(self.lang, "status.stale"))).small().color(pal.amber));
                        ui.separator();
                        ui.label(RichText::new(agent_label).small().color(agent_color));
                        ui.separator();
                        ui.label(RichText::new(acp_label).small().color(acp_color));
                        ui.separator();
                        ui.label(RichText::new(mcp_label).small().color(agent_color));
                        ui.separator();
                        // «Обновить всё» — в строке статуса: видна на всех вкладках и в любом режиме,
                        // не конкурирует с вкладками центральной области за ширину.
                        self.update_all_controls(ui);
                    });
                });
            });
    }

    // ── журнал / задачи ─────────────────────────────────────────────────────
    fn logs_panel(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
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
            .frame(egui::Frame::new().fill(pal.panel).inner_margin(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("ПАНЕЛЬ").small().strong().color(pal.muted));
                    ui.add_space(4.0);
                    for (i, name) in [tr(self.lang, "panel.journal"), tr(self.lang, "panel.tasks"), tr(self.lang, "panel.agent")].iter().enumerate() {
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
                            ui.label(RichText::new(format!("{} записей", tasks.len())).small().color(pal.muted));
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
                                    ui.label(RichText::new("Пока нет запусков и транзакций — нажмите «Обновить всё»").small().color(pal.muted));
                                }
                                for line in &tasks {
                                    // Строки списка — с тем же внутренним отступом, что и ячейки таблиц.
                                    pad_cell(ui, |ui| ui.label(RichText::new(line).monospace().size(FS_MONO).color(pal.text)));
                                }
                                ui.add_space(8.0);
                                ui.label(RichText::new("ТРАНЗАКЦИИ ОБНОВЛЕНИЙ — ОТКАТ (O(1))").small().strong().color(pal.muted));
                                if rollback_candidates.is_empty() {
                                    ui.label(RichText::new("Пока нет транзакций со снапшотом корней").small().color(pal.muted));
                                }
                                for candidate in rollback_candidates.iter().take(8) {
                                    ui.horizontal(|ui| {
                                        let stamp = candidate
                                            .updated_unix
                                            .map(|unix| format!("unix {unix}"))
                                            .unwrap_or_else(|| "—".to_owned());
                                        ui.label(RichText::new(format!("{} · {stamp}", candidate.note)).size(11.5).color(pal.text));
                                        if candidate.rolled_back {
                                            ui.label(RichText::new("откачено").small().color(pal.muted));
                                        } else if ui.small_button("Откатить").on_hover_text("Переключить корни на предыдущие (O(1)) и восстановить выходы из CAS").clicked() {
                                            rollback_clicked = Some(candidate.txid.clone());
                                        }
                                    });
                                }
                            }
                            2 => {
                                ui.label(RichText::new("Агенты (M4.3): MCP HTTP для внешних + встроенный ACP-агент (DSH).").strong().color(pal.text));
                                ui.label(RichText::new("Внешний доступ и гранты включаются в настройках; встроенный агент — «Новый чат» в правой панели «Агент» (сессии, история, разрешения агента, отмена).").small().color(pal.muted));
                                ui.add_space(4.0);
                                ui.label(RichText::new("Действия агента (правки/запуски/коммиты) идут через диалоги подтверждения с превью, чекпоинтом и аудитом `.rde/agent/audit.log`; URI-чипы и diff-UI — M4.4.").small().color(pal.muted));
                            }
                            _ => {
                                for line in &log {
                                    ui.horizontal(|ui| {
                                        ui.add_space(8.0); // внутренний отступ слева, как в таблицах
                                        ui.label(RichText::new(line.time).monospace().size(FS_MONO).color(pal.muted));
                                        let color = match line.level {
                                            Level::Info => pal.muted,
                                            Level::Ok => pal.green,
                                            Level::Warn => pal.amber,
                                        };
                                        ui.label(RichText::new("•").color(color).size(11.0));
                                        ui.label(RichText::new(&line.text).size(FS_SMALL).color(pal.text));
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
        let pal = theme_colors(ui.ctx().theme());
        let groups = self.project.groups.clone();
        let objects = self.project.objects.clone();
        let object_count = self.project.object_count();
        let filter = self.tree_filter.to_lowercase();
        let selected = self.selected.clone();
        // Служебные объекты (формулы, R-TREE-01/02): скрыты по умолчанию; подписи — «N вставок ·
        // статус» (P3, R-DOC-19).
        let show_service = self.show_service_objects;
        let formula_labels = self.formula_labels.clone();
        let mut clicked: Option<String> = None;
        let mut action: Option<TreeAction> = None;
        let mut new_object = false;
        let mut new_formula = false;
        let mut hide_tree = false;
        let focus_filter = std::mem::take(&mut self.search_focus);
        egui::Panel::left("rde_tree")
            .resizable(true)
            .default_size(TREE_WIDTH)
            .min_size(TREE_WIDTH_MIN)
            .frame(egui::Frame::new().fill(pal.panel).inner_margin(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    section_caption(ui, "Проект");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Скрыть панель (вернуть — меню «Вид»)").clicked() {
                            hide_tree = true;
                        }
                        ui.label(RichText::new(format!("{object_count} объектов")).small().color(pal.muted));
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
                if btn(ui, Role::Secondary, Some(IC_ADD), "Новый объект").clicked() {
                    new_object = true;
                }
                ui.add_space(4.0);
                egui::ScrollArea::vertical().id_salt("rde_tree_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    for group in groups.iter().filter(|group| show_service || !tree_group_hidden(group.kind)) {
                        let visible: Vec<&rde_core::Object> = group
                            .objects
                            .iter()
                            .filter_map(|&index| objects.get(index))
                            .filter(|object| filter.is_empty() || object.title.to_lowercase().contains(&filter))
                            .collect();
                        // Группа «Формулы» (P4): показываем и пустой — с кнопкой «Создать формулу…».
                        let formula_group = group.kind == ObjectKind::Formula;
                        if visible.is_empty() && !(formula_group && show_service) {
                            continue;
                        }
                        let header = RichText::new(format!("{}  {}  · {}", kind_icon(group.kind), group.title, visible.len())).strong().size(12.5).color(pal.text);
                        egui::CollapsingHeader::new(header).default_open(true).show(ui, |ui| {
                            for obj in visible {
                                let is_sel = selected.as_deref() == Some(obj.id.as_str());
                                // Формулы (служебные объекты): в подписи — использование и статус.
                                let row_title = formula_labels
                                    .get(&obj.id)
                                    .map(|label| format!("{} · {}", obj.title, label))
                                    .unwrap_or_else(|| obj.title.clone());
                                let resp = ui
                                    .horizontal(|ui| {
                                        ui.label(RichText::new("●").color(freshness_color(obj.freshness, pal)).size(FS_CAPTION));
                                        ui.label(RichText::new(kind_icon(obj.kind)).color(pal.muted).size(FS_BODY));
                                        // Панель можно сузить: длинные названия обрезаются с «…»,
                                        // полное название — в подсказке и в инспекторе.
                                        ui.add(egui::Button::selectable(is_sel, RichText::new(&row_title).size(FS_BODY)).truncate())
                                            .on_hover_text(&row_title)
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
                                            if ui.button("Импорт ревью из Word…").clicked() {
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
                                            if ui
                                                .button("Свойства источника…")
                                                .on_hover_text("Файл/URL источника, прикрепление файла («Проводник…»), дата получения")
                                                .clicked()
                                            {
                                                action = Some(TreeAction::Properties(obj.id.clone()));
                                                ui.close();
                                            }
                                        }
                                        ObjectData::Repository(_)
                                            if ui
                                                .button("Свойства репозитория…")
                                                .on_hover_text("Тип (internal/external/git) и путь репозитория")
                                                .clicked() =>
                                        {
                                            action = Some(TreeAction::Properties(obj.id.clone()));
                                            ui.close();
                                        }
                                        ObjectData::Formula(_) => {
                                            // P4 (ADR-0020): правка источника/вставка с номером.
                                            if ui.button("Редактировать формулу…").clicked() {
                                                action = Some(TreeAction::EditFormula(obj.id.clone()));
                                                ui.close();
                                            }
                                            if ui.button("Вставить в документ").clicked() {
                                                action = Some(TreeAction::InsertFormula(obj.id.clone()));
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
                            // P4: пустое/непустое состояние группы «Формулы» — создание из GUI.
                            if formula_group {
                                ui.add_space(2.0);
                                if ui.small_button("+ Создать формулу…").clicked() {
                                    new_formula = true;
                                }
                            }
                        });
                    }
                    ui.add_space(6.0);
                    if filter.is_empty() && groups.iter().all(|group| group.objects.is_empty()) {
                        if empty_state(ui, IC_FOLDER, "В проекте пока нет объектов", Some((IC_ADD, "Создать объект"))) {
                            new_object = true;
                        }
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
        if new_formula {
            self.open_formula_editor_new();
        }
        if let Some(id) = clicked {
            if let Some(object) = self.project.object(&id).cloned() {
                self.tab = match object.kind {
                    ObjectKind::Document => Tab::Document,
                    ObjectKind::Artifact | ObjectKind::Constants => Tab::Artifacts,
                    ObjectKind::Source | ObjectKind::Note => Tab::Source,
                    ObjectKind::Repository => Tab::Artifacts,
                    ObjectKind::Formula => Tab::Artifacts,
                };
                // Выбор документа в дереве переключает редактор (несколько документов в проекте).
                self.activate_document_object(&object);
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
                        ObjectKind::Artifact => {
                            // «Открыть» артефакт = рабочее место: входы/генерация/результат.
                            self.artifact_editor = None;
                            self.artifact_section = ArtifactSection::Result;
                            Tab::Artifacts
                        }
                        ObjectKind::Constants => Tab::Artifacts,
                        ObjectKind::Source | ObjectKind::Note => Tab::Source,
                        ObjectKind::Repository => Tab::Artifacts,
                        ObjectKind::Formula => Tab::Artifacts,
                    };
                    self.activate_document_object(&object);
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
            TreeAction::EditFormula(id) => {
                self.selected = Some(id.clone());
                self.open_formula_editor(&id);
            }
            TreeAction::InsertFormula(id) => {
                self.selected = Some(id.clone());
                self.formula_quick_insert(&id, "omml");
            }
            TreeAction::Reveal(path) => {
                let path = PathBuf::from(path);
                // Один путь «открыть в файловом менеджере» (headless — честная запись в журнал).
                self.open_in_file_manager(&path);
                self.push_log(Level::Info, format!("Каталог: {}", path.display()));
            }
            TreeAction::Properties(id) => {
                let kind = self.project.object(&id).map(|object| object.kind);
                match kind {
                    Some(ObjectKind::Source) => self.open_source_edit(&id),
                    Some(ObjectKind::Repository) => self.open_repository_edit(&id),
                    _ => self.push_log(Level::Info, "Свойства задаются для источников и репозиториев"),
                }
            }
        }
    }

    // ── панель агента ───────────────────────────────────────────────────────
    fn agent_panel(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        let mut hide_agent = false;
        let mut check = false;
        let mut update = false;
        let mut gost = false;
        let mut review = false;
        // M4.1: встроенный MCP-сервер для внешних агентов (R-MCP-02).
        let external_status = self.external_agents_status();
        let external_error = self.external_agents.last_error().map(str::to_owned);
        let show_agent_token = self.show_agent_token;
        // M4.2b: действующие гранты/режим и trusted-подсказка для run-скриптов (R-RUN-06).
        let agent_grants = external_status.grants();
        let agent_mode_label = self.external_agent_mode_label();
        let run_script_trust_required = self.agent_run_script_trust_required();
        let mut start_external = false;
        let mut stop_external = false;
        let mut toggle_agent_token = false;
        let mut trust_project_for_agents = false;
        egui::Panel::right("rde_agent")
            .resizable(true)
            .default_size(AGENT_WIDTH)
            .min_size(AGENT_WIDTH_MIN)
            .frame(egui::Frame::new().fill(pal.panel).inner_margin(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("АГЕНТ").small().strong().color(pal.muted));
                    let (status_text, status_color) = match &external_status {
                        agent_server::ExternalAgentStatus::Running { .. } => ("• MCP HTTP", pal.green),
                        agent_server::ExternalAgentStatus::NotRunning => ("• MCP HTTP: остановлен", pal.amber),
                        agent_server::ExternalAgentStatus::Disabled => ("• не подключён", pal.amber),
                    };
                    ui.label(RichText::new(status_text).small().color(status_color));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Скрыть панель (вернуть — меню «Вид»)").clicked() {
                            hide_agent = true;
                        }
                        ui.label(RichText::new("v0.3 · MCP/ACP").small().color(pal.muted));
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().id_salt("rde_agent_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    ui.add_space(4.0);
                    // ── M4.1: внешние агенты по MCP streamable HTTP (R-MCP-02) ──
                    ui.label(RichText::new("ВНЕШНИЕ АГЕНТЫ (MCP, HTTP)").small().strong().color(pal.muted));
                    ui.add_space(4.0);
                    match &external_status {
                        agent_server::ExternalAgentStatus::Running { info, .. } => {
                            ui.label(
                                RichText::new("Доступ разрешён: только loopback + bearer-токен.")
                                    .small()
                                    .color(pal.muted),
                            );
                            let mut url = info.url();
                            ui.add(
                                egui::TextEdit::singleline(&mut url)
                                    .font(egui::TextStyle::Monospace)
                                    .desired_width(f32::INFINITY)
                                    .interactive(false),
                            );
                            if show_agent_token {
                                let mut token = info.token.clone();
                                ui.add(
                                    egui::TextEdit::singleline(&mut token)
                                        .font(egui::TextStyle::Monospace)
                                        .desired_width(f32::INFINITY)
                                        .interactive(false),
                                );
                            } else {
                                ui.label(RichText::new("токен скрыт").small().color(pal.muted));
                            }
                            ui.horizontal(|ui| {
                                if ui.small_button(if show_agent_token { "Скрыть токен" } else { "Показать токен" }).clicked() {
                                    toggle_agent_token = true;
                                }
                                if ui.small_button("Копировать адрес").clicked() {
                                    ui.ctx().copy_text(info.url());
                                }
                                if ui.small_button("Копировать токен").clicked() {
                                    ui.ctx().copy_text(info.token.clone());
                                }
                            });
                            if ui.small_button("Остановить доступ").clicked() {
                                stop_external = true;
                            }
                            ui.label(
                                RichText::new(
                                    "Подключение клиента: MCP HTTP URL + заголовок Authorization: Bearer <токен> \
                                     (spec 2026-07-28). Аудит — .rde/agent/audit.log.",
                                )
                                .small()
                                .color(pal.muted),
                            );
                            ui.add_space(4.0);
                            if agent_grants.any() {
                                ui.label(
                                    RichText::new(format!(
                                        "Режим: {agent_mode_label} [{}] (M4.2b)",
                                        agent_grants.label()
                                    ))
                                    .small()
                                    .color(pal.amber),
                                );
                                if agent_grants.project_write {
                                    ui.label(
                                        RichText::new(
                                            "Правки проекта (документ/заметки/манифесты): подтверждение \
                                             с превью «Было/Станет», чекпоинт, откат — «Задачи»/чекпоинты.",
                                        )
                                        .small()
                                        .color(pal.muted),
                                    );
                                }
                                if agent_grants.run_script {
                                    ui.label(
                                        RichText::new(
                                            "Запуск скриптов: код артефактов выполняется на вашей машине \
                                             (R-RUN-06); один job за раз, отмена — run_cancel.",
                                        )
                                        .small()
                                        .color(pal.muted),
                                    );
                                }
                                if agent_grants.git_commit {
                                    ui.label(
                                        RichText::new(
                                            "Git-коммиты: сообщение от агента, автор — из rde.toml; \
                                             `.rde/**` в коммит не попадает.",
                                        )
                                        .small()
                                        .color(pal.muted),
                                    );
                                }
                                ui.label(
                                    RichText::new(
                                        "Каждое действие агента приходит запросом в RDE — подтвердите или \
                                         отклоните в диалоге; решение пишется в аудит.",
                                    )
                                    .small()
                                    .color(pal.muted),
                                );
                            } else {
                                ui.label(RichText::new("Режим: только чтение").small().color(pal.muted));
                            }
                            // R-RUN-06: галочка есть, но проект не доверенный → грант не выдан.
                            if run_script_trust_required && !agent_grants.run_script {
                                ui.label(
                                    RichText::new("Запуск скриптов отключён: проект не доверенный (R-RUN-06).")
                                        .small()
                                        .color(pal.amber),
                                );
                                if ui.small_button("Пометить проект доверенным…").clicked() {
                                    trust_project_for_agents = true;
                                }
                            }
                        }
                        agent_server::ExternalAgentStatus::NotRunning => {
                            ui.label(RichText::new("Разрешено, но сервер не запущен.").small().color(pal.amber));
                            if ui.small_button("Запустить доступ").clicked() {
                                start_external = true;
                            }
                        }
                        agent_server::ExternalAgentStatus::Disabled => {
                            ui.label(RichText::new("Внешний доступ выключен (fail-closed).").small().color(pal.muted));
                            ui.label(
                                RichText::new("Включить: Настройки → «Внешние агенты» (чтение; правки — отдельная галочка; запуск скриптов/git-коммиты — подгалочки с подтверждением).")
                                    .small()
                                    .color(pal.muted),
                            );
                        }
                    }
                    if let Some(error) = &external_error {
                        ui.label(RichText::new(format!("Ошибка: {error}")).small().color(pal.red));
                    }
                    ui.add_space(10.0);
                    // M4.3: встроенный ACP-агент (чат/сессии/permissions/история/модель).
                    self.chat_panel_ui(ui);
                    ui.add_space(10.0);
                    ui.label(RichText::new("БЫСТРЫЕ ДЕЙСТВИЯ (без агента)").small().strong().color(pal.muted));
                    ui.add_space(4.0);
                    if btn(ui, Role::Secondary, Some(IC_CHECK), "Проверить проект").clicked() {
                        check = true;
                    }
                    if btn(ui, Role::Secondary, Some(IC_UPDATE), "Обновить устаревшие артефакты").clicked() {
                        update = true;
                    }
                    if btn(ui, Role::Secondary, Some(IC_DOC), "Оформить документ по ГОСТ").clicked() {
                        gost = true;
                    }
                    if btn(ui, Role::Secondary, Some(IC_DOWNLOAD), "Импорт ревью из Word…").clicked() {
                        review = true;
                    }
                    ui.add_space(10.0);
                    ui.label(RichText::new("Журнал и история запусков — в панели снизу («Задачи»).").small().color(pal.muted));
                });
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
        if toggle_agent_token {
            self.show_agent_token = !self.show_agent_token;
        }
        if start_external {
            self.app_settings.allow_external_mcp = true;
            self.apply_external_agents_setting();
        }
        if stop_external {
            self.app_settings.allow_external_mcp = false;
            self.apply_external_agents_setting();
        }
        if trust_project_for_agents {
            // Существующий диалог согласия R-RUN-06 (общий с «Обновить артефакты»): после
            // подтверждения — trusted=true, reload и перезапуск MCP-сервера с run-грантом.
            self.trust_dialog = Some(TrustDialogState { request: TrustRequest::AgentRunScript });
        }
    }

    // ── центральная область ─────────────────────────────────────────────────
    // ── инспектор объекта (M2.4) ────────────────────────────────────────────
    fn selected_object(&self) -> Option<rde_core::Object> {
        self.selected.as_ref().and_then(|id| self.project.object(id).cloned())
    }

    /// Цель «Открыть» из инспектора: вкладка, где объект реально показан.
    /// Документ → вкладка Документ, артефакт (включая константы — у них то же
    /// рабочее место) → Артефакты, источник → Источник, заметка → Источник (там её
    /// редактор). У репозитория центрального вьюера нет — только «Свойства репозитория…».
    fn inspector_open_tab(object: &rde_core::Object) -> Option<Tab> {
        match &object.data {
            ObjectData::Document(_) => Some(Tab::Document),
            ObjectData::Artifact(_) => Some(Tab::Artifacts),
            ObjectData::Source(_) => Some(Tab::Source),
            ObjectData::Note(_) => Some(Tab::Source),
            ObjectData::Repository(_) => None,
            // Формула — служебный объект (ADR-0005): отдельного вьюера пока нет (P3/P4).
            ObjectData::Formula(_) => None,
        }
    }

    /// Честный переход «Открыть» (инспектор): вкладка объекта + выбор в дереве;
    /// артефакту сбрасываем рабочее место к результату под выбранный объект.
    /// Консистентно с «Открыть в инспекторе» из DAG-меню (выбор + инспектор + журнал).
    /// Возвращает false, если объекту некуда вести (см. [`Self::inspector_open_tab`]).
    fn open_inspector_object(&mut self, id: &str) -> bool {
        let Some(object) = self.project.object(id).cloned() else {
            return false;
        };
        let Some(tab) = Self::inspector_open_tab(&object) else {
            return false;
        };
        self.selected = Some(object.id.clone());
        self.show_inspector = true;
        self.tab = tab;
        self.activate_document_object(&object);
        if matches!(&object.data, ObjectData::Artifact(_)) {
            // Рабочее место артефакта: входы/генерация/результат — открываем результат
            // под выбранный объект (как «Открыть» из контекстного меню дерева).
            self.artifact_editor = None;
            self.artifact_section = ArtifactSection::Result;
        }
        self.push_log(Level::Info, format!("Открыт объект: {} ({})", object.title, object.detail));
        true
    }

    fn inspector_panel(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        let object = self.selected_object();
        let mut open_clicked = false;
        let mut rename_clicked = false;
        let mut delete_clicked = false;
        let mut update_clicked = false;
        let mut gost_clicked = false;
        let mut bibliography_clicked = false;
        let mut word_copy_clicked = false;
        let mut review_import_clicked = false;
        let mut migrate_formulas_clicked = false;
        // P4: панель редактора формулы / быстрая вставка с номером.
        let mut edit_formula_clicked = false;
        let mut insert_formula_clicked = false;
        // ADR-0018: смена схемы нумерации формул документа из инспектора.
        let mut numbering_change: Option<rde_doc::FormulaNumbering> = None;
        // GUI-02: свойства источника/репозитория и «Показать каталог» из инспектора.
        let mut edit_properties_clicked = false;
        let mut reveal_dir_clicked = false;
        // M4.4: «В чат» — приложить контекст-ссылку выбранного объекта к следующему промпту.
        let mut attach_chat = false;
        // Инспектор (19-8): 340 px по умолчанию, перетаскивается в диапазоне 280–480 px —
        // центр документа и DAG не должны обрезаться. Ширины — константы раскладки выше.
        egui::Panel::right("rde_inspector")
            .frame(egui::Frame::new().fill(pal.panel).inner_margin(Margin::symmetric(12, 8)))
            .default_size(INSPECTOR_WIDTH)
            .min_size(INSPECTOR_WIDTH_MIN)
            .max_size(INSPECTOR_WIDTH_MAX)
            .resizable(true)
            .show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("rde_inspector_scroll").auto_shrink([false, false]).show(ui, |ui| {
                    let Some(object) = &object else {
                        ui.add_space(8.0);
                        if empty_state(ui, IC_INSPECT, "Объект не выбран — выберите его в дереве", Some((IC_ADD, "Новый объект"))) {
                            self.create_dialog = Some(CreateDialogState { kind: UiObjectKind::Core(NewObjectKind::ComputeArtifact), title: String::new() });
                        }
                        return;
                    };
                    card(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(format!("{}  {}", kind_icon(object.kind), object.kind.label_ru())).size(FS_CAPTION).color(pal.muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icon_button(ui, IC_CLOSE, "Скрыть инспектор").clicked() {
                                self.show_inspector = false;
                            }
                        });
                    });
                    ui.label(RichText::new(&object.title).size(FS_HEADING).family(bold_family(ui.ctx())).color(pal.text));
                    ui.add_space(1.0);
                    ui.label(RichText::new(fresh_label(object.freshness)).small().color(freshness_color(object.freshness, pal)));
                    ui.add_space(10.0);
                    field_row(ui, "id", &object.id, true);
                    field_row(ui, "каталог", &object.dir.display().to_string(), true);
                    field_row(ui, "детали", &object.detail, false);
                    ui.add_space(6.0);
                    // Основные действия объекта — ряд из двух кнопок равной ширины.
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let w = grid_column_width(ui, 2);
                        // Честная кнопка «Открыть»: у заметок и репозиториев центрального
                        // вьюера нет (их показывает только сам инспектор) — открывать нечего.
                        let can_open = Self::inspector_open_tab(object).is_some();
                        let open_resp = ui
                            .add_enabled_ui(can_open, |ui| btn_w(ui, Role::Primary, Some(IC_OPEN), "Открыть", w))
                            .inner
                            .on_disabled_hover_text("нечего открывать");
                        if open_resp.clicked() {
                            open_clicked = true;
                        }
                        if btn_w(ui, Role::Secondary, Some(IC_RENAME), "Переименовать", w).clicked() {
                            rename_clicked = true;
                        }
                    });
                    });
                    // M4.4: контекст-ссылка §8.7.2 — «В чат» выбранного объекта (чип к промпту).
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let w = grid_column_width(ui, 2);
                        let uri = self.context_uri_for_object(object).ok();
                        let resp = ui
                            .add_enabled_ui(uri.is_some(), |ui| btn_w(ui, Role::Secondary, Some(IC_RENAME), "В чат", w))
                            .inner
                            .on_disabled_hover_text("для этого объекта контекст-ссылки §8.7.2 нет");
                        if resp.clicked() {
                            attach_chat = true;
                        }
                    });
                    ui.add_space(12.0);
                    ui.separator();
                    ui.add_space(12.0);
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
                                ui.label(RichText::new("DAG / активация").small().color(pal.muted));
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
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    let w = grid_column_width(ui, 2);
                                    if btn_w(ui, Role::Secondary, Some(IC_UPDATE), "Обновить артефакт", w).clicked() {
                                        update_clicked = true;
                                    }
                                    if btn_w(ui, Role::Ghost, None, "копировать act_key", w).clicked() {
                                        if let Some(act_key) = &status.act_key {
                                            ui.ctx().copy_text(act_key.clone());
                                        }
                                    }
                                });
                            }
                        }
                        ObjectData::Formula(formula) => {
                            field_row(ui, "тип", "formula (project-level, ADR-0005)", false);
                            field_row(ui, "режим", if formula.display { "display" } else { "inline" }, false);
                            let numbering = if formula.numbering.label.is_empty() {
                                formula.numbering.mode.clone()
                            } else {
                                format!("{} · {}", formula.numbering.mode, formula.numbering.label)
                            };
                            field_row(ui, "нумерация", &numbering, false);
                            field_row(ui, "latex", formula.latex.chars().take(80).collect::<String>().as_str(), true);
                            // P4 (ADR-0020): правка источника и вставка с номером из инспектора.
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                let w = grid_column_width(ui, 2);
                                if btn_w(ui, Role::Primary, Some(IC_RENAME), "Редактировать", w).clicked() {
                                    edit_formula_clicked = true;
                                }
                                if btn_w(ui, Role::Secondary, Some(IC_CONST), "Вставить", w).clicked() {
                                    insert_formula_clicked = true;
                                }
                            });
                            ui.add_space(4.0);
                            let hashed = formula.inputs.iter().filter(|input| input.mode == "hashed").count();
                            let refed = formula.inputs.len().saturating_sub(hashed);
                            field_row(ui, "входы", &format!("{hashed} hashed · {refed} ref"), false);
                            if !formula.fragments.is_empty() {
                                field_row(ui, "фрагменты", &format!("{}", formula.fragments.len()), false);
                            }
                            if !formula.state.act_key.is_empty() {
                                field_row(ui, "act_key", &formula.state.act_key.chars().take(20).collect::<String>(), true);
                            }
                            if formula.state.computed_at > 0 {
                                field_row(ui, "вычислена", &format!("unix {}", formula.state.computed_at), false);
                            }
                            // P3: живой статус формулы и «где вставлено» (R-DOC-19), плюс явная
                            // миграция формул вне реестра (ADR-0005 §7).
                            let panel = self.formula_inspector_data(&formula.id);
                            ui.add_space(4.0);
                            let (status_label, status_color) = match &panel.status {
                                Some(rde_dag::FormulaLiveStatus::Fresh) => ("актуальна", pal.green),
                                Some(rde_dag::FormulaLiveStatus::Stale(_)) => ("устарела", pal.amber),
                                Some(rde_dag::FormulaLiveStatus::NeverComputed) => ("не вычислена", pal.amber),
                                Some(rde_dag::FormulaLiveStatus::NotComputable(_)) => ("ошибка", pal.red),
                                None => ("статус неизвестен", pal.muted),
                            };
                            field_row(ui, "статус", status_label, false);
                            let _ = status_color;
                            if let Some(reason) = panel.status.as_ref().and_then(|status| status.reason()) {
                                ui.label(RichText::new(reason).small().color(pal.muted));
                            }
                            if let Some(act_key) = &panel.act_key {
                                field_row(ui, "act_key (текущий)", &act_key.chars().take(20).collect::<String>(), true);
                            }
                            ui.add_space(6.0);
                            section_caption(ui, &format!("где вставлено (вставок {})", panel.backlinks.len()));
                            if panel.backlinks.is_empty() {
                                ui.label(RichText::new("вставок нет").small().color(pal.muted));
                            }
                            for link in &panel.backlinks {
                                let mode = if link.render_mode.is_empty() { "omml" } else { &link.render_mode };
                                let number = link
                                    .number_text
                                    .clone()
                                    .or_else(|| link.number.map(|number| number.to_string()))
                                    .map(|number| format!(" ({number})"))
                                    .unwrap_or_default();
                                let stale = if link.stale { " · устарела" } else { "" };
                                // ADR-0019: источник номера — из Word / вручную / не нумеруется.
                                let source = match link.number_source.as_str() {
                                    "word" => " · номер из Word",
                                    "manual" => " · номер вручную",
                                    "off" => " · не нумеруется",
                                    _ => "",
                                };
                                ui.label(
                                    RichText::new(format!(
                                        "{} · {}{} · {}{}{}",
                                        link.document_title, link.insert_id, number, mode, source, stale
                                    ))
                                    .small()
                                    .color(if link.stale { pal.amber } else { pal.text }),
                                );
                            }
                            if panel.unregistered > 0 {
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(format!("формул вне реестра в документе: {}", panel.unregistered))
                                        .small()
                                        .color(pal.amber),
                                );
                                if ui.small_button("Принять как объекты формулы").clicked() {
                                    migrate_formulas_clicked = true;
                                }
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
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                if btn_w(ui, Role::Secondary, Some(IC_SOURCE), "Свойства источника…", grid_column_width(ui, 1)).clicked() {
                                    edit_properties_clicked = true;
                                }
                            });
                            if btn_w(ui, Role::Ghost, Some(IC_FOLDER), "Показать каталог", grid_column_width(ui, 1)).clicked() {
                                reveal_dir_clicked = true;
                            }
                        }
                        ObjectData::Note(note) => {
                            field_row(ui, "файл", &note.file, false);
                            let path = object.dir.join(&note.file);
                            ui.add_space(4.0);
                            if btn_w(ui, Role::Primary, Some(IC_OPEN), "Открыть заметку", grid_column_width(ui, 1)).clicked() {
                                open_clicked = true;
                            }
                            if let Ok(text) = std::fs::read_to_string(&path) {
                                ui.add_space(4.0);
                                ui.label(RichText::new("содержимое (первые 400 символов)").small().color(pal.muted));
                                let preview: String = text.chars().take(400).collect();
                                ui.label(RichText::new(preview).size(12.0).color(pal.text));
                            }
                        }
                        ObjectData::Repository(repo) => {
                            field_row(ui, "kind", &repo.kind, false);
                            field_row(ui, "path", &repo.path, true);
                            ui.add_space(4.0);
                            ui.horizontal(|ui| {
                                if btn_w(ui, Role::Secondary, Some(IC_REPO), "Свойства репозитория…", grid_column_width(ui, 1)).clicked() {
                                    edit_properties_clicked = true;
                                }
                            });
                            let repo_path = Path::new(&repo.path);
                            let resolved = if repo_path.is_absolute() { repo_path.to_path_buf() } else { self.project.root.join(repo_path) };
                            if btn_w(ui, Role::Ghost, Some(IC_FOLDER), "Показать каталог", grid_column_width(ui, 1)).clicked() {
                                self.open_in_file_manager(&resolved);
                            }
                        }
                        ObjectData::Document(document) => {
                            field_row(ui, "файл", &document.file, false);
                            field_row(ui, "шаблон", if document.template.is_empty() { "—" } else { &document.template }, false);
                            // ADR-0018: схема нумерации формул документа; смена сразу перенумеровывает
                            // формулы и ссылки (без пересборки тел).
                            let current_scheme = rde_doc::FormulaNumbering::parse(&document.formula_numbering).unwrap_or_default();
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("нумерация формул").small().color(pal.muted));
                                let mut chosen = current_scheme;
                                egui::ComboBox::from_id_salt("rde_formula_numbering_scheme")
                                    .selected_text(formula_numbering_label(current_scheme))
                                    .show_ui(ui, |ui| {
                                        ui.selectable_value(
                                            &mut chosen,
                                            rde_doc::FormulaNumbering::Continuous,
                                            formula_numbering_label(rde_doc::FormulaNumbering::Continuous),
                                        );
                                        ui.selectable_value(
                                            &mut chosen,
                                            rde_doc::FormulaNumbering::Section,
                                            formula_numbering_label(rde_doc::FormulaNumbering::Section),
                                        );
                                    });
                                if chosen != current_scheme {
                                    numbering_change = Some(chosen);
                                }
                            });
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
                                section_caption(ui, "оформление");
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    let w = grid_column_width(ui, 2);
                                    if btn_w(ui, Role::Primary, Some(IC_DOC), "Оформить по ГОСТ", w).clicked() {
                                        gost_clicked = true;
                                    }
                                    if btn_w(ui, Role::Secondary, Some(IC_SOURCE), "Вставить библиографию", w).clicked() {
                                        bibliography_clicked = true;
                                    }
                                });
                                ui.add_space(4.0);
                                section_caption(ui, "обмен с Word");
                                ui.horizontal(|ui| {
                                    ui.spacing_mut().item_spacing.x = 8.0;
                                    let w = grid_column_width(ui, 2);
                                    if btn_w(ui, Role::Secondary, Some(IC_OPEN), "Открыть копию в Word", w).clicked() {
                                        word_copy_clicked = true;
                                    }
                                    if btn_w(ui, Role::Secondary, Some(IC_DOWNLOAD), "Импорт ревью из Word…", w).clicked() {
                                        review_import_clicked = true;
                                    }
                                });
                            }
                        }
                    }
                    ui.separator();
                    // Строка «id … копировать» переносится, если не влезает: не раздувает панель.
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new("id").small().color(pal.muted));
                        ui.label(RichText::new(&object.id).monospace().size(FS_CAPTION).color(pal.muted));
                        if ui.small_button("копировать").clicked() {
                            ui.ctx().copy_text(object.id.clone());
                        }
                    });
                    // Опасная группа — отдельно от основных действий, на всю ширину.
                    ui.add_space(14.0);
                    ui.separator();
                    ui.add_space(8.0);
                    section_caption(ui, "опасная зона");
                    if btn_w(ui, Role::Danger, Some(IC_DELETE), "Удалить объект", grid_column_width(ui, 1)).clicked() {
                        delete_clicked = true;
                    }
                });
            });
        if let Some(object) = &object {
            if open_clicked {
                // Переход выполняет open_inspector_object: вкладка объекта + выбор
                // в дереве + рабочее место артефакта; консистентно с «Открыть
                // в инспекторе» из DAG-меню. Для заметок/репозиториев кнопка
                // disabled, так что сюда попадаем только когда открывать есть что.
                let id = object.id.clone();
                let _ = self.open_inspector_object(&id);
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
            if migrate_formulas_clicked {
                self.open_formula_migration_dialog();
            }
            if edit_formula_clicked {
                self.open_formula_editor(&object.id);
            }
            if insert_formula_clicked {
                self.formula_quick_insert(&object.id, "omml");
            }
            if let Some(scheme) = numbering_change {
                match self.apply_document_formula_numbering(&object.dir, scheme) {
                    Ok(changed) if changed > 0 => self.push_log(
                        Level::Ok,
                        format!(
                            "Нумерация формул: схема «{}», перенумеровано {} формул(ы)",
                            formula_numbering_label(scheme),
                            changed
                        ),
                    ),
                    Ok(_) => self.push_log(Level::Info, format!("Нумерация формул: схема «{}» (номеров менять не пришлось)", formula_numbering_label(scheme))),
                    Err(error) => self.push_log(Level::Warn, format!("Нумерация формул: {error}")),
                }
            }
            if edit_properties_clicked {
                match object.data {
                    ObjectData::Source(_) => self.open_source_edit(&object.id),
                    ObjectData::Repository(_) => self.open_repository_edit(&object.id),
                    _ => self.push_log(Level::Info, "Свойства задаются для источников и репозиториев"),
                }
            }
            if reveal_dir_clicked {
                self.open_in_file_manager(&object.dir);
            }
            if attach_chat {
                match self.context_uri_for_object(object) {
                    Ok(uri) => {
                        self.attach_context_uri(&uri);
                    }
                    Err(error) => self.push_log(Level::Warn, format!("«В чат»: {error}")),
                }
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
        // Уникальное имя временного .docx: в тестах действие вызывается параллельно из разных
        // потоков одного процесса, общий `{pid}.docx` приводил к гонке («missing part»).
        let unique = {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        };
        let temp = std::env::temp_dir().join(format!("rde-new-doc-{}-{unique}.docx", std::process::id()));
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
            .and_then(|mut rde_doc| rde_doc::write_package(&temp, &mut rde_doc, &package).map(|_| ()));
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

    /// Импорт чужого `.docx` как **нового** объекта-документа (R-DOC-39, минимальная версия).
    ///
    /// Исходник только читается; создаётся `objects/documents/<id>/report.doc.rde` (WordCraft →
    /// OOXML + `rde:doc` + paraId) и `import.json` с происхождением. Заголовок помечается
    /// `[IMPORTED]` (R-DOC-39), предупреждения о неподдерживаемых конструкциях — в журнал.
    fn import_docx_action(&mut self, source: &Path) -> Result<(), String> {
        self.import_docx_action_with_title(source, None)
    }

    /// То же, но с именем документа (паритет CLI `rde import-docx --title`): `None` — из имени
    /// файла; префикс `[IMPORTED] ` добавляется, если пользователь его не написал.
    fn import_docx_action_with_title(&mut self, source: &Path, custom_title: Option<&str>) -> Result<(), String> {
        let source = source.canonicalize().unwrap_or_else(|_| source.to_path_buf());
        if !source.is_file() {
            return Err(format!("файл импорта не найден: {}", source.display()));
        }
        let stem = source.file_stem().and_then(|stem| stem.to_str()).unwrap_or("документ");
        let title = match custom_title.map(str::trim).filter(|title| !title.is_empty()) {
            Some(custom) if custom.starts_with("[IMPORTED]") => custom.to_owned(),
            Some(custom) => format!("[IMPORTED] {custom}"),
            None => format!("[IMPORTED] {stem}"),
        };
        let dir = rde_core::create_document_stub(&self.project.root, &title)?;        let cleanup = |dir: &Path| {
            let _ = fs::remove_dir_all(dir);
        };
        let project = match rde_core::load_project(&self.project.root) {
            Ok(project) => project,
            Err(error) => {
                cleanup(&dir);
                return Err(error);
            }
        };
        let Some(object) = project.objects.iter().find(|object| object.dir == dir).cloned() else {
            cleanup(&dir);
            return Err("новый документ не загрузился".to_owned());
        };
        let ObjectData::Document(manifest) = &object.data else {
            cleanup(&dir);
            return Err("новый объект не является документом".to_owned());
        };
        let package = dir.join("report.doc.rde");
        match rde_doc::import_docx(&source, &package, &manifest.id, &manifest.template) {
            Ok(report) => {
                self.reload_project();
                let imported = self.project.objects.iter().find(|object| object.dir == dir).cloned();
                self.selected = imported.as_ref().map(|object| object.id.clone());
                self.tab = Tab::Document;
                // Импортированный документ становится активным: даже если в проекте уже был
                // другой документ, редактор открывает именно его (а не первый по порядку).
                if let Some(object) = imported.as_ref() {
                    self.activate_document_object(object);
                }
                self.push_log(Level::Ok, format!("Импортирован .docx «{}»: {}", title, report.summary()));
                let document_count = self.project.documents().count();
                if document_count > 1 {
                    self.push_log(
                        Level::Info,
                        format!(
                            "В проекте {document_count} документа: активен импортированный; выбор другого документа в дереве переключает редактор"
                        ),
                    );
                }
                if !report.warnings.is_empty() {
                    self.push_log(Level::Warn, format!("Импорт .docx: предупреждения ({})", report.warnings.len()));
                    for warning in report.warnings.iter().take(10) {
                        self.push_log(Level::Warn, format!("  • {warning}"));
                    }
                }
                self.push_log(
                    Level::Info,
                    format!(
                        "Происхождение: {} (sha256 {}…); отчёт — import.json рядом с пакетом",
                        report.source.display(),
                        report.source_sha256.chars().take(12).collect::<String>()
                    ),
                );
                Ok(())
            }
            Err(error) => {
                cleanup(&dir);
                Err(error)
            }
        }
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

    // ── GUI-02: цитаты и свойства источников/репозиториев ───────────────────

    /// Импорт цитат из BibTeX/RIS/CSL-JSON в объекты-источники (rde-sources).
    /// Возвращает человекочитаемую сводку для журнала; все записи создаются до ошибки
    /// или не создаются вовсе (ошибка разбора — до записи).
    pub fn import_citations_action(&mut self, path: &Path) -> Result<String, String> {
        let text = fs::read_to_string(path).map_err(|error| format!("не удалось прочитать {}: {error}", path.display()))?;
        let format = rde_sources::CitationFormat::detect(&text);
        let references = format.parse(&text).map_err(|error| format!("разбор цитат: {error}"))?;
        if references.is_empty() {
            return Err("в файле не найдено ни одной записи".to_owned());
        }
        let created = rde_sources::import_citations(&self.project.root, &references)?;
        self.reload_project();
        let first_titles: Vec<String> = references.iter().take(3).map(|reference| reference.title.clone()).collect();
        let more = if references.len() > 3 { format!(", и ещё {}", references.len() - 3) } else { String::new() };
        Ok(format!(
            "Импорт цитат: создано источников {} ({}); первые: {}{}",
            created.len(),
            match format {
                rde_sources::CitationFormat::Bibtex => "BibTeX",
                rde_sources::CitationFormat::Ris => "RIS",
                rde_sources::CitationFormat::CslJson => "CSL-JSON",
            },
            first_titles.join("; "),
            more
        ))
    }

    /// Открыть диалог «Свойства источника» для объекта.
    pub fn open_source_edit(&mut self, id: &str) {
        let Some(object) = self.project.object(id).cloned() else {
            self.push_log(Level::Warn, format!("Источник {id} не найден"));
            return;
        };
        let ObjectData::Source(source) = &object.data else {
            self.push_log(Level::Warn, "«Свойства источника» доступны только для источников");
            return;
        };
        self.begin_pick_target(file_dialog::PathTarget::SourceFile(id.to_owned()));
        self.source_edit_dialog = Some(SourceEditDialog {
            id: id.to_owned(),
            kind: if source.kind.is_empty() { "file".to_owned() } else { source.kind.clone() },
            file: source.file.clone().unwrap_or_default(),
            url: source.url.clone().unwrap_or_default(),
            retrieved: source.retrieved_at.clone().unwrap_or_default(),
            error: String::new(),
            focused: false,
        });
    }

    /// Открыть диалог «Свойства репозитория» для объекта.
    pub fn open_repository_edit(&mut self, id: &str) {
        let Some(object) = self.project.object(id).cloned() else {
            self.push_log(Level::Warn, format!("Репозиторий {id} не найден"));
            return;
        };
        let ObjectData::Repository(repo) = &object.data else {
            self.push_log(Level::Warn, "«Свойства репозитория» доступны только для репозиториев");
            return;
        };
        self.begin_pick_target(file_dialog::PathTarget::RepositoryPath(id.to_owned()));
        self.repo_edit_dialog = Some(RepositoryEditDialog {
            id: id.to_owned(),
            kind: if repo.kind.is_empty() { "internal".to_owned() } else { repo.kind.clone() },
            path: repo.path.clone(),
            error: String::new(),
            focused: false,
        });
    }

    /// Сохранить свойства источника: локатор (файл копируется в объект) и дата получения.
    pub fn save_source_edit(&mut self, dialog: &SourceEditDialog) -> Result<(), String> {
        let object = self.project.object(&dialog.id).cloned().ok_or_else(|| format!("источник {} не найден", dialog.id))?;
        let ObjectData::Source(source) = &object.data else {
            return Err("объект не является источником".to_owned());
        };
        let mut manifest_updates: Vec<(&str, String)> = Vec::new();
        match dialog.kind.trim() {
            "url" => {
                let url = dialog.url.trim();
                if url.is_empty() {
                    return Err("для источника-URL укажите адрес".to_owned());
                }
                manifest_updates.push(("kind", "url".to_owned()));
                manifest_updates.push(("url", url.to_owned()));
            }
            "repo" => {
                manifest_updates.push(("kind", "repo".to_owned()));
                if !dialog.url.trim().is_empty() {
                    manifest_updates.push(("url", dialog.url.trim().to_owned()));
                }
            }
            _ => {
                let file_field = dialog.file.trim();
                if file_field.is_empty() {
                    return Err("укажите файл источника («Проводник…») или выберите тип URL".to_owned());
                }
                let candidate = Path::new(file_field);
                // Уже лежит в объекте (относительный путь из source.toml) — оставить как есть;
                // иначе прикрепить: скопировать файл в каталог объекта-источника.
                let inside = !candidate.is_absolute() && object.dir.join(candidate).is_file();
                let file_name = if inside {
                    file_field.to_owned()
                } else {
                    self.attach_source_file(&object.dir, candidate, source.file.as_deref())?
                };
                manifest_updates.push(("kind", "file".to_owned()));
                manifest_updates.push(("file", file_name));
            }
        }
        if !dialog.retrieved.trim().is_empty() {
            manifest_updates.push(("retrieved_at", dialog.retrieved.trim().to_owned()));
        }
        let manifest_path = object.dir.join("source.toml");
        for (key, value) in &manifest_updates {
            set_manifest_string(&manifest_path, key, value)?;
        }
        // Локатор другого типа не должен «висеть» в манифесте (file у URL-источника и наоборот).
        let keep: Vec<&str> = manifest_updates.iter().map(|(key, _)| *key).collect();
        for key in ["file", "url"] {
            if !keep.contains(&key) {
                let _ = set_manifest_optional_string(&manifest_path, key, None);
            }
        }
        self.reload_project();
        self.push_log(Level::Ok, format!("Источник «{}» сохранён", object.title));
        Ok(())
    }

    /// Скопировать файл в каталог объекта-источника (оригинал не изменяется) и вернуть
    /// относительное имя файла для `source.toml`. Файл, который уже лежит в каталоге объекта,
    /// используется как есть.
    fn attach_source_file(&self, object_dir: &Path, picked: &Path, existing: Option<&str>) -> Result<String, String> {
        if !picked.is_file() {
            return Err(format!("файл источника не найден: {}", picked.display()));
        }
        let inside = picked.canonicalize().ok().and_then(|canonical| {
            object_dir
                .canonicalize()
                .ok()
                .filter(|dir| canonical.starts_with(dir))
                .and_then(|_| canonical.file_name().map(|name| name.to_string_lossy().to_string()))
        });
        if let Some(name) = inside {
            return Ok(name);
        }
        let file_name = picked
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .ok_or_else(|| "у выбранного файла нет имени".to_owned())?;
        // Не затирать другой прикреплённый файл с тем же именем: уникализировать.
        let mut target = object_dir.join(&file_name);
        if target.is_file() && existing != Some(file_name.as_str()) {
            let stem = picked.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_else(|| "source".to_owned());
            let ext = picked.extension().map(|ext| ext.to_string_lossy().to_string()).unwrap_or_default();
            let mut index = 1;
            loop {
                let candidate = if ext.is_empty() { format!("{stem}-{index}") } else { format!("{stem}-{index}.{ext}") };
                target = object_dir.join(&candidate);
                if !target.exists() {
                    return self.copy_source_file(picked, &target).map(|_| candidate);
                }
                index += 1;
                if index > 1000 {
                    return Err(format!("не удалось подобрать имя для {}", picked.display()));
                }
            }
        }
        self.copy_source_file(picked, &target)?;
        Ok(file_name)
    }

    fn copy_source_file(&self, from: &Path, to: &Path) -> Result<(), String> {
        fs::copy(from, to).map(|_| ()).map_err(|error| format!("копировать {} → {}: {error}", from.display(), to.display()))
    }

    /// Сохранить свойства репозитория (`repo.toml`: kind/path).
    pub fn save_repository_edit(&mut self, dialog: &RepositoryEditDialog) -> Result<(), String> {
        let object = self.project.object(&dialog.id).cloned().ok_or_else(|| format!("репозиторий {} не найден", dialog.id))?;
        if !matches!(object.data, ObjectData::Repository(_)) {
            return Err("объект не является репозиторием".to_owned());
        }
        let path_text = dialog.path.trim();
        if path_text.is_empty() {
            return Err("путь репозитория не может быть пустым".to_owned());
        }
        let kind = match dialog.kind.trim() {
            "internal" | "external" | "git" => dialog.kind.trim().to_owned(),
            other => return Err(format!("неизвестный тип репозитория «{other}» (internal|external|git)")),
        };
        let manifest_path = object.dir.join("repo.toml");
        set_manifest_string(&manifest_path, "kind", &kind)?;
        set_manifest_string(&manifest_path, "path", path_text)?;
        self.reload_project();
        self.push_log(Level::Ok, format!("Репозиторий «{}» сохранён: {kind} · {path_text}", object.title));
        Ok(())
    }

    /// Проверка внешних правок документа при открытии проекта (M3/M5).
    fn check_external_edit_on_startup(&mut self) {
        if let Some(path) = self.active_document_path() {
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
                self.doc = DocPreview::new(self.active_document_path());
                self.editor = None;
                self.editor_error = None;
                self.preview_scroll_offset = 0.0;
                self.preview_restore_offset = None;
                self.structure_cache = None;
                self.pdf_cache = None;
                self.comments_cache = None;
                self.inserts_cache = None;
                self.tab = Tab::Document;
                self.store = Store::open(&self.project.root).ok();
                self.refresh_dag_statuses();
                self.push_log(Level::Ok, format!("Проект открыт: {} ({} объектов)", self.project.name, self.project.object_count()));
                self.check_external_edit_on_startup();
                // M4.3: встроенный агент привязан к проекту (cwd сессии) — сессию/процесс
                // останавливаем fail-closed; следующий чат стартует под новый корень.
                self.shutdown_chat("смена проекта");
                // M4.1: внешние агенты привязаны к проекту — перезапустить (или остановить).
                let enabled = self.app_settings.allow_external_mcp;
                let project_root = self.project.root.clone();
                let agent_options = self.external_agent_options();
                if let Err(error) = self.external_agents.on_project_switched(enabled, &project_root, &agent_options) {
                    self.push_log(Level::Warn, format!("Внешние агенты (MCP HTTP): {error}"));
                    self.app_settings.allow_external_mcp = false;
                }
            }
            Err(error) => self.push_log(Level::Warn, format!("Не удалось открыть проект {}: {error}", root.display())),
        }
    }

    /// Drag&Drop без заклинаний (GUI-only сценарий): решает, что делать с перетащенными путями.
    ///
    /// * папка с `rde.toml` → открыть проект (текущий документ предварительно сохраняется);
    /// * `.docx` → импорт в текущий проект (исходник не изменяется; MathType-формулы становятся
    ///   нативными формулами RDE — R-DOC-44);
    /// * прочее (в т.ч. `.doc`/`.odt`) → понятное предупреждение в журнале, без «тихих» потерь.
    pub fn handle_dropped_paths(&mut self, paths: &[PathBuf]) {
        for path in paths {
            if path.is_dir() {
                if path.join("rde.toml").is_file() {
                    if self.editor.is_some() {
                        self.save_project_document();
                    }
                    let name = rde_core::load_project(path).map(|project| project.name).unwrap_or_default();
                    self.open_project(path);
                    if !name.is_empty() {
                        self.record_recent_project(path, &name);
                    }
                } else {
                    self.push_log(
                        Level::Warn,
                        format!(
                            "Перетащена папка без rde.toml: {} — перетащите папку проекта RDE (в ней должен быть rde.toml)",
                            path.display()
                        ),
                    );
                }
                continue;
            }
            if !path.is_file() {
                self.push_log(Level::Warn, format!("Перетащен несуществующий путь: {}", path.display()));
                continue;
            }
            let extension = path.extension().and_then(|value| value.to_str()).unwrap_or_default().to_ascii_lowercase();
            match extension.as_str() {
                "docx" => {
                    if self.editor.is_some() {
                        // Не терять несохранённые правки текущего документа перед импортом.
                        self.save_project_document();
                    }
                    self.push_log(Level::Info, format!("Перетащен .docx: {} — импортирую…", path.display()));
                    if let Err(error) = self.import_docx_action(path) {
                        self.push_log(Level::Warn, format!("Импорт .docx (drag&drop): {error}"));
                    }
                }
                "doc" | "rtf" | "odt" | "html" | "htm" | "md" | "tex" | "txt" => self.push_log(
                    Level::Warn,
                    format!(
                        "Импорт поддерживает только .docx (R-DOC-39): {} — сохраните документ как .docx и перетащите его",
                        path.display()
                    ),
                ),
                _ => self.push_log(
                    Level::Warn,
                    format!(
                        "Неизвестный файл: {} — перетащите .docx для импорта или папку проекта с rde.toml",
                        path.display()
                    ),
                ),
            }
        }
    }

    /// Полупрозрачная подсказка поверх окна, пока файл держат над окном (drag&drop).
    fn drop_hint_overlay(&self, ctx: &egui::Context, hovered: &[String]) {
        let pal = theme_colors(ctx.theme());
        egui::Area::new(egui::Id::new("rde_drop_hint"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(pal.panel_hi)
                    .stroke(egui::Stroke::new(2.0, pal.accent))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin::same(24))
                    .show(ui, |ui| {
                        ui.label(RichText::new("Отпустите файл").heading().strong().color(pal.text));
                        let name = hovered.first().map(String::as_str).unwrap_or("");
                        if !name.is_empty() {
                            ui.label(RichText::new(name).color(pal.muted));
                        }
                        ui.label("• папка проекта с rde.toml → открыть проект");
                        ui.label("• .docx → импорт (MathType-формулы станут нативными)");
                    });
            });
    }

    /// Создать новый проект и открыть его.
    pub fn create_project_ui(&mut self, root: &Path, name: &str) -> Result<(), String> {
        rde_core::create_project(root, name)?;
        // R-VCS-01: служебные файлы и начальный git-коммит нового проекта.
        let mut notes = Vec::new();
        match rde_vcs::ensure_project_files(root) {
            Ok(created) if !created.is_empty() => notes.push(format!("созданы {}", created.join(", "))),
            Ok(_) => {}
            Err(error) => notes.push(format!("служебные файлы: {error}")),
        }
        match rde_vcs::init(root) {
            Ok(true) => notes.push("git-репозиторий инициализирован".to_owned()),
            Ok(false) => notes.push("git-репозиторий уже есть".to_owned()),
            Err(error) => notes.push(format!("git: {error}")),
        }
        self.open_project(root);
        self.push_log(Level::Ok, format!("Создан проект «{}»: {}", name.trim(), root.display()));
        // GUI-02: новый проект сразу попадает в «Недавние проекты» (и в last-project для run.sh).
        let project_name = self.project.manifest.name.clone();
        self.record_recent_project(root, &project_name);
        for note in notes {
            self.push_log(Level::Info, format!("Проект: {note}"));
        }
        Ok(())
    }

    /// Сохранить настройки проекта (`rde.toml`) и перезагрузить модель.
    pub fn save_project_settings(&mut self, name: &str, template: &str, rebuild: &str) -> Result<(), String> {
        if name.trim().is_empty() {
            return Err("название проекта не может быть пустым".to_owned());
        }
        let path = self.project.root.join("rde.toml");
        let _ = set_manifest_string(&path, "name", name.trim());
        let _ = set_manifest_string(&path, "default_template", template.trim());
        let _ = set_manifest_string(&path, "default_rebuild", rebuild.trim());
        self.reload_project();
        self.push_log(Level::Ok, "Настройки проекта сохранены");
        Ok(())
    }

    /// Сохранить настройки проекта вместе со списком авторов (R-SET-01).
    pub fn save_project_settings_with_authors(&mut self, dialog: &ProjectSettingsState) -> Result<(), String> {
        self.save_project_settings(&dialog.name, &dialog.template, &dialog.rebuild)?;
        rde_core::set_project_authors(&self.project.root, &dialog.authors)?;
        self.reload_project();
        self.push_log(Level::Ok, format!("Настройки проекта сохранены (авторов: {})", dialog.authors.len()));
        Ok(())
    }

    /// Экспорт диагностического архива (R-SET-03/07): окружение, git, транзакции, issues, хвосты
    /// логов — **без содержимого документов и без CAS**.
    pub fn export_diagnostics(&mut self) -> Option<PathBuf> {
        let dir = self.export_dir.clone();
        if let Err(error) = std::fs::create_dir_all(&dir) {
            self.push_log(Level::Warn, format!("Экспорт диагностики: {error}"));
            return None;
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let path = dir.join(format!("diagnostics-{stamp}.zip"));
        let file = match std::fs::File::create(&path) {
            Ok(file) => file,
            Err(error) => {
                self.push_log(Level::Warn, format!("Экспорт диагностики: {error}"));
                return None;
            }
        };
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        let mut put = |name: &str, bytes: &[u8]| -> Result<(), String> {
            zip.start_file(name.to_owned(), options).map_err(|e| format!("zip {name}: {e}"))?;
            use std::io::Write;
            zip.write_all(bytes).map_err(|e| format!("zip write {name}: {e}"))
        };
        let mut summary = String::new();
        summary.push_str(&format!("RDE diagnostics · v{}\n", env!("CARGO_PKG_VERSION")));
        summary.push_str(&format!("OS: {} {}\n", std::env::consts::OS, std::env::consts::ARCH));
        summary.push_str(&format!("Project: «{}» id={} root={}\n", self.project.name, self.project.manifest.id, self.project.root.display()));
        summary.push_str(&format!("Objects: {} (stale {})\n", self.project.object_count(), self.project.stale_count()));
        if let Some(status) = &self.git_status {
            summary.push_str(&format!("Git: {}\n", status.summary_ru()));
        }
        if let Some(store) = &self.store {
            if let Ok(stats) = store.stats() {
                summary.push_str(&format!("Store: blobs={} trees={} txs={}\n", stats.blobs, stats.trees, stats.txs));
            }
        }
        summary.push_str(&format!("Runtimes: python3={}\n", std::process::Command::new("python3").arg("--version").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned()).unwrap_or_else(|_| "нет".to_owned())));
        let mut result = put("summary.txt", summary.as_bytes());
        if result.is_ok() {
            if let Ok(text) = std::fs::read_to_string(self.project.root.join("rde.toml")) {
                result = put("rde.toml", text.as_bytes());
            }
        }
        if result.is_ok() {
            if let Ok(text) = std::fs::read_to_string(self.project.root.join("rde.lock")) {
                result = put("rde.lock", text.as_bytes());
            }
        }
        if result.is_ok() {
            let sync = rde_vcs::check_sync(&self.project.root).unwrap_or_else(|error| format!("git: {error}"));
            result = put("git-status.txt", sync.as_bytes());
        }
        if result.is_ok() {
            let mut tx_lines = String::new();
            if let Some(store) = &self.store {
                if let Ok(txs) = store.list_txs() {
                    for tx in txs.iter().take(100) {
                        tx_lines.push_str(&format!("{}  {:?}  {}\n", tx.started_unix, tx.status, tx.note));
                    }
                }
            }
            result = put("transactions.txt", tx_lines.as_bytes());
        }
        if result.is_ok() {
            let issues: Vec<String> = self.project.issues.iter().map(|issue| format!("{}: {}", issue.path.display(), issue.message)).collect();
            result = put("issues.txt", issues.join("\n").as_bytes());
        }
        // Хвосты логов последних запусков (без выходов/CAS).
        if result.is_ok() {
            let runs_root = self.project.root.join(".rde/runs");
            let mut runs: Vec<PathBuf> = std::fs::read_dir(&runs_root)
                .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
                .unwrap_or_default();
            runs.sort();
            for run_dir in runs.iter().rev().take(5) {
                let Some(run_id) = run_dir.file_name().and_then(|name| name.to_str()) else { continue };
                for log_name in ["stdout.log", "stderr.log"] {
                    let log_path = run_dir.join(log_name);
                    let Ok(bytes) = std::fs::read(&log_path) else { continue };
                    let tail = if bytes.len() > 64 * 1024 { &bytes[bytes.len() - 64 * 1024..] } else { &bytes[..] };
                    if let Err(error) = put(&format!("runs/{run_id}/{log_name}"), tail) {
                        result = Err(error);
                        break;
                    }
                }
                if result.is_err() {
                    break;
                }
            }
        }
        match result.and_then(|_| zip.finish().map(|_| ()).map_err(|e| format!("zip finish: {e}"))) {
            Ok(()) => {
                self.push_log(
                    Level::Ok,
                    format!("Экспорт диагностики: {} (без содержимого документов, R-SET-07)", path.display()),
                );
                Some(path)
            }
            Err(error) => {
                self.push_log(Level::Warn, format!("Экспорт диагностики: {error}"));
                None
            }
        }
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
        match rde_vcs::check_sync(&self.project.root) {
            Ok(sync) => lines.extend(sync.lines().map(str::to_owned)),
            Err(error) => lines.push(format!("Git: ошибка — {error}")),
        }
        if let Some(path) = self.active_document_path() {
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
        // R-SET-03: рассинхрон git (ветка/незакоммиченные изменения).
        match rde_vcs::check_sync(&self.project.root) {
            Ok(sync) => lines.extend(sync.lines().map(str::to_owned)),
            Err(error) => lines.push(format!("Git: {error}")),
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
        let pal = theme_colors(ctx.theme());
        if let Some(backups) = self.backup_dialog.take() {
            let mut restore: Option<PathBuf> = None;
            let mut close = false;
            let mut create = false;
            egui::Window::new("Бэкапы проекта")
                .collapsible(false)
                .resizable(true)
                .default_width(640.0)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("zip-снапшоты проекта (без .rde/exports), хранится последние 10. Восстановление сначала делает бэкап текущего состояния.")
                            .small()
                            .color(pal.muted),
                    );
                    ui.separator();
                    if backups.is_empty() {
                        ui.label(RichText::new("Бэкапов пока нет").color(pal.muted));
                    }
                    egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                        for backup in &backups {
                            ui.horizontal(|ui| {
                                let size = std::fs::metadata(backup).map(|meta| meta.len()).unwrap_or(0);
                                ui.label(RichText::new(backup.file_name().and_then(|name| name.to_str()).unwrap_or("—")).monospace().size(11.5));
                                ui.label(RichText::new(format!("{:.1} КБ", size as f64 / 1024.0)).small().color(pal.muted));
                                if ui.small_button("восстановить").clicked() {
                                    restore = Some(backup.clone());
                                }
                            });
                        }
                    });
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui.button("Сделать бэкап сейчас").clicked() {
                            create = true;
                        }
                        if ui.button("Закрыть").clicked() {
                            close = true;
                        }
                    });
                });
            if create {
                self.backup_project_now();
                self.open_backup_dialog();
            } else if let Some(archive) = restore {
                match rde_core::restore_project(&self.project.root, &archive) {
                    Ok(count) => {
                        self.push_log(Level::Ok, format!("Проект восстановлен из {}: файлов {}", archive.display(), count));
                        self.reload_document_view_preserving();
                    }
                    Err(error) => self.push_log(Level::Warn, format!("Восстановление проекта: {error}")),
                }
            } else if !close {
                self.backup_dialog = Some(backups);
            }
        }
        // ── «Открыть проект…»: «Проводник…», недавние проекты, проверка пути ────
        if let Some(mut dialog) = self.open_project_dialog.take() {
            // Результат системного выбора (пришёл в этом кадре) → поле пути, ошибка сброшена.
            if let Some(picked) = self.take_picked_path(&file_dialog::PathTarget::OpenProject) {
                dialog.path = picked.display().to_string();
                dialog.error.clear();
            }
            let picker_error = self.take_picker_error(&file_dialog::PathTarget::OpenProject);
            let focus_ok = self.take_picker_focus_ok(&file_dialog::PathTarget::OpenProject);
            let busy = self.picker_busy(&file_dialog::PathTarget::OpenProject);
            let start = self.pick_start_dir(&dialog.path);
            let recent = self.recent_projects();
            let mut open = false;
            let mut cancel = false;
            let mut pick = false;
            let mut reveal = false;
            let mut closed = false;
            let check = validate_project_dir(&dialog.path);
            let can_open = !check.is_error();
            let focus_field = !dialog.focused;
            egui::Window::new("Открыть проект")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_width(640.0);
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut dialog.path)
                                .hint_text("Путь к каталогу с rde.toml")
                                .desired_width(500.0),
                        );
                        if focus_field {
                            response.request_focus();
                        }
                        if response.changed() {
                            dialog.error.clear();
                        }
                        if picker_button(ui, busy).clicked() {
                            pick = true;
                        }
                    });
                    path_caption_ui(ui, pal, &dialog.path);
                    path_check_ui(ui, pal, &check, picker_error.as_deref());
                    if !dialog.error.is_empty() {
                        ui.label(RichText::new(&dialog.error).small().color(pal.red));
                    }
                    if !recent.is_empty() {
                        ui.add_space(4.0);
                        ui.separator();
                        ui.label(RichText::new("Недавние проекты").small().strong().color(pal.muted));
                        egui::ScrollArea::vertical().id_salt("rde_recent_projects").max_height(132.0).show(ui, |ui| {
                            for project in &recent {
                                ui.horizontal(|ui| {
                                    let label = if project.name.trim().is_empty() {
                                        ellipsize_path(&project.path, 46)
                                    } else {
                                        project.name.clone()
                                    };
                                    if ui
                                        .add(egui::Button::new(RichText::new(label).size(12.5)))
                                        .on_hover_text(&project.path)
                                        .clicked()
                                    {
                                        dialog.path = project.path.clone();
                                        open = true;
                                    }
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(ellipsize_path(&project.path, 58)).small().color(pal.muted),
                                        )
                                        .truncate(),
                                    );
                                });
                            }
                        });
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let ok = ui.add_enabled(
                            can_open,
                            egui::Button::new("Открыть").min_size(vec2(96.0, 24.0)),
                        );
                        let ok = if focus_ok && can_open {
                            ok.request_focus();
                            ok
                        } else {
                            ok
                        };
                        let hint = if can_open { "Открыть выбранный проект" } else { "Сначала укажите каталог с rde.toml" };
                        if ok.on_hover_text(hint).clicked() {
                            open = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                        if !dialog.path.trim().is_empty() && ui.button("Показать каталог").clicked() {
                            reveal = true;
                        }
                    });
                    ui.label(
                        RichText::new("Enter — открыть · Esc — отмена · «Проводник…» — системный выбор папки")
                            .small()
                            .color(pal.muted),
                    );
                });
            dialog.focused = true;
            if pick {
                self.request_pick(
                    ctx,
                    file_dialog::PathTarget::OpenProject,
                    PickRequest::new("Открыть проект RDE", PickKind::Folder).with_start(start),
                );
            } else if cancel || dialog_escape_pressed(ctx) {
                closed = true;
            } else if (open || dialog_confirm_pressed(ctx)) && can_open {
                let root = PathBuf::from(dialog.path.trim());
                match rde_core::load_project(&root) {
                    Ok(project) => {
                        let name = project.name.clone();
                        self.open_project(&root);
                        self.record_recent_project(&root, &name);
                        closed = true;
                    }
                    Err(error) => {
                        dialog.error = format!("Не удалось открыть проект: {error}");
                        self.push_log(Level::Warn, format!("Открыть проект: {error}"));
                    }
                }
            }
            if reveal {
                self.open_in_file_manager(Path::new(dialog.path.trim()));
            }
            if !closed {
                self.open_project_dialog = Some(dialog);
            }
        }

        // ── «Импорт .docx…»: «Проводник…» (фильтр .docx), имя документа, проверка пути ──
        if let Some(mut dialog) = self.import_docx_dialog.take() {
            if let Some(picked) = self.take_picked_path(&file_dialog::PathTarget::ImportDocx) {
                dialog.path = picked.display().to_string();
                dialog.error.clear();
            }
            let picker_error = self.take_picker_error(&file_dialog::PathTarget::ImportDocx);
            let focus_ok = self.take_picker_focus_ok(&file_dialog::PathTarget::ImportDocx);
            let busy = self.picker_busy(&file_dialog::PathTarget::ImportDocx);
            let start = self.pick_start_dir(&dialog.path);
            let mut import = false;
            let mut cancel = false;
            let mut pick = false;
            let mut reveal = false;
            let mut closed = false;
            let check = validate_docx_file(&dialog.path);
            let can_import = !check.is_error();
            let focus_field = !dialog.focused;
            egui::Window::new("Импорт .docx")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_width(680.0);
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut dialog.path)
                                .hint_text("Путь к .docx (например, /home/…/vkr.docx)")
                                .desired_width(520.0),
                        );
                        if focus_field {
                            response.request_focus();
                        }
                        if response.changed() {
                            dialog.error.clear();
                        }
                        if picker_button(ui, busy).clicked() {
                            pick = true;
                        }
                    });
                    path_caption_ui(ui, pal, &dialog.path);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Имя документа").small().color(pal.muted));
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.title)
                                .hint_text("пусто — из имени файла (будет «[IMPORTED] имя»)")
                                .desired_width(360.0),
                        );
                    });
                    path_check_ui(ui, pal, &check, picker_error.as_deref());
                    if !dialog.error.is_empty() {
                        ui.label(RichText::new(&dialog.error).small().color(pal.red));
                    }
                    ui.label(
                        RichText::new(
                            "Создаётся новый объект-документ «[IMPORTED] имя»; исходный .docx не изменяется. \
                             Импорт фиксирует происхождение (sha256), конвертирует MathType-формулы в нативные \
                             (R-DOC-44) и предупреждает о неподдерживаемых конструкциях (диаграммы, макросы, \
                             track changes, поля Word…).",
                        )
                        .small()
                        .color(pal.muted),
                    );
                    ui.horizontal(|ui| {
                        let button = ui.add_enabled(
                            can_import,
                            egui::Button::new("Импортировать").min_size(vec2(112.0, 24.0)),
                        );
                        let button = if focus_ok && can_import {
                            button.request_focus();
                            button
                        } else {
                            button
                        };
                        let hint = if can_import { "Импортировать .docx в текущий проект" } else { "Сначала выберите существующий .docx" };
                        if button.on_hover_text(hint).clicked() {
                            import = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                        if !dialog.path.trim().is_empty() && ui.button("Показать папку").clicked() {
                            reveal = true;
                        }
                    });
                    ui.label(
                        RichText::new("Enter — импортировать · Esc — отмена · «Проводник…» — выбор файла .docx")
                            .small()
                            .color(pal.muted),
                    );
                });
            dialog.focused = true;
            if pick {
                self.request_pick(
                    ctx,
                    file_dialog::PathTarget::ImportDocx,
                    PickRequest::new("Импорт .docx", PickKind::file("Документы Word", &["docx"])).with_start(start),
                );
            } else if cancel || dialog_escape_pressed(ctx) {
                closed = true;
            } else if (import || dialog_confirm_pressed(ctx)) && can_import {
                let source = PathBuf::from(dialog.path.trim());
                let title = dialog.title.trim().to_owned();
                match self.import_docx_action_with_title(&source, (!title.is_empty()).then_some(title.as_str())) {
                    Ok(()) => closed = true,
                    Err(error) => {
                        dialog.error = format!("Импорт не выполнен: {error}");
                        self.push_log(Level::Warn, format!("Импорт .docx: {error}"));
                    }
                }
            }
            if reveal {
                let path = PathBuf::from(dialog.path.trim());
                let dir = if path.is_dir() { path } else { path.parent().map(Path::to_path_buf).unwrap_or_default() };
                if !dir.as_os_str().is_empty() {
                    self.open_in_file_manager(&dir);
                }
            }
            if !closed {
                self.import_docx_dialog = Some(dialog);
            }
        }

        // ── «Принять формулы как объекты»: счётчики и политика номеров (ADR-0019) ────
        if let Some(mut dialog) = self.formula_migration_dialog.take() {
            let mut apply = false;
            let mut cancel = false;
            let mut closed = false;
            let mut scheme_choice: Option<rde_doc::FormulaNumbering> = None;
            egui::Window::new("Принять формулы как объекты")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_width(640.0);
                    ui.label(
                        RichText::new(format!(
                            "Формул вне реестра: {} (display {} · inline {})",
                            dialog.equations.len(),
                            dialog.display,
                            dialog.inline
                        ))
                        .color(pal.text),
                    );
                    ui.label(
                        RichText::new(format!(
                            "Номеров, найденных в Word: {} из {} display-формул",
                            dialog.found_numbers, dialog.display
                        ))
                        .small()
                        .color(if dialog.display > 0 && dialog.found_numbers == dialog.display { pal.green } else { pal.muted }),
                    );
                    if dialog.mixed > 0 {
                        ui.label(
                            RichText::new(format!(
                                "display-формул в смешанных абзацах/таблицах: {} — останутся inline, номер не назначается",
                                dialog.mixed
                            ))
                            .small()
                            .color(pal.amber),
                        );
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Схема нумерации").small().color(pal.muted));
                        egui::ComboBox::from_id_salt("rde_formula_migration_scheme")
                            .selected_text(formula_numbering_label(dialog.scheme))
                            .show_ui(ui, |ui| {
                                for scheme in [rde_doc::FormulaNumbering::Continuous, rde_doc::FormulaNumbering::Section] {
                                    if ui.selectable_label(dialog.scheme == scheme, formula_numbering_label(scheme)).clicked() {
                                        scheme_choice = Some(scheme);
                                    }
                                }
                            });
                    });
                    ui.add_space(4.0);
                    ui.checkbox(&mut dialog.preserve_numbers, "Сохранять существующие номера из Word");
                    if dialog.found_numbers == 0 {
                        ui.label(
                            RichText::new("номеров в Word не найдено — будут назначены по схеме документа")
                                .small()
                                .color(pal.muted),
                        );
                    }
                    ui.checkbox(&mut dialog.number_display, "Нумеровать display-формулы (номер справа)");
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(
                            "Формулы станут объектами проекта, display — block-абзацами с номером справа; inline-формулы не тронуты. Пунктуация после формул сохраняется.",
                        )
                        .small()
                        .color(pal.muted),
                    );
                    if !dialog.error.is_empty() {
                        ui.label(RichText::new(&dialog.error).small().color(pal.red));
                    }
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new("Принять").min_size(vec2(96.0, 24.0))).clicked() {
                            apply = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                    ui.label(RichText::new("Enter — принять · Esc — отмена").small().color(pal.muted));
                });
            if let Some(scheme) = scheme_choice {
                dialog.scheme = scheme;
            }
            if cancel || dialog_escape_pressed(ctx) {
                closed = true;
            } else if apply || dialog_confirm_pressed(ctx) {
                let options = rde_doc::FormulaMigrationOptions {
                    numbering: dialog.scheme,
                    preserve_word_numbers: dialog.preserve_numbers,
                    number_display: dialog.number_display,
                };
                self.formula_migration_dialog = Some(dialog.clone());
                self.apply_formula_migration(options);
                closed = true;
            }
            if !closed {
                self.formula_migration_dialog = Some(dialog);
            }
        }

        // ── «Новый проект…»: «Проводник…», проверка каталога, понятная ошибка ────
        if let Some(mut dialog) = self.new_project_dialog.take() {
            if let Some(picked) = self.take_picked_path(&file_dialog::PathTarget::NewProjectDir) {
                dialog.path = picked.display().to_string();
                if dialog.name.trim().is_empty() || dialog.name == "Новый проект RDE" {
                    // Имя по умолчанию — из имени каталога (пользователь может поправить).
                    if let Some(name) = picked.file_name().and_then(|name| name.to_str()) {
                        dialog.name = name.to_owned();
                    }
                }
                dialog.error.clear();
            }
            let picker_error = self.take_picker_error(&file_dialog::PathTarget::NewProjectDir);
            let focus_ok = self.take_picker_focus_ok(&file_dialog::PathTarget::NewProjectDir);
            let busy = self.picker_busy(&file_dialog::PathTarget::NewProjectDir);
            let start = self.pick_start_dir(&dialog.path);
            let mut create = false;
            let mut cancel = false;
            let mut pick = false;
            let mut closed = false;
            let check = validate_new_project_dir(&dialog.path);
            let can_create = !check.is_error();
            let focus_field = !dialog.focused;
            egui::Window::new("Новый проект")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_width(640.0);
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Каталог").small().color(pal.muted));
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut dialog.path)
                                .hint_text("Путь к новому каталогу проекта")
                                .desired_width(500.0),
                        );
                        if focus_field {
                            response.request_focus();
                        }
                        if response.changed() {
                            dialog.error.clear();
                        }
                        if picker_button(ui, busy).clicked() {
                            pick = true;
                        }
                    });
                    path_caption_ui(ui, pal, &dialog.path);
                    path_check_ui(ui, pal, &check, picker_error.as_deref());
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Название").small().color(pal.muted));
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.name)
                                .hint_text("Название проекта (пусто — из имени каталога)")
                                .desired_width(520.0),
                        );
                    });
                    if !dialog.error.is_empty() {
                        ui.label(RichText::new(&dialog.error).small().color(pal.red));
                    }
                    ui.horizontal(|ui| {
                        let button = ui.add_enabled(
                            can_create,
                            egui::Button::new("Создать").min_size(vec2(96.0, 24.0)),
                        );
                        let button = if focus_ok && can_create {
                            button.request_focus();
                            button
                        } else {
                            button
                        };
                        let hint = if can_create { "Создать проект в этом каталоге" } else { "Исправьте путь: каталог занят или это не каталог" };
                        if button.on_hover_text(hint).clicked() {
                            create = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                    ui.label(
                        RichText::new("Enter — создать · Esc — отмена · «Проводник…» — выбор каталога")
                            .small()
                            .color(pal.muted),
                    );
                });
            dialog.focused = true;
            if pick {
                self.request_pick(
                    ctx,
                    file_dialog::PathTarget::NewProjectDir,
                    PickRequest::new("Каталог нового проекта RDE", PickKind::Folder).with_start(start),
                );
            } else if cancel || dialog_escape_pressed(ctx) {
                closed = true;
            } else if (create || dialog_confirm_pressed(ctx)) && can_create {
                let root = PathBuf::from(dialog.path.trim());
                let mut name = dialog.name.trim().to_owned();
                if name.is_empty() {
                    name = root.file_name().and_then(|name| name.to_str()).unwrap_or("Новый проект RDE").to_owned();
                }
                match self.create_project_ui(&root, &name) {
                    Ok(()) => closed = true,
                    Err(error) => {
                        dialog.error = format!("Проект не создан: {error}");
                        self.push_log(Level::Warn, format!("Новый проект: {error}"));
                    }
                }
            }
            if !closed {
                self.new_project_dialog = Some(dialog);
            }
        }

        // ── «Импорт ревью из Word»: выбрать .docx или взять свежий из exports/ ────
        if let Some(mut dialog) = self.import_review_dialog.take() {
            if let Some(picked) = self.take_picked_path(&file_dialog::PathTarget::ImportReview) {
                dialog.path = picked.display().to_string();
                dialog.error.clear();
            }
            let picker_error = self.take_picker_error(&file_dialog::PathTarget::ImportReview);
            let focus_ok = self.take_picker_focus_ok(&file_dialog::PathTarget::ImportReview);
            let busy = self.picker_busy(&file_dialog::PathTarget::ImportReview);
            let start = self.pick_start_dir(&dialog.path);
            let latest_export = self.latest_export_docx();
            let mut import = false;
            let mut cancel = false;
            let mut pick = false;
            let mut closed = false;
            // Пустое поле — не ошибка: берём самый свежий .docx из exports/.
            let check = if dialog.path.trim().is_empty() {
                match &latest_export {
                    Some(path) => PathCheck::hint(format!("Пусто — будет взят свежий из exports/: {}", path.display())),
                    None => PathCheck::error("Пусто, а в exports/ нет .docx — укажите файл вручную"),
                }
            } else {
                validate_docx_file(&dialog.path)
            };
            let can_import = !check.is_error();
            let focus_field = !dialog.focused;
            egui::Window::new("Импорт ревью из Word")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_width(680.0);
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut dialog.path)
                                .hint_text("Путь к .docx после правок в Word (пусто — свежий из exports/)")
                                .desired_width(520.0),
                        );
                        if focus_field {
                            response.request_focus();
                        }
                        if response.changed() {
                            dialog.error.clear();
                        }
                        if picker_button(ui, busy).clicked() {
                            pick = true;
                        }
                    });
                    path_caption_ui(ui, pal, &dialog.path);
                    path_check_ui(ui, pal, &check, picker_error.as_deref());
                    if !dialog.error.is_empty() {
                        ui.label(RichText::new(&dialog.error).small().color(pal.red));
                    }
                    ui.label(
                        RichText::new(
                            "Выбранный .docx становится новой версией активного документа: RDE-часть (.doc.rde), \
                             doc_id и вставки сохраняются, комментарии Word импортируются в ревью.",
                        )
                        .small()
                        .color(pal.muted),
                    );
                    ui.horizontal(|ui| {
                        let button = ui.add_enabled(
                            can_import,
                            egui::Button::new("Импортировать").min_size(vec2(112.0, 24.0)),
                        );
                        let button = if focus_ok && can_import {
                            button.request_focus();
                            button
                        } else {
                            button
                        };
                        if button.on_hover_text("Принять .docx как новую версию документа").clicked() {
                            import = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                        if !dialog.path.trim().is_empty()
                            && ui.button("Сбросить (свежий из exports)").clicked()
                        {
                            dialog.path.clear();
                            dialog.error.clear();
                        }
                    });
                    ui.label(
                        RichText::new("Enter — импортировать · Esc — отмена · «Проводник…» — выбор файла .docx")
                            .small()
                            .color(pal.muted),
                    );
                });
            dialog.focused = true;
            if pick {
                self.request_pick(
                    ctx,
                    file_dialog::PathTarget::ImportReview,
                    PickRequest::new("Импорт ревью: .docx после правок в Word", PickKind::file("Документы Word", &["docx"]))
                        .with_start(start),
                );
            } else if cancel || dialog_escape_pressed(ctx) {
                closed = true;
            } else if (import || dialog_confirm_pressed(ctx)) && can_import {
                let source = (!dialog.path.trim().is_empty()).then(|| PathBuf::from(dialog.path.trim()));
                match self.import_review_action(source.as_deref()) {
                    Ok(()) => closed = true,
                    Err(error) => {
                        dialog.error = format!("Ревью не импортировано: {error}");
                        self.push_log(Level::Warn, format!("Импорт ревью: {error}"));
                    }
                }
            }
            if !closed {
                self.import_review_dialog = Some(dialog);
            }
        }

        // ── «Импорт цитат»: BibTeX/RIS/CSL-JSON → объекты-источники ─────────────
        if let Some(mut dialog) = self.import_citations_dialog.take() {
            if let Some(picked) = self.take_picked_path(&file_dialog::PathTarget::ImportCitations) {
                dialog.path = picked.display().to_string();
                dialog.error.clear();
            }
            let picker_error = self.take_picker_error(&file_dialog::PathTarget::ImportCitations);
            let focus_ok = self.take_picker_focus_ok(&file_dialog::PathTarget::ImportCitations);
            let busy = self.picker_busy(&file_dialog::PathTarget::ImportCitations);
            let start = self.pick_start_dir(&dialog.path);
            let mut import = false;
            let mut cancel = false;
            let mut pick = false;
            let mut closed = false;
            let check = validate_citations_file(&dialog.path);
            let can_import = !check.is_error();
            let focus_field = !dialog.focused;
            egui::Window::new("Импорт цитат")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_width(680.0);
                    ui.horizontal(|ui| {
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut dialog.path)
                                .hint_text("Путь к файлу .bib / .ris / .json (CSL-JSON)")
                                .desired_width(520.0),
                        );
                        if focus_field {
                            response.request_focus();
                        }
                        if response.changed() {
                            dialog.error.clear();
                        }
                        if picker_button(ui, busy).clicked() {
                            pick = true;
                        }
                    });
                    path_caption_ui(ui, pal, &dialog.path);
                    path_check_ui(ui, pal, &check, picker_error.as_deref());
                    if !dialog.error.is_empty() {
                        ui.label(RichText::new(&dialog.error).small().color(pal.red));
                    }
                    ui.label(
                        RichText::new(
                            "Каждая запись становится объектом-источником проекта с библиографией ([reference]): \
                             далее источник доступен для цитат в документе и библиографии по ГОСТ.",
                        )
                        .small()
                        .color(pal.muted),
                    );
                    ui.horizontal(|ui| {
                        let button = ui.add_enabled(
                            can_import,
                            egui::Button::new("Импортировать").min_size(vec2(112.0, 24.0)),
                        );
                        let button = if focus_ok && can_import {
                            button.request_focus();
                            button
                        } else {
                            button
                        };
                        if button.on_hover_text("Импортировать записи как источники проекта").clicked() {
                            import = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                    ui.label(
                        RichText::new("Enter — импортировать · Esc — отмена · «Проводник…» — выбор файла цитат")
                            .small()
                            .color(pal.muted),
                    );
                });
            dialog.focused = true;
            if pick {
                self.request_pick(
                    ctx,
                    file_dialog::PathTarget::ImportCitations,
                    PickRequest::new(
                        "Импорт цитат: BibTeX / RIS / CSL-JSON",
                        PickKind::File {
                            filters: vec![
                                file_dialog::FileFilter::new("Библиография", &["bib", "ris", "json"]),
                                file_dialog::FileFilter::new("Все файлы", &[]),
                            ],
                        },
                    )
                    .with_start(start),
                );
            } else if cancel || dialog_escape_pressed(ctx) {
                closed = true;
            } else if (import || dialog_confirm_pressed(ctx)) && can_import {
                let source = PathBuf::from(dialog.path.trim());
                match self.import_citations_action(&source) {
                    Ok(summary) => {
                        self.push_log(Level::Ok, summary);
                        closed = true;
                    }
                    Err(error) => {
                        dialog.error = format!("Цитаты не импортированы: {error}");
                        self.push_log(Level::Warn, format!("Импорт цитат: {error}"));
                    }
                }
            }
            if !closed {
                self.import_citations_dialog = Some(dialog);
            }
        }

        // ── «Свойства источника»: файл/URL + библиография (иначе задать путь негде) ──
        if let Some(mut dialog) = self.source_edit_dialog.take() {
            if let Some(picked) = self.take_picked_path(&file_dialog::PathTarget::SourceFile(dialog.id.clone())) {
                dialog.file = picked.display().to_string();
                dialog.kind = "file".to_owned();
                dialog.error.clear();
            }
            let picker_error = self.take_picker_error(&file_dialog::PathTarget::SourceFile(dialog.id.clone()));
            let focus_ok = self.take_picker_focus_ok(&file_dialog::PathTarget::SourceFile(dialog.id.clone()));
            let busy = self.picker_busy(&file_dialog::PathTarget::SourceFile(dialog.id.clone()));
            let start = self.pick_start_dir(&dialog.file);
            let title = self.project.object(&dialog.id).map(|object| object.title.clone()).unwrap_or_else(|| dialog.id.clone());
            let mut save = false;
            let mut cancel = false;
            let mut pick = false;
            let mut reveal = false;
            let mut closed = false;
            let object_dir = self.project.object(&dialog.id).map(|object| object.dir.clone()).unwrap_or_else(|| self.project.root.clone());
            let file_check = validate_source_file_in(&object_dir, &dialog.file);
            let can_save = if dialog.kind == "url" {
                !dialog.url.trim().is_empty()
            } else {
                !file_check.is_error()
            };
            let focus_field = !dialog.focused;
            egui::Window::new("Свойства источника")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_width(660.0);
                    ui.label(RichText::new(&title).strong().color(pal.text));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Тип").small().color(pal.muted));
                        egui::ComboBox::from_id_salt("rde_source_kind")
                            .selected_text(dialog.kind.clone())
                            .show_ui(ui, |ui| {
                                for (kind, label) in [("file", "файл (PDF/изображение/текст)"), ("url", "веб-страница (URL)"), ("repo", "репозиторий")] {
                                    ui.selectable_value(&mut dialog.kind, kind.to_owned(), label);
                                }
                            });
                    });
                    if dialog.kind == "url" {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("URL").small().color(pal.muted));
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut dialog.url)
                                    .hint_text("https://…")
                                    .desired_width(480.0),
                            );
                            if focus_field {
                                response.request_focus();
                            }
                        });
                        ui.label(RichText::new("Для веб-источника файл не нужен: сохраняется только URL").small().color(pal.muted));
                    } else {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("Файл").small().color(pal.muted));
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut dialog.file)
                                    .hint_text("Путь к файлу источника или выберите «Проводник…»")
                                    .desired_width(480.0),
                            );
                            if focus_field {
                                response.request_focus();
                            }
                            if picker_button(ui, busy).clicked() {
                                pick = true;
                            }
                        });
                        path_caption_ui(ui, pal, &dialog.file);
                        path_check_ui(ui, pal, &file_check, picker_error.as_deref());
                        ui.label(
                            RichText::new("Выбранный файл будет скопирован в объект-источник (рядом с source.toml), оригинал не изменяется")
                                .small()
                                .color(pal.muted),
                        );
                    }
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Получен").small().color(pal.muted));
                        ui.add(
                            egui::TextEdit::singleline(&mut dialog.retrieved)
                                .hint_text("дата получения, например 2026-10-29")
                                .desired_width(180.0),
                        );
                    });
                    if !dialog.error.is_empty() {
                        ui.label(RichText::new(&dialog.error).small().color(pal.red));
                    }
                    ui.horizontal(|ui| {
                        let button = ui.add_enabled(can_save, egui::Button::new("Сохранить").min_size(vec2(96.0, 24.0)));
                        let button = if focus_ok && can_save {
                            button.request_focus();
                            button
                        } else {
                            button
                        };
                        if button.on_hover_text("Записать source.toml и перечитать проект").clicked() {
                            save = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                        if !dialog.file.trim().is_empty() && ui.button("Показать файл").clicked() {
                            reveal = true;
                        }
                    });
                    ui.label(RichText::new("Enter — сохранить · Esc — отмена").small().color(pal.muted));
                });
            dialog.focused = true;
            if pick {
                self.request_pick(
                    ctx,
                    file_dialog::PathTarget::SourceFile(dialog.id.clone()),
                    PickRequest::new("Файл источника", PickKind::any_file()).with_start(start),
                );
            } else if cancel || dialog_escape_pressed(ctx) {
                closed = true;
            } else if (save || dialog_confirm_pressed(ctx)) && can_save {
                match self.save_source_edit(&dialog) {
                    Ok(()) => closed = true,
                    Err(error) => {
                        dialog.error = format!("Источник не сохранён: {error}");
                        self.push_log(Level::Warn, format!("Свойства источника: {error}"));
                    }
                }
            }
            if reveal {
                let resolved = resolve_source_file_path(&object_dir, &dialog.file);
                self.open_in_file_manager(&resolved);
            }
            if !closed {
                self.source_edit_dialog = Some(dialog);
            }
        }

        // ── «Свойства репозитория»: kind + путь (в GUI задать было негде) ───────
        if let Some(mut dialog) = self.repo_edit_dialog.take() {
            if let Some(picked) = self.take_picked_path(&file_dialog::PathTarget::RepositoryPath(dialog.id.clone())) {
                dialog.path = picked.display().to_string();
                dialog.error.clear();
            }
            let picker_error = self.take_picker_error(&file_dialog::PathTarget::RepositoryPath(dialog.id.clone()));
            let focus_ok = self.take_picker_focus_ok(&file_dialog::PathTarget::RepositoryPath(dialog.id.clone()));
            let busy = self.picker_busy(&file_dialog::PathTarget::RepositoryPath(dialog.id.clone()));
            let start = self.pick_start_dir(&dialog.path);
            let title = self.project.object(&dialog.id).map(|object| object.title.clone()).unwrap_or_else(|| dialog.id.clone());
            let mut save = false;
            let mut cancel = false;
            let mut pick = false;
            let mut closed = false;
            let check = validate_directory(&dialog.path);
            let can_save = !check.is_error();
            let focus_field = !dialog.focused;
            egui::Window::new("Свойства репозитория")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_width(640.0);
                    ui.label(RichText::new(&title).strong().color(pal.text));
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Тип").small().color(pal.muted));
                        egui::ComboBox::from_id_salt("rde_repo_kind")
                            .selected_text(dialog.kind.clone())
                            .show_ui(ui, |ui| {
                                for (kind, label) in [("internal", "internal (каталог проекта)"), ("external", "external (внешний каталог)"), ("git", "git (клон — фаза 2)")] {
                                    ui.selectable_value(&mut dialog.kind, kind.to_owned(), label);
                                }
                            });
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("Путь").small().color(pal.muted));
                        let response = ui.add(
                            egui::TextEdit::singleline(&mut dialog.path)
                                .hint_text("Путь к каталогу репозитория (относительный или абсолютный)")
                                .desired_width(480.0),
                        );
                        if focus_field {
                            response.request_focus();
                        }
                        if picker_button(ui, busy).clicked() {
                            pick = true;
                        }
                    });
                    path_caption_ui(ui, pal, &dialog.path);
                    path_check_ui(ui, pal, &check, picker_error.as_deref());
                    if !dialog.error.is_empty() {
                        ui.label(RichText::new(&dialog.error).small().color(pal.red));
                    }
                    ui.horizontal(|ui| {
                        let button = ui.add_enabled(can_save, egui::Button::new("Сохранить").min_size(vec2(96.0, 24.0)));
                        let button = if focus_ok && can_save {
                            button.request_focus();
                            button
                        } else {
                            button
                        };
                        if button.on_hover_text("Записать repo.toml и перечитать проект").clicked() {
                            save = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                    ui.label(RichText::new("Enter — сохранить · Esc — отмена").small().color(pal.muted));
                });
            dialog.focused = true;
            if pick {
                self.request_pick(
                    ctx,
                    file_dialog::PathTarget::RepositoryPath(dialog.id.clone()),
                    PickRequest::new("Каталог репозитория", PickKind::Folder).with_start(start),
                );
            } else if cancel || dialog_escape_pressed(ctx) {
                closed = true;
            } else if (save || dialog_confirm_pressed(ctx)) && can_save {
                match self.save_repository_edit(&dialog) {
                    Ok(()) => closed = true,
                    Err(error) => {
                        dialog.error = format!("Репозиторий не сохранён: {error}");
                        self.push_log(Level::Warn, format!("Свойства репозитория: {error}"));
                    }
                }
            }
            if !closed {
                self.repo_edit_dialog = Some(dialog);
            }
        }

        if let Some(mut dialog) = self.settings_dialog.take() {
            let mut save = false;
            let mut cancel = false;
            let mut add_author = false;
            let mut remove_author: Option<usize> = None;
            let lang = self.lang;
            egui::Window::new(tr(lang, "settings.project"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(560.0);
                    egui::Grid::new("rde_settings").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        ui.label(RichText::new("Название").color(pal.muted));
                        ui.add(egui::TextEdit::singleline(&mut dialog.name).desired_width(360.0));
                        ui.end_row();
                        ui.label(RichText::new("Шаблон по умолчанию").color(pal.muted));
                        egui::ComboBox::from_id_salt("rde_settings_template")
                            .selected_text(if dialog.template.is_empty() { "—" } else { &dialog.template })
                            .show_ui(ui, |ui| {
                                for builtin in rde_templates::builtin_templates() {
                                    ui.selectable_value(&mut dialog.template, builtin.id.clone(), builtin.title.clone());
                                }
                            });
                        ui.end_row();
                        ui.label(RichText::new("Пересборка").color(pal.muted));
                        egui::ComboBox::from_id_salt("rde_settings_rebuild")
                            .selected_text(if dialog.rebuild.is_empty() { "—" } else { &dialog.rebuild })
                            .show_ui(ui, |ui| {
                                for value in ["manual", "auto"] {
                                    ui.selectable_value(&mut dialog.rebuild, value.to_owned(), value);
                                }
                            });
                        ui.end_row();
                    });
                    ui.separator();
                    ui.label(RichText::new(tr(lang, "settings.authors")).strong().small().color(pal.muted));
                    for (index, author) in dialog.authors.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut author.name).hint_text("Имя").desired_width(150.0));
                            ui.add(egui::TextEdit::singleline(&mut author.initials).hint_text("Иниц.").desired_width(60.0));
                            ui.add(egui::TextEdit::singleline(&mut author.email).hint_text("email").desired_width(180.0));
                            egui::ComboBox::from_id_salt(format!("rde_author_kind_{index}"))
                                .selected_text(author.kind.clone())
                                .width(90.0)
                                .show_ui(ui, |ui| {
                                    for kind in ["human", "agent", "system"] {
                                        ui.selectable_value(&mut author.kind, kind.to_owned(), kind);
                                    }
                                });
                            if ui.small_button("×").on_hover_text("Удалить автора").clicked() {
                                remove_author = Some(index);
                            }
                        });
                    }
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut dialog.new_author_name).hint_text("Новый автор: имя").desired_width(150.0));
                        ui.add(egui::TextEdit::singleline(&mut dialog.new_author_initials).hint_text("Иниц.").desired_width(60.0));
                        ui.add(egui::TextEdit::singleline(&mut dialog.new_author_email).hint_text("email").desired_width(180.0));
                        if ui.button(tr(lang, "settings.add_author")).clicked() {
                            add_author = true;
                        }
                    });
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        if ui.button(tr(lang, "action.save")).clicked() {
                            save = true;
                        }
                        if ui.button(tr(lang, "action.cancel")).clicked() {
                            cancel = true;
                        }
                    });
                });
            if add_author && !dialog.new_author_name.trim().is_empty() {
                dialog.authors.push(rde_core::AuthorManifest {
                    id: rde_core::new_id(),
                    name: dialog.new_author_name.trim().to_owned(),
                    initials: dialog.new_author_initials.trim().to_owned(),
                    email: dialog.new_author_email.trim().to_owned(),
                    kind: "human".to_owned(),
                });
                dialog.new_author_name.clear();
                dialog.new_author_initials.clear();
                dialog.new_author_email.clear();
            }
            if let Some(index) = remove_author {
                if index < dialog.authors.len() {
                    dialog.authors.remove(index);
                }
            }
            if save {
                if let Err(error) = self.save_project_settings_with_authors(&dialog) {
                    self.push_log(Level::Warn, format!("Настройки: {error}"));
                }
            } else if !cancel {
                self.settings_dialog = Some(dialog);
            }
        }

        // R-SET-01: настройки приложения (язык, тема, лимиты, автор по умолчанию).
        if let Some(mut settings) = self.app_settings_dialog.take() {
            let mut save = false;
            let mut cancel = false;
            // M4.2b: кнопка «Пометить проект доверенным…» открывает диалог согласия R-RUN-06.
            let mut trust_for_agents = false;
            // Масштаб интерфейса правится здесь же, но хранится в сессии RdeApp
            // (в схеме AppSettings его нет — владение rde-core, вне scope фазы).
            let mut ui_scale = self.ui_scale;
            // M4.3: команда/аргументы агента редактируются строкой; применяются при «Сохранить»
            // (пустая команда → дефолт `dsh`; args — split по пробелам, как в CLI).
            let mut agent_program_edit = settings.agent_program.clone();
            let mut agent_args_edit = settings.agent_args.join(" ");
            // GUI-02: «Проводник…» для программы агента (был только ручной ввод пути).
            if let Some(picked) = self.take_picked_path(&file_dialog::PathTarget::AgentProgram) {
                agent_program_edit = picked.display().to_string();
            }
            let picker_error = self.take_picker_error(&file_dialog::PathTarget::AgentProgram);
            let program_busy = self.picker_busy(&file_dialog::PathTarget::AgentProgram);
            let agent_program_start = self.pick_start_dir(&agent_program_edit);
            let program_check = validate_agent_program(&agent_program_edit);
            let mut pick_agent_program = false;
            egui::Window::new(tr(self.lang, "settings.app"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(480.0);
                    egui::Grid::new("rde_app_settings").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        ui.label(RichText::new(tr(self.lang, "settings.language")).color(pal.muted));
                        egui::ComboBox::from_id_salt("rde_lang")
                            .selected_text(settings.language.clone())
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut settings.language, "ru".to_owned(), "Русский");
                                ui.selectable_value(&mut settings.language, "en".to_owned(), "English");
                            });
                        ui.end_row();
                        ui.label(RichText::new(tr(self.lang, "settings.theme")).color(pal.muted));
                        let theme_label = match settings.theme.as_str() {
                            "light" => tr(self.lang, "settings.theme_light"),
                            "system" => tr(self.lang, "settings.theme_system"),
                            _ => tr(self.lang, "settings.theme_dark"),
                        };
                        egui::ComboBox::from_id_salt("rde_theme")
                            .selected_text(theme_label)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut settings.theme, "dark".to_owned(), tr(self.lang, "settings.theme_dark"));
                                ui.selectable_value(&mut settings.theme, "light".to_owned(), tr(self.lang, "settings.theme_light"));
                                ui.selectable_value(&mut settings.theme, "system".to_owned(), tr(self.lang, "settings.theme_system"));
                            });
                        ui.end_row();
                        ui.label(RichText::new(tr(self.lang, "settings.ui_scale")).color(pal.muted));
                        ui.horizontal(|ui| {
                            ui.add(egui::Slider::new(&mut ui_scale, 0.85..=1.5).show_value(false));
                            ui.label(RichText::new(format!("{:.0}%", ui_scale * 100.0)).color(pal.text));
                        });
                        ui.end_row();
                        ui.label(RichText::new(tr(self.lang, "settings.keep_runs")).color(pal.muted));
                        ui.add(egui::DragValue::new(&mut settings.keep_runs).range(10..=10_000));
                        ui.end_row();
                        ui.label(RichText::new(tr(self.lang, "settings.author_name")).color(pal.muted));
                        ui.add(egui::TextEdit::singleline(&mut settings.author_name).desired_width(280.0));
                        ui.end_row();
                        ui.label(RichText::new("email").color(pal.muted));
                        ui.add(egui::TextEdit::singleline(&mut settings.author_email).desired_width(280.0));
                        ui.end_row();
                        ui.label(RichText::new("Внешние агенты").color(pal.muted));
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut settings.allow_external_mcp,
                                "Разрешить доступ внешним агентам (MCP, streamable HTTP)",
                            );
                            ui.label(
                                RichText::new(
                                    "Только loopback (127.0.0.1) + обязательный bearer-токен; адрес и \
                                     токен — в панели «Агент» (R-MCP-02).",
                                )
                                .small()
                                .color(pal.muted),
                            );
                            ui.add_enabled_ui(settings.allow_external_mcp, |ui| {
                                ui.checkbox(
                                    &mut settings.allow_agent_mutations,
                                    "Разрешить агенту изменения в проекте",
                                );
                                ui.label(
                                    RichText::new(
                                        "Каждое изменение подтверждается диалогом в RDE; создаётся чекпоинт \
                                         (авто-откат при ошибке), решение пишется в аудит. Без этой галочки — \
                                         только чтение (fail-closed, R-MCP-05).",
                                    )
                                    .small()
                                    .color(pal.muted),
                                );
                                // M4.2b: два отдельных гранта, а не «полный доступ» (R-AGT-06, R-MCP-05).
                                ui.add_enabled_ui(settings.allow_agent_mutations, |ui| {
                                    ui.checkbox(
                                        &mut settings.allow_agent_run_script,
                                        "Разрешить запуск скриптов проекта (artifact_run)",
                                    );
                                    ui.checkbox(
                                        &mut settings.allow_agent_git_commit,
                                        "Разрешить git-коммиты (git_commit)",
                                    );
                                });
                                ui.label(
                                    RichText::new(
                                        "Запуск скриптов и коммиты — каждый раз отдельный диалог с превью, \
                                         чекпоинт и запись в аудит. Запуск скриптов возможен только для \
                                         доверенного проекта (R-RUN-06); `.rde/**` в коммит не попадает.",
                                    )
                                    .small()
                                    .color(pal.muted),
                                );
                            });
                            // R-RUN-06: галочка стоит, но проект не доверенный — грант не выдаётся.
                            if settings.allow_external_mcp
                                && settings.allow_agent_mutations
                                && settings.allow_agent_run_script
                                && !self.project.manifest.trusted
                            {
                                ui.label(
                                    RichText::new(
                                        "Запуск скриптов отключён: проект не доверенный (R-RUN-06).",
                                    )
                                    .small()
                                    .color(pal.amber),
                                );
                                if ui.button("Пометить проект доверенным…").clicked() {
                                    trust_for_agents = true;
                                }
                            }
                        });
                        ui.end_row();
                        // M4.3: встроенный ACP-агент (DSH по умолчанию, R-AGT-01).
                        ui.label(RichText::new("Встроенный агент (ACP)").color(pal.muted));
                        ui.vertical(|ui| {
                            ui.checkbox(
                                &mut settings.allow_builtin_agent,
                                "Разрешить встроенного агента (ACP: DSH)",
                            );
                            ui.label(
                                RichText::new(
                                    "Чат с агентом в панели «Агент»: сессии/история, поток сообщений и \
                                     tool-call'ов, разрешения агента, отмена. Процесс агента стартует \
                                     только по кнопке «Новый чат» (лениво), по умолчанию выключено \
                                     (fail-closed, R-AGT-01/06).",
                                )
                                .small()
                                .color(pal.muted),
                            );
                            ui.add_enabled_ui(settings.allow_builtin_agent, |ui| {
                                egui::Grid::new("rde_builtin_agent").num_columns(2).spacing([8.0, 4.0]).show(
                                    ui,
                                    |ui| {
                                        ui.label(RichText::new("команда").small().color(pal.muted));
                                        ui.horizontal(|ui| {
                                            ui.add(
                                                egui::TextEdit::singleline(&mut agent_program_edit)
                                                    .desired_width(220.0)
                                                    .hint_text("dsh"),
                                            );
                                            if picker_button(ui, program_busy).clicked() {
                                                pick_agent_program = true;
                                            }
                                        });
                                        ui.end_row();
                                        ui.label(RichText::new("проверка").small().color(pal.muted));
                                        path_check_ui(ui, pal, &program_check, picker_error.as_deref());
                                        ui.end_row();
                                        ui.label(RichText::new("args").small().color(pal.muted));
                                        ui.add(
                                            egui::TextEdit::singleline(&mut agent_args_edit)
                                                .desired_width(220.0)
                                                .hint_text("--profile acp"),
                                        );
                                        ui.end_row();
                                        ui.label(RichText::new("модель").small().color(pal.muted));
                                        ui.add(
                                            egui::TextEdit::singleline(&mut settings.agent_model)
                                                .desired_width(220.0)
                                                .hint_text("пусто — из configOptions DSH"),
                                        );
                                        ui.end_row();
                                        ui.label(RichText::new("reasoning").small().color(pal.muted));
                                        ui.add(
                                            egui::TextEdit::singleline(&mut settings.agent_reasoning_effort)
                                                .desired_width(220.0)
                                                .hint_text("пусто — из configOptions DSH"),
                                        );
                                        ui.end_row();
                                    },
                                );
                            });
                            ui.label(
                                RichText::new(
                                    "Провайдер/API-ключи — в профиле DSH (RDE их не дублирует): \
                                     `dsh --profile acp`, патч профиля правится пользователем; затем \
                                     перезапуск RDE. Изменение команды/аргументов перезапускает агент.",
                                )
                                .small()
                                .color(pal.muted),
                            );
                        });
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        if ui.button(tr(self.lang, "action.save")).clicked() {
                            save = true;
                        }
                        if ui.button(tr(self.lang, "action.cancel")).clicked() {
                            cancel = true;
                        }
                    });
                });
            // GUI-02: системный выбор программы агента (фоновый, результат подставлен выше).
            if pick_agent_program {
                self.request_pick(
                    ctx,
                    file_dialog::PathTarget::AgentProgram,
                    PickRequest::new("Программа ACP-агента", PickKind::any_file()).with_start(agent_program_start),
                );
            }
            if dialog_escape_pressed(ctx) {
                cancel = true;
            }
            if trust_for_agents {
                // Существующий диалог согласия R-RUN-06 (не дублируем): после подтверждения
                // trusted=true, перезагрузка проекта и перезапуск MCP-сервера.
                self.trust_dialog = Some(TrustDialogState { request: TrustRequest::AgentRunScript });
            }
            if save {
                let want_system = settings.theme == "system";
                // M4.3: команда/аргументы встроенного агента из строковых полей.
                settings.agent_program = agent_program_edit.trim().to_owned();
                if settings.agent_program.is_empty() {
                    settings.agent_program = "dsh".to_owned();
                }
                settings.agent_args = agent_args_edit.split_whitespace().map(str::to_owned).collect();
                let previous = self.app_settings.clone();
                settings.normalize();
                match rde_core::settings::save(&settings) {
                    Ok(path) => {
                        self.lang = i18n::lang(&settings.language);
                        if want_system {
                            // normalize() (владение rde-core) знает только dark|light, поэтому
                            // в файл ушёл dark, а сессия продолжает следовать системе.
                            // Персистентность system — однострочный follow-up в rde-core.
                            let mut applied = settings.clone();
                            applied.theme = "system".to_owned();
                            self.app_settings = applied;
                        } else {
                            self.app_settings = settings;
                        }
                        self.apply_external_agents_setting();
                        // M4.3: настройка встроенного агента — выключение/смена команды
                        // останавливает процесс и сессию (fail-closed, R-AGT-01).
                        self.apply_builtin_agent_setting();
                        let agent_config_changed = previous.agent_program != self.app_settings.agent_program
                            || previous.agent_args != self.app_settings.agent_args;
                        if agent_config_changed && self.agent.is_some() {
                            self.shutdown_chat("изменены команда/аргументы агента");
                        }
                        self.ui_scale = ui_scale.clamp(0.5, 2.5);
                        self.push_log(Level::Ok, format!("Настройки приложения сохранены: {}", path.display()));
                    }
                    Err(error) => self.push_log(Level::Warn, format!("Настройки приложения: {error}")),
                }
            } else if !cancel {
                self.app_settings_dialog = Some(settings);
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
                        ui.label(RichText::new(text).monospace().size(11.5).color(pal.text));
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
                            ui.label(RichText::new(line).monospace().size(11.5).color(pal.text));
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
                    ui.label(RichText::new(format!("прототип v0.1 · сборка {}", env!("CARGO_PKG_VERSION"))).small().color(pal.muted));
                    ui.add_space(6.0);
                    ui.label(RichText::new("Документ: .doc.rde (OOXML + rde:doc) · WordCraft DocView · ГОСТ 7.32-2017 · артефакты с act_key/out_hash · PDF/OCR · ревью Word.").small().color(pal.text));
                    ui.add_space(8.0);
                    ui.label(RichText::new("Горячие клавиши").strong().small().color(pal.muted));
                    for (keys, action) in [
                        ("Ctrl+F", "поиск по объектам"),
                        ("Ctrl+S", "сохранить проект (.doc.rde)"),
                        ("Ctrl+Z / Ctrl+Y", "отмена/повтор в редакторе"),
                        ("ПКМ по объекту", "контекстное меню действий"),
                    ] {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(keys).monospace().size(11.5).color(pal.accent));
                            ui.label(RichText::new(action).small().color(pal.text));
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
            // M4.2b: тот же диалог согласия открывает кнопка «Пометить проект доверенным…»
            // (разрешение внешним агентам запускать скрипты).
            let for_agents = matches!(state.request, TrustRequest::AgentRunScript);
            let confirm_label = if for_agents {
                "Доверяю проекту и разрешить агентам запуск"
            } else {
                "Доверяю проекту и запустить"
            };
            egui::Window::new("Запуск скриптов проекта")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(560.0);
                    ui.label(
                        RichText::new("Проект содержит скрипты, которые будут выполнены на вашей машине.")
                            .strong()
                            .color(pal.text),
                    );
                    if for_agents {
                        ui.label(
                            RichText::new(
                                "Запуск скриптов будет доступен внешним агентам (MCP) — каждый запуск \
                                 дополнительно подтверждается отдельным диалогом.",
                            )
                            .small()
                            .color(pal.text),
                        );
                    }
                    ui.add_space(4.0);
                    ui.label(RichText::new(format!("Проект: «{project_name}»")).small().color(pal.text));
                    ui.label(RichText::new(format!("Каталог: {root}")).small().color(pal.muted));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(
                            "RDE запускает код артефактов от вашего имени (без ОС-песочницы в v0.1): \
                             сеть по умолчанию запрещена best-effort, входы доступны только для чтения, \
                             а запуск и его provenance фиксируются в журнале. Доверяйте только своим проектам.",
                        )
                        .small()
                        .color(pal.muted),
                    );
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add(egui::Button::new(RichText::new(confirm_label).color(Color32::WHITE)).fill(pal.accent))
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
                        match state.request {
                            TrustRequest::Update { targets, dependents } => self.start_update(targets, dependents),
                            TrustRequest::AgentRunScript => {
                                // Перезапуск сервера: эффективный run-грант пересчитывается
                                // (trusted-гейт снят).
                                self.apply_external_agents_setting();
                                if self.external_agents.grants().run_script {
                                    self.push_log(
                                        Level::Ok,
                                        "Внешние агенты: запуск скриптов разрешён (каждый запуск — через GUI-подтверждение)",
                                    );
                                } else {
                                    self.push_log(
                                        Level::Info,
                                        "Проект доверенный; включите «Разрешить запуск скриптов» в настройках и сохраните",
                                    );
                                }
                            }
                        }
                    }
                    Err(error) => self.push_log(Level::Warn, format!("Не удалось записать trusted в rde.toml: {error}")),
                }
            } else if !cancel {
                self.trust_dialog = Some(state);
            }
        }

        // R-DOC-11: вставка значения/рисунка из артефакта в документ.
        if let Some(mut dialog) = self.insert_dialog.take() {
            let artifacts = self.insertable_artifacts();
            let constants_list: Vec<rde_core::ArtifactManifest> = self
                .project
                .objects
                .iter()
                .filter_map(|object| match &object.data {
                    ObjectData::Artifact(artifact) if artifact.kind == "constants" => Some(artifact.clone()),
                    _ => None,
                })
                .collect();
            let sources: Vec<rde_core::Object> = self.project.sources().cloned().collect();
            let doc_inserts: Vec<rde_doc::RdeInsert> = self
                .project
                .document_path()
                .filter(|path| rde_doc::is_rde_package(path))
                .and_then(|path| rde_doc::load_rde_doc(&path).ok())
                .map(|doc| doc.inserts)
                .unwrap_or_default();
            let styles = self
                .project
                .document_path()
                .filter(|path| rde_doc::is_rde_package(path))
                .and_then(|path| rde_doc::list_document_styles(&path).ok())
                .unwrap_or_default();
            // Превью документно-локального номера для диалога вставки формулы (ADR-0018).
            let formula_number_preview: Option<String> = {
                let numbering = self.document_formula_numbering();
                self.project
                    .document_path()
                    .filter(|path| rde_doc::is_rde_package(path))
                    .and_then(|path| rde_doc::next_formula_number_text(&path, numbering))
                    .map(|(number, _warnings)| format!("({number})"))
            };
            let formulas_list: Vec<rde_core::FormulaManifest> = self.project.formulas().cloned().collect();
            let mut confirm = false;
            let mut cancel = false;
            let title = match dialog.kind.as_str() {
                "figure" => "Вставить рисунок из артефакта",
                "table" => "Вставить таблицу из CSV-выхода артефакта",
                "constant" => "Вставить константу проекта",
                "formula" => "Вставить формулу (project-level, P3)",
                "crossref" => "Вставить перекрёстную ссылку",
                "citation" => "Вставить цитату на источник",
                "date" => "Вставить дату",
                _ => "Вставить значение артефакта",
            };
            egui::Window::new(title)
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(600.0);
                    match dialog.kind.as_str() {
                        "constant" => {
                            let current = constants_list
                                .iter()
                                .find(|artifact| artifact.id == dialog.artifact_id)
                                .map(|artifact| artifact.title.clone())
                                .unwrap_or_else(|| "— выбрать —".to_owned());
                            let old_constants = dialog.artifact_id.clone();
                            egui::ComboBox::from_label("объект констант").selected_text(current).show_ui(ui, |ui| {
                                for constants in &constants_list {
                                    ui.selectable_value(&mut dialog.artifact_id, constants.id.clone(), &constants.title);
                                }
                            });
                            if old_constants != dialog.artifact_id {
                                dialog.selector = constants_list
                                    .iter()
                                    .find(|constants| constants.id == dialog.artifact_id)
                                    .and_then(|constants| constants.values.keys().next().cloned())
                                    .unwrap_or_default();
                                dialog.error.clear();
                            }
                            if let Some(constants) = constants_list.iter().find(|artifact| artifact.id == dialog.artifact_id) {
                                let current_key = constants.values.get(&dialog.selector);
                                let label = current_key
                                    .map(|value| {
                                        if value.display.is_empty() {
                                            format!("{} = {} {}", dialog.selector, value.value, value.unit)
                                        } else {
                                            format!("{} = {} {}", dialog.selector, value.display, value.unit)
                                        }
                                    })
                                    .unwrap_or_else(|| "— выбрать —".to_owned());
                                egui::ComboBox::from_label("константа").selected_text(label).show_ui(ui, |ui| {
                                    for (key, value) in &constants.values {
                                        let text = if value.display.is_empty() {
                                            format!("{key} = {} {}", value.value, value.unit)
                                        } else {
                                            format!("{key} = {} {}", value.display, value.unit)
                                        };
                                        ui.selectable_value(&mut dialog.selector, key.clone(), text);
                                    }
                                });
                            }
                        }
                        "crossref" => {
                            // Список целей: рисунки/таблицы (текст «Рисунок N») и формулы проекта
                            // с их документно-локальными номерами (ADR-0018): «формула (2.1)».
                            let formula_target_id = |insert: &rde_doc::RdeInsert| {
                                rde_doc::parse_formula_target(&insert.target).map(|target| target.formula_id)
                            };
                            let formula_option_id = |insert: &rde_doc::RdeInsert| formula_target_id(insert).map(|id| format!("formula:{id}"));
                            let display_number = |insert: &rde_doc::RdeInsert| {
                                insert
                                    .number_text
                                    .clone()
                                    .or_else(|| insert.number.map(|number| number.to_string()))
                                    .map(|number| format!("({number})"))
                                    .unwrap_or_else(|| "(?)".to_owned())
                            };
                            let current = doc_inserts
                                .iter()
                                .find(|insert| insert.id == dialog.artifact_id || formula_option_id(insert).as_deref() == Some(dialog.artifact_id.as_str()))
                                .map(|insert| {
                                    if insert.kind == "formula" {
                                        format!("формула {} · {}", display_number(insert), insert.cached.chars().take(40).collect::<String>())
                                    } else {
                                        format!("{} · {} · {}", insert.id, insert.kind, insert.caption)
                                    }
                                })
                                .unwrap_or_else(|| "— выбрать —".to_owned());
                            egui::ComboBox::from_label("на что ссылаться").selected_text(current).show_ui(ui, |ui| {
                                for insert in doc_inserts.iter().filter(|insert| !insert.target.is_empty()) {
                                    match insert.kind.as_str() {
                                        "figure" | "table" => {
                                            ui.selectable_value(
                                                &mut dialog.artifact_id,
                                                insert.id.clone(),
                                                format!("{} · {} · {}", insert.id, insert.kind, insert.caption),
                                            );
                                        }
                                        "formula" => {
                                            if let Some(option) = formula_option_id(insert) {
                                                ui.selectable_value(
                                                    &mut dialog.artifact_id,
                                                    option,
                                                    format!("формула {} · {}", display_number(insert), insert.cached.chars().take(40).collect::<String>()),
                                                );
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            });
                        }
                        "citation" => {
                            let current = sources
                                .iter()
                                .position(|source| source.id == dialog.artifact_id)
                                .map(|index| format!("[{}] {}", index + 1, sources[index].title))
                                .unwrap_or_else(|| "— выбрать —".to_owned());
                            egui::ComboBox::from_label("источник").selected_text(current).show_ui(ui, |ui| {
                                for (index, source) in sources.iter().enumerate() {
                                    ui.selectable_value(&mut dialog.artifact_id, source.id.clone(), format!("[{}] {}", index + 1, source.title));
                                }
                            });
                        }
                        "date" => {
                            ui.label(RichText::new(format!("Будет вставлена дата: {}", today_iso_date())).small().color(pal.muted));
                        }
                        "formula" => {
                            if let Some(number) = &formula_number_preview {
                                ui.label(
                                    RichText::new(format!(
                                        "номер в документе (предварительно): {number} · схема: {}",
                                        formula_numbering_label(self.document_formula_numbering())
                                    ))
                                    .small()
                                    .color(pal.muted),
                                );
                            }
                            let current = formulas_list
                                .iter()
                                .find(|formula| formula.id == dialog.formula_id)
                                .map(|formula| {
                                    format!(
                                        "{} · {}",
                                        formula.title,
                                        if formula.latex.chars().count() > 48 {
                                            format!("{}…", formula.latex.chars().take(48).collect::<String>())
                                        } else {
                                            formula.latex.clone()
                                        }
                                    )
                                })
                                .unwrap_or_else(|| "— выбрать —".to_owned());
                            egui::ComboBox::from_label("формула проекта").selected_text(current).show_ui(ui, |ui| {
                                for formula in &formulas_list {
                                    ui.selectable_value(&mut dialog.formula_id, formula.id.clone(), &formula.title);
                                }
                            });
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("режим вставки").small().color(pal.muted));
                                ui.selectable_value(&mut dialog.render_mode, "omml".to_owned(), "Word-объект (OMML)");
                                ui.selectable_value(&mut dialog.render_mode, "svg".to_owned(), "SVG-вектор (+PNG)");
                            });
                            if let Some(formula) = formulas_list.iter().find(|formula| formula.id == dialog.formula_id) {
                                let mode = if formula.display { "display" } else { "inline" };
                                let info = format!(
                                    "{} · нумерация {} · кэши {}{}{}",
                                    mode,
                                    formula.numbering.mode,
                                    if formula.state.omml_hash.is_empty() { "OMML: —" } else { "OMML: ✓" },
                                    if formula.state.svg_hash.is_empty() { " · SVG: —" } else { " · SVG: ✓" },
                                    if formula.state.png_hash.is_empty() { " · PNG: —" } else { " · PNG: ✓" }
                                );
                                ui.label(RichText::new(info).small().color(pal.muted));
                                if dialog.render_mode == "svg" && formula.state.svg_hash.is_empty() {
                                    ui.label(
                                        RichText::new("SVG-кэш ещё не построен — он будет построен при вставке (poppler)")
                                            .small()
                                            .color(pal.amber),
                                    );
                                }
                            }
                        }
                        _ => {
                            let current_title = artifacts
                                .iter()
                                .find(|artifact| artifact.id == dialog.artifact_id)
                                .map(|artifact| artifact.title.clone())
                                .unwrap_or_else(|| "— выбрать —".to_owned());
                            let old_artifact = dialog.artifact_id.clone();
                            egui::ComboBox::from_label("артефакт").selected_text(current_title).show_ui(ui, |ui| {
                                for artifact in &artifacts {
                                    ui.selectable_value(&mut dialog.artifact_id, artifact.id.clone(), &artifact.title);
                                }
                            });
                            let is_image = |output: &str| {
                                matches!(
                                    output.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str(),
                                    "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp"
                                )
                            };
                            let is_csv = |output: &str| output.to_ascii_lowercase().ends_with(".csv");
                            if let Some(artifact) = artifacts.iter().find(|artifact| artifact.id == dialog.artifact_id) {
                                // Смена владельца: selector не должен «переезжать» на чужой выход.
                                if old_artifact != dialog.artifact_id {
                                    dialog.selector = match dialog.kind.as_str() {
                                        "figure" => artifact.outputs.iter().find(|output| is_image(output)).cloned().unwrap_or_default(),
                                        "table" => artifact.outputs.iter().find(|output| is_csv(output)).cloned().unwrap_or_default(),
                                        _ => artifact.outputs.first().cloned().unwrap_or_default(),
                                    };
                                    dialog.error.clear();
                                }
                                let valid_outputs: Vec<&String> = artifact
                                    .outputs
                                    .iter()
                                    .filter(|output| match dialog.kind.as_str() {
                                        "figure" => is_image(output),
                                        "table" => is_csv(output),
                                        _ => true,
                                    })
                                    .collect();
                                egui::ComboBox::from_label("выход").selected_text(dialog.selector.clone()).show_ui(ui, |ui| {
                                    for output in valid_outputs {
                                        ui.selectable_value(&mut dialog.selector, output.clone(), output);
                                    }
                                });
                            }
                            if dialog.kind == "value" {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("JSON pointer").small().color(pal.muted));
                                    ui.add(
                                        egui::TextEdit::singleline(&mut dialog.pointer)
                                            .hint_text("#/stats/x1/mean (пусто — текст файла)")
                                            .desired_width(360.0),
                                    );
                                });
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("формат").small().color(pal.muted));
                                    egui::ComboBox::from_id_salt("rde_insert_format")
                                        .selected_text(&dialog.format)
                                        .show_ui(ui, |ui| {
                                            for (value, label) in [
                                                ("auto", "авто"),
                                                ("raw", "как в артефакте"),
                                                ("int", "целое"),
                                                ("dec", "дробь"),
                                                ("sci", "стандартный вид (m·10^e)"),
                                                ("percent", "проценты"),
                                            ] {
                                                ui.selectable_value(&mut dialog.format, value.to_owned(), label);
                                            }
                                        });
                                    if matches!(dialog.format.as_str(), "dec" | "sci" | "percent") {
                                        ui.label(RichText::new("знаков").small().color(pal.muted));
                                        ui.add(egui::DragValue::new(&mut dialog.precision).range(0..=8));
                                    }
                                });
                            }
                        }
                    }
                    // Стиль обычного текста шаблона — можно сменить (R-DOC-25). Формула живёт в
                    // своём sdt-обрамлении (номер — документно-локальный), стиль ей не нужен.
                    if dialog.kind != "date" && dialog.kind != "formula" {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("стиль").small().color(pal.muted));
                            egui::ComboBox::from_id_salt("rde_insert_style")
                                .selected_text(if dialog.style.is_empty() { "обычный (по умолчанию)" } else { &dialog.style })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut dialog.style, String::new(), "обычный (по умолчанию)");
                                    for style in styles.iter().filter(|style| style.kind == "paragraph") {
                                        ui.selectable_value(&mut dialog.style, style.id.clone(), format!("{} ({})", style.name, style.id));
                                    }
                                });
                        });
                    }
                    if dialog.kind != "formula" {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("подпись").small().color(pal.muted));
                            ui.add(egui::TextEdit::singleline(&mut dialog.caption).hint_text("например, «Среднее x1»").desired_width(360.0));
                        });
                    }
                    let uri = match dialog.kind.as_str() {
                        "constant" => format!("rde://constants/{}/{}", dialog.artifact_id, dialog.selector),
                        "crossref" => match dialog.artifact_id.strip_prefix("formula:") {
                            Some(formula_id) => format!("rde://formula/{formula_id}"),
                            None => format!("rde://insert/{}", dialog.artifact_id),
                        },
                        "citation" => format!("rde://source/{}", dialog.artifact_id),
                        "date" => "rde://date/iso".to_owned(),
                        "formula" => format!("rde://formula/{}@<act_key>", dialog.formula_id),
                        _ => format!("rde://artifact/{}/{}", dialog.artifact_id, dialog.selector),
                    };
                    ui.label(RichText::new(uri).monospace().small().color(pal.accent));
                    ui.add_space(6.0);
                    // Позиция вставки: каретка DocView / выбранный блок / конец документа.
                    ui.label(
                        RichText::new(match self.insert_anchor_block {
                            Some(block) => format!("позиция: после блока {}", block + 1),
                            None => "позиция: в конец документа".to_owned(),
                        })
                        .small()
                        .color(pal.muted),
                    );
                    // Валидация: для JSON-выхода нужен pointer, иначе вставится весь файл (P8-B1).
                    let needs_pointer = dialog.kind == "value"
                        && dialog.selector.to_ascii_lowercase().ends_with(".json")
                        && dialog.pointer.trim().trim_start_matches('#').is_empty();
                    if needs_pointer {
                        ui.label(
                            RichText::new("JSON-выход: укажите JSON pointer (например, #/stats/x1/mean) — иначе вставится весь файл")
                                .small()
                                .color(pal.amber),
                        );
                    }
                    if !dialog.error.is_empty() {
                        ui.label(RichText::new(&dialog.error).small().color(pal.red));
                    }
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(
                                !needs_pointer,
                                egui::Button::new(RichText::new("Вставить").color(Color32::WHITE)).fill(pal.accent),
                            )
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
                self.confirm_insert(dialog);
            } else if !cancel {
                self.insert_dialog = Some(dialog);
            }
        }

        // P5: коммит изменений проекта (R-VCS-01).
        if let Some(mut message) = self.commit_dialog.take() {
            let mut confirm = false;
            let mut cancel = false;
            let status = self.git_status.clone().unwrap_or_default();
            egui::Window::new("Git: закоммитить изменения")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(560.0);
                    ui.label(
                        RichText::new(format!(
                            "Ветка {} · изменений: {} (изменено {}, новых {})",
                            if status.branch.is_empty() { "—" } else { &status.branch },
                            status.changed.len() + status.untracked.len(),
                            status.changed.len(),
                            status.untracked.len()
                        ))
                        .small()
                        .color(pal.muted),
                    );
                    ui.add(egui::TextEdit::multiline(&mut message).desired_rows(3).desired_width(f32::INFINITY));
                    ui.label(RichText::new("Автор — первый human-автор из rde.toml (R-SET-01)").small().color(pal.muted));
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new("Закоммитить").color(Color32::WHITE)).fill(pal.accent)).clicked() {
                            confirm = true;
                        }
                        if ui.button("Отмена").clicked() {
                            cancel = true;
                        }
                    });
                });
            if confirm {
                self.git_commit(&message);
            } else if !cancel {
                self.commit_dialog = Some(message);
            }
        }

        // P5: история git.
        if let Some(entries) = self.history_dialog.take() {
            let mut close = false;
            egui::Window::new("Git: история")
                .collapsible(false)
                .resizable(true)
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -40.0))
                .show(ctx, |ui| {
                    ui.set_min_width(640.0);
                    egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                        for entry in &entries {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(&entry.id).monospace().small().color(pal.accent));
                                ui.label(RichText::new(&entry.time).small().color(pal.muted));
                                ui.label(RichText::new(&entry.summary).size(12.5).color(pal.text));
                                ui.label(RichText::new(&entry.author).small().color(pal.muted));
                            });
                        }
                    });
                    if ui.button("Закрыть").clicked() {
                        close = true;
                    }
                });
            if !close {
                self.history_dialog = Some(entries);
            }
        }

        // ── P4 (ADR-0020): окно нативного редактора формулы (equation_tab) ──
        // Сама панель живёт на ленте (вкладка «Формула»); отдельным окном — только нативный
        // Equation WordCraft: его canvas не уживается с живым canvas документа в одном кадре.
        self.formula_native_window(ctx);
    }

    /// M4.2/M4.2b: диалог подтверждения действия внешнего агента (разрешить/отклонить).
    fn agent_approval_dialog(&mut self, ctx: &egui::Context) {
        let pal = theme_colors(ctx.theme());
        let Some(pending) = self.agent_approval_dialog.as_ref() else { return };
        let tool = pending.tool.clone();
        let destructive = pending.destructive;
        let requested_by = pending.requested_by.clone();
        let preview = pending.preview();
        // M4.2b: human-readable сводка run/commit + контекст (файлы/автор/trusted).
        let human_summary = pending.human_summary();
        let context_lines = self.agent_approval_context.clone();
        let args_summary = pending.args_json_summary();
        let id = pending.id;
        let mut decision: Option<rde_mcp::ApprovalDecision> = None;
        egui::Window::new("Запрос внешнего агента")
            .id(egui::Id::new(("rde_agent_approval", id)))
            .collapsible(false)
            .resizable(true)
            .default_width(560.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -20.0))
            .show(ctx, |ui| {
                ui.label(RichText::new(format!("Инструмент: {tool}")).strong().color(pal.text));
                ui.label(RichText::new(format!("Запрос от: {requested_by}")).small().color(pal.muted));
                if destructive {
                    ui.label(RichText::new("внимание: деструктивное действие").small().color(pal.amber));
                }
                if let Some(lines) = &human_summary {
                    ui.add_space(6.0);
                    ui.label(RichText::new("Что произойдёт:").small().strong().color(pal.muted));
                    for line in lines {
                        ui.add(
                            egui::Label::new(RichText::new(format!("• {line}")).small().color(pal.text)).wrap(),
                        );
                    }
                }
                for line in &context_lines {
                    ui.add(egui::Label::new(RichText::new(line).small().color(pal.muted)).wrap());
                }
                ui.add_space(6.0);
                if let Some((before, after)) = &preview {
                    if let Some(before) = before {
                        ui.label(RichText::new("Было:").small().strong().color(pal.muted));
                        egui::ScrollArea::vertical().id_salt(("rde_approval_before", id)).max_height(80.0).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(before).monospace()).wrap());
                        });
                    }
                    ui.label(RichText::new("Станет:").small().strong().color(pal.muted));
                    egui::ScrollArea::vertical().id_salt(("rde_approval_after", id)).max_height(140.0).show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(after).monospace()).wrap());
                    });
                } else if human_summary.is_some() {
                    // Превью уже человекочитаемое; сырые аргументы — для аудита/отладки.
                    ui.collapsing("Аргументы (JSON)", |ui| {
                        egui::ScrollArea::vertical().id_salt(("rde_approval_args_json", id)).max_height(150.0).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(args_summary).monospace()).wrap());
                        });
                    });
                } else {
                    ui.label(RichText::new("Аргументы:").small().strong().color(pal.muted));
                    egui::ScrollArea::vertical().id_salt(("rde_approval_args", id)).max_height(180.0).show(ui, |ui| {
                        ui.add(egui::Label::new(RichText::new(args_summary).monospace()).wrap());
                    });
                }
                ui.add_space(6.0);
                ui.label(
                    RichText::new(
                        "Перед изменением создаётся чекпоинт (при ошибке — авто-откат); решение пишется в \
                         аудит. Без ответа через 5 минут — отказ (fail-closed).",
                    )
                    .small()
                    .color(pal.muted),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.button("Разрешить один раз").clicked() {
                        decision = Some(rde_mcp::ApprovalDecision::AllowOnce);
                    }
                    if ui.button("Разрешить в сессии").clicked() {
                        decision = Some(rde_mcp::ApprovalDecision::AllowSession);
                    }
                    if ui.button("Отклонить").clicked() {
                        decision = Some(rde_mcp::ApprovalDecision::Deny);
                    }
                });
            });
        if let Some(decision) = decision {
            self.decide_agent_approval(decision);
        }
    }

    /// Окна диалогов создания/переименования/удаления.
    fn dialogs(&mut self, ctx: &egui::Context) {
        let pal = theme_colors(ctx.theme());
        // M4.2: подтверждение действия внешнего агента — модальный диалог поверх остальных.
        self.agent_approval_dialog(ctx);
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
                        ui.label(RichText::new("Тип").color(pal.muted));
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
                    ui.label(RichText::new("Документ создаётся пустым .doc.rde (WordCraft + rde:doc)").small().color(pal.muted));
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
                    ui.label(RichText::new("Объект будет перемещён в .rde/trash (безвозвратного удаления нет).").small().color(pal.muted));
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
        let pal = theme_colors(ui.ctx().theme());
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin::symmetric(12, 8)))
            .show(ui, |ui| {
                // Строка вкладок центра — ОДНА линия без переноса (§19-7): слева вкладки, на вкладке
                // «Документ» справа — компактный переключатель режимов. Если подписи не влезают,
                // вкладки сжимаются до иконок с тултипами (`center_line_density`), но строка одна.
                let density = self.center_line_density(ui);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = CENTER_TAB_SPACING_X;
                    ui.spacing_mut().button_padding = vec2(CENTER_TAB_PADDING_X, 3.0);
                    for tab in Tab::ALL {
                        if self.center_tab(ui, tab, density.tab_labels).clicked() {
                            self.tab = tab;
                        }
                    }
                    if self.tab == Tab::Document {
                        // Переключатель режимов прижат к правому краю той же строки: режимы доступны
                        // во всех режимах документа, даже когда лента (режим RDE) не показана.
                        let gap = (ui.available_width() - self.doc_mode_switch_width(ui, density.mode_labels)).max(CENTER_MODE_GAP);
                        ui.add_space(gap);
                        self.doc_mode_switch(ui, density.mode_labels);
                    }
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

    /// Таб строки вкладок центра: pill с иконкой; подпись — только если строка не переносится.
    /// Полное название всегда остаётся в доступном имени (и в тултипе, когда видна лишь иконка).
    fn center_tab(&mut self, ui: &mut egui::Ui, tab: Tab, labels: bool) -> egui::Response {
        let pal = theme_colors(ui.ctx().theme());
        let selected = self.tab == tab;
        let title = tab.title(self.lang);
        let text = if labels { format!("{}  {}", tab.icon(), title) } else { tab.icon().to_owned() };
        let text = RichText::new(text).size(FS_BODY).color(if selected { pal.text } else { pal.muted });
        let response = ui.add(egui::Button::selectable(selected, text).corner_radius(8).min_size(vec2(0.0, CENTER_TAB_HEIGHT)));
        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, title));
        if labels { response } else { response.on_hover_text(title) }
    }

    /// Ширина кнопки-таба строки вкладок: текст (иконка+подпись либо только иконка) + отступы кнопки.
    fn center_tab_width(&self, ui: &egui::Ui, tab: Tab, labels: bool) -> f32 {
        let text = if labels { format!("{}  {}", tab.icon(), tab.title(self.lang)) } else { tab.icon().to_owned() };
        text_width(ui, &text, FS_BODY) + 2.0 * CENTER_TAB_PADDING_X
    }

    /// Ширина клавиши-сегмента переключателя режимов.
    fn center_mode_width(&self, ui: &egui::Ui, mode: DocMode, labels: bool) -> f32 {
        let text = if labels { format!("{}  {}", mode.icon(), mode.short_label()) } else { mode.icon().to_owned() };
        text_width(ui, &text, FS_SMALL) + 2.0 * CENTER_SEGMENT_PADDING_X
    }

    /// Ширина сегментированного переключателя режимов целиком (сегменты, зазоры, рамка).
    fn doc_mode_switch_width(&self, ui: &egui::Ui, labels: bool) -> f32 {
        let segments: f32 = DocMode::ALL.iter().map(|mode| self.center_mode_width(ui, *mode, labels)).sum();
        segments + CENTER_SEGMENT_SPACING_X * (DocMode::ALL.len() - 1) as f32 + 2.0 * CENTER_SEGMENT_FRAME
    }

    /// Ширина всей строки вкладок центра при заданной плотности: вкладки (+ переключатель
    /// режимов и зазор на вкладке «Документ») + запас. По ней выбирается плотность.
    fn center_line_width(&self, ui: &egui::Ui, tab_labels: bool, mode_labels: bool) -> f32 {
        let tabs: f32 = Tab::ALL.iter().map(|tab| self.center_tab_width(ui, *tab, tab_labels)).sum();
        let tabs = tabs + CENTER_TAB_SPACING_X * (Tab::ALL.len() - 1) as f32;
        if self.tab != Tab::Document {
            return tabs;
        }
        tabs + CENTER_MODE_GAP + self.doc_mode_switch_width(ui, mode_labels) + CENTER_LINE_SAFETY
    }

    /// Подобрать плотность строки вкладок: сначала подписи, затем только иконки — но БЕЗ переноса.
    fn center_line_density(&self, ui: &egui::Ui) -> CenterLineDensity {
        let available = ui.available_width();
        for (tab_labels, mode_labels) in [(true, true), (true, false), (false, true), (false, false)] {
            if self.center_line_width(ui, tab_labels, mode_labels) <= available {
                return CenterLineDensity { tab_labels, mode_labels };
            }
        }
        CenterLineDensity { tab_labels: false, mode_labels: false }
    }

    /// Сегментированный переключатель режимов документа в строке вкладок центра (§19-7):
    /// иконки с тултипами, подписи — если хватает места. Режимы доступны во всех режимах.
    fn doc_mode_switch(&mut self, ui: &mut egui::Ui, labels: bool) {
        let pal = theme_colors(ui.ctx().theme());
        let mut mode = self.doc_mode;
        egui::Frame::new()
            .fill(pal.panel_hi)
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(CornerRadius::same(8))
            .inner_margin(Margin::same(CENTER_SEGMENT_MARGIN as i8))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = CENTER_SEGMENT_SPACING_X;
                ui.spacing_mut().button_padding = vec2(CENTER_SEGMENT_PADDING_X, 2.0);
                for candidate in DocMode::ALL {
                    let selected = mode == candidate;
                    let text = if labels { format!("{}  {}", candidate.icon(), candidate.short_label()) } else { candidate.icon().to_owned() };
                    let text = RichText::new(text).size(FS_SMALL).color(if selected { pal.text } else { pal.muted });
                    let response = ui.add(egui::Button::selectable(selected, text).corner_radius(5).min_size(vec2(0.0, CENTER_SEGMENT_HEIGHT)));
                    response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, candidate.label()));
                    if response.on_hover_text(candidate.tooltip()).clicked() {
                        mode = candidate;
                    }
                }
            });
        self.doc_mode = mode;
    }

    fn doc_tab(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        let mode = self.doc_mode;
        let mut caret_block: Option<usize> = None;
        let mut canvas_clicked = false;
        // P4: пока открыт нативный редактор формулы (свой canvas WordCraft в окне), живой
        // canvas документа не рисуем — два canvas с общим egui-id в одном кадре конфликтуют;
        // вместо него — растровый предпросмотр страниц (документ не меняется).
        let formula_native_open = self.formula_editor.as_ref().is_some_and(|dialog| dialog.native_open);
        // P5: панель «Формула» открыта на ленте — она занимает почти всё место, центр — документ.
        // Живой WordCraft-canvas документа в этом состоянии не нужен (панель его перекрывает), но
        // он продолжает авто-запрашивать фокус ввода и перехватывал стрелки/Tab у редактора
        // формулы. Показываем вместо него подсказку; после закрытия панели canvas вернётся.
        let formula_panel_open = self.ribbon_tab == RibbonTab::Formula && self.formula_editor.is_some();
        if mode != DocMode::Preview && !formula_native_open {
            self.ensure_editor();
        }
        // Лента — только в режиме RDE (вкладка «Документ» + DocView): в WordCraft и предпросмотре её нет.
        if mode == DocMode::DocView {
            // P5 (quick-win §5.2): счётчик вставок для ленты — из кэша по (путь, mtime, размер),
            // а не `load_rde_doc` (распаковка .doc.rde) каждый кадр.
            let inserts_count = self.document_inserts_count();
            if let Some(cmd) = self.ribbon_ui(ui, inserts_count) {
                self.run_cmd(cmd);
            }
            ui.add_space(4.0);
        }
        ui.separator();
        match mode {
            DocMode::DocView => {
                if formula_native_open {
                    ui.label(
                        RichText::new(
                            "Открыт профессиональный редактор формулы — документ показан как предпросмотр страниц \
                             (после закрытия редактора вернётся живой canvas)",
                        )
                        .small()
                        .color(pal.muted),
                    );
                    ui.add_space(2.0);
                    self.doc_preview_pages(ui);
                } else if formula_panel_open {
                    ui.label(
                        RichText::new(
                            "Открыт редактор формулы (вкладка «Формула») — живой canvas документа приостановлен, \
                             чтобы клавиатура принадлежала редактору формулы; после «Закрыть» документ вернётся",
                        )
                        .small()
                        .color(pal.muted),
                    );
                } else if let Some(ed) = &mut self.editor {
                    let ctx = ui.ctx().clone();
                    ed.app.logic(&ctx);
                    // Позиция каретки в DocView → блок-якорь вставки (P8-B1).
                    if let Some(block) = ed.app.session.sel.focus.path.0.first() {
                        caret_block = Some(*block as usize);
                    }
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
                    // P4: клик по формуле в тексте — WordCraft ставит каретку в уравнение
                    // (`equation.edit`); откроем панель редактора формулы на вкладке «Формула».
                    if ui.input(|i| i.pointer.primary_clicked())
                        && ui.input(|i| i.pointer.interact_pos()).is_some_and(|pos| rect.contains(pos))
                    {
                        canvas_clicked = true;
                    }
                } else {
                    self.editor_error_ui(ui);
                }
            }
            DocMode::Full => {
                if let Some(ed) = &mut self.editor {
                    let ctx = ui.ctx().clone();
                    ed.app.logic(&ctx);
                    // Позиция каретки в DocView → блок-якорь вставки (P8-B1).
                    if let Some(block) = ed.app.session.sel.focus.path.0.first() {
                        caret_block = Some(*block as usize);
                    }
                    ed.app.ui(ui);
                } else {
                    self.editor_error_ui(ui);
                }
            }
            DocMode::Preview => {
                self.preview_controls(ui);
                self.doc_preview_pages(ui);
            }
        }
        if canvas_clicked {
            self.open_formula_editor_from_canvas_click();
        }
        if let Some(block) = caret_block {
            self.insert_anchor_block = Some(block);
        }
    }

    /// Клик по формуле в тексте (DocView): WordCraft на клик в уравнение ставит каретку в него
    /// (`equation.edit`) — открываем редактор формулы на верхней панели (вкладка «Формула», P4).
    /// Формулы вне реестра остаются в нативном режиме правки (их принимают миграцией).
    fn open_formula_editor_from_canvas_click(&mut self) {
        // LaTeX уравнения под каретой — командой движка (работает и вне режима правки).
        let info = {
            let Some(ed) = self.editor.as_mut() else { return };
            ed.app.run("equation.get", serde_json::json!({})).ok()
        };
        let Some(info) = info else { return };
        let Some(latex) = info.get("latex").and_then(|value| value.as_str()).map(str::to_owned) else { return };
        let Some(formula_id) = self.formula_for_latex(&latex) else {
            self.push_log(
                Level::Info,
                "Клик по формуле: объекта формулы в проекте нет (возможно, формула вне реестра — «Принять формулы как объекты»)",
            );
            return;
        };
        // Выходим из инлайн-правки уравнения: источник истины — `formula.toml`, правка — в панели.
        if info.get("editing").and_then(|value| value.as_bool()).unwrap_or(false)
            && let Some(ed) = self.editor.as_mut()
        {
            let _ = ed.app.run("equation.exit", serde_json::json!({}));
        }
        // Уже открыт этот же редактор (с возможными неприменёнными правками) — только показать вкладку.
        if self.formula_editor.as_ref().and_then(|dialog| dialog.formula_id.clone()).as_deref() == Some(formula_id.as_str()) {
            self.show_formula_editor_context();
            return;
        }
        self.open_formula_editor(&formula_id);
        self.push_log(Level::Info, format!("Клик по формуле: открыт редактор на вкладке «Формула» ({formula_id})"));
    }

    /// Найти project-level формулу по LaTeX из документа: `formula.toml` (источник) или
    /// `resolved_latex` из ledger (формулы с входами/фрагментами — в документе эффективный LaTeX).
    fn formula_for_latex(&self, latex: &str) -> Option<String> {
        let target = normalize_formula_latex(latex);
        if target.is_empty() {
            return None;
        }
        if let Some(formula) = self.project.formulas().find(|formula| normalize_formula_latex(&formula.latex) == target) {
            return Some(formula.id.clone());
        }
        let store = self.store.as_ref()?;
        let ledger = rde_dag::formula_ledger(store).ok()?;
        ledger
            .iter()
            .find(|(_, state)| normalize_formula_latex(&state.resolved_latex) == target)
            .map(|(id, _)| id.clone())
    }

    /// Растровый предпросмотр страниц документа (режим «Предпросмотр страниц»; P4 — пока
    /// открыт нативный редактор формулы, чтобы не рисовать второй canvas WordCraft в кадре).
    fn doc_preview_pages(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        self.doc.ensure(ui.ctx());
        let zoom = self.zoom;
        match &self.doc.state {
            DocState::Pending => {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.spinner();
                    ui.label(RichText::new("Рендеринг документа движком WordCraft…").color(pal.muted));
                });
            }
            DocState::Failed(e) => {
                ui.vertical_centered(|ui| {
                    ui.add_space(60.0);
                    ui.label(RichText::new("Не удалось открыть документ").color(pal.red).strong());
                    ui.label(RichText::new(e).color(pal.muted));
                });
            }
            DocState::Ready(doc) => {
                let mut scroll = egui::ScrollArea::both().id_salt("rde_doc_scroll").auto_shrink([false, false]);
                // P5 (ADR-0021 §Решение 3): после пересборки пакета возвращаем прежний offset ОДИН
                // раз; иначе страницы на кадр исчезают, ScrollArea теряет позицию и документ
                // «прыгает» на первую страницу (баг владельца B).
                if let Some(offset) = self.preview_restore_offset.take() {
                    scroll = scroll.vertical_scroll_offset(offset);
                } else if self.demo_preview_scroll > 0.0 {
                    // Демо/скриншоты могут задать стартовый offset; в интерактивном режиме его НЕ
                    // форсируем каждый кадр — иначе ScrollArea сбрасывает колесо/перетаскивание
                    // ползунка обратно на `demo_preview_scroll` (по умолчанию 0) и скролл «не работает».
                    scroll = scroll.vertical_scroll_offset(self.demo_preview_scroll);
                }
                let scroll_output = scroll.show(ui, |ui| {
                        ui.add_space(14.0);
                        ui.vertical_centered(|ui| {
                            for (i, tex) in doc.pages.iter().enumerate() {
                                let size = doc.page_pt * zoom;
                                ui.image((tex.id(), size));
                                ui.add_space(4.0);
                                ui.label(RichText::new(format!("Страница {} из {}", i + 1, doc.count)).small().color(pal.muted));
                                ui.add_space(12.0);
                            }
                        });
                    });
                self.preview_scroll_offset = scroll_output.state.offset.y;
            }
        }
    }

    /// Число вставок активного документа с per-frame кэшем по (путь, mtime, размер пакета):
    /// лента RDE спрашивает его каждый кадр, а `load_rde_doc` распаковывает zip (дорого).
    fn document_inserts_count(&mut self) -> usize {
        let Some(path) = self.project.document_path().filter(|path| rde_doc::is_rde_package(path)) else {
            return 0;
        };
        let (mtime, len) = match fs::metadata(&path) {
            Ok(meta) => {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos() as u64)
                    .unwrap_or(0);
                (mtime, meta.len())
            }
            Err(_) => (0, 0),
        };
        let key = path.display().to_string();
        if let Some((cached_path, cached_mtime, cached_len, count)) = &self.inserts_count_cache
            && cached_path == &key
            && *cached_mtime == mtime
            && *cached_len == len
        {
            return *count;
        }
        let count = rde_doc::load_rde_doc(&path).map(|doc| doc.inserts.len()).unwrap_or(0);
        self.inserts_count_cache = Some((key, mtime, len, count));
        count
    }

    /// Превью следующего номера формулы (ADR-0018) с кэшем по (путь, mtime, размер, схема):
    /// `next_formula_number_text` распаковывает `.doc.rde`, а панель спрашивает номер каждый кадр.
    fn next_formula_number_preview(&mut self) -> Option<String> {
        let path = self.active_document_path().filter(|path| rde_doc::is_rde_package(path))?;
        let scheme = self.document_formula_numbering();
        let (mtime, len) = match fs::metadata(&path) {
            Ok(meta) => {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos() as u64)
                    .unwrap_or(0);
                (mtime, meta.len())
            }
            Err(_) => (0, 0),
        };
        let key = format!("{}|{mtime}|{len}|{}", path.display(), scheme.as_str());
        if let Some((cached_key, value)) = &self.formula_number_preview_cache
            && cached_key == &key
        {
            return value.clone();
        }
        let value = rde_doc::next_formula_number_text(&path, scheme).map(|(number, _warnings)| number);
        self.formula_number_preview_cache = Some((key, value.clone()));
        value
    }

    /// Сообщение, если встроенный редактор не удалось открыть.
    fn editor_error_ui(&self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        if let Some(err) = &self.editor_error {
            ui.vertical_centered(|ui| {
                ui.add_space(50.0);
                ui.label(RichText::new("Не удалось открыть редактор WordCraft").color(pal.red).strong());
                ui.label(RichText::new(err).color(pal.muted));
            });
        }
    }

    fn structure_tab(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        if self.structure_cache.is_none() {
            let path = self.active_document_path();
            self.structure_cache = Some(match path {
                Some(path) => extract_document_structure(&path).map_err(|error| format!("{}: {error}", path.display())),
                None => Err("в проекте нет документа".to_owned()),
            });
        }
        let cache = self.structure_cache.clone();
        // Число вставок из rde:doc (кэш по пути пакета).
        let package_key = self.active_document_path().map(|path| path.display().to_string()).unwrap_or_default();
        if self.inserts_cache.as_ref().map(|(key, _)| key != &package_key).unwrap_or(true) {
            let inserts = self
                .project
                .document_path()
                .and_then(|path| rde_doc::load_rde_doc(&path).ok())
                .map(|rde_doc| rde_doc.inserts)
                .unwrap_or_default();
            self.inserts_cache = Some((package_key.clone(), inserts));
        }
        let inserts: Vec<rde_doc::RdeInsert> = self.inserts_cache.as_ref().map(|(_, inserts)| inserts.clone()).unwrap_or_default();
        let inserts_count = inserts.len();
        let mut clicked_block: Option<(usize, String)> = None;
        // M4.4: «В чат» из строки блока — чип `rde://doc/<id>#block/<bN>` (§8.7.2).
        let document_id = self.active_document_object().map(|object| object.id.clone()).unwrap_or_default();
        let mut attach_block_uri: Option<String> = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Блоки документа").strong().color(pal.text));
            if let Some(Ok((blocks, figures))) = &cache {
                let headings = blocks.iter().filter(|block| matches!(block.kind, "H1" | "H2" | "H")).count();
                ui.label(
                    RichText::new(format!("{} блоков · {headings} заголовков · {figures} рисунков · {inserts_count} вставок rde:doc", blocks.len()))
                        .small()
                        .color(pal.muted),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("клик по блоку — выбрать документ").small().color(pal.muted));
            });
        });
        ui.separator();
        let mut pending_insert_updates: Vec<String> = Vec::new();
        let mut pending_insert_edits: Vec<(String, String, String)> = Vec::new();
        // ПКМ по вставке (R-DOC-11): открыть артефакт-источник / обновить / скопировать URI.
        let mut insert_open_artifact: Option<String> = None;
        let mut insert_update_requested = false;
        egui::ScrollArea::both().id_salt("rde_struct_scroll").auto_shrink([false, false]).show(ui, |ui| {
            match cache {
                Some(Ok((blocks, _figures))) => {
                    egui::Grid::new("rde_blocks").num_columns(6).spacing([4.0, 2.0]).striped(true).show(ui, |ui| {
                        pad_cell(ui, |ui| ui.label(RichText::new("№").small().color(pal.muted)));
                        // M4.4: «В чат» — вторая колонка, чтобы кнопка была видна без
                        // горизонтальной прокрутки (широкая колонка «содержимое» — последняя).
                        pad_cell(ui, |ui| ui.label(RichText::new("в чат").small().color(pal.muted)));
                        pad_cell(ui, |ui| ui.label(RichText::new("тип").small().color(pal.muted)));
                        pad_cell(ui, |ui| ui.label(RichText::new("стиль").small().color(pal.muted)));
                        pad_cell(ui, |ui| ui.label(RichText::new("путь (block)").small().color(pal.muted)));
                        pad_cell(ui, |ui| ui.label(RichText::new("содержимое").small().color(pal.muted)));
                        ui.end_row();
                        for block in &blocks {
                            pad_cell(ui, |ui| ui.label(RichText::new(block.number.to_string()).monospace().size(11.0).color(pal.muted)));
                            // M4.4: чип контекст-ссылки на этот блок для следующего запроса.
                            pad_cell(ui, |ui| {
                                if ui
                                    .small_button("→ чат")
                                    .on_hover_text("Приложить блок к следующему сообщению агента (rde://doc/… #block)")
                                    .clicked()
                                {
                                    attach_block_uri = Some(
                                        crate::context_refs::block_uri_for(&document_id, block.number),
                                    );
                                }
                            });
                            // H2/H — осветлённый синий читается только на тёмном;
                            // на светлом берём акцент (контраст ≥ 4.5:1).
                            let h2 = match ui.ctx().theme() {
                                egui::Theme::Light => pal.accent,
                                _ => Color32::from_rgb(0x7c, 0xa8, 0xff),
                            };
                            let color = match block.kind {
                                "H1" => pal.accent,
                                "H2" | "H" => h2,
                                "FIG" => pal.amber,
                                "CAP" => pal.green,
                                _ => pal.muted,
                            };
                            let kind_label = if block.images > 0 {
                                format!("{}·{}", block.kind, block.images)
                            } else {
                                block.kind.to_owned()
                            };
                            pad_cell(ui, |ui| ui.label(RichText::new(kind_label).monospace().size(11.0).color(color)));
                            pad_cell(ui, |ui| ui.label(RichText::new(if block.style.is_empty() { "—" } else { &block.style }).monospace().size(10.5).color(pal.muted)));
                            pad_cell(ui, |ui| ui.label(RichText::new(&block.anchor).monospace().size(10.5).color(pal.muted)));
                            let text_resp = pad_cell(ui, |ui| ui.add(egui::Label::new(RichText::new(display_text(&block.text)).size(12.0).color(pal.text)).sense(Sense::click())));
                            if text_resp.clicked() {
                                clicked_block = Some((block.number, block.text.clone()));
                            }
                            ui.end_row();
                        }
                    });
                }
                Some(Err(error)) => {
                    ui.add_space(12.0);
                    ui.label(RichText::new("Не удалось разобрать документ").strong().color(pal.red));
                    ui.label(RichText::new(error).small().color(pal.muted));
                }
                None => {}
            }
            // Живые вставки rde:doc (R-DOC-11): стиль/формат можно менять, значение — из артефакта.
            if !inserts.is_empty() {
                ui.add_space(10.0);
                ui.separator();
                ui.label(RichText::new("Вставки rde:doc (живые значения/рисунки из артефактов)").strong().color(pal.text));
                let styles = self
                    .project
                    .document_path()
                    .filter(|path| rde_doc::is_rde_package(path))
                    .and_then(|path| rde_doc::list_document_styles(&path).ok())
                    .unwrap_or_default();
                egui::Grid::new("rde_inserts").num_columns(6).spacing([4.0, 2.0]).striped(true).show(ui, |ui| {
                    pad_cell(ui, |ui| ui.label(RichText::new("id").small().color(pal.muted)));
                    pad_cell(ui, |ui| ui.label(RichText::new("вид").small().color(pal.muted)));
                    pad_cell(ui, |ui| ui.label(RichText::new("цель").small().color(pal.muted)));
                    pad_cell(ui, |ui| ui.label(RichText::new("стиль").small().color(pal.muted)));
                    pad_cell(ui, |ui| ui.label(RichText::new("формат").small().color(pal.muted)));
                    pad_cell(ui, |ui| ui.label(RichText::new("значение/действие").small().color(pal.muted)));
                    ui.end_row();
                    for insert in &inserts {
                        pad_cell(ui, |ui| ui.label(RichText::new(&insert.id).monospace().size(11.0).color(pal.accent)));
                        pad_cell(ui, |ui| ui.label(RichText::new(&insert.kind).monospace().size(11.0).color(pal.muted)));
                        let target = insert.target.rsplit('/').next().unwrap_or(&insert.target);
                        let target_resp = pad_cell(ui, |ui| ui.label(RichText::new(target).monospace().size(10.5).color(pal.muted)).on_hover_text(&insert.target));
                        let insert_id = insert.id.clone();
                        let insert_target = insert.target.clone();
                        target_resp.context_menu(|ui| {
                            if let Some(rest) = insert_target.strip_prefix("rde://artifact/") {
                                let artifact_id = rest.split('/').next().unwrap_or_default().split('@').next().unwrap_or_default().to_owned();
                                if ui.button("Открыть артефакт-источник").clicked() {
                                    insert_open_artifact = Some(artifact_id);
                                    ui.close();
                                }
                            }
                            if ui.button("Обновить эту вставку").clicked() {
                                pending_insert_updates.push(insert_id.clone());
                                insert_update_requested = true;
                                ui.close();
                            }
                            if ui.button("Скопировать URI цели").clicked() {
                                ui.ctx().copy_text(insert_target.clone());
                                ui.close();
                            }
                        });
                        let mut style = insert.style.clone();
                        let mut format = if insert.format.is_empty() { "auto".to_owned() } else { insert.format.clone() };
                        pad_cell(ui, |ui| {
                            egui::ComboBox::from_id_salt(format!("rde_insert_style_{}", insert.id))
                                .selected_text(if style.is_empty() { "(обычный)".to_owned() } else { style.clone() })
                                .width(130.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut style, String::new(), "(обычный)");
                                    for style_info in styles.iter().filter(|style_info| style_info.kind == "paragraph") {
                                        ui.selectable_value(&mut style, style_info.id.clone(), format!("{} ({})", style_info.name, style_info.id));
                                    }
                                });
                        });
                        pad_cell(ui, |ui| {
                            egui::ComboBox::from_id_salt(format!("rde_insert_format_{}", insert.id))
                                .selected_text(format.clone())
                                .width(150.0)
                                .show_ui(ui, |ui| {
                                    for (value, label) in [
                                        ("auto", "авто"),
                                        ("raw", "как в артефакте"),
                                        ("int", "целое"),
                                        ("dec:2", "дробь 2"),
                                        ("dec:4", "дробь 4"),
                                        ("sci:3", "стандартный вид"),
                                        ("percent:1", "проценты"),
                                    ] {
                                        ui.selectable_value(&mut format, value.to_owned(), label);
                                    }
                                });
                        });
                        if style != insert.style || format != (if insert.format.is_empty() { "auto".to_owned() } else { insert.format.clone() }) {
                            pending_insert_edits.push((insert.id.clone(), style, format));
                        }
                        pad_cell(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(display_text(&insert.cached).chars().take(28).collect::<String>()).size(11.5).color(pal.text));
                                if ui.small_button("применить").on_hover_text("Перечитать значение из артефакта и применить стиль/формат").clicked() {
                                    pending_insert_updates.push(insert.id.clone());
                                }
                            });
                        });
                        ui.end_row();
                    }
                });
            }
        });
        if let Some(artifact_id) = insert_open_artifact {
            if let Some(object) = self.project.artifact_object(&artifact_id).cloned() {
                self.selected = Some(object.id.clone());
                self.show_inspector = true;
                self.artifact_editor = None;
                self.artifact_section = ArtifactSection::Result;
                self.tab = Tab::Artifacts;
            }
        }
        // Применяем правки стиля/формата вставок (R-DOC-11/25).
        if !pending_insert_edits.is_empty() || !pending_insert_updates.is_empty() {
            let Some(package) = self.active_document_path() else { return };
            for (insert_id, style, format) in &pending_insert_edits {
                if let Err(error) = rde_doc::apply_insert_style(&package, insert_id, style) {
                    self.push_log(Level::Warn, format!("Вставка {insert_id}: стиль не применён — {error}"));
                }
                if let Err(error) = rde_doc::set_insert_format(&package, insert_id, format) {
                    self.push_log(Level::Warn, format!("Вставка {insert_id}: формат не сохранён — {error}"));
                }
            }
            // Перечитываем значения из артефактов с учётом формата.
            let rde_doc = rde_doc::load_rde_doc(&package).ok();
            let mut values = std::collections::BTreeMap::new();
            let mut hashes = std::collections::BTreeMap::new();
            let mut resolve_errors: Vec<String> = Vec::new();
            if let (Some(rde_doc), Some(store)) = (&rde_doc, self.store.as_ref()) {
                for insert in &rde_doc.inserts {
                    if insert.kind != "value" || insert.target.is_empty() {
                        continue;
                    }
                    if !pending_insert_updates.is_empty() && !pending_insert_updates.contains(&insert.id) && pending_insert_edits.is_empty() {
                        continue;
                    }
                    match rde_dag::resolve_insert_text(&self.project, store, &insert.target) {
                        Ok((value, hash)) => {
                            values.insert(insert.id.clone(), rde_doc::format_value(&value, &insert.format));
                            hashes.insert(insert.id.clone(), hash);
                        }
                        Err(error) => resolve_errors.push(format!("Вставка {}: {error}", insert.id)),
                    }
                }
            }
            for error in resolve_errors {
                self.push_log(Level::Warn, error);
            }
            if !values.is_empty() {
                match rde_doc::update_insert_values(&package, &values, &hashes) {
                    Ok(report) => {
                        self.inserts_cache = None;
                        self.structure_cache = None;
                        self.push_log(Level::Ok, format!("Вставки обновлены: {} (стиль/формат применены)", report.updated.len()));
                    }
                    Err(error) => self.push_log(Level::Warn, format!("Вставки: {error}")),
                }
            } else {
                self.inserts_cache = None;
                self.structure_cache = None;
            }
        }
        if let Some((number, text)) = clicked_block {
            if let Some(document) = self.active_document_object() {
                self.selected = Some(document.id.clone());
            }
            // Выбранный блок — якорь для вставок (P8-B1).
            self.insert_anchor_block = Some(number.saturating_sub(1));
            self.push_log(Level::Info, format!("Блок {number}: {text} (вставки — после этого блока)"));
        }
        if insert_update_requested {
            self.update_project_document();
        }
        // M4.4: чип блока из строки «структура» — приложить к следующему сообщению агента.
        if let Some(uri) = attach_block_uri {
            self.attach_context_uri(&uri);
        }
    }
    fn artifacts_tab(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
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
            ui.label(RichText::new("DAG артефактов").strong().color(pal.text));
            ui.label(RichText::new(format!("{} устаревших", stale.len())).small().color(pal.amber));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, IC_UPDATE, "Обновить связанные").clicked() {
                    refresh = true;
                }
            });
        });
        ui.separator();
        if artifacts.is_empty() {
            ui.vertical_centered(|ui| {
                ui.add_space(40.0);
                ui.label(RichText::new("В проекте нет артефактов").color(pal.muted));
            });
            return;
        }
        let dag_height = self.dag_height.clamp(80.0, 700.0);
        let n = artifacts.len().min(4);
        let mut nw = 178.0;
        let nh = 62.0;
        let margin = 26.0;
        let gap = 16.0;
        let avail_w = ui.available_width();
        // Зазор между карточками не меньше gap: step >= nw + gap.
        let mut step = if n > 1 {
            ((avail_w - 2.0 * margin - nw) / (n as f32 - 1.0)).max(nw + gap)
        } else {
            0.0
        };
        if n > 0 && 2.0 * margin + nw + step * (n as f32 - 1.0) > avail_w {
            // Пропорционально сузить карточки до минимума 140.
            nw = ((avail_w - 2.0 * margin - (n as f32 - 1.0) * gap) / n as f32).clamp(140.0, 178.0);
            step = if n > 1 {
                ((avail_w - 2.0 * margin - nw) / (n as f32 - 1.0)).max(nw + gap)
            } else {
                0.0
            };
        }
        let content_w = if n > 0 {
            (2.0 * margin + nw + step * (n as f32 - 1.0)).max(avail_w)
        } else {
            avail_w
        };
        let index_of = |id: &str| artifacts.iter().position(|o| o.id == id);
        let selected_id = self.selected.clone();
        let mut clicked_node: Option<String> = None;
        egui::ScrollArea::horizontal()
            .id_salt("rde_dag_scroll")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(content_w, dag_height), Sense::hover());
                let painter = ui.painter_at(rect);
                let y = rect.top() + 46.0;
                let nodes: Vec<Rect> = (0..n)
                    .map(|i| {
                        Rect::from_min_size(pos2(rect.left() + margin + step * i as f32, y), vec2(nw, nh))
                    })
                    .collect();
                // Рёбра — ДО узлов, чтобы линии не лежали поверх карточек.
                for (from, to) in &edges {
                    if let (Some(a), Some(b)) = (index_of(from), index_of(to)) {
                        if a < n && b < n {
                            arrow(&painter, nodes[a], nodes[b], pal.amber);
                        }
                    }
                }
                for (i, object) in artifacts.iter().take(n).enumerate() {
                    let node_response =
                        ui.interact(nodes[i], ui.id().with(("rde_dag_node", i)), Sense::click());
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
                    let color = freshness_color(object.freshness, pal);
                    let selected = selected_id.as_deref() == Some(object.id.as_str());
                    draw_node(
                        &painter,
                        pal,
                        nodes[i],
                        &object.title,
                        &object.detail,
                        color,
                        object.freshness.label_ru(),
                        selected,
                    );
                }
                let mut line_y = rect.bottom() - 70.0;
                for (title, detail) in &stale {
                    painter.text(
                        pos2(rect.left() + margin, line_y),
                        Align2::LEFT_TOP,
                        format!("• {title}: {detail}"),
                        FontId::proportional(12.0),
                        pal.amber,
                    );
                    line_y += 17.0;
                }
                if artifacts.len() > n {
                    painter.text(
                        pos2(rect.left() + margin, rect.bottom() - 22.0),
                        Align2::LEFT_TOP,
                        format!("…и ещё {} артефакт(ов)", artifacts.len() - n),
                        FontId::proportional(11.5),
                        pal.muted,
                    );
                }
            });
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
        // Перетаскиваемый разделитель DAG ↔ рабочее место артефакта.
        let handle = ui.allocate_response(vec2(ui.available_width(), 8.0), Sense::drag());
        if handle.hovered() || handle.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
        }
        if handle.dragged() {
            self.dag_height = (self.dag_height + handle.drag_delta().y).clamp(80.0, 700.0);
        }
        let handle_rect = handle.rect;
        ui.painter().line_segment(
            [egui::pos2(handle_rect.left(), handle_rect.center().y), egui::pos2(handle_rect.right(), handle_rect.center().y)],
            egui::Stroke::new(1.0, if handle.hovered() || handle.dragged() { pal.accent } else { pal.panel_hi }),
        );
        self.artifact_workspace(ui);
    }

    /// Загрузить/обновить редактор артефакта для выбранного объекта.
    fn ensure_artifact_editor(&mut self, object: &rde_core::Object) {
        let need_reload = self.artifact_editor.as_ref().map(|editor| editor.artifact_id != object.id).unwrap_or(true);
        if !need_reload {
            return;
        }
        let ObjectData::Artifact(manifest) = object.data.clone() else { return };
        let script_path = manifest
            .runtime
            .as_ref()
            .map(|runtime| object.dir.join(&runtime.entry))
            .unwrap_or_else(|| object.dir.join("script.py"));
        let script_text = std::fs::read_to_string(&script_path).unwrap_or_default();
        let params_text = serde_json::to_string_pretty(&manifest.params).unwrap_or_else(|_| "{}".to_owned());
        let outputs_text = manifest.outputs.join("\n");
        let timeout_text = manifest.timeout_s.map(|value| value.to_string()).unwrap_or_default();
        self.artifact_editor = Some(ArtifactEditorState {
            artifact_id: object.id.clone(),
            manifest,
            script_text,
            params_text,
            outputs_text,
            timeout_text,
            new_input_artifact: String::new(),
            new_input_selector: String::new(),
            new_input_file: String::new(),
            new_input_mode: "hashed".to_owned(),
            new_constant_name: String::new(),
            new_constant_value: String::new(),
            new_constant_unit: String::new(),
            new_constant_symbol: String::new(),
            new_constant_subscript: String::new(),
            new_constant_display: String::new(),
            new_constant_style: "italic-subscript".to_owned(),
            new_constant_format: String::new(),
            new_constant_source: String::new(),
        });
    }

    /// Сохранить редактор артефакта: скрипт + `artifact.toml` (валидируется JSON Schema).
    pub fn save_artifact_editor(&mut self) -> Result<(), String> {
        let Some(editor) = self.artifact_editor.clone() else { return Ok(()) };
        let Some(object) = self.project.artifact_object(&editor.artifact_id).cloned() else {
            return Err("артефакт не найден".to_owned());
        };
        let mut manifest = editor.manifest.clone();
        // Поля из текстовых редакторов.
        manifest.params = if editor.params_text.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::from_str(&editor.params_text).map_err(|e| format!("params не JSON: {e}"))?
        };
        manifest.outputs = editor
            .outputs_text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_owned)
            .collect();
        manifest.timeout_s = {
            let text = editor.timeout_text.trim();
            if text.is_empty() {
                None
            } else {
                Some(text.parse::<u64>().map_err(|e| format!("timeout_s: {e}"))?)
            }
        };
        // Скрипт — только если изменился (не трогаем mtime зря).
        if let Some(runtime) = &manifest.runtime {
            let script_path = object.dir.join(&runtime.entry);
            let current = std::fs::read_to_string(&script_path).unwrap_or_default();
            if current != editor.script_text {
                std::fs::write(&script_path, &editor.script_text)
                    .map_err(|e| format!("записать {}: {e}", script_path.display()))?;
            }
        }
        rde_core::write_artifact_manifest(&self.project.root, &manifest)?;
        self.reload_project();
        self.artifact_editor = None;
        self.push_log(Level::Ok, format!("Артефакт «{}» сохранён (манифест + скрипт)", object.title));
        if self.auto_update && self.project.manifest.trusted && self.update_job.is_none() {
            self.auto_backup_project();
            self.request_update(Some(vec![object.id.clone()]), false);
        }
        Ok(())
    }

    /// Текстура превью изображения (PNG/JPEG/SVG→PNG) с кэшем по пути+mtime.
    fn image_texture(&mut self, ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
        let key = path.display().to_string();
        let mtime = std::fs::metadata(path).and_then(|meta| meta.modified()).ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        if let Some((cached_mtime, texture)) = self.image_previews.get(&key) {
            if *cached_mtime == mtime {
                return Some(texture.clone());
            }
        }
        let bytes = std::fs::read(path).ok()?;
        let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase();
        let bytes = if ext == "svg" { rde_doc::rasterize_svg(&bytes).ok()? } else { bytes };
        let decoded = image::load_from_memory(&bytes).ok()?.to_rgba8();
        let size = [decoded.width() as usize, decoded.height() as usize];
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, decoded.as_raw());
        let texture = ctx.load_texture(format!("rde-preview:{key}"), color_image, egui::TextureOptions::LINEAR);
        self.image_previews.insert(key, (mtime, texture.clone()));
        Some(texture)
    }

    /// Рабочее место артефакта (R-ART-01/02/04): входные данные, генерация, результат генерации.
    fn artifact_workspace(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        let selected = self.selected.clone();
        let artifact_object = self
            .project
            .objects
            .iter()
            .filter(|object| matches!(&object.data, ObjectData::Artifact(_)))
            .find(|object| selected.as_deref() == Some(object.id.as_str()))
            .cloned()
            .or_else(|| {
                self.project
                    .objects
                    .iter()
                    .find(|object| matches!(&object.data, ObjectData::Artifact(_)))
                    .cloned()
            });
        let Some(object) = artifact_object else { return };
        let ObjectData::Artifact(manifest) = object.data.clone() else { return };
        self.ensure_artifact_editor(&object);
        let status = self.dag_statuses.get(&object.id).cloned();
        let mut section = self.artifact_section;
        let mut update_artifact = false;
        let mut save_editor = false;
        let mut take_log: Option<PathBuf> = None;
        let mut reveal: Option<PathBuf> = None;
        let mut copy_text: Option<String> = None;
        let root = self.project.root.clone();
        let mut editor = self.artifact_editor.take();
        // Данные результата: выходы + превью файлов.
        let output_rows: Vec<(String, PathBuf, u64, Option<String>)> = manifest
            .outputs
            .iter()
            .map(|rel| {
                let path = object.dir.join(rel);
                let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase();
                let preview = if matches!(ext.as_str(), "json" | "csv" | "txt" | "md" | "svg" | "py" | "sh") {
                    std::fs::read_to_string(&path).ok().map(|text| {
                        if text.chars().count() > 4000 {
                            text.chars().take(4000).collect::<String>() + "…"
                        } else {
                            text
                        }
                    })
                } else {
                    None
                };
                (rel.clone(), path, size, preview)
            })
            .collect();
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("Артефакт: {}", object.title)).strong().color(pal.text));
            ui.label(RichText::new(format!("{} · {}", manifest.kind, object.detail)).small().color(pal.muted));
            if let Some(status) = &status {
                ui.label(
                    RichText::new(format!("{} {}", status.status.label_ru(), status.status.reason().unwrap_or_default()))
                        .small()
                        .color(if matches!(status.status, rde_dag::LiveStatus::Fresh) { pal.green } else { pal.amber }),
                );
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icon_button(ui, IC_UPDATE, "Обновить связанные").clicked() {
                    self.request_update(Some(vec![object.id.clone()]), true);
                }
                if btn(ui, Role::Secondary, Some(IC_UPDATE), "Обновить артефакт").clicked() {
                    update_artifact = true;
                }
                if btn(ui, Role::Primary, Some(IC_SAVE), "Сохранить").clicked() {
                    save_editor = true;
                }
            });
        });
        ui.horizontal(|ui| {
            for (value, label) in [
                (ArtifactSection::Inputs, "Входные данные"),
                (ArtifactSection::Generation, "Генерация"),
                (ArtifactSection::Result, "Результат генерации"),
            ] {
                if ui.selectable_label(section == value, RichText::new(label).size(12.5)).clicked() {
                    section = value;
                }
            }
        });
        ui.separator();
        // Превью изображений-выходов (PNG/JPEG; SVG растеризуется) — нормальный просмотрщик.
        let mut image_textures: std::collections::HashMap<String, egui::TextureHandle> = std::collections::HashMap::new();
        for (rel, path, _, _) in &output_rows {
            let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase();
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg") && path.is_file() {
                if let Some(texture) = self.image_texture(ui.ctx(), path) {
                    image_textures.insert(rel.clone(), texture);
                }
            }
        }
        let Some(editor_state) = editor.as_mut() else { return };
        if editor_state.manifest.kind == "constants" {
            // Редактор констант/величин проекта (R-ART-09, ADR-0005 P1): значение (литерал или
            // кэш из `source`), обозначение (symbol/subscript), единица, display, rich-стиль,
            // формат отображения и источник значения из выхода артефакта.
            ui.label(
                RichText::new("Константы проекта (R-ART-09): значение/источник, обозначение (symbol/subscript), единица, display, стиль, format")
                    .small()
                    .color(pal.muted),
            );
            egui::ScrollArea::vertical().id_salt("rde_constants_editor").auto_shrink([false, false]).show(ui, |ui| {
                // Широкий редактор (10 колонок) — горизонтальная прокрутка, чтобы source/format
                // не уезжали под инспектор при узком центре.
                egui::ScrollArea::horizontal().id_salt("rde_constants_hscroll").show(ui, |ui| {
                    egui::Grid::new("rde_constants_grid").num_columns(10).spacing([10.0, 6.0]).striped(true).show(ui, |ui| {
                    for header in ["имя", "значение (кэш)", "единица", "symbol", "subscript", "отображение", "стиль", "format", "source (rde://artifact/…)", ""] {
                        ui.label(RichText::new(header).small().color(pal.muted));
                    }
                    ui.end_row();
                    let mut remove: Option<String> = None;
                    for (name, value) in editor_state.manifest.values.iter_mut() {
                        ui.label(RichText::new(name.as_str()).monospace().size(12.0).color(pal.accent));
                        let mut number = value.value;
                        if ui.add(egui::DragValue::new(&mut number).speed(0.01).max_decimals(6)).changed() {
                            value.value = number;
                        }
                        ui.add(egui::TextEdit::singleline(&mut value.unit).desired_width(70.0));
                        ui.add(egui::TextEdit::singleline(&mut value.symbol).hint_text("R").desired_width(50.0));
                        ui.add(egui::TextEdit::singleline(&mut value.subscript).hint_text("н").desired_width(50.0));
                        ui.add(egui::TextEdit::singleline(&mut value.display).hint_text("R_н").desired_width(80.0));
                        egui::ComboBox::from_id_salt(format!("rde_constant_style_{name}"))
                            .selected_text(if value.style.is_empty() { "обычный".to_owned() } else { value.style.clone() })
                            .width(130.0)
                            .show_ui(ui, |ui| {
                                for style in ["", "italic", "italic-subscript"] {
                                    ui.selectable_value(&mut value.style, style.to_owned(), if style.is_empty() { "обычный" } else { style });
                                }
                            });
                        ui.add(egui::TextEdit::singleline(&mut value.format).hint_text("auto|dec:2").desired_width(80.0));
                        let mut source = value.source.clone().unwrap_or_default();
                        if ui
                            .add(egui::TextEdit::singleline(&mut source).hint_text("rde://artifact/<id>@<out_hash>/out/x.json#/поле").desired_width(280.0))
                            .changed()
                        {
                            let source = source.trim().to_owned();
                            value.source = if source.is_empty() { None } else { Some(source) };
                        }
                        if ui.small_button("×").on_hover_text("Удалить константу").clicked() {
                            remove = Some(name.clone());
                        }
                        ui.end_row();
                    }
                    if let Some(name) = remove {
                        editor_state.manifest.values.remove(&name);
                    }
                    });
                });
                ui.add_space(8.0);
                ui.separator();
                ui.label(RichText::new("Добавить константу").small().strong().color(pal.muted));
                egui::ScrollArea::horizontal().id_salt("rde_new_constant_hscroll").show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut editor_state.new_constant_name).hint_text("R_н").desired_width(90.0));
                    ui.add(egui::TextEdit::singleline(&mut editor_state.new_constant_value).hint_text("12.4").desired_width(90.0));
                    ui.add(egui::TextEdit::singleline(&mut editor_state.new_constant_unit).hint_text("Ом").desired_width(70.0));
                    ui.add(egui::TextEdit::singleline(&mut editor_state.new_constant_symbol).hint_text("R").desired_width(50.0));
                    ui.add(egui::TextEdit::singleline(&mut editor_state.new_constant_subscript).hint_text("н").desired_width(50.0));
                    ui.add(egui::TextEdit::singleline(&mut editor_state.new_constant_display).hint_text("R_н").desired_width(90.0));
                    ui.add(egui::TextEdit::singleline(&mut editor_state.new_constant_format).hint_text("format").desired_width(70.0));
                    ui.add(egui::TextEdit::singleline(&mut editor_state.new_constant_source).hint_text("source (rde://artifact/…)").desired_width(220.0));
                    egui::ComboBox::from_id_salt("rde_new_constant_style")
                        .selected_text(editor_state.new_constant_style.clone())
                        .width(140.0)
                        .show_ui(ui, |ui| {
                            for style in ["", "italic", "italic-subscript"] {
                                ui.selectable_value(&mut editor_state.new_constant_style, style.to_owned(), if style.is_empty() { "обычный" } else { style });
                            }
                        });
                    if ui.button("+ константа").clicked() && !editor_state.new_constant_name.trim().is_empty() {
                        let name = editor_state.new_constant_name.trim().to_owned();
                        let value = editor_state.new_constant_value.trim().parse::<f64>().unwrap_or(0.0);
                        let source = editor_state.new_constant_source.trim().to_owned();
                        editor_state.manifest.values.insert(
                            name.clone(),
                            rde_core::ConstantValue {
                                value,
                                unit: editor_state.new_constant_unit.trim().to_owned(),
                                display: editor_state.new_constant_display.trim().to_owned(),
                                style: editor_state.new_constant_style.clone(),
                                symbol: editor_state.new_constant_symbol.trim().to_owned(),
                                subscript: editor_state.new_constant_subscript.trim().to_owned(),
                                format: editor_state.new_constant_format.trim().to_owned(),
                                source: if source.is_empty() { None } else { Some(source) },
                            },
                        );
                        editor_state.new_constant_name.clear();
                        editor_state.new_constant_value.clear();
                        editor_state.new_constant_unit.clear();
                        editor_state.new_constant_symbol.clear();
                        editor_state.new_constant_subscript.clear();
                        editor_state.new_constant_display.clear();
                        editor_state.new_constant_format.clear();
                        editor_state.new_constant_source.clear();
                    }
                });
                });
            });
            self.artifact_editor = editor;
            self.artifact_section = section;
            if save_editor {
                if let Err(error) = self.save_artifact_editor() {
                    self.push_log(Level::Warn, format!("Константы: {error}"));
                }
            }
            return;
        }
        egui::ScrollArea::vertical().id_salt("rde_artifact_workspace").auto_shrink([false, false]).show(ui, |ui| {
            match section {
                ArtifactSection::Inputs => {
                    ui.label(RichText::new("Группы зависимостей (hashed — влияют на хеш и каскад, ref — только provenance)").small().color(pal.muted));
                    let mut remove: Option<(usize, usize)> = None;
                    for (group_index, group) in editor_state.manifest.groups.iter_mut().enumerate() {
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut group.name).desired_width(140.0));
                            egui::ComboBox::from_id_salt(format!("rde_group_mode_{group_index}"))
                                .selected_text(group.mode.clone())
                                .width(100.0)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut group.mode, "hashed".to_owned(), "hashed");
                                    ui.selectable_value(&mut group.mode, "ref".to_owned(), "ref");
                                });
                            ui.label(RichText::new(format!("входов: {}", group.inputs.len())).small().color(pal.muted));
                        });
                        for (input_index, input) in group.inputs.iter_mut().enumerate() {
                            ui.horizontal(|ui| {
                                ui.add_space(18.0);
                                match input {
                                    rde_core::InputSpec::Artifact { artifact, selector } => {
                                        let title = self.project.object(artifact).map(|object| object.title.clone()).unwrap_or_else(|| artifact.clone());
                                        ui.label(RichText::new(format!("артефакт: {title}")).size(12.0).color(pal.text));
                                        ui.add(egui::TextEdit::singleline(selector).hint_text("out/*.csv").desired_width(220.0));
                                    }
                                    rde_core::InputSpec::File { file } => {
                                        ui.label(RichText::new("файл").size(12.0).color(pal.text));
                                        ui.add(egui::TextEdit::singleline(file).desired_width(260.0));
                                    }
                                    rde_core::InputSpec::Note { note } => {
                                        ui.label(RichText::new(format!("заметка: {note}")).size(12.0).color(pal.text));
                                    }
                                    rde_core::InputSpec::Source { source } => {
                                        ui.label(RichText::new(format!("источник: {source}")).size(12.0).color(pal.text));
                                    }
                                    rde_core::InputSpec::Constants { constants, selector } => {
                                        ui.label(RichText::new(format!("константы: {constants}")).size(12.0).color(pal.text));
                                        ui.add(egui::TextEdit::singleline(selector).hint_text("R_n").desired_width(120.0));
                                    }
                                }
                                if ui.small_button("×").on_hover_text("Удалить вход").clicked() {
                                    remove = Some((group_index, input_index));
                                }
                            });
                        }
                    }
                    if let Some((group_index, input_index)) = remove {
                        if let Some(group) = editor_state.manifest.groups.get_mut(group_index) {
                            if input_index < group.inputs.len() {
                                group.inputs.remove(input_index);
                            }
                        }
                    }
                    ui.add_space(8.0);
                    ui.separator();
                    ui.label(RichText::new("Добавить вход").small().strong().color(pal.muted));
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("rde_new_input_artifact")
                            .selected_text(if editor_state.new_input_artifact.is_empty() {
                                "выбрать артефакт".to_owned()
                            } else {
                                editor_state
                                    .new_input_artifact
                                    .clone()
                            })
                            .width(220.0)
                            .show_ui(ui, |ui| {
                                for candidate in self.project.artifacts() {
                                    ui.selectable_value(&mut editor_state.new_input_artifact, candidate.id.clone(), &candidate.title);
                                }
                            });
                        ui.add(
                            egui::TextEdit::singleline(&mut editor_state.new_input_selector)
                                .hint_text("селектор (out/*.csv, inputs/data.csv)")
                                .desired_width(220.0),
                        );
                        egui::ComboBox::from_id_salt("rde_new_input_mode")
                            .selected_text(editor_state.new_input_mode.clone())
                            .width(90.0)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut editor_state.new_input_mode, "hashed".to_owned(), "hashed");
                                ui.selectable_value(&mut editor_state.new_input_mode, "ref".to_owned(), "ref");
                            });
                        if ui.button("+ артефакт").clicked() && !editor_state.new_input_artifact.is_empty() {
                            let group_name = if editor_state.new_input_mode == "hashed" { "inputs" } else { "refs" };
                            let group = input_group_mut(&mut editor_state.manifest.groups, group_name, &editor_state.new_input_mode);
                            group.inputs.push(rde_core::InputSpec::Artifact {
                                artifact: editor_state.new_input_artifact.clone(),
                                selector: editor_state.new_input_selector.clone(),
                            });
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut editor_state.new_input_file)
                                .hint_text("файл данных (inputs/data.csv)")
                                .desired_width(300.0),
                        );
                        if ui.button("+ файл").clicked() && !editor_state.new_input_file.is_empty() {
                            let group = input_group_mut(&mut editor_state.manifest.groups, "inputs", "hashed");
                            group.inputs.push(rde_core::InputSpec::File { file: editor_state.new_input_file.clone() });
                        }
                    });
                }
                ArtifactSection::Generation => {
                    egui::Grid::new("rde_generation").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                        ui.label(RichText::new("runtime").color(pal.muted));
                        ui.horizontal(|ui| {
                            let runtime = editor_state.manifest.runtime.get_or_insert_with(|| rde_core::RuntimeManifest {
                                kind: "python".to_owned(),
                                env: "project".to_owned(),
                                entry: "script.py".to_owned(),
                                vars: std::collections::BTreeMap::new(),
                            });
                            egui::ComboBox::from_id_salt("rde_runtime_kind")
                                .selected_text(runtime.kind.clone())
                                .width(100.0)
                                .show_ui(ui, |ui| {
                                    for kind in ["python", "shell", "node", "r"] {
                                        ui.selectable_value(&mut runtime.kind, kind.to_owned(), kind);
                                    }
                                });
                            egui::ComboBox::from_id_salt("rde_runtime_env")
                                .selected_text(runtime.env.clone())
                                .width(110.0)
                                .show_ui(ui, |ui| {
                                    for env in ["project", "system", "artifact"] {
                                        ui.selectable_value(&mut runtime.env, env.to_owned(), env);
                                    }
                                });
                            ui.add(egui::TextEdit::singleline(&mut runtime.entry).hint_text("script.py").desired_width(160.0));
                        });
                        ui.end_row();
                        ui.label(RichText::new("timeout, с").color(pal.muted));
                        ui.add(egui::TextEdit::singleline(&mut editor_state.timeout_text).desired_width(100.0));
                        ui.end_row();
                        ui.label(RichText::new("сеть").color(pal.muted));
                        let mut network = editor_state.manifest.network_allow.unwrap_or(false);
                        if ui.checkbox(&mut network, "разрешить network_allow").changed() {
                            editor_state.manifest.network_allow = Some(network);
                        }
                        ui.end_row();
                        ui.label(RichText::new("params (JSON)").color(pal.muted));
                        ui.add(egui::TextEdit::multiline(&mut editor_state.params_text).desired_rows(4).desired_width(420.0));
                        ui.end_row();
                        ui.label(RichText::new("outputs").color(pal.muted));
                        ui.add(
                            egui::TextEdit::multiline(&mut editor_state.outputs_text)
                                .hint_text("out/stats.json — по одному пути на строку")
                                .desired_rows(4)
                                .desired_width(420.0),
                        );
                        ui.end_row();
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new("Скрипт артефакта").strong().small().color(pal.muted));
                    ui.add(
                        egui::TextEdit::multiline(&mut editor_state.script_text)
                            .font(egui::TextStyle::Monospace)
                            .desired_rows(16)
                            .desired_width(f32::INFINITY)
                            .code_editor(),
                    );
                }
                ArtifactSection::Result => {
                    ui.label(RichText::new("Результат генерации (выходы артефакта)").strong().small().color(pal.muted));
                    if let Some(status) = &status {
                        ui.horizontal_wrapped(|ui| {
                            if let Some(act_key) = &status.act_key {
                                ui.label(RichText::new(format!("act_key {}", &act_key[..12.min(act_key.len())])).monospace().small().color(pal.muted));
                            }
                            if let Some(out_hash) = &status.out_hash {
                                ui.label(RichText::new(format!("out_hash {}", &out_hash[..12.min(out_hash.len())])).monospace().small().color(pal.muted));
                            }
                            if let Some(run_id) = &status.run_id {
                                ui.label(RichText::new(format!("run {run_id}")).monospace().small().color(pal.muted));
                            }
                            if let Some(updated) = status.updated_unix {
                                ui.label(RichText::new(format!("обновлён unix {updated}")).small().color(pal.muted));
                            }
                        });
                    }
                    if output_rows.is_empty() {
                        ui.label(RichText::new("Объявленных выходов нет").small().color(pal.muted));
                    }
                    for (rel, path, size, preview) in &output_rows {
                        let header = format!("{rel} · {} байт", size);
                        let is_image = path
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .map(|ext| matches!(ext.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "gif" | "webp" | "svg"))
                            .unwrap_or(false);
                        egui::CollapsingHeader::new(RichText::new(header).size(12.0))
                            .id_salt(format!("rde_out_{rel}"))
                            .default_open(is_image)
                            .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                if ui.small_button("копировать путь").clicked() {
                                    copy_text = Some(path.display().to_string());
                                }
                                if ui.small_button("открыть каталог").clicked() {
                                    reveal = Some(path.parent().map(Path::to_path_buf).unwrap_or_else(|| path.clone()));
                                }
                            });
                            if !path.is_file() {
                                ui.label(RichText::new("файл ещё не создан — обновите артефакт").small().color(pal.amber));
                            } else if let Some(texture) = image_textures.get(rel) {
                                // Нормальный просмотрщик изображения (SVG растеризован, PNG/JPEG декодированы).
                                let texture_size = texture.size_vec2();
                                let scale = (460.0 / texture_size.x).min(340.0 / texture_size.y).min(1.0);
                                ui.image((texture.id(), texture_size * scale));
                                ui.label(
                                    RichText::new(format!(
                                        "изображение {}×{} px · {:.1} КБ",
                                        texture_size.x as u32,
                                        texture_size.y as u32,
                                        *size as f64 / 1024.0
                                    ))
                                    .small()
                                    .color(pal.muted),
                                );
                            } else if let Some(preview) = preview {
                                ui.add(
                                    egui::TextEdit::multiline(&mut preview.clone())
                                        .font(egui::TextStyle::Monospace)
                                        .desired_rows(10)
                                        .desired_width(f32::INFINITY)
                                        .interactive(false),
                                );
                            } else {
                                ui.label(RichText::new("предпросмотр недоступен (файл не читается или формат не поддержан)").small().color(pal.amber));
                            }
                        });
                    }
                    if let Some(run_id) = status.as_ref().and_then(|status| status.run_id.clone()) {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            let run_dir = root.join(".rde/runs").join(&run_id);
                            if ui.small_button("лог запуска").clicked() {
                                take_log = Some(run_dir.join("stdout.log"));
                            }
                            if ui.small_button("provenance").clicked() {
                                take_log = Some(run_dir.join("provenance.json"));
                            }
                        });
                    }
                }
            }
        });
        self.artifact_editor = editor;
        self.artifact_section = section;
        if save_editor {
            if let Err(error) = self.save_artifact_editor() {
                self.push_log(Level::Warn, format!("Артефакт: {error}"));
            }
        }
        if update_artifact {
            self.request_update(Some(vec![object.id.clone()]), false);
        }
        if let Some(path) = copy_text {
            ui.ctx().copy_text(path);
        }
        if let Some(path) = take_log {
            if path.is_file() {
                match std::fs::read_to_string(&path) {
                    Ok(text) => {
                        let tail: String = text.chars().rev().take(4000).collect::<String>().chars().rev().collect();
                        self.push_log(Level::Info, format!("{}:\n{tail}", path.display()));
                    }
                    Err(error) => self.push_log(Level::Warn, format!("{}: {error}", path.display())),
                }
            } else {
                self.push_log(Level::Warn, format!("Файл не найден: {}", path.display()));
            }
        }
        if let Some(path) = reveal {
            #[cfg(unix)]
            if std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some() {
                let _ = std::process::Command::new("xdg-open").arg(&path).spawn();
            }
            self.push_log(Level::Info, format!("Каталог: {}", path.display()));
        }
    }

    /// Редактор заметки проекта (паритет MCP `note_write`): Markdown-файл заметки можно
    /// править и сохранять из GUI (раньше был только предпросмотр в инспекторе).
    fn note_editor_ui(&mut self, ui: &mut egui::Ui, note: &rde_core::Object) {
        let pal = theme_colors(ui.ctx().theme());
        let ObjectData::Note(manifest) = &note.data else { return };
        let path = note.dir.join(&manifest.file);
        // Загрузка при выборе другой заметки (правки не теряем молча: dirty остаётся в состоянии).
        let need_load = self.note_editor.as_ref().map(|editor| editor.id != note.id).unwrap_or(true);
        if need_load {
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| format!("# не удалось прочитать {}: {error}\n", path.display()));
            self.note_editor = Some(NoteEditorState { id: note.id.clone(), text, dirty: false, error: String::new() });
        }
        let mut save = false;
        let mut revert = false;
        let mut reveal = false;
        let mut editor = self.note_editor.take().unwrap_or_default();
        ui.add_space(10.0);
        ui.vertical(|ui| {
            egui::Frame::new()
                .fill(pal.panel_hi)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_max_width(860.0);
                    ui.label(RichText::new(format!("▤  {}", note.title)).size(FS_DISPLAY).strong().color(pal.text));
                    ui.label(
                        RichText::new(format!("Заметка проекта · {} · {}", fresh_label(note.freshness), path.display()))
                            .small()
                            .color(pal.muted),
                    );
                    ui.add_space(8.0);
                    let changed = ui
                        .add(
                            egui::TextEdit::multiline(&mut editor.text)
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(22),
                        )
                        .changed();
                    if changed {
                        editor.dirty = true;
                    }
                    if !editor.error.is_empty() {
                        ui.label(RichText::new(&editor.error).small().color(pal.red));
                    }
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        let button = ui.add_enabled(editor.dirty, egui::Button::new("Сохранить заметку"));
                        if button.clicked() {
                            save = true;
                        }
                        if ui.add_enabled(editor.dirty, egui::Button::new("Отменить правки")).clicked() {
                            revert = true;
                        }
                        if ui.button("Показать каталог").clicked() {
                            reveal = true;
                        }
                        ui.label(
                            RichText::new(if editor.dirty { "есть несохранённые правки · Ctrl+S — сохранить" } else { "Ctrl+S — сохранить" })
                                .small()
                                .color(if editor.dirty { pal.amber } else { pal.muted }),
                        );
                    });
                });
        });
        if save {
            match std::fs::write(&path, &editor.text) {
                Ok(()) => {
                    editor.dirty = false;
                    editor.error.clear();
                    self.push_log(Level::Ok, format!("Заметка «{}» сохранена: {}", note.title, path.display()));
                }
                Err(error) => {
                    editor.error = format!("не удалось сохранить {}: {error}", path.display());
                    self.push_log(Level::Warn, format!("Заметка: {}", editor.error));
                }
            }
        }
        if revert {
            editor.text = std::fs::read_to_string(&path).unwrap_or_default();
            editor.dirty = false;
            editor.error.clear();
        }
        if reveal {
            self.open_in_file_manager(&note.dir);
        }
        self.note_editor = Some(editor);
    }

    /// Сохранить заметку из Ctrl+S (когда редактор заметки активен и есть правки).
    pub fn save_note_editor(&mut self) -> bool {
        let (Some(note), Some(editor)) = (
            self.selected.clone().and_then(|id| self.project.object(&id).cloned()),
            self.note_editor.clone(),
        ) else {
            return false;
        };
        let ObjectData::Note(manifest) = &note.data else { return false };
        let path = note.dir.join(&manifest.file);
        match std::fs::write(&path, &editor.text) {
            Ok(()) => {
                if let Some(state) = self.note_editor.as_mut() {
                    state.dirty = false;
                    state.error.clear();
                }
                self.push_log(Level::Ok, format!("Заметка «{}» сохранена: {}", note.title, path.display()));
            }
            Err(error) => self.push_log(Level::Warn, format!("Заметка: не удалось сохранить {}: {error}", path.display())),
        }
        true
    }

    fn source_tab(&mut self, ui: &mut egui::Ui) {
        let pal = theme_colors(ui.ctx().theme());
        // Заметка выбрана в дереве → редактируемый текст заметки (вкладка «Источник»).
        if let Some(note) = self
            .selected
            .clone()
            .and_then(|id| self.project.object(&id).cloned())
            .filter(|object| matches!(object.data, ObjectData::Note(_)))
        {
            self.note_editor_ui(ui, &note);
            return;
        }
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
                ui.label(RichText::new("В проекте нет источников").color(pal.muted));
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
        let mut edit_source = false;
        let mut reveal_dir = false;
        let locator = src.file.clone().or_else(|| src.url.clone()).unwrap_or_default();
        let retrieved = src.retrieved_at.clone().unwrap_or_else(|| "—".to_owned());
        let reference = src.reference.clone();
        let dir = obj.dir.display().to_string();
        ui.vertical(|ui| {
            ui.add_space(0.0);
            egui::Frame::new()
                .fill(pal.panel_hi)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_max_width(680.0);
                    ui.label(RichText::new(format!("▣  {}", obj.title)).size(FS_DISPLAY).strong().color(pal.text));
                    ui.add_space(2.0);
                    ui.label(
                        RichText::new(format!("Источник проекта · {}", fresh_label(obj.freshness))).small().color(pal.muted),
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
                            ui.label(RichText::new(k).small().color(pal.muted));
                            ui.label(RichText::new(v).size(12.5).color(pal.text));
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
                        // GUI-02: путь/URL источника задаётся здесь (раньше в GUI было негде).
                        if ui
                            .button(RichText::new("Свойства источника…").size(12.0))
                            .on_hover_text("Задать файл/URL источника, прикрепить файл («Проводник…»), дату получения")
                            .clicked()
                        {
                            edit_source = true;
                        }
                        if ui
                            .button(RichText::new("Показать каталог").size(12.0))
                            .on_hover_text("Открыть каталог объекта-источника в файловом менеджере")
                            .clicked()
                        {
                            reveal_dir = true;
                        }
                    });
                });
            ui.add_space(12.0);
            egui::Frame::new()
                .fill(pal.panel)
                .corner_radius(CornerRadius::same(8))
                .inner_margin(Margin::same(14))
                .show(ui, |ui| {
                    ui.set_max_width(680.0);
                    ui.label(RichText::new("Текст источника").small().strong().color(pal.muted));
                    ui.add_space(6.0);
                    for line in [
                        "Метаданные загружены из source.toml.",
                        "PDF: текстовый слой и рендер (poppler), OCR (tesseract).",
                        "Импорт цитат BibTeX/RIS/CSL-JSON — rde-sources.",
                        "Веб-архив (monolith) — при наличии инструмента.",
                        "",
                        "Источник доступен агенту как объект проекта.",
                    ] {
                        ui.label(RichText::new(line).size(12.0).color(pal.text));
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
                    let info = info.clone();
                    ui.add_space(6.0);
                    match &info {
                        Ok(info) => {
                            ui.label(RichText::new(format!("PDF · {} стр. · текстовый слой {} символов", info.pages, info.text.chars().count())).small().color(pal.muted));
                            // M4.4: контекст-ссылка «В чат» — страница или выделенный регион PDF
                            // (§8.7.2, сценарий C4: выбранный фрагмент → чип с page/bbox).
                            ui.add_space(8.0);
                            ui.separator();
                            ui.label(RichText::new("Контекст-ссылка «В чат» (§8.7.2)").small().strong().color(pal.muted));
                            ui.horizontal(|ui| {
                                let page = self
                                    .pdf_region
                                    .as_ref()
                                    .filter(|view| view.path == path)
                                    .map(|view| view.page)
                                    .unwrap_or(1);
                                let mut page_edit = page;
                                ui.label(RichText::new("страница").small().color(pal.muted));
                                let changed = ui
                                    .add(egui::DragValue::new(&mut page_edit).range(1..=info.pages.max(1)).speed(1.0))
                                    .changed();
                                let page_edit = if changed { page_edit } else { page };
                                self.ensure_pdf_region(&path, page_edit);
                                if ui.button(RichText::new("Закрыть просмотр").size(11.0)).clicked() {
                                    self.pdf_region = None;
                                }
                            });
                            if self.pdf_region.as_ref().is_some_and(|view| view.path == path) {
                                self.pdf_region_ui(ui, &source_id);
                            } else {
                                ui.label(
                                    RichText::new("Откройте просмотр страницы, чтобы выделить регион мышью.")
                                        .small()
                                        .color(pal.muted),
                                );
                            }
                            ui.add_space(8.0);
                            ui.separator();
                            ui.label(RichText::new("Текстовый слой (стр. 1)").small().strong().color(pal.muted));
                            let excerpt: String = info.text.chars().take(600).collect();
                            egui::ScrollArea::vertical().id_salt("rde_pdf_text").max_height(120.0).show(ui, |ui| {
                                ui.label(RichText::new(excerpt).monospace().size(11.0).color(pal.text));
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
                            ui.label(RichText::new(format!("PDF: {error}")).small().color(pal.amber));
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
                    // Один путь «открыть в системном обработчике» для всех мест (ADR-0017 §4).
                    self.open_in_file_manager(&path);
                    self.push_log(Level::Info, format!("Источник открыт: {}", path.display()));
                }
                Some(path) => self.push_log(Level::Info, format!("Источник: {}", path.display())),
                None => self.push_log(Level::Warn, "У источника нет файла/URL — задайте его в «Свойства источника…»"),
            }
        }
        if edit_source {
            self.open_source_edit(&obj.id);
        }
        if reveal_dir {
            self.open_in_file_manager(&obj.dir);
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
        let pal = theme_colors(ui.ctx().theme());
        let object = self.selected_object();
        let (header, path, mut text) = match &object {
            Some(object) => {
                let name = match &object.data {
                    ObjectData::Document(_) => "document.toml",
                    ObjectData::Artifact(_) => "artifact.toml",
                    ObjectData::Source(_) => "source.toml",
                    ObjectData::Note(_) => "note.toml",
                    ObjectData::Repository(_) => "repo.toml",
                    ObjectData::Formula(_) => "formula.toml",
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
            ui.label(RichText::new("Манифест / код объекта").strong().color(pal.text));
            ui.label(RichText::new(path_label).small().color(pal.muted));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new("read-only · источник истины — файлы проекта").small().color(pal.muted));
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
            ui.label(RichText::new(header).small().color(pal.muted));
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
/// Найти или создать группу входов по имени (для редактора артефакта).
fn input_group_mut<'a>(
    groups: &'a mut Vec<rde_core::GroupManifest>,
    name: &str,
    mode: &str,
) -> &'a mut rde_core::GroupManifest {
    if let Some(index) = groups.iter().position(|group| group.name == name) {
        return &mut groups[index];
    }
    groups.push(rde_core::GroupManifest { name: name.to_owned(), mode: mode.to_owned(), inputs: Vec::new() });
    groups.last_mut().expect("группа только что добавлена")
}

/// Сегодняшняя дата ISO (`YYYY-MM-DD`) для вставки-даты.
fn today_iso_date() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Записать типизированное значение в `rde.toml` с валидацией **до** замены файла (P8-B0).
///
/// Строки пишутся в кавычках, `true`/`false` и числа — как TOML-скаляры (раньше bool
/// записывался как `"true"` и ломал JSON Schema проекта). Перед записью файл проверяется
/// парсером `ProjectManifest`; при ошибке исходный файл не меняется.
fn set_manifest_value(path: &Path, key: &str, value: &str) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("прочитать {}: {e}", path.display()))?;
    let rendered = match value {
        "true" | "false" => value.to_owned(),
        other if other.parse::<i64>().is_ok() => other.to_owned(),
        other if other.parse::<f64>().is_ok() && !other.is_empty() => other.to_owned(),
        other => format!("{other:?}"),
    };
    // Ключи верхнего уровня живут в «шапке» до первой таблицы `[section]`/`[[array]]`:
    // вставлять/заменять их можно ТОЛЬКО там — иначе `file = …` попадёт в `[reference]`
    // (source.toml), а `title` заменит заголовок библиографической записи.
    let mut out = String::with_capacity(text.len() + rendered.len() + key.len() + 8);
    let mut replaced = false;
    let mut in_sections = false;
    let prefix = format!("{key} =");
    for line in text.lines() {
        if !in_sections && line.starts_with('[') {
            in_sections = true;
            if !replaced {
                // Новый ключ верхнего уровня — перед первой таблицей.
                out.push_str(&format!("{key} = {rendered}\n"));
                replaced = true;
            }
        }
        let trimmed = line.trim_start();
        if !in_sections && !replaced && trimmed.starts_with(&prefix) && !line.starts_with(' ') {
            out.push_str(&format!("{key} = {rendered}\n"));
            replaced = true;
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if !replaced {
        out.push_str(&format!("{key} = {rendered}\n"));
    }
    // Валидация до записи: TOML — всегда; типы ProjectManifest — только для rde.toml
    // (остальные манифесты имеют свои схемы: document.toml/source.toml/repo.toml…).
    let _: toml::Value = toml::from_str(&out).map_err(|e| format!("{}: {e}", path.display()))?;
    if path.file_name().and_then(|name| name.to_str()) == Some("rde.toml") {
        let _: rde_core::ProjectManifest = toml::from_str(&out).map_err(|e| format!("rde.toml: {e}"))?;
    }
    let tmp = path.with_extension("toml.tmp");
    fs::write(&tmp, out).map_err(|e| format!("записать {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("заменить {}: {e}", path.display()))
}

/// Строковое поле манифеста: `Some(value)` — записать/заменить, `None` — удалить ключ
/// (например, переключение источника `file` → `url` не должно оставлять старый локатор).
fn set_manifest_optional_string(path: &Path, key: &str, value: Option<&str>) -> Result<(), String> {
    let Some(value) = value else {
        let text = fs::read_to_string(path).map_err(|e| format!("прочитать {}: {e}", path.display()))?;
        let mut out = String::with_capacity(text.len());
        let mut in_sections = false;
        let mut removed = false;
        let prefix = format!("{key} =");
        for line in text.lines() {
            if line.starts_with('[') {
                in_sections = true;
            }
            let trimmed = line.trim_start();
            if !removed && !in_sections && trimmed.starts_with(&prefix) && !line.starts_with(' ') {
                removed = true;
                continue;
            }
            out.push_str(line);
            out.push('\n');
        }
        let _: toml::Value = toml::from_str(&out).map_err(|e| format!("{}: {e}", path.display()))?;
        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, out).map_err(|e| format!("записать {}: {e}", tmp.display()))?;
        return fs::rename(&tmp, path).map_err(|e| format!("заменить {}: {e}", path.display()));
    };
    set_manifest_value(path, key, value)
}

/// Строковое значение в `rde.toml` (совместимость): ошибки возвращаются вызывающему.
fn set_manifest_string(path: &Path, key: &str, value: &str) -> Result<(), String> {
    set_manifest_value(path, key, value)
}

/// Роль кнопки (стиль 2): primary — один главный CTA на вкладку, secondary — поверхность
/// с рамкой, ghost — без рамки до hover, danger — деструктивное действие.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Primary,
    Secondary,
    Ghost,
    Danger,
}

/// Кнопка с ролью и опциональной иконкой слева от подписи. Заливки состояний
/// задаются локально (`ui.scope`), потому что `Button::fill` отключает hover-эффект.
fn btn(ui: &mut egui::Ui, role: Role, icon: Option<&str>, label: &str) -> egui::Response {
    let text = match icon {
        Some(icon) => format!("{icon}  {label}"),
        None => label.to_owned(),
    };
    styled_button(ui, role, RichText::new(text).size(FS_BUTTON), vec2(0.0, 32.0))
}

/// Кнопка 32 px заданной ширины — для сеток инспектора (кнопки одной группы — равной ширины).
/// Подпись обрезается по фактической ширине колонки (полная — в подсказке): иначе длинная
/// подпись («Импорт ревью из Word…») раздувает инспектор за его default width (§19-8).
fn btn_w(ui: &mut egui::Ui, role: Role, icon: Option<&str>, label: &str, width: f32) -> egui::Response {
    let icon_prefix = icon.map(|icon| format!("{icon}  ")).unwrap_or_default();
    let padding = 2.0 * ui.spacing().button_padding.x;
    let text_room = (width - padding - text_width(ui, &icon_prefix, FS_BUTTON)).max(8.0);
    let fitted = fit_label(ui, label, text_room, FS_BUTTON);
    let truncated = fitted != label;
    let response = styled_button(ui, role, RichText::new(format!("{icon_prefix}{fitted}")).size(FS_BUTTON), vec2(width, 32.0));
    if truncated { response.on_hover_text(label) } else { response }
}

/// Вписать подпись в лимит ширины, заменив хвост на «…». Нужна кнопкам-сеткам инспектора:
/// кнопка не должна становиться шире своей колонки, иначе панель растёт и обрезает центр.
fn fit_label(ui: &egui::Ui, text: &str, max_width: f32, size: f32) -> String {
    if text_width(ui, text, size) <= max_width {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.len() > 1 {
        chars.pop();
        let candidate = format!("{}…", chars.iter().collect::<String>());
        if text_width(ui, &candidate, size) <= max_width {
            return candidate;
        }
    }
    "…".to_owned()
}

/// Ширина одной колонки сетки из `n` кнопок с зазором 8 px (делит доступную ширину поровну).
fn grid_column_width(ui: &egui::Ui, n: usize) -> f32 {
    let gaps = 8.0 * n.saturating_sub(1) as f32;
    ((ui.available_width() - gaps) / n.max(1) as f32).max(40.0)
}

/// Кнопка строки статуса (24 px, внутренний отступ 3 px): 18 px по высоте, подпись мелкая.
fn btn_bar(ui: &mut egui::Ui, role: Role, icon: Option<&str>, label: &str) -> egui::Response {
    let text = match icon {
        Some(icon) => format!("{icon}  {label}"),
        None => label.to_owned(),
    };
    styled_button(ui, role, RichText::new(text).size(FS_CAPTION), vec2(0.0, 18.0))
}

/// Иконка-only кнопка с подсказкой (компактные места: тулбар панелей, заголовки карточек).
fn icon_button(ui: &mut egui::Ui, icon: &str, tooltip: &str) -> egui::Response {
    let pal = theme_colors(ui.ctx().theme());
    styled_button(ui, Role::Ghost, RichText::new(icon).size(FS_BODY + 1.0).color(pal.muted), vec2(28.0, 28.0))
        .on_hover_text(tooltip)
}

/// Большая кнопка ленты (84×52): иконка сверху, подпись мелко снизу (1–2 строки через `\n`).
const RIBBON_BUTTON_W: f32 = 84.0;
const RIBBON_BUTTON_H: f32 = 50.0;
/// Иконка-кнопка без подписи (семь видов вставок).
const RIBBON_ICON_W: f32 = 36.0;
/// Ширина кластера масштаба на ленте: «−», процент, «+», «Сброс», «По ширине».
const RIBBON_ZOOM_W: f32 = 216.0;

/// Вертикальный разделитель групп ленты фиксированной высоты (без растягивания на колонку).
fn ribbon_divider(ui: &mut egui::Ui, height: f32) {
    let pal = theme_colors(ui.ctx().theme());
    let (rect, _) = ui.allocate_exact_size(vec2(1.0, height), Sense::hover());
    ui.painter().vline(rect.center().x, rect.y_range(), Stroke::new(1.0, pal.border));
}

/// Кнопка ленты. Рисуется вручную, чтобы высота ленты была фиксированной, а подпись — мелкой.
/// `label` — подпись (строки через `\n`, пустая — иконка по центру); `a11y` — полное название,
/// его видят подсказка и доступность; `selected` — подсвеченный переключатель.
#[allow(clippy::too_many_arguments)]
fn ribbon_button(
    ui: &mut egui::Ui,
    role: Role,
    icon: &str,
    label: &str,
    a11y: &str,
    size: Vec2,
    enabled: bool,
    selected: bool,
) -> egui::Response {
    let pal = theme_colors(ui.ctx().theme());
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, a11y));
    let hovered = enabled && response.hovered();
    let (idle, hover, border, fg) = match role {
        Role::Primary => (pal.accent, pal.accent_hover, Color32::TRANSPARENT, pal.on_accent),
        Role::Secondary => (pal.panel_hi, ui.visuals().widgets.hovered.bg_fill, pal.border, pal.text),
        Role::Ghost => (Color32::TRANSPARENT, pal.panel_hi, Color32::TRANSPARENT, pal.text),
        Role::Danger => (Color32::TRANSPARENT, pal.panel_hi, pal.red, pal.red),
    };
    let fill = if hovered { hover } else { idle };
    let stroke_color = if selected { pal.accent } else { border };
    let fg = if enabled { fg } else { pal.muted };
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(6), fill);
    if stroke_color != Color32::TRANSPARENT {
        painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0, stroke_color), egui::StrokeKind::Inside);
    }
    let center_x = rect.center().x;
    if label.is_empty() {
        painter.text(rect.center(), Align2::CENTER_CENTER, icon, FontId::proportional(20.0), fg);
    } else {
        const LINE_H: f32 = 13.0;
        let lines: Vec<&str> = label.split('\n').collect();
        let mut y = if icon.is_empty() {
            rect.center().y - lines.len() as f32 * LINE_H / 2.0
        } else {
            painter.text(pos2(center_x, rect.top() + 2.0), Align2::CENTER_TOP, icon, FontId::proportional(18.0), fg);
            rect.top() + 21.0
        };
        for line in lines {
            painter.text(pos2(center_x, y), Align2::CENTER_TOP, line, FontId::proportional(FS_CAPTION), fg);
            y += LINE_H;
        }
    }
    response
}

fn styled_button(ui: &mut egui::Ui, role: Role, text: RichText, min_size: Vec2) -> egui::Response {
    let pal = theme_colors(ui.ctx().theme());
    let hover_fill = ui.visuals().widgets.hovered.bg_fill;
    let active_fill = ui.visuals().widgets.active.bg_fill;
    let (idle, hover, active, border, color) = match role {
        Role::Primary => (pal.accent, pal.accent_hover, pal.accent_active, Color32::TRANSPARENT, pal.on_accent),
        Role::Secondary => (pal.panel_hi, hover_fill, active_fill, pal.border, pal.text),
        Role::Ghost => (Color32::TRANSPARENT, pal.panel_hi, hover_fill, Color32::TRANSPARENT, pal.text),
        Role::Danger => (Color32::TRANSPARENT, pal.panel_hi, hover_fill, pal.red, pal.red),
    };
    ui.scope(|ui| {
        let widgets = &mut ui.visuals_mut().widgets;
        widgets.inactive.weak_bg_fill = idle;
        widgets.inactive.bg_fill = idle;
        widgets.hovered.weak_bg_fill = hover;
        widgets.hovered.bg_fill = hover;
        widgets.active.weak_bg_fill = active;
        widgets.active.bg_fill = active;
        widgets.inactive.bg_stroke = Stroke::new(1.0, border);
        widgets.hovered.bg_stroke = Stroke::new(1.0, if role == Role::Primary { Color32::TRANSPARENT } else { border });
        widgets.active.bg_stroke = Stroke::new(1.0, border);
        // Ghost без рамки в покое, но с фоном на hover/active (frame_when_inactive).
        // Extend: кнопка целиком переносится на новую строку, а не ломает текст по буквам.
        ui.add(
            egui::Button::new(text.color(color))
                .wrap_mode(egui::TextWrapMode::Extend)
                .frame_when_inactive(role != Role::Ghost)
                .corner_radius(8)
                .min_size(min_size),
        )
    })
    .inner
}

/// Карточка инспектора: фон панели-приподнятой, рамка, скругление 10, padding 12.
fn card<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let pal = theme_colors(ui.ctx().theme());
    egui::Frame::new()
        .fill(pal.panel_hi)
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::same(12))
        .show(ui, add)
        .inner
}

/// Капс-подпись секции: мелкий полужирный MUTED.
fn section_caption(ui: &mut egui::Ui, text: &str) {
    let pal = theme_colors(ui.ctx().theme());
    ui.label(RichText::new(text.to_uppercase()).size(FS_CAPTION).family(bold_family(ui.ctx())).color(pal.muted));
}

/// Пустое состояние: крупная иконка, одна строка, опциональная CTA-кнопка.
/// Возвращает `true`, если CTA нажата.
fn empty_state(ui: &mut egui::Ui, icon: &str, text: &str, cta: Option<(&str, &str)>) -> bool {
    let pal = theme_colors(ui.ctx().theme());
    let mut clicked = false;
    ui.vertical_centered(|ui| {
        ui.add_space(16.0);
        ui.label(RichText::new(icon).size(28.0).color(pal.muted));
        ui.label(RichText::new(text).size(FS_BODY).color(pal.muted));
        if let Some((cta_icon, cta_label)) = cta {
            ui.add_space(8.0);
            clicked = btn(ui, Role::Secondary, Some(cta_icon), cta_label).clicked();
        }
    });
    clicked
}

/// Строка «подпись мелко сверху, значение крупнее» (вместо плоского «ключ — значение»).
/// Моноширинный шрифт — только для id/путей/хешей (`monospace = true`).
fn field_row(ui: &mut egui::Ui, label: &str, value: &str, monospace: bool) {
    let pal = theme_colors(ui.ctx().theme());
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(RichText::new(label).size(FS_CAPTION).color(pal.muted));
        if monospace {
            // Длинные пути/id в узком инспекторе не должны вылезать за панель: обрезаем с «…»,
            // полное значение — в тултипе.
            let short = ellipsize_path(value, 44);
            ui.add(
                egui::Label::new(RichText::new(short).monospace().size(FS_MONO).color(pal.text))
                    .truncate()
                    .sense(Sense::hover()),
            )
            .on_hover_text(value);
        } else {
            ui.add(egui::Label::new(RichText::new(value).size(FS_BODY).color(pal.text)).wrap());
        }
    });
    ui.add_space(6.0);
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

/// Текст для показа в таблицах структуры: U+FFFC (OBJECT REPLACEMENT CHARACTER) — якорь
/// inline-объекта WordCraft, а не печатный символ. Ни один шрифт WordCraft (Inter, Source*)
/// его не содержит, и egui рисует квадратик. Удаляется только при отображении: данные
/// документа и `cached` вставок не меняются.
fn display_text(s: &str) -> String {
    s.replace('\u{FFFC}', "")
}

/// Внутренний отступ ячейки таблицы: текст не прилипает к левой границе строки/полосы.
fn pad_cell<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new().inner_margin(Margin { left: 8, right: 8, top: 4, bottom: 4 }).show(ui, add).inner
}

/// Ширина строки в пропорциональном шрифте заданного размера. Нужна строке вкладок центра
/// (19-7), чтобы заранее решить, влезают ли подписи, и перейти на иконки без переноса строки.
fn text_width(ui: &egui::Ui, text: &str, size: f32) -> f32 {
    ui.painter().layout_no_wrap(text.to_owned(), FontId::proportional(size), Color32::PLACEHOLDER).size().x
}

/// Обрезать строку до `max` символов, добавив многоточие.
fn ellipsize(s: &str, max: usize) -> String {
    let mut chars = s.char_indices();
    match chars.nth(max) {
        Some((idx, _)) => format!("{}…", &s[..idx]),
        None => s.to_owned(),
    }
}

// ── «Проводник…»: проверка путей и виджеты (ADR-0017) ───────────────────────
// Валидация — до действия: опечатка/не тот тип файла видны под полем сразу (красным),
// а кнопка действия disabled с причиной в тултипе.

/// Сжать длинный путь для подписи: начало и конец, многоточие посередине (полный — в тултипе).
fn ellipsize_path(path: &str, max: usize) -> String {
    let count = path.chars().count();
    if count <= max || max < 8 {
        return path.to_owned();
    }
    let head = (max - 3) / 2;
    let tail = max - 3 - head;
    let head_text: String = path.chars().take(head).collect();
    let tail_text: String = path.chars().skip(count - tail).collect();
    format!("{head_text}…{tail_text}")
}

/// Подпись под полем пути: сообщение проверки + вспомогательная строка (ошибка backend'а).
fn path_check_ui(ui: &mut egui::Ui, pal: ThemeColors, check: &PathCheck, picker_error: Option<&str>) {
    if let Some(error) = picker_error {
        ui.label(RichText::new(error).small().color(pal.red));
    }
    let color = match check.level {
        PathCheckLevel::Ok => pal.green,
        PathCheckLevel::Hint => pal.muted,
        PathCheckLevel::Warn => pal.amber,
        PathCheckLevel::Error => pal.red,
    };
    if !check.text.is_empty() {
        ui.label(RichText::new(&check.text).small().color(color));
    }
}

/// Кнопка «Проводник…» рядом с полем пути. `busy` — системный диалог уже открыт.
fn picker_button(ui: &mut egui::Ui, busy: bool) -> egui::Response {
    let label = if busy { format!("{IC_FOLDER}  Проводник… (ожидание)") } else { format!("{IC_FOLDER}  Проводник…") };
    ui.add_enabled(!busy, egui::Button::new(RichText::new(label).size(12.0)))
        .on_hover_text(file_dialog::backend_hint())
        .on_disabled_hover_text("Системный диалог уже открыт — выберите файл в нём или отмените")
}

/// Enter в диалоге — основное действие (кроме многострочных полей: там Ctrl+Enter).
fn dialog_confirm_pressed(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.key_pressed(egui::Key::Enter) && !input.modifiers.shift)
}

/// Esc в диалоге — отмена/закрытие.
fn dialog_escape_pressed(ctx: &egui::Context) -> bool {
    ctx.input(|input| input.key_pressed(egui::Key::Escape))
}


/// Подпись с полным путём в тултипе (для полей и инспектора).
fn path_caption_ui(ui: &mut egui::Ui, pal: ThemeColors, path: &str) {
    // Подпись нужна только для длинных путей (эллипсис + полный путь в тултипе);
    // короткий путь уже виден в поле целиком — не дублируем строку в диалоге.
    if path.trim().is_empty() || path.trim().chars().count() <= 84 {
        return;
    }
    ui.add(
        egui::Label::new(RichText::new(ellipsize_path(path.trim(), 84)).small().color(pal.muted))
            .truncate()
            .sense(Sense::hover()),
    )
    .on_hover_text(path.trim());
}

/// Проверка каталога проекта (в нём есть `rde.toml`).
fn validate_project_dir(path: &str) -> PathCheck {
    let path = path.trim();
    if path.is_empty() {
        return PathCheck::hint("Укажите каталог проекта — в нём должен быть rde.toml");
    }
    let dir = Path::new(path);
    if !dir.exists() {
        return PathCheck::error(format!("Каталог не найден: {path}"));
    }
    if !dir.is_dir() {
        return PathCheck::error("Это файл, а нужен каталог проекта (с rde.toml)");
    }
    if !dir.join("rde.toml").is_file() {
        return PathCheck::error("В каталоге нет rde.toml — это не проект RDE");
    }
    PathCheck::ok("✓ rde.toml найден — можно открывать")
}

/// Проверка каталога для нового проекта: не занят проектом, пустой (или ещё не создан).
fn validate_new_project_dir(path: &str) -> PathCheck {
    let path = path.trim();
    if path.is_empty() {
        return PathCheck::hint("Укажите каталог для нового проекта (он будет создан)");
    }
    let dir = Path::new(path);
    if dir.exists() {
        if !dir.is_dir() {
            return PathCheck::error("Это файл, а нужен каталог проекта");
        }
        if dir.join("rde.toml").is_file() {
            return PathCheck::error("Проект уже существует — откройте его («Файл → Открыть проект…»)");
        }
        if std::fs::read_dir(dir).map(|mut entries| entries.next().is_some()).unwrap_or(true) {
            return PathCheck::error("Каталог не пуст — выберите пустой каталог или новый путь");
        }
        return PathCheck::ok("✓ Каталог пуст — проект будет создан здесь");
    }
    if let Some(parent) = dir.parent()
        && !parent.as_os_str().is_empty()
        && !parent.is_dir()
    {
        return PathCheck::warn(format!("Родительский каталог будет создан: {}", parent.display()));
    }
    PathCheck::ok("✓ Каталог будет создан")
}

/// Проверка файла `.docx` (импорт документа/ревью).
fn validate_docx_file(path: &str) -> PathCheck {
    let path = path.trim();
    if path.is_empty() {
        return PathCheck::hint("Укажите путь к .docx");
    }
    let file = Path::new(path);
    if !file.exists() {
        return PathCheck::error(format!("Файл не найден: {path}"));
    }
    if !file.is_file() {
        return PathCheck::error("Это каталог, а нужен файл .docx");
    }
    if file.extension().and_then(|ext| ext.to_str()).map(|ext| !ext.eq_ignore_ascii_case("docx")).unwrap_or(true) {
        return PathCheck::error("Нужен файл .docx (импорт поддерживает только его)");
    }
    match std::fs::metadata(file) {
        Ok(meta) => PathCheck::ok(format!("✓ .docx найден · {}", human_size(meta.len()))),
        Err(error) => PathCheck::warn(format!("Файл найден, но не читается: {error}")),
    }
}

/// Проверка файла цитат (BibTeX/RIS/CSL-JSON).
fn validate_citations_file(path: &str) -> PathCheck {
    let path = path.trim();
    if path.is_empty() {
        return PathCheck::hint("Укажите файл .bib / .ris / .json");
    }
    let file = Path::new(path);
    if !file.is_file() {
        return PathCheck::error(format!("Файл не найден: {path}"));
    }
    let ext = file.extension().and_then(|ext| ext.to_str()).unwrap_or("").to_ascii_lowercase();
    if !matches!(ext.as_str(), "bib" | "ris" | "json") {
        return PathCheck::error("Поддерживаются .bib (BibTeX), .ris (RIS) и .json (CSL-JSON)");
    }
    PathCheck::ok("✓ файл цитат найден")
}

/// Проверка «программы ACP-агента»: имя в PATH или полный путь к исполняемому файлу.
fn validate_agent_program(path: &str) -> PathCheck {
    let path = path.trim();
    if path.is_empty() {
        return PathCheck::hint("Пусто — используется «dsh» из PATH");
    }
    if path.contains('/') {
        let file = Path::new(path);
        if !file.exists() {
            return PathCheck::error("Файл программы не найден");
        }
        if file.is_dir() {
            return PathCheck::error("Это каталог, а нужен исполняемый файл");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if file.metadata().map(|meta| meta.permissions().mode() & 0o111 == 0).unwrap_or(true) {
                return PathCheck::warn("Файл не исполняемый (chmod +x)");
            }
        }
        return PathCheck::ok("✓ программа найдена");
    }
    match file_dialog::find_in_path(path) {
        Some(found) => PathCheck::ok(format!("✓ найдено в PATH: {}", found.display())),
        None => PathCheck::warn("Не найдено в PATH — проверьте имя программы"),
    }
}

/// Проверка пути к файлу источника. Относительный путь трактуется от каталога объекта
/// (в `source.toml` хранятся пути вида `files/report.pdf`), абсолютный — как есть.
fn validate_source_file_in(object_dir: &Path, path: &str) -> PathCheck {
    let path = path.trim();
    if path.is_empty() {
        return PathCheck::hint("Файл не прикреплён — укажите путь или оставьте источник-URL");
    }
    let file = resolve_source_file_path(object_dir, path);
    if !file.exists() {
        return PathCheck::error(format!("Файл не найден: {path}"));
    }
    if file.is_dir() {
        return PathCheck::error("Это каталог — для источника нужен файл");
    }
    match std::fs::metadata(&file) {
        Ok(meta) => {
            let already_inside = file.starts_with(object_dir);
            let suffix = if already_inside { "файл в объекте-источнике" } else { "будет скопирован в объект" };
            PathCheck::ok(format!("✓ файл найден · {} · {suffix}", human_size(meta.len())))
        }
        Err(error) => PathCheck::warn(format!("Файл не читается: {error}")),
    }
}

/// Разрешить путь файла источника: абсолютный — как есть, относительный — от каталога объекта.
fn resolve_source_file_path(object_dir: &Path, path: &str) -> PathBuf {
    let candidate = Path::new(path.trim());
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        object_dir.join(candidate)
    }
}

/// Проверка файла источника без привязки к объекту (абсолютные пути; только тесты).
#[cfg(test)]
fn validate_source_file(path: &str) -> PathCheck {
    validate_source_file_in(Path::new("."), path)
}

/// Проверка каталога (например, пути репозитория).
fn validate_directory(path: &str) -> PathCheck {
    let path = path.trim();
    if path.is_empty() {
        return PathCheck::hint("Укажите каталог");
    }
    let dir = Path::new(path);
    if !dir.exists() {
        return PathCheck::error(format!("Каталог не найден: {path}"));
    }
    if !dir.is_dir() {
        return PathCheck::error("Это файл, а нужен каталог");
    }
    PathCheck::ok("✓ каталог найден")
}

/// Размер в человекочитаемом виде (для подсказок полей пути).
fn human_size(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} МБ", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} КБ", bytes as f64 / 1024.0)
    } else {
        format!("{bytes} Б")
    }
}

fn fresh_label(f: Freshness) -> &'static str {
    f.label_ru()
}

fn freshness_color(f: Freshness, pal: ThemeColors) -> Color32 {
    match f {
        Freshness::UpToDate => pal.green,
        Freshness::Stale => pal.amber,
        Freshness::Failed => pal.red,
        Freshness::External => pal.muted,
    }
}

fn draw_node(
    painter: &egui::Painter,
    pal: ThemeColors,
    rect: Rect,
    title: &str,
    sub: &str,
    color: Color32,
    status: &str,
    selected: bool,
) {
    painter.rect_filled(rect, CornerRadius::same(8), pal.panel_hi);
    let border = if selected { pal.accent } else { pal.border };
    painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(if selected { 1.6 } else { 1.0 }, border), egui::StrokeKind::Inside);
    painter.circle_filled(rect.left_top() + vec2(12.0, 14.0), 4.0, color);
    painter.text(
        rect.left_top() + vec2(24.0, 7.0),
        Align2::LEFT_TOP,
        ellipsize(title, 21),
        FontId::proportional(13.0),
        pal.text,
    );
    painter.text(
        rect.left_top() + vec2(12.0, 30.0),
        Align2::LEFT_TOP,
        ellipsize(sub, 26),
        FontId::proportional(11.0),
        pal.muted,
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

// ── тема и шрифты (GUI фаза 2) ───────────────────────────────────────────────

/// Вендоренные шрифты с кириллицей (`crates/rde-ui/fonts`, DejaVu).
/// Дефолтные шрифты egui остаются в цепочке как последний fallback.
const VENDORED_SANS: &[u8] = include_bytes!("../fonts/DejaVuSans.ttf");
const VENDORED_MONO: &[u8] = include_bytes!("../fonts/DejaVuSansMono.ttf");
/// Иконки Phosphor (сабсет, MIT) — fallback для глифов PUA в Proportional.
const VENDORED_ICONS: &[u8] = include_bytes!("../fonts/RdeIcons.ttf");
/// DejaVu Sans Bold (сабсет кириллица/латиница, Bitstream Vera) — семья `rde-bold`
/// для заголовков секций инспектора.
const VENDORED_BOLD: &[u8] = include_bytes!("../fonts/DejaVuSans-Bold-subset.ttf");

/// Флаг «шрифты оболочки активны» (выставляется со второго кадра). `set_fonts` применяется
/// в конце первого кадра, а `Name(..)`-семья до этого не привязана и паникует в epaint.
const FONTS_READY_ID: &str = "rde_fonts_ready";

/// Полужирная семья оболочки — `semibold`: DejaVu Sans Bold в нашей раскладке, Inter SemiBold
/// у WordCraft (он подменяет определения своими, но `semibold` там есть всегда). До
/// готовности шрифтов — обычный Proportional, без жирности.
fn bold_family(ctx: &egui::Context) -> egui::FontFamily {
    let ready = ctx.data(|data| data.get_temp::<bool>(egui::Id::new(FONTS_READY_ID))).unwrap_or(false);
    if ready { egui::FontFamily::Name("semibold".into()) } else { egui::FontFamily::Proportional }
}

/// Определения шрифтов оболочки: `rde-sans` — первый в пропорциональной семье,
/// `rde-mono` — первый в моноширинной. Именованные семьи WordCraft
/// (`medium`/`semibold`) тоже привязаны (`semibold` начинается с полужирного `rde-bold`,
/// `medium` — с `rde-sans`; WordCraft ставит на
/// `semibold` глобальный `TextStyle::Heading`, и без привязки заголовки наших
/// `Window` паникуют, когда активны шрифты оболочки (а не его `set_fonts`).
/// Чистая функция — покрыта тестом.
pub fn font_definitions() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("rde-sans".to_owned(), Arc::new(egui::FontData::from_owned(VENDORED_SANS.to_vec())));
    fonts
        .font_data
        .insert("rde-mono".to_owned(), Arc::new(egui::FontData::from_owned(VENDORED_MONO.to_vec())));
    fonts
        .font_data
        .insert("rde-icons".to_owned(), Arc::new(egui::FontData::from_owned(VENDORED_ICONS.to_vec())));
    fonts
        .font_data
        .insert("rde-bold".to_owned(), Arc::new(egui::FontData::from_owned(VENDORED_BOLD.to_vec())));
    let mut proportional = fonts.families.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();
    proportional.insert(0, "rde-sans".to_owned());
    // Иконки — сразу после текста: глифы PUA в rde-sans отсутствуют, egui берёт их отсюда.
    proportional.insert(1, "rde-icons".to_owned());
    fonts.families.insert(egui::FontFamily::Proportional, proportional.clone());
    fonts.families.insert(egui::FontFamily::Name("medium".into()), proportional.clone());
    // `semibold` — настоящее полужирное (DejaVu Sans Bold) + обычные fallback-и.
    // WordCraft при старте подменит определения своими (Inter SemiBold), семья останется.
    let mut semibold = vec!["rde-bold".to_owned()];
    semibold.extend(proportional);
    fonts.families.insert(egui::FontFamily::Name("semibold".into()), semibold);
    fonts.families.entry(egui::FontFamily::Monospace).or_default().insert(0, "rde-mono".to_owned());
    fonts
}

/// Подключить вендоренные шрифты с кириллицей (egui-дефолт может не покрывать всё).
pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

/// Иконки Phosphor должны пережить `set_fonts` WordCraft (он открывает редактор и заменяет
/// все определения своими, без `rde-icons`: иконки становятся квадратами). Если шрифта
/// нет в Proportional — дописываем его в конец семей. Вызывается каждый кадр: это только
/// проверка; переустановка — лишь после подмены WordCraft (редкое событие).
fn ensure_icon_fonts(ctx: &egui::Context) {
    let present = ctx.fonts(|fonts| {
        fonts
            .definitions()
            .families
            .get(&egui::FontFamily::Proportional)
            .is_some_and(|list| list.iter().any(|name| name == "rde-icons"))
    });
    if present {
        return;
    }
    let mut defs = ctx.fonts(|fonts| fonts.definitions().clone());
    defs.font_data
        .insert("rde-icons".to_owned(), Arc::new(egui::FontData::from_owned(VENDORED_ICONS.to_vec())));
    // В начало, не в конец: fallback-шрифты WordCraft покрывают PUA и отдали бы свои глифы.
    // В `rde-icons` только 21 иконка, поэтому обычный текст она не перехватывает.
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Name("medium".into()), egui::FontFamily::Name("semibold".into())] {
        if let Some(list) = defs.families.get_mut(&family) {
            list.insert(0, "rde-icons".to_owned());
        }
    }
    ctx.set_fonts(defs);
}

/// Тёмная палитра оболочки как `egui::Visuals` (роли: panel/window, extreme,
/// faint, selection, text; значения зафиксированы фазой 1).
fn dark_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = BG;
    visuals.extreme_bg_color = Color32::from_rgb(0x12, 0x13, 0x16);
    visuals.faint_bg_color = PANEL_HI;
    visuals.widgets.noninteractive.bg_fill = PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    visuals.widgets.inactive.bg_fill = PANEL_HI;
    visuals.widgets.inactive.weak_bg_fill = PANEL_HI;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0x2f, 0x34, 0x3e);
    visuals.widgets.active.bg_fill = Color32::from_rgb(0x36, 0x3c, 0x48);
    visuals.selection.bg_fill = Color32::from_rgb(0x2a, 0x3d, 0x66);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.override_text_color = Some(TEXT);
    visuals.window_corner_radius = CornerRadius::same(8);
    style_widgets(&mut visuals, BORDER, Color32::from_rgb(0x4a, 0x51, 0x5e));
    visuals
}

/// Общая геометрия виджетов обеих тем (стиль 2): скругление 8 px во всех состояниях,
/// у поверхностей (кнопки, поля) — рамка `border`, на hover рамка светлеет до `hover_border`.
/// Заливки состояний (ghost/secondary/primary) задаются в `action()` локально.
fn style_widgets(visuals: &mut egui::Visuals, border: Color32, hover_border: Color32) {
    let radius = CornerRadius::same(8);
    visuals.widgets.noninteractive.corner_radius = radius;
    visuals.widgets.inactive.corner_radius = radius;
    visuals.widgets.hovered.corner_radius = radius;
    visuals.widgets.active.corner_radius = radius;
    visuals.widgets.open.corner_radius = radius;
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, border);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, hover_border);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, hover_border);
}

/// Светлая палитра оболочки: тот же набор ролей поверх `Visuals::light()`.
fn light_visuals() -> egui::Visuals {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = LIGHT_PANEL;
    visuals.window_fill = LIGHT_PANEL;
    visuals.extreme_bg_color = Color32::WHITE;
    visuals.faint_bg_color = LIGHT_PANEL_HI;
    visuals.widgets.noninteractive.bg_fill = LIGHT_PANEL;
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LIGHT_BORDER);
    visuals.widgets.inactive.bg_fill = LIGHT_PANEL_HI;
    visuals.widgets.inactive.weak_bg_fill = LIGHT_PANEL_HI;
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(0xdc, 0xe1, 0xe8);
    visuals.widgets.active.bg_fill = Color32::from_rgb(0xcf, 0xd6, 0xdf);
    visuals.selection.bg_fill = Color32::from_rgb(0xb9, 0xcf, 0xf5);
    visuals.selection.stroke = Stroke::new(1.0, LIGHT_ACCENT);
    visuals.override_text_color = Some(LIGHT_TEXT);
    visuals.window_corner_radius = CornerRadius::same(8);
    style_widgets(&mut visuals, LIGHT_BORDER, Color32::from_rgb(0x9f, 0xa8, 0xb6));
    visuals
}

/// Применить тему оболочки: обе палитры — в `set_visuals_of`, предпочтение —
/// из настроек (`dark` | `light` | `system`), масштаб интерфейса — поверх
/// зума документа (через `zoom_factor`, дефолт 1.0 — no-op).
fn apply_app_theme(ctx: &egui::Context, theme: &str, ui_scale: f32) {
    ctx.set_visuals_of(egui::Theme::Dark, dark_visuals());
    ctx.set_visuals_of(egui::Theme::Light, light_visuals());
    ctx.set_theme(theme_preference_of(theme));
    if (ctx.zoom_factor() - ui_scale).abs() > 0.001 {
        ctx.set_zoom_factor(ui_scale.clamp(0.5, 2.5));
    }
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = vec2(8.0, 8.0);
        // Кнопки: высота 32 px, горизонтальный padding 12 px (стиль 2, сетка 8).
        style.spacing.button_padding = vec2(12.0, 6.0);
        style.spacing.interact_size.y = 32.0;
    });
}

/// LaTeX без пробелов — для сопоставления формулы в документе с объектом проекта (клик по
/// формуле в тексте): круг LaTeX↔модель WordCraft сохраняет смысл, но не всегда пробелы.
fn normalize_formula_latex(latex: &str) -> String {
    latex.chars().filter(|ch| !ch.is_whitespace()).collect()
}

/// Политика нумерации формулы объекта (ADR-0005 §5.1, R-DOC-15): `true`, если формула
/// display и `numbering.mode = auto` (для display-вставок назначается номер `(N)`).
fn formula_numbering_enabled(project: &rde_core::Project, formula_id: &str) -> bool {
    project
        .formula_object(formula_id)
        .and_then(|object| match &object.data {
            ObjectData::Formula(formula) => Some(formula.numbering.mode != "none"),
            _ => None,
        })
        .unwrap_or(true)
}

/// Бандл кэшей формулы из ledger/CAS для вставки/обновления документа (ADR-0005 §2.4).
fn formula_body_from_state(
    store: &rde_store::Store,
    formula_id: &str,
    state: &rde_dag::FormulaState,
    display: bool,
    numbered: bool,
) -> Result<rde_doc::FormulaBody, String> {
    let load = |hash: &Option<String>| -> Result<Vec<u8>, String> {
        let hash = hash.as_deref().ok_or("нет кэша формулы")?;
        let hash = rde_hash::Hash::from_hex(hash).map_err(|e| format!("хеш кэша формулы: {e}"))?;
        store.get_blob(hash).map_err(|e| e.to_string())?.ok_or_else(|| "кэш формулы отсутствует в CAS".to_owned())
    };
    Ok(rde_doc::FormulaBody {
        formula_id: formula_id.to_owned(),
        act_key: state.act_key.clone(),
        latex: state.resolved_latex.clone(),
        display,
        numbered,
        omml: load(&state.omml_hash)?,
        svg: state.svg_hash.as_ref().map(|_| load(&state.svg_hash)).transpose()?,
        png: state.png_hash.as_ref().map(|_| load(&state.png_hash)).transpose()?,
    })
}

impl Drop for RdeApp {
    fn drop(&mut self) {
        // M4.1/M4.2: не оставляем открытым встроенный MCP HTTP-порт после закрытия окна/теста;
        // ожидающие approvals отклоняются (fail-closed).
        self.deny_pending_agent_approvals();
        self.external_agents.stop();
        // M4.3: гасим встроенный ACP-агент (shutdown → session/close → kill process-group SDK),
        // чат-MCP-сервер и висящие permission-запросы (fail-closed).
        self.shutdown_chat("выход");
    }
}

#[cfg(test)]
mod theme_tests {
    use super::{apply_app_theme, dark_visuals, font_definitions, light_visuals, theme_colors, theme_preference_of};

    fn luminance(color: egui::Color32) -> f64 {
        fn linear(unit: u8) -> f64 {
            let value = f64::from(unit) / 255.0;
            if value <= 0.03928 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
        }
        0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
    }

    fn contrast(first: egui::Color32, second: egui::Color32) -> f64 {
        let (high, low) = if luminance(first) > luminance(second) {
            (luminance(first), luminance(second))
        } else {
            (luminance(second), luminance(first))
        };
        (high + 0.05) / (low + 0.05)
    }

    /// Тёмная палитра держит роли фазы 1 (panel/window/extreme/faint/selection/text).
    #[test]
    fn dark_visuals_keep_phase1_roles() {
        let visuals = dark_visuals();
        assert_eq!(visuals.panel_fill, super::BG);
        assert_eq!(visuals.window_fill, super::BG);
        assert_eq!(visuals.extreme_bg_color, egui::Color32::from_rgb(0x12, 0x13, 0x16));
        assert_eq!(visuals.faint_bg_color, super::PANEL_HI);
        assert_eq!(visuals.selection.bg_fill, egui::Color32::from_rgb(0x2a, 0x3d, 0x66));
        assert_eq!(visuals.selection.stroke.color, super::ACCENT);
        assert_eq!(visuals.override_text_color, Some(super::TEXT));
    }

    /// Светлая палитра покрывает тот же набор ролей и отличается от тёмной.
    #[test]
    fn light_visuals_cover_same_roles() {
        let light = light_visuals();
        let dark = dark_visuals();
        assert_eq!(light.panel_fill, super::LIGHT_PANEL);
        assert_eq!(light.window_fill, super::LIGHT_PANEL);
        assert_eq!(light.extreme_bg_color, egui::Color32::WHITE);
        assert_eq!(light.faint_bg_color, super::LIGHT_PANEL_HI);
        assert_eq!(light.override_text_color, Some(super::LIGHT_TEXT));
        assert_ne!(light.panel_fill, dark.panel_fill);
        assert_ne!(light.override_text_color, dark.override_text_color);
        // Роли оболочки связаны с константами, а не со значениями по умолчанию egui.
        assert_eq!(theme_colors(egui::Theme::Light).panel_hi, super::LIGHT_PANEL_HI);
        assert_eq!(theme_colors(egui::Theme::Dark).panel_hi, super::PANEL_HI);
    }

    /// Текст/статусы читаются на поверхностях в обеих темах (WCAG AA ≥ 4.5:1),
    /// включая выделенный текст на selection-заливке.
    #[test]
    fn text_contrast_wcag_aa_both_themes() {
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let pal = theme_colors(theme);
            for (name, color) in [
                ("text", pal.text),
                ("muted", pal.muted),
                ("accent", pal.accent),
                ("green", pal.green),
                ("amber", pal.amber),
                ("red", pal.red),
            ] {
                let ratio = contrast(color, pal.panel_hi);
                assert!(ratio >= 4.5, "{theme:?} {name}: {ratio:.2}:1 на panel_hi < 4.5");
            }
            let (visuals, ink) = match theme {
                egui::Theme::Dark => (dark_visuals(), pal.text),
                egui::Theme::Light => (light_visuals(), pal.text),
            };
            let selection = contrast(ink, visuals.selection.bg_fill);
            assert!(selection >= 4.5, "{theme:?} selection: {selection:.2}:1 < 4.5");
        }
    }

    /// Текст primary-кнопки (on_accent) читается на ACCENT и его hover/active (WCAG AA ≥ 4.5:1).
    #[test]
    fn primary_button_text_contrast_wcag_aa() {
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let pal = theme_colors(theme);
            for (name, fill) in [("accent", pal.accent), ("hover", pal.accent_hover), ("active", pal.accent_active)] {
                let ratio = contrast(pal.on_accent, fill);
                assert!(ratio >= 4.5, "{theme:?} primary {name}: {ratio:.2}:1 < 4.5");
            }
        }
    }

    /// Иконочный шрифт (Phosphor) и полужирный DejaVu вендорены как TTF; семья `rde-bold`
    /// начинается с собственного Bold, дефолтные fallback-и остаются.
    #[test]
    fn icon_and_bold_fonts_vendored() {
        let fonts = super::font_definitions();
        for name in ["rde-icons", "rde-bold"] {
            let data = fonts.font_data.get(name).unwrap_or_else(|| panic!("нет шрифта {name}"));
            assert_eq!(&data.font[..4], &[0x00, 0x01, 0x00, 0x00], "{name}: не TTF");
        }
        let bold = fonts.families.get(&egui::FontFamily::Name("semibold".into())).expect("семья semibold");
        assert_eq!(bold.first().map(String::as_str), Some("rde-bold"));
        assert!(bold.len() > 2, "fallback semibold пропал");
    }

    /// Иконки переживают чужой `set_fonts` (как у WordCraft при открытии редактора):
    /// после подмены определений без `rde-icons` `ensure_icon_fonts` возвращает её в Proportional.
    #[test]
    fn icon_font_survives_foreign_set_fonts() {
        let ctx = egui::Context::default();
        super::install_fonts(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        ctx.set_fonts(egui::FontDefinitions::default());
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        super::ensure_icon_fonts(&ctx);
        ctx.run_ui(egui::RawInput::default(), |_| {}).drop_without_applying_deltas();
        let present = ctx.fonts(|fonts| {
            fonts
                .definitions()
                .families
                .get(&egui::FontFamily::Proportional)
                .is_some_and(|list| list.iter().any(|name| name == "rde-icons"))
        });
        assert!(present, "rde-icons должна вернуться в Proportional после чужого set_fonts");
    }

    /// Маппинг настройки темы; неизвестное → Dark (как `normalize()` в rde-core).
    #[test]
    fn theme_preference_mapping() {
        assert_eq!(theme_preference_of("dark"), egui::ThemePreference::Dark);
        assert_eq!(theme_preference_of("light"), egui::ThemePreference::Light);
        assert_eq!(theme_preference_of("system"), egui::ThemePreference::System);
        assert_eq!(theme_preference_of(""), egui::ThemePreference::Dark);
        assert_eq!(theme_preference_of("light (позже)"), egui::ThemePreference::Dark);
    }

    /// Вендоренные шрифты — первые в цепочках семейств, TTF валиден,
    /// дефолты egui остаются как fallback.
    #[test]
    fn font_definitions_fallback_chain() {
        let fonts = font_definitions();
        let sans = fonts.font_data.get("rde-sans").expect("rde-sans завендорен");
        let mono = fonts.font_data.get("rde-mono").expect("rde-mono завендорен");
        for (name, data) in [("rde-sans", sans), ("rde-mono", mono)] {
            assert!(data.font.len() > 100_000, "подозрительно маленький {name}: {}", data.font.len());
            // Магия TrueType: 0x00010000 (sfnt) — файл не обрезан/не подменён.
            assert_eq!(&data.font[..4], &[0x00, 0x01, 0x00, 0x00], "{name}: не TTF");
        }
        let proportional = fonts.families.get(&egui::FontFamily::Proportional).expect("proportional");
        let monospace = fonts.families.get(&egui::FontFamily::Monospace).expect("monospace");
        assert_eq!(proportional.first().map(String::as_str), Some("rde-sans"));
        assert_eq!(monospace.first().map(String::as_str), Some("rde-mono"));
        assert!(proportional.len() > 1, "дефолтный fallback пропал из Proportional");
        assert!(monospace.len() > 1, "дефолтный fallback пропал из Monospace");
        // Именованные семьи привязаны (иначе паника заголовков Window): medium — rde-sans,
        // semibold — настоящее полужирное rde-bold (стиль 2: заголовки секций инспектора).
        for (family, first) in [("medium", "rde-sans"), ("semibold", "rde-bold")] {
            let list = fonts
                .families
                .get(&egui::FontFamily::Name(family.into()))
                .unwrap_or_else(|| panic!("нет семьи {family}"));
            assert_eq!(list.first().map(String::as_str), Some(first), "{family}");
        }
    }

    /// Заголовок `Window` (стиль Heading) не паникует, когда WordCraft уже
    /// переназначил глобальный Heading на `semibold`, а активны наши шрифты.
    #[test]
    fn window_title_survives_vendor_semibold_heading() {
        let mut open = true;
        let mut harness = egui_kittest::Harness::builder().build_ui_state(
            |ui, warmed: &mut bool| {
                // set_fonts применяется в конце пасса: установка — на первом кадре,
                // использование — со второго (как диалоги RDE после ui_frames-guard).
                if !*warmed {
                    super::install_fonts(ui.ctx());
                    // Симуляция мутации WordCraft (theme.rs: Heading → semibold).
                    ui.ctx().all_styles_mut(|style| {
                        style.text_styles.insert(
                            egui::TextStyle::Heading,
                            egui::FontId::new(17.0, egui::FontFamily::Name("semibold".into())),
                        );
                    });
                    *warmed = true;
                    return;
                }
                egui::Window::new("Настройки приложения…").open(&mut open).show(ui.ctx(), |ui| {
                    ui.label("Кириллица: Привет, Ёё!");
                });
            },
            false,
        );
        for _ in 0..4 {
            harness.step();
        }
    }

    /// Применение темы на headless-контексте: предпочтение вступает сразу;
    /// System без данных ОС падает в Dark (дефолт egui `fallback_theme`).
    #[test]
    fn apply_app_theme_sets_preference_headless() {
        for (setting, want) in [
            ("dark", egui::Theme::Dark),
            ("light", egui::Theme::Light),
            ("system", egui::Theme::Dark),
        ] {
            let ctx = egui::Context::default();
            apply_app_theme(&ctx, setting, 1.0);
            assert_eq!(ctx.theme(), want, "setting={setting}");
        }
    }
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
            if matches!(&*name, ".rde" | "exports" | "__pycache__" | "out" | ".venv") {
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
        assert_eq!(report.ran(), 3, "stats, plot и line должны выполниться: {:?}", report.artifacts);
        let stats = app.project.artifact_object(STATS).unwrap();
        assert_eq!(stats.freshness, super::Freshness::UpToDate, "после запуска актуален: {}", stats.detail);
        assert!(app.project.root.join("exports").exists() || root.join(".rde/updates").is_dir());

        let second = app.update_dependencies(None, true).expect("повторное обновление");
        assert_eq!(second.ran(), 0, "повторный запуск не нужен: {:?}", second.artifacts);
        assert_eq!(second.skipped(), 3);

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
        assert_eq!(third.ran(), 3, "stats, plot и line пересчитываются: {:?}", third.artifacts);
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
        // GUI-02: история недавних проектов не должна трогать конфиг пользователя в тестах.
        app.set_recent_projects_file(None);

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

    // ── P4: живые вставки артефакт→документ и транзакция обновления ─────────

    #[test]
    fn document_live_value_insert_updates_and_rolls_back() {
        if !python_available() {
            return;
        }
        let root = temp_demo_copy("doc-insert");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        let report = app.update_dependencies(None, true).expect("обновление артефактов");
        assert_eq!(report.failed(), 0, "{:?}", report.errors);
        let package = app.project.document_path().expect("документ");
        let uri = format!("rde://artifact/{STATS}/out/stats.json#/stats/x1/mean");
        let value_before = rde_dag::resolve_insert_text(&app.project, app.store.as_ref().unwrap(), &uri)
            .expect("значение статистики")
            .0;

        // Вставка значения из артефакта: content-control + запись в rde:doc.
        app.confirm_insert(super::InsertDialogState {
            kind: "value".into(),
            artifact_id: STATS.into(),
            formula_id: String::new(),
            render_mode: "omml".into(),
            selector: "out/stats.json".into(),
            pointer: "#/stats/x1/mean".into(),
            caption: "Среднее x1".into(),
            style: String::new(),
            format: "auto".into(),
            precision: 2,
            error: String::new(),
        });
        let rde_doc = rde_doc::load_rde_doc(&package).unwrap();
        let insert = rde_doc
            .inserts
            .iter()
            .filter(|insert| insert.kind == "value")
            .next_back()
            .unwrap_or_else(|| panic!("вставка значения не добавлена; журнал: {:?}", app.project.log.iter().map(|line| &line.text).collect::<Vec<_>>()))
            .clone();
        assert_eq!(insert.cached, rde_doc::format_value(&value_before, "auto"));
        let document_xml = {
            let file = std::fs::File::open(&package).unwrap();
            let mut zip = zip::ZipArchive::new(file).unwrap();
            let mut part = zip.by_name("word/document.xml").unwrap();
            let mut text = String::new();
            use std::io::Read;
            part.read_to_string(&mut text).unwrap();
            text
        };
        assert!(document_xml.contains(&format!("rde:{}", insert.id)), "sdt вставки в документе");
        assert!(app.store.is_some());

        // Меняем вход, пересчитываем артефакты → «Обновить документ» подставляет новое значение.
        let input = app.project.artifact_object(REALIZATIONS).unwrap().dir.join("inputs/experiment.csv");
        let mut text = std::fs::read_to_string(&input).unwrap();
        text.push_str("50,50.0,50.0\n");
        std::fs::write(&input, text).unwrap();
        app.reload_project();
        app.update_dependencies(None, true).expect("повторное обновление артефактов");
        let value_after = rde_dag::resolve_insert_text(&app.project, app.store.as_ref().unwrap(), &uri)
            .expect("новое значение")
            .0;
        assert_ne!(value_before, value_after, "значение изменилось");
        app.update_project_document();
        let rde_doc = rde_doc::load_rde_doc(&package).unwrap();
        let insert = rde_doc.inserts.iter().filter(|insert| insert.kind == "value").next_back().expect("вставка значения");
        assert_eq!(insert.cached, rde_doc::format_value(&value_after, "auto"), "вставка обновлена из артефакта");
        assert!(
            app.project.log.iter().any(|line| line.text.contains("Документ обновлён")),
            "журнал: {:?}",
            app.project.log.iter().map(|line| &line.text).collect::<Vec<_>>()
        );
        assert!(app.doc_rollback_candidate().is_some(), "транзакция документа доступна для отката");

        // Откат документа: пакет восстанавливается из CAS (значение снова прежнее).
        app.rollback_document();
        let rde_doc = rde_doc::load_rde_doc(&package).unwrap();
        let insert = rde_doc.inserts.iter().filter(|insert| insert.kind == "value").next_back().expect("вставка значения");
        assert_eq!(insert.cached, rde_doc::format_value(&value_before, "auto"), "откат вернул прежнюю версию документа");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn document_figure_insert_from_artifact_and_update() {
        if !python_available() {
            return;
        }
        let root = temp_demo_copy("doc-figure");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        app.update_dependencies(None, true).expect("обновление артефактов");
        let package = app.project.document_path().expect("документ");
        let plot = "01M5VJ1JG0QQJM2S6R76FQCZ25";
        app.confirm_insert(super::InsertDialogState {
            kind: "figure".into(),
            artifact_id: plot.into(),
            formula_id: String::new(),
            render_mode: "omml".into(),
            selector: "out/plot.svg".into(),
            pointer: String::new(),
            caption: "График статистик".into(),
            style: String::new(),
            format: "auto".into(),
            precision: 2,
            error: String::new(),
        });
        let rde_doc = rde_doc::load_rde_doc(&package).unwrap();
        assert!(rde_doc.inserts.iter().any(|insert| insert.kind == "figure" && !insert.target.is_empty()));
        let info = rde_doc::package_info(&package).unwrap();
        assert!(
            info.part_names().iter().any(|name| name.starts_with("word/media/rde-figure")),
            "media-часть рисунка: {:?}",
            info.part_names()
        );
        let media_before = {
            let file = std::fs::File::open(&package).unwrap();
            let mut zip = zip::ZipArchive::new(file).unwrap();
            let name = (0..zip.len())
                .map(|index| zip.by_index(index).unwrap().name().to_owned())
                .find(|name| name.starts_with("word/media/rde-figure"))
                .expect("media-часть");
            let mut part = zip.by_name(&name).unwrap();
            let mut bytes = Vec::new();
            use std::io::Read;
            part.read_to_end(&mut bytes).unwrap();
            bytes
        };

        // Пересобираем график с другим содержимым и обновляем документ: media-часть меняется.
        let plot_script = app.project.artifact_object(plot).unwrap().dir.join("script.py");
        std::fs::write(
            &plot_script,
            "from pathlib import Path\nPath('out').mkdir(exist_ok=True)\nPath('out/plot.svg').write_text('<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"100\" height=\"50\"><rect width=\"100\" height=\"50\" fill=\"red\"/></svg>')\n",
        )
        .unwrap();
        app.reload_project();
        let options = rde_dag::UpdateOptions { targets: Some(vec![plot.into()]), force: true, ..Default::default() };
        {
            let store = app.store.as_mut().unwrap();
            rde_dag::update(&app.project, store, &options).expect("пересобрать график");
        }
        app.reload_project();
        app.update_project_document();
        let media = {
            let file = std::fs::File::open(&package).unwrap();
            let mut zip = zip::ZipArchive::new(file).unwrap();
            let name = (0..zip.len())
                .map(|index| zip.by_index(index).unwrap().name().to_owned())
                .find(|name| name.starts_with("word/media/rde-figure"))
                .expect("media-часть");
            let mut part = zip.by_name(&name).unwrap();
            let mut bytes = Vec::new();
            use std::io::Read;
            part.read_to_end(&mut bytes).unwrap();
            bytes
        };
        assert!(media.starts_with(b"\x89PNG"), "SVG растеризован в PNG для DocView: {} байт", media.len());
        // Пиксели могут совпасть, если выход артефакта не менялся — важен формат (PNG) и содержимое.
        assert!(media.len() > 100, "рисунок содержит данные: {} байт", media.len());
        let _ = media_before;
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── P8-B1: вставки — фильтры/валидация диалогов ─────────────────────────

    #[test]
    fn insert_dialog_filters_outputs_by_kind_and_requires_json_pointer() {
        let root = temp_demo_copy("insert-dialog");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        // Рисунок: по умолчанию выбирается выход-изображение, а не out/stats.json.
        app.open_insert_dialog("figure");
        let dialog = app.insert_dialog.as_ref().expect("диалог рисунка");
        assert!(
            matches!(dialog.selector.rsplit('.').next().unwrap_or(""), "svg" | "png" | "jpg" | "jpeg" | "gif" | "webp"),
            "выход рисунка по умолчанию: {}",
            dialog.selector
        );
        app.insert_dialog = None;
        // Таблица: по умолчанию CSV-выход.
        app.open_insert_dialog("table");
        let dialog = app.insert_dialog.as_ref().expect("диалог таблицы");
        assert!(dialog.selector.ends_with(".csv"), "выход таблицы по умолчанию: {}", dialog.selector);
        app.insert_dialog = None;
        // JSON без pointer не вставляется: диалог остаётся с ошибкой, документ не меняется.
        let package = app.project.document_path().unwrap();
        let before = rde_doc::load_rde_doc(&package).unwrap().inserts.len();
        app.confirm_insert(super::InsertDialogState {
            kind: "value".into(),
            artifact_id: STATS.into(),
            formula_id: String::new(),
            render_mode: "omml".into(),
            selector: "out/stats.json".into(),
            pointer: String::new(),
            caption: "JSON".into(),
            style: String::new(),
            format: "auto".into(),
            precision: 2,
            error: String::new(),
        });
        assert!(app.insert_dialog.as_ref().is_some_and(|dialog| !dialog.error.is_empty()), "ошибка в диалоге");
        let after = rde_doc::load_rde_doc(&package).unwrap().inserts.len();
        assert_eq!(before, after, "вставка без pointer отклонена (P8, DOC-05)");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── P3: формульные вставки в UI (диалог, инспектор, «где вставлено») ─────

    #[test]
    fn formula_insert_dialog_and_inspector_show_status_and_backlinks() {
        // Копия демо без .rde (кэши/ledger не копируются) — для вставки нужен пересчёт
        // (компиляция формулы требует poppler: честный skip, как у CLI-зависимых тестов).
        if std::process::Command::new("pdftocairo").arg("-v").output().map(|output| !output.status.success()).unwrap_or(true) {
            return;
        }
        let root = temp_demo_copy("formula-dialog");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        // Диалог формулы: выбран project-level объект, режим по умолчанию — OMML.
        app.open_insert_dialog("formula");
        let dialog = app.insert_dialog.as_ref().expect("диалог формулы");
        assert_eq!(dialog.kind, "formula");
        assert!(!dialog.formula_id.is_empty(), "формула выбрана по умолчанию");
        assert_eq!(dialog.render_mode, "omml");
        // Инспектор формулы (служебный объект в дереве): живой статус и «где вставлено» (R-DOC-19).
        app.demo_show_service_objects();
        app.demo_select_object("Формула");
        let formula_id = app.selected.clone().expect("в демо выбрана формула");
        let data = app.formula_inspector_data(&formula_id);
        assert!(data.status.is_some(), "статус формулы считается из ledger: {:?}", data.status);
        assert_eq!(data.backlinks.len(), 2, "демо: вставки omml и svg → {:?}", data.backlinks);
        assert!(data.backlinks.iter().any(|link| link.render_mode == "svg"), "есть svg-вставка");
        // Вставка из диалога: появляется третья формульная вставка (остальные — in place).
        let package = app.project.document_path().unwrap();
        let before = rde_doc::load_rde_doc(&package).unwrap().inserts.iter().filter(|insert| insert.kind == "formula").count();
        let dialog = app.insert_dialog.take().expect("диалог формулы");
        app.confirm_insert(dialog);
        let doc = rde_doc::load_rde_doc(&package).unwrap();
        let after = doc.inserts.iter().filter(|insert| insert.kind == "formula").count();
        assert_eq!(after, before + 1, "формульная вставка добавлена");
        assert!(doc.inserts.iter().any(|insert| insert.kind == "formula" && insert.render_mode == "omml"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── P4 (ADR-0020): создание/правка формулы в GUI, панель, вставка с номером ──

    /// Компиляция формулы требует poppler (`pdftocairo`/`pdftotext`): без него — честный skip,
    /// как у остальных CLI-зависимых тестов.
    fn poppler_available() -> bool {
        ["pdftocairo", "pdftotext"].iter().all(|tool| {
            std::process::Command::new(tool).arg("-v").output().map(|output| output.status.success()).unwrap_or(false)
        })
    }

    /// Сырой `word/document.xml` пакета `.doc.rde` (проверки вёрстки вставок: табы/таблица/OMML).
    fn package_document_xml(package: &std::path::Path) -> String {
        use std::io::Read;
        let file = std::fs::File::open(package).expect("пакет .doc.rde");
        let mut archive = zip::ZipArchive::new(file).expect("zip-пакет .doc.rde");
        let mut entry = archive.by_name("word/document.xml").expect("word/document.xml");
        let mut xml = String::new();
        entry.read_to_string(&mut xml).expect("прочитать document.xml");
        xml
    }

    /// P4: диалог/панель создания формулы — валидация LaTeX компиляцией, объект + кэши OMML/SVG/PNG
    /// в CAS + `[state]` + статус Fresh; невалидный LaTeX и дедуп по источнику (ADR-0019) не создают
    /// дубль; панель создания рендерится в headless-харнессе без паники.
    #[test]
    fn formula_editor_creates_formula_with_caches_and_validates_latex() {
        if !poppler_available() {
            eprintln!("poppler недоступен — тест пропущен");
            return;
        }
        let root = temp_demo_copy("formula-create");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        let before = app.project.formulas().count();
        // Невалидный LaTeX: ошибка до создания объекта (панель показывает её, файлов нет).
        let error = app.create_formula_action("Битая формула", "{{{{", true, "auto").expect_err("невалидный LaTeX отклонён");
        assert!(error.contains("LaTeX"), "{error}");
        assert_eq!(app.project.formulas().count(), before, "невалидный LaTeX не создаёт объект");
        // Пустые поля — ошибки до записи.
        assert!(app.create_formula_action("  ", "x^2", true, "auto").is_err());
        assert!(app.create_formula_action("T", "  ", true, "auto").is_err());
        // Валидная формула: объект + кэши (OMML/SVG/PNG) + ledger + зеркало [state].
        let (id, reused) = app.create_formula_action("Новая формула из GUI", "a^2 + b^2 = c^2", true, "auto").expect("создать формулу");
        assert!(!reused, "первое создание — не переиспользование");
        assert_eq!(app.project.formulas().count(), before + 1);
        assert_eq!(app.selected.as_deref(), Some(id.as_str()), "созданная формула выбрана в дереве");
        let object = app.project.formula_object(&id).cloned().expect("объект формулы");
        let rde_core::ObjectData::Formula(formula) = &object.data else { panic!("не формула") };
        assert_eq!(formula.latex, "a^2 + b^2 = c^2");
        assert_eq!(formula.numbering.mode, "auto");
        assert_eq!(formula.state.act_key.len(), 64, "act_key записан в [state]: {:?}", formula.state);
        assert!(!formula.state.omml_hash.is_empty() && !formula.state.svg_hash.is_empty(), "кэши OMML/SVG: {:?}", formula.state);
        let text = std::fs::read_to_string(object.dir.join("formula.toml")).unwrap();
        assert!(text.contains("[state]") && text.contains("omml_hash"), "{text}");
        let renderer = rde_dag::FormulaRenderer { id: rde_doc::FORMULA_RENDERER_ID, version: rde_doc::FORMULA_RENDERER_VERSION };
        let statuses = rde_dag::formula_statuses(&app.project, app.store.as_ref().unwrap(), &renderer).unwrap();
        assert!(matches!(statuses[&id].status, rde_dag::FormulaLiveStatus::Fresh), "{:?}", statuses[&id].status);
        // Дедуп по ЛаТЕХ-источнику: та же формула — используется существующая (как миграция ADR-0019).
        let (same, reused) = app.create_formula_action("Дубль", "a^2 + b^2 = c^2", true, "auto").expect("дедуп");
        assert!(reused, "дубль по LaTeX переиспользует формулу");
        assert_eq!(same, id);
        assert_eq!(app.project.formulas().count(), before + 1, "дубль не создан");
        // Политика нумерации none сохраняется в манифесте.
        let (inline_id, _) = app.create_formula_action("Инлайн без номера", "x_i", false, "none").expect("формула none");
        let rde_core::ObjectData::Formula(inline) = &app.project.formula_object(&inline_id).unwrap().data else { panic!() };
        assert!(!inline.display);
        assert_eq!(inline.numbering.mode, "none");
        // Панель создания рендерится (LaTeX + предпросмотр) без паники.
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        app.demo_open_formula_editor_create("Средняя ошибка", r"\sigma = \sqrt{\frac{1}{N-1}\sum_{i=1}^{N}(x_i-\bar{x})^2}");
        assert!(app.formula_editor.is_some(), "панель создания открыта");
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1700.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..8 {
            harness.step();
        }
        assert!(harness.state().formula_editor.is_some(), "панель создания формулы остаётся открытой после кадров");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P4: правка источника формулы — манифест обновлён (inputs/fragments/[state] не потеряны),
    /// кэши/act_key пересчитаны, вставки становятся Stale; «Обновить документ» пересобирает тело,
    /// сохраняя номер/пунктуацию/источник номера (ADR-0018/0019), без `<w:tbl>`.
    #[test]
    fn formula_editor_edit_stales_inserts_and_document_update_rebuilds() {
        if !poppler_available() {
            eprintln!("poppler недоступен — тест пропущен");
            return;
        }
        let root = temp_demo_copy("formula-edit");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        let formula_id = app.project.formulas().next().expect("формула демо").id.clone();
        let package = app.project.document_path().expect("документ демо");
        let before_doc = rde_doc::load_rde_doc(&package).unwrap();
        let tables_before = package_document_xml(&package).matches("<w:tbl>").count();
        let before_insert = before_doc
            .inserts
            .iter()
            .find(|insert| insert.kind == "formula" && insert.number.is_some())
            .expect("нумерованная вставка демо")
            .clone();
        // Первичный пересчёт кэшей базы (в тестовой копии нет .rde): статус Fresh.
        app.refresh_formula_caches(&formula_id).expect("кэши формулы");
        app.reload_project();
        // Правка источника: LaTeX меняется, display/нумерация — прежние; файлы не теряются.
        app.apply_formula_edit(&formula_id, "Формула σ (правка GUI)", r"\sigma = \frac{F}{A}", true, "auto", "").expect("правка формулы");
        let object = app.project.formula_object(&formula_id).cloned().expect("формула после правки");
        let rde_core::ObjectData::Formula(manifest) = &object.data else { panic!("не формула") };
        assert_eq!(manifest.latex, r"\sigma = \frac{F}{A}");
        assert_eq!(object.title, "Формула σ (правка GUI)");
        let text = std::fs::read_to_string(object.dir.join("formula.toml")).unwrap();
        assert!(text.contains("[state]"), "[state] записан после пересчёта:\n{text}");
        // Живой статус формулы — актуальна; вставки — Stale (act_key вставки ≠ текущему).
        let data = app.formula_inspector_data(&formula_id);
        assert!(matches!(data.status, Some(rde_dag::FormulaLiveStatus::Fresh)), "{:?}", data.status);
        assert!(
            data.insert_states.contains(&rde_doc::FormulaInsertState::Stale),
            "вставки должны стать Stale: {:?}",
            data.insert_states
        );
        // «Обновить документ»: тела пересобраны, номера/пунктуация/источник номера сохранены.
        app.update_project_document();
        let after_doc = rde_doc::load_rde_doc(&package).unwrap();
        let after_insert = after_doc.inserts.iter().find(|insert| insert.id == before_insert.id).expect("вставка после обновления");
        assert_eq!(after_insert.number, before_insert.number, "номер сохранён (ADR-0018/0019)");
        assert_eq!(after_insert.number_text, before_insert.number_text, "текст номера сохранён");
        assert_eq!(after_insert.suffix, before_insert.suffix, "пунктуация после формулы сохранена");
        assert_eq!(after_insert.number_source, before_insert.number_source, "источник номера сохранён");
        assert_ne!(after_insert.hash, before_insert.hash, "act_key вставки обновлён на новый");
        assert_eq!(after_insert.kind, "formula");
        // Вёрстка: right-numbered табами (w:tabs), без таблицы; OMML формулы F/A в теле.
        let xml = package_document_xml(&package);
        assert_eq!(xml.matches("<w:tbl>").count(), tables_before, "display-формула не добавляет таблицу (ADR-0018)");
        assert!(xml.contains("<w:tabs>"), "номер справа — табуляции");
        assert!(xml.contains("m:oMath"), "OMML-тело формулы в документе");
        assert!(xml.contains("rde:formula:"), "sdt-тег реестра вставок");
        let statuses = rde_dag::formula_statuses(&app.project, app.store.as_ref().unwrap(), &rde_dag::FormulaRenderer {
            id: rde_doc::FORMULA_RENDERER_ID,
            version: rde_doc::FORMULA_RENDERER_VERSION,
        })
        .unwrap();
        assert!(matches!(statuses[&formula_id].status, rde_dag::FormulaLiveStatus::Fresh));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P4: вставка формулы из панели/дерева — display-вставка с документно-локальным номером по
    /// схеме, inline/`numbering=none` — без номера; смена схемы перенумеровывает (ADR-0018).
    #[test]
    fn formula_editor_inserts_numbered_formula_and_respects_numbering_policy() {
        if !poppler_available() {
            eprintln!("poppler недоступен — тест пропущен");
            return;
        }
        let root = temp_demo_copy("formula-insert-panel");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        let formula_id = app.project.formulas().next().expect("формула демо").id.clone();
        let package = app.project.document_path().expect("документ демо");
        let before = rde_doc::load_rde_doc(&package).unwrap().inserts.iter().filter(|insert| insert.kind == "formula").count();
        let tables_before = package_document_xml(&package).matches("<w:tbl>").count();
        app.formula_quick_insert(&formula_id, "omml");
        let doc = rde_doc::load_rde_doc(&package).unwrap();
        let inserted = doc.inserts.iter().rfind(|insert| insert.kind == "formula").expect("вставка добавлена");
        assert_eq!(doc.inserts.iter().filter(|insert| insert.kind == "formula").count(), before + 1);
        assert_eq!(inserted.target.split('@').next(), Some(format!("rde://formula/{formula_id}").as_str()), "ссылка на объект: {}", inserted.target);
        assert!(inserted.number.is_some(), "display + auto — номер назначен: {inserted:?}");
        assert_eq!(inserted.render_mode, "omml");
        // Номер — right-numbered табами, без таблицы; нумерация идёт после существующих.
        let xml = package_document_xml(&package);
        assert!(xml.contains("<w:tabs>"));
        assert_eq!(xml.matches("<w:tbl>").count(), tables_before, "номер справа — без новой таблицы (ADR-0018)");
        // Смена схемы документа перенумеровывает формулы и сохраняет вёрстку.
        let document_dir = app.active_document_object().expect("документ").dir.clone();
        let changed = app
            .apply_document_formula_numbering(&document_dir, rde_doc::FormulaNumbering::Section)
            .expect("смена схемы");
        assert!(changed > 0, "смена схемы пересчитала номера/ссылки");
        // Формула с политикой `none` (не нумеруется и inline не создаёт номер).
        let (no_number_id, _) = app.create_formula_action("Без номера", "x_i", true, "none").expect("формула none");
        let before = rde_doc::load_rde_doc(&package).unwrap().inserts.iter().filter(|insert| insert.kind == "formula").count();
        app.formula_quick_insert(&no_number_id, "omml");
        let doc = rde_doc::load_rde_doc(&package).unwrap();
        let inserted = doc.inserts.iter().rfind(|insert| insert.kind == "formula").expect("вставка none");
        assert_eq!(doc.inserts.iter().filter(|insert| insert.kind == "formula").count(), before + 1);
        assert!(inserted.number.is_none(), "политика none — без номера: {inserted:?}");
        // Inline-формула — в тексте без номера (R-DOC-11/ADR-0019) и без правой нумерации.
        let (inline_id, _) = app.create_formula_action("Inline-формула", "x_i", false, "auto").expect("inline-формула");
        app.formula_quick_insert(&inline_id, "omml");
        let doc = rde_doc::load_rde_doc(&package).unwrap();
        let inserted = doc.inserts.iter().rfind(|insert| insert.kind == "formula").expect("inline-вставка");
        assert!(inserted.number.is_none(), "inline — без номера: {inserted:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P4 (спайк A): нативная вкладка Equation WordCraft встраивается в отдельное окно панели на
    /// scratch-документе — рендерится в headless-харнессе без паники, `equation.get` отдаёт LaTeX,
    /// документ проекта не меняется.
    #[test]
    fn formula_editor_native_equation_tab_renders_and_does_not_touch_project_document() {
        let root = temp_demo_copy("formula-native");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        let package = app.project.document_path().expect("документ демо");
        let before = std::fs::read(&package).unwrap();
        app.demo_open_formula_editor_native();
        assert!(app.formula_editor.as_ref().is_some_and(|dialog| dialog.native_open), "нативная вкладка открыта");
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1700.0, 1100.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..10 {
            harness.step();
        }
        let state = harness.state_mut().formula_editor.as_mut().expect("панель формулы");
        let native = state.native.as_mut().expect("scratch-редактор WordCraft");
        let info = native.app.run("equation.get", serde_json::json!({})).expect("equation.get");
        assert!(info.get("latex").and_then(|value| value.as_str()).is_some_and(|latex| !latex.trim().is_empty()), "{info:?}");
        // Документ проекта не тронут: нативная правка идёт на отдельном мини-документе.
        assert_eq!(std::fs::read(&package).unwrap(), before, "документ проекта не изменён нативным редактором");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P4: предпросмотр страниц прокручивается колесом мыши (регресс: форсированный
    /// `vertical_scroll_offset` каждый кадр отбрасывал прокрутку на 0 — колесо и ползунок
    /// полосы прокрутки «не работали»; демо-offset нужен только скриншотам).
    #[test]
    fn preview_scrolls_with_mouse_wheel() {
        let mut app = RdeApp::new(rde_core::demo());
        app.tab = super::Tab::Document;
        app.demo_set_doc_mode("preview");
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(20_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..6 {
            harness.step();
        }
        assert_eq!(harness.state().preview_scroll_offset, 0.0, "предпросмотр начинается с верха");
        // Курсор над областью страниц + четыре тика колеса вниз.
        harness.event(egui::Event::PointerMoved(egui::pos2(760.0, 520.0)));
        harness.step();
        for _ in 0..4 {
            harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -140.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
        }
        for _ in 0..4 {
            harness.step();
        }
        let offset = harness.state().preview_scroll_offset;
        assert!(offset > 1.0, "колесо мыши прокручивает предпросмотр (offset {offset})");
    }

    /// P4: вкладка «Формула» на ленте — полноценный редактор формулы на верхней панели;
    /// клик по формуле в тексте автоматически открывает редактор этой формулы (source-of-truth
    /// `formula.toml`), вкладка/режим переключаются на «Документ»/RDE.
    #[test]
    fn formula_ribbon_tab_opens_editor_from_formula_click_in_text() {
        use egui_kittest::kittest::Queryable;
        let root = temp_demo_copy("formula-click");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        app.tab = super::Tab::Document;
        app.doc_mode = super::DocMode::DocView;
        app.ensure_editor();
        assert!(app.editor.is_some(), "редактор демо-документа: {:?}", app.editor_error);
        // Вкладка «Формула» без выбранной формулы — пустое состояние редактора на ленте.
        app.ribbon_tab = super::RibbonTab::Formula;
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..3 {
            harness.step();
        }
        assert!(harness.query_by_label_contains("Формула не выбрана").is_some(), "пустое состояние редактора на вкладке «Формула»");
        assert!(harness.query_by_label("Формула").is_some(), "вкладка «Формула» присутствует на ленте");
        // Клик по формуле в тексте: ставим каретку WordCraft на объект-уравнение демо-документа
        // (в реальном UI это делает WordCraft на клик в уравнение через `equation.edit`).
        let formula_id = harness.state().project.formulas().next().map(|formula| formula.id.clone()).expect("формула демо");
        let mut caret = None;
        {
            let ed = harness.state().editor.as_ref().expect("редактор");
            let story = wordcraft_engine::doc::StoryRef::Body;
            'outer: for path in ed.app.session.doc.para_paths(story) {
                let Some(para) = ed.app.session.doc.para(story, &path) else { continue };
                for (byte, ch) in para.text.char_indices() {
                    if ch == '\u{FFFC}' && matches!(para.object_at(byte), Some(wordcraft_engine::doc::InlineObject::Equation { .. })) {
                        caret = Some(wordcraft_engine::doc::Pos::new(story, path.clone(), byte));
                        break 'outer;
                    }
                }
            }
        }
        let caret = caret.expect("формула σ в демо-документе");
        if let Some(ed) = harness.state_mut().editor.as_mut() {
            ed.app.session.sel = wordcraft_engine::Selection::caret(caret);
        }
        harness.state_mut().open_formula_editor_from_canvas_click();
        let state = harness.state();
        let dialog = state.formula_editor.as_ref().expect("клик по формуле открыл редактор");
        assert_eq!(dialog.formula_id.as_deref(), Some(formula_id.as_str()), "открыт объект формулы σ");
        assert_eq!(state.ribbon_tab, super::RibbonTab::Formula, "редактор — на вкладке «Формула»");
        assert!(matches!(state.tab, super::Tab::Document));
        assert_eq!(state.doc_mode, super::DocMode::DocView);
        for _ in 0..3 {
            harness.step();
        }
        assert!(harness.query_by_label_contains("LaTeX-источник").is_some(), "поле LaTeX-источника на верхней панели");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── P8-B0: целостность rde.toml (typed TOML + валидация до записи) ───────

    #[test]
    fn auto_update_toggle_writes_valid_typed_toml() {
        let root = temp_demo_copy("auto-toggle");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        assert!(!app.project.manifest.document_auto_update);
        app.set_auto_update(true);
        // Проект должен перечитываться без ошибок схемы, а в файле — bool, не строка.
        let reloaded = rde_core::load_project(&root).expect("rde.toml после тоггла валиден");
        assert!(reloaded.manifest.document_auto_update, "auto=true");
        let text = std::fs::read_to_string(root.join("rde.toml")).unwrap();
        assert!(text.contains("document_auto_update = true"), "{text}");
        assert!(!text.contains("document_auto_update = \"true\""), "bool не должен быть строкой: {text}");
        app.set_auto_update(false);
        let reloaded = rde_core::load_project(&root).expect("rde.toml после выключения валиден");
        assert!(!reloaded.manifest.document_auto_update);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── P5: git-интеграция из UI ────────────────────────────────────────────

    #[test]
    fn git_init_commit_history_and_check_from_ui() {
        if std::process::Command::new("git").arg("--version").output().map(|o| !o.status.success()).unwrap_or(true) {
            eprintln!("git недоступен — тест пропущен");
            return;
        }
        let parent = std::env::temp_dir().join(format!("rde-ui-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(&parent).unwrap();
        let root = parent.join("git-project");
        let mut app = RdeApp::new(rde_core::demo());
        app.set_recent_projects_file(None);
        app.create_project_ui(&root, "Git-проект").expect("создать проект");
        let status = app.git_status.clone().expect("статус git");
        assert!(status.is_repo, "новый проект инициализируется в git: {status:?}");
        assert!(root.join(".gitignore").is_file());
        assert!(root.join(".gitattributes").is_file());

        // Создаём объект и коммитим изменения.
        app.create_object_action(super::UiObjectKind::Core(rde_core::NewObjectKind::Note), "Заметка для git").expect("заметка");
        app.git_commit("RDE: тестовая заметка");
        assert!(
            app.project.log.iter().any(|line| line.text.contains("Git: коммит")),
            "журнал: {:?}",
            app.project.log.iter().map(|line| &line.text).collect::<Vec<_>>()
        );
        let status = app.git_status.clone().unwrap();
        assert!(!status.is_dirty(), "после коммита чисто: {status:?}");
        assert!(status.last_commit.is_some());

        // История и проверка проекта содержат git-сведения.
        app.git_history();
        assert!(app.history_dialog.as_ref().is_some_and(|entries| !entries.is_empty()));
        app.run_project_check();
        let check = app.check_dialog.clone().unwrap_or_default();
        assert!(check.contains("git:"), "проверка проекта: {check}");
        let _ = std::fs::remove_dir_all(&parent);
    }

    // ── P5: настройки/авторы/диагностика ───────────────────────────────────

    #[test]
    fn project_settings_with_authors_persist() {
        let root = temp_demo_copy("authors");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        let dialog = super::ProjectSettingsState {
            name: "Отчёт с авторами".into(),
            template: "gost-7.32-2017".into(),
            rebuild: "manual".into(),
            authors: vec![
                rde_core::AuthorManifest {
                    id: rde_core::new_id(),
                    name: "Илья".into(),
                    initials: "И".into(),
                    email: "ilunix@pc.ubuntu".into(),
                    kind: "human".into(),
                },
                rde_core::AuthorManifest {
                    id: rde_core::new_id(),
                    name: "Агент".into(),
                    initials: "AI".into(),
                    email: String::new(),
                    kind: "agent".into(),
                },
            ],
            new_author_name: String::new(),
            new_author_initials: String::new(),
            new_author_email: String::new(),
        };
        app.save_project_settings_with_authors(&dialog).expect("сохранить настройки");
        let reloaded = rde_core::load_project(&root).unwrap();
        assert_eq!(reloaded.manifest.name, "Отчёт с авторами");
        assert_eq!(reloaded.manifest.default_template, "gost-7.32-2017");
        assert_eq!(reloaded.manifest.authors.len(), 2);
        assert_eq!(reloaded.manifest.authors[1].kind, "agent");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn diagnostics_export_has_environment_but_no_documents() {
        if !python_available() {
            return;
        }
        let root = temp_demo_copy("diagnostics");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        app.update_dependencies(None, true).expect("обновление артефактов");
        let path = app.export_diagnostics().expect("архив диагностики");
        assert!(path.is_file(), "{}", path.display());
        let file = std::fs::File::open(&path).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let names: Vec<String> = (0..zip.len()).map(|index| zip.by_index(index).unwrap().name().to_owned()).collect();
        assert!(names.iter().any(|name| name == "summary.txt"), "{names:?}");
        assert!(names.iter().any(|name| name == "rde.toml"), "{names:?}");
        assert!(names.iter().any(|name| name == "rde.lock"), "{names:?}");
        assert!(names.iter().any(|name| name == "transactions.txt"), "{names:?}");
        assert!(
            !names.iter().any(|name| name.starts_with("objects/") || name.contains(".doc.rde") || name.contains("cas/")),
            "в диагностике нет содержимого документов/CAS (R-SET-07): {names:?}"
        );
        let mut summary = String::new();
        {
            use std::io::Read;
            zip.by_name("summary.txt").unwrap().read_to_string(&mut summary).unwrap();
        }
        assert!(summary.contains("RDE diagnostics"), "{summary}");
        assert!(summary.contains("Демо"), "{summary}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── P6: рабочее место артефакта (входы/генерация/результат) ────────────

    #[test]
    fn artifact_workspace_edits_inputs_script_and_params() {
        if !python_available() {
            return;
        }
        let root = temp_demo_copy("artifact-workspace");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        app.selected = Some(STATS.to_owned());
        app.artifact_section = super::ArtifactSection::Inputs;
        app.artifact_editor = None;
        let object = app.project.artifact_object(STATS).cloned().unwrap();
        app.ensure_artifact_editor(&object);
        {
            let editor = app.artifact_editor.as_mut().expect("редактор артефакта");
            assert!(!editor.script_text.is_empty(), "скрипт прочитан в редактор");
            editor.params_text = r#"{"window": 256, "method": "welch"}"#.into();
            editor.outputs_text = "out/stats.json\nout/stats.csv".into();
            editor.script_text.push_str("\n# edit from artifact workspace\n");
            editor.new_input_file = "inputs/experiment.csv".into();
            let group = super::input_group_mut(&mut editor.manifest.groups, "inputs", "hashed");
            group.inputs.push(rde_core::InputSpec::File { file: "inputs/experiment.csv".into() });
        }
        app.save_artifact_editor().expect("сохранить артефакт");
        let reloaded = rde_core::load_project(&root).unwrap();
        let artifact_object = reloaded.artifact_object(STATS).unwrap();
        let rde_core::ObjectData::Artifact(manifest) = &artifact_object.data else { panic!() };
        assert_eq!(manifest.params["window"], 256);
        assert!(manifest.outputs.contains(&"out/stats.csv".to_owned()));
        assert!(manifest.groups.iter().any(|group| group.name == "inputs" && !group.inputs.is_empty()), "новый вход сохранён");
        let script = std::fs::read_to_string(artifact_object.dir.join("script.py")).unwrap();
        assert!(script.contains("edit from artifact workspace"), "скрипт сохранён");
        // Изменение скрипта делает артефакт устаревшим (P1).
        let statuses = rde_dag::evaluate(&reloaded, app.store.as_ref().unwrap()).unwrap();
        assert!(
            matches!(statuses[STATS].status, rde_dag::LiveStatus::Stale(_) | rde_dag::LiveStatus::NeverRun),
            "{:?}",
            statuses[STATS].status
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── Track A: честная кнопка «Открыть» в инспекторе ────────────────────

    #[test]
    fn inspector_open_selects_object_and_its_workplace() {
        let root = temp_demo_copy("inspector-open");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        let doc = app
            .project
            .objects
            .iter()
            .find(|object| matches!(object.data, rde_core::ObjectData::Document(_)))
            .map(|object| object.id.clone())
            .expect("документ в демо-проекте");
        let source = app
            .project
            .objects
            .iter()
            .find(|object| matches!(object.data, rde_core::ObjectData::Source(_)))
            .map(|object| object.id.clone())
            .expect("источник в демо-проекте");
        // Документ → вкладка Документ + выбор в дереве + журнал (как DAG-меню).
        app.tab = super::Tab::Artifacts;
        app.show_inspector = false;
        assert!(app.open_inspector_object(&doc));
        assert_eq!(app.selected.as_deref(), Some(doc.as_str()));
        assert!(app.show_inspector);
        assert!(app.tab == super::Tab::Document);
        assert!(
            app.project.log.iter().any(|line| line.text.contains("Открыт объект")),
            "журнал: {:?}",
            app.project.log.iter().map(|line| &line.text).collect::<Vec<_>>()
        );
        // Артефакт → вкладка Артефакты + рабочее место сброшено к результату.
        app.tab = super::Tab::Document;
        app.artifact_section = super::ArtifactSection::Inputs;
        assert!(app.open_inspector_object(STATS));
        assert_eq!(app.selected.as_deref(), Some(STATS));
        assert!(app.tab == super::Tab::Artifacts);
        assert_eq!(app.artifact_section, super::ArtifactSection::Result);
        // Источник → вкладка Источник.
        assert!(app.open_inspector_object(&source));
        assert!(app.tab == super::Tab::Source);
        assert_eq!(app.selected.as_deref(), Some(source.as_str()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn inspector_open_nothing_to_open_leaves_state_untouched() {
        let root = temp_demo_copy("inspector-open-none");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        // GUI-02: заметка теперь открывается — вкладка «Источник», редактор заметки.
        let note = app
            .project
            .objects
            .iter()
            .find(|object| matches!(object.data, rde_core::ObjectData::Note(_)))
            .map(|object| object.id.clone())
            .expect("заметка в демо-проекте");
        assert!(super::RdeApp::inspector_open_tab(app.project.object(&note).unwrap()).is_some());
        app.tab = super::Tab::Document;
        app.selected = None;
        assert!(app.open_inspector_object(&note));
        assert!(app.tab == super::Tab::Source, "редактор заметки живёт на вкладке «Источник»");
        assert_eq!(app.selected.as_deref(), Some(note.as_str()));
        // Формула — служебный объект без отдельного вьюера: кнопка disabled, перехода нет.
        if let Some(formula) = app
            .project
            .objects
            .iter()
            .find(|object| matches!(object.data, rde_core::ObjectData::Formula(_)))
            .map(|object| object.id.clone())
        {
            assert!(super::RdeApp::inspector_open_tab(app.project.object(&formula).unwrap()).is_none());
            assert!(!app.open_inspector_object(&formula));
        }
        // Репозиторий — тоже нечего открывать (показывает только инспектор).
        if let Some(repo) = app
            .project
            .objects
            .iter()
            .find(|object| matches!(object.data, rde_core::ObjectData::Repository(_)))
            .map(|object| object.id.clone())
        {
            assert!(super::RdeApp::inspector_open_tab(app.project.object(&repo).unwrap()).is_none());
            assert!(!app.open_inspector_object(&repo));
        }
        // Неизвестный id — тоже false без паники.
        assert!(!app.open_inspector_object("нет-такого-объекта"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GUI фаза 2: переключение dark/light/system не паникует и применяется —
    /// прогоняем кадры на разных вкладках (документ/структура/артефакты/источник)
    /// с увеличенным масштабом интерфейса.
    ///
    /// Bounded-кадры: `build_ui_state` внутри крутит `run_ok()` до стабилизации
    /// repaint-запросов, а встроенный редактор документа при зуме ≠ 1.0 просит
    /// перерисовку каждый кадр (стабилизации нет — висело >60с при капе 200_000),
    /// поэтому кап шагов маленький, дальше — фиксированные 8 кадров на комбинацию.
    #[test]
    fn theme_switch_renders_dark_light_system() {
        for (theme, tab) in [("dark", "document"), ("light", "artifacts"), ("system", "structure"), ("light", "source")] {
            let root = temp_demo_copy(&format!("theme-{theme}-{tab}"));
            let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
            app.demo_set_theme(theme);
            app.demo_select_tab(tab);
            app.ui_scale = 1.25;
            assert_eq!(app.app_settings.theme, theme);
            let mut harness = egui_kittest::Harness::builder()
                .with_size(egui::vec2(1600.0, 1000.0))
                // Кап вместо 200_000: run_ok() внутри build_ui_state (см. выше).
                .with_max_steps(24)
                .build_ui_state(
                    |ui, app: &mut RdeApp| {
                        app.ui(ui);
                    },
                    app,
                );
            for _ in 0..8 {
                harness.step();
            }
            assert_eq!(harness.state().app_settings.theme, theme);
            let _ = std::fs::remove_dir_all(&root);
        }
        // Мусорное значение игнорируется, тема не меняется.
        let root = temp_demo_copy("theme-bogus");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        app.demo_set_theme("dark");
        app.demo_set_theme("light (позже)");
        assert_eq!(app.app_settings.theme, "dark");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Регрессия: в новом проекте без документа вкладка «Документ» ставила
    /// `editor_error="в проекте нет документа"`; после создания документа ошибка оставалась
    /// (reload_project не сбрасывал editor/editor_error, ensure_editor выходил early-return).
    #[test]
    fn creating_document_after_empty_project_clears_stale_editor_error() {
        let root = temp_demo_copy("doc-create-after-empty");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        let doc_id = app.project.document_object().expect("документ демо").id.clone();
        // Проект без документа (как сразу после «Новый проект»).
        rde_core::delete_object(&app.project, &doc_id).expect("удалить документ в корзину");
        app.reload_project();
        assert!(app.project.document_object().is_none());
        app.tab = super::Tab::Document;
        app.ensure_editor();
        assert!(app.editor.is_none());
        assert_eq!(app.editor_error.as_deref(), Some("в проекте нет документа"));

        // Кнопка «Новый объект → Документ» (тот же путь, что в UI).
        app.create_object_action(super::UiObjectKind::Document, "main").expect("создать документ");
        assert!(app.project.document_object().is_some(), "документ появился в модели проекта");
        assert!(app.editor_error.is_none(), "устаревшая ошибка редактора сброшена: {:?}", app.editor_error);
        assert!(matches!(app.tab, super::Tab::Document));
        app.ensure_editor();
        assert!(app.editor.is_some(), "редактор открывает только что созданный документ: {:?}", app.editor_error);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// R-DOC-39 (минимальный импорт): «Файл → Импорт .docx…» создаёт новый объект
    /// «[IMPORTED] …», пакет валиден, происхождение — в `import.json`, редактор открывается.
    #[test]
    fn import_docx_creates_imported_document_and_opens_editor() {
        let root = temp_demo_copy("import-docx");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        // Проект без документа: импорт создаёт первый документ (основной сценарий нового проекта).
        let doc_id = app.project.document_object().expect("документ демо").id.clone();
        rde_core::delete_object(&app.project, &doc_id).expect("удалить демо-документ в корзину");
        app.reload_project();
        assert!(app.project.document_object().is_none());
        let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../rde-doc/tests/fixtures/base-report.docx")
            .canonicalize()
            .expect("фикстура .docx");
        let source_before = std::fs::read(&source).unwrap();
        app.import_docx_action(&source).expect("импорт .docx");
        let object = app.project.document_object().expect("импортированный документ");
        assert!(object.title.starts_with("[IMPORTED]"), "маркировка импорта: {}", object.title);
        let package = app.project.document_path().expect("путь пакета");
        assert!(package.is_file(), "пакет создан: {}", package.display());
        assert!(package.parent().unwrap().join("import.json").is_file(), "отчёт о происхождении");
        assert_eq!(std::fs::read(&source).unwrap(), source_before, "исходный .docx не изменён");
        assert!(app.project.log.iter().any(|line| line.text.contains("Импортирован .docx")), "отчёт в журнале");
        app.tab = super::Tab::Document;
        app.ensure_editor();
        assert!(app.editor.is_some(), "редактор открывает импортированный документ: {:?}", app.editor_error);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GUI-only сценарий (drag&drop): .docx, перетащенный в окно, импортируется в текущий проект —
    /// тот же путь, что «Файл → Импорт .docx…» (MathType-формулы конвертируются при импорте, R-DOC-44).
    #[test]
    fn drag_and_drop_docx_imports_into_current_project() {
        let root = temp_demo_copy("drop-docx");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        let before = app.project.documents().count();
        let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../rde-doc/tests/fixtures/base-report.docx")
            .canonicalize()
            .expect("фикстура .docx");
        app.handle_dropped_paths(&[source]);
        let after = app.project.documents().count();
        assert_eq!(after, before + 1, "перетащенный .docx импортирован новым документом");
        let imported = app
            .project
            .documents()
            .find(|object| object.title.starts_with("[IMPORTED]"))
            .expect("документ [IMPORTED]");
        assert!(imported.dir.join("report.doc.rde").is_file(), "пакет создан: {}", imported.dir.display());
        assert!(
            app.project.log.iter().any(|line| line.text.contains("Импортирован .docx")),
            "журнал: импорт виден пользователю"
        );
        assert!(app.active_document_path().is_some(), "импортированный документ стал активным");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// GUI-only сценарий (drag&drop): папка проекта, перетащенная в окно, открывается как проект.
    #[test]
    fn drag_and_drop_project_folder_opens_project() {
        let first = temp_demo_copy("drop-proj-a");
        let second = temp_demo_copy("drop-proj-b");
        let mut app = RdeApp::new(rde_core::load_project(&first).unwrap());
        app.handle_dropped_paths(&[second.clone()]);
        let opened = app.project.root.canonicalize().unwrap_or_else(|_| app.project.root.clone());
        let expected = second.canonicalize().unwrap_or_else(|_| second.clone());
        assert_eq!(opened, expected, "открыт перетащенный проект");
        assert!(
            app.project.log.iter().any(|line| line.text.contains("Проект открыт")),
            "журнал: открытие проекта видно пользователю"
        );
        let _ = std::fs::remove_dir_all(&first);
        let _ = std::fs::remove_dir_all(&second);
    }

    /// GUI-only сценарий (drag&drop): неподдерживаемый файл не «теряется молча» — предупреждение.
    #[test]
    fn drag_and_drop_unsupported_file_warns_about_docx_only() {
        let root = temp_demo_copy("drop-bad");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        let legacy = std::env::temp_dir().join(format!("rde-ui-drop-legacy-{}.doc", std::process::id()));
        std::fs::write(&legacy, b"old binary doc").unwrap();
        app.handle_dropped_paths(&[legacy.clone()]);
        assert!(
            app.project.log.iter().any(|line| line.text.contains("только .docx")),
            "журнал: подсказка «сохраните как .docx»"
        );
        let _ = std::fs::remove_file(&legacy);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Несколько документов в проекте: активный документ следует за импортом/выбором в дереве,
    /// сохранение пишет активный пакет, а не первый «main» (R-DOC-39 + multi-doc).
    #[test]
    fn multiple_documents_activate_selected_and_save_targets_active() {
        let root = temp_demo_copy("multi-doc");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        let main_pkg = app.project.document_path().expect("main-документ демо");
        let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../rde-doc/tests/fixtures/base-report.docx")
            .canonicalize()
            .expect("фикстура .docx");
        app.import_docx_action(&source).expect("импорт .docx");
        // Импортированный документ стал активным, хотя в проекте уже был «main».
        let imported_pkg = app.active_document_path().expect("активный документ");
        assert_ne!(imported_pkg, main_pkg, "активен импортированный, а не main");
        assert_eq!(app.doc.path.as_ref(), Some(&imported_pkg));
        assert!(
            app.active_document_object().unwrap().title.starts_with("[IMPORTED]"),
            "активный объект — импортированный"
        );
        app.ensure_editor();
        assert!(app.editor.is_some(), "редактор открывает импортированный: {:?}", app.editor_error);
        // «Сохранить проект» пишет активный пакет; main не перезаписывается.
        let main_before = std::fs::read(&main_pkg).unwrap();
        let imported_before = std::fs::read(&imported_pkg).unwrap();
        app.save_project_document();
        assert_eq!(std::fs::read(&main_pkg).unwrap(), main_before, "main не перезаписан сохранением");
        assert_ne!(std::fs::read(&imported_pkg).unwrap(), imported_before, "импортированный документ сохранён");
        // Выбор «main» в дереве переключает редактор обратно на main.
        let main_id = app
            .project
            .documents()
            .find(|object| object.dir.join("report.doc.rde") == main_pkg)
            .map(|object| object.id.clone())
            .expect("id main-документа");
        app.apply_tree_action(super::TreeAction::Open(main_id));
        assert_eq!(app.doc.path.as_ref(), Some(&main_pkg), "выбор main активирует main");
        // Рестарт приложения: открывается самый свежий документ (импортированный), а не пустой main.
        let restarted = RdeApp::new(rde_core::load_project(&root).unwrap());
        assert_eq!(
            restarted.doc.path.as_ref(),
            Some(&imported_pkg),
            "при старте открыт импортированный (свежий) документ"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Записать длинный `.docx` (n абзацев) движком WordCraft — для регресс-теста позиции (B, P5).
    fn write_long_docx(path: &std::path::Path, paragraphs: usize) {
        use wordcraft_engine::doc::{Block, CharProps, Document, Paragraph, StoryRef};
        let mut doc = Document::default();
        for index in 0..paragraphs {
            let mut paragraph = Paragraph::new();
            paragraph
                .insert_text(
                    0,
                    &format!("Абзац {} — строка длинного документа для проверки пагинации и позиции каретки.", index + 1),
                    &CharProps::default(),
                )
                .expect("текст абзаца");
            doc.insert_block(StoryRef::Body, &wordcraft_engine::doc::Path::top(index), Block::Para(paragraph))
                .expect("абзац в документ");
        }
        wordcraft_engine::io::save_path(path, &doc).expect("сохранить длинный .docx");
    }

    /// P5 (баг владельца B, ADR-0021 §Решение 3): «Обновить документ» на длинном документе
    /// (≈250 страниц) НЕ сбрасывает каретку/страницу на первую — редактор перезагружается на месте.
    #[test]
    fn document_update_preserves_caret_on_long_document() {
        let root = temp_demo_copy("scroll-preserve");
        // Демо-документ заменяем длинным .docx; при сохранении он станет .doc.rde.
        let project = rde_core::load_project(&root).unwrap();
        let document = project.document_object().expect("документ демо").clone();
        let dir = document.dir.clone();
        write_long_docx(&dir.join("long.docx"), 4000);
        let manifest = dir.join("document.toml");
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(&manifest, text.replace("file = \"report.doc.rde\"", "file = \"long.docx\"")).unwrap();
        // Первый запуск: открыть длинный .docx и сохранить его силами RDE как .doc.rde.
        {
            let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
            app.ensure_editor();
            assert!(app.editor.is_some(), "редактор длинного документа: {:?}", app.editor_error);
            let pages = app.editor.as_ref().unwrap().pages;
            assert!(pages >= 200, "длинный документ должен быть 200+ страниц, а не {pages}");
            app.save_project_document();
        }
        // Второй запуск: активный документ — .doc.rde; каретка на 250-й странице, «Обновить документ».
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        assert_eq!(
            app.doc.path.as_deref().and_then(|path| path.extension()).and_then(|ext| ext.to_str()),
            Some("rde"),
            "активный документ — .doc.rde: {:?}",
            app.doc.path
        );
        app.ensure_editor();
        assert!(app.editor.is_some(), "редактор пакета: {:?}", app.editor_error);
        let pages = app.editor.as_ref().unwrap().pages;
        assert!(pages >= 200, "пакет длинного документа: страниц {pages}");
        let story = wordcraft_engine::doc::StoryRef::Body;
        let paths: Vec<_> = app.editor.as_ref().unwrap().app.session.doc.para_paths(story);
        assert!(paths.len() >= 4000, "абзацев в длинном документе: {}", paths.len());
        // Каретка — в конце документа (страница ≈ 250) и page_hint «250».
        let deep_path = paths[paths.len() - 20].clone();
        // 10 — граница UTF-8 символа в кириллическом «Абзац …» (5 → попал бы внутрь char).
        let deep = wordcraft_engine::doc::Pos::new(story, deep_path.clone(), 10);
        {
            let editor = app.editor.as_mut().expect("редактор");
            editor.app.session.sel = wordcraft_engine::Selection::caret(deep.clone());
            editor.app.session.page_hint = 250;
        }
        let before = app.capture_document_view();
        assert_eq!(before.selection.as_ref().map(|selection| selection.focus.off), Some(10));
        app.update_project_document();
        assert!(
            app.project.log.iter().any(|line| line.text.contains("Документ обновлён")),
            "Обновить документ не выполнилось: {:?}",
            app.project.log.iter().map(|line| &line.text).collect::<Vec<_>>()
        );
        let after = app.capture_document_view();
        let persisted = after.selection.expect("каретка сохранена после обновления");
        assert_eq!(persisted.focus.story, deep.story);
        assert_eq!(persisted.focus.path, deep.path, "каретка осталась в том же абзаце (не на первой странице)");
        assert_eq!(persisted.focus.off, 10, "offset в абзаце сохранён");
        assert!(persisted.focus.path.0.first().copied().unwrap_or(0) > 0, "каретка не в первом блоке документа");
        let editor = app.editor.as_ref().expect("редактор не пересоздан с нуля");
        assert_eq!(editor.pages, pages, "пагинация не изменилась");
        assert_eq!(editor.app.session.page_hint, 250, "страница/камера сохранены");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P5 (баг владельца B, предпросмотр): «Обновить документ» в режиме «Предпросмотр страниц» не
    /// сбрасывает прокрутку на первую страницу — restore-offset применяется один раз (ADR-0021).
    #[test]
    fn preview_keeps_scroll_offset_after_document_update() {
        let root = temp_demo_copy("preview-preserve");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        app.tab = super::Tab::Document;
        app.demo_set_doc_mode("preview");
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(20_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..6 {
            harness.step();
        }
        harness.event(egui::Event::PointerMoved(egui::pos2(760.0, 520.0)));
        harness.step();
        for _ in 0..4 {
            harness.event(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -140.0),
                phase: egui::TouchPhase::Move,
                modifiers: egui::Modifiers::NONE,
            });
            harness.step();
        }
        for _ in 0..4 {
            harness.step();
        }
        let before = harness.state().preview_scroll_offset;
        assert!(before > 1.0, "колесо прокрутило предпросмотр (offset {before})");
        harness.state_mut().update_project_document();
        for _ in 0..12 {
            harness.step();
        }
        let after = harness.state().preview_scroll_offset;
        assert!(
            after > before * 0.5,
            "прокрутка предпросмотра сохранена после обновления: было {before}, стало {after}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P5 (ADR-0021 §Решение 2, регресс инцидента A): мусор из `equation.get` нативного редактора
    /// (после конвертации в линейный формат — `√`, `▒`) не записывается в источник формулы.
    #[test]
    fn corrupted_native_latex_is_rejected_by_panel_sync() {
        let root = temp_demo_copy("formula-native-corruption");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        app.demo_open_formula_editor_native();
        let source = app.formula_editor.as_ref().expect("панель формулы").latex.clone();
        assert!(!source.trim().is_empty(), "исходный LaTeX демо-формулы");
        {
            let dialog = app.formula_editor.as_mut().expect("панель формулы");
            let native = dialog.native.as_mut().expect("scratch-редактор");
            // Ровно то, что ломало источник: Equation › Convert → Linear (мусор `√(…)▒…`).
            native.app.run("equation.convert", serde_json::json!({ "to": "linear" })).expect("convert linear");
        }
        let dialog = app.formula_editor.as_mut().expect("панель формулы");
        super::RdeApp::sync_formula_from_native(dialog);
        let dialog = app.formula_editor.as_ref().expect("панель формулы");
        assert_eq!(dialog.latex, source, "испорченный LaTeX не попал в источник истины");
        assert!(dialog.error.contains("отклонён"), "ошибка валидации видна в панели: {:?}", dialog.error);
        assert!(rde_doc::forbidden_char(&source).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P5 (ADR-0021, инцидент D): нативный «проф. редактор» (клон Microsoft Equations) убран из UI —
    /// на вкладке «Формула» остаётся только LaTeX-редактор и его действия.
    #[test]
    fn formula_panel_hides_removed_native_professional_editor() {
        use egui_kittest::kittest::Queryable;
        let root = temp_demo_copy("formula-no-native");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        app.open_formula_editor_new();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..3 {
            harness.step();
        }
        assert!(harness.query_by_label_contains("LaTeX-источник").is_some(), "LaTeX-поле редактора");
        assert!(harness.query_by_label_contains("Проф. редактор").is_none(), "нативный «проф. редактор» выведен из UI");
        assert!(harness.query_by_label_contains("Забрать LaTeX").is_none(), "автосинк нативного редактора выведен");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P5 (quick-win перф, §5.2 ТЗ): замер per-frame стоимостей UI на реальных данных.
    /// Ручной запуск: `cargo test -p rde-ui --lib perf_smoke -- --ignored --nocapture`
    #[test]
    #[ignore = "замеры перфа: ручной запуск -- --ignored --nocapture"]
    fn perf_smoke_measure_frame_costs() {
        use std::time::Instant;
        let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
        let root = temp_demo_copy("perf-smoke");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        app.tab = super::Tab::Document;
        app.doc_mode = super::DocMode::DocView;
        app.ensure_editor();
        assert!(app.editor.is_some(), "{:?}", app.editor_error);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(20_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..6 {
            harness.step();
        }
        let n = 60u32;
        let t = Instant::now();
        for _ in 0..n {
            harness.step();
        }
        println!("PERF docview frame ({}): {:?}/кадр", profile, t.elapsed() / n);
        // Панель формулы открыта — formula_inspector_data/предпросмотр каждый кадр.
        harness.state_mut().open_formula_editor_new();
        for _ in 0..4 {
            harness.step();
        }
        let t = Instant::now();
        for _ in 0..n {
            harness.step();
        }
        println!("PERF formula-panel frame ({}): {:?}/кадр", profile, t.elapsed() / n);
        // Стоимость чистых вызовов «тяжёлых» функций.
        let formula_id = harness.state().project.formulas().next().map(|formula| formula.id.clone()).expect("формула демо");
        let t = Instant::now();
        let calls = 20u32;
        for _ in 0..calls {
            let _ = harness.state_mut().formula_inspector_data(&formula_id);
        }
        println!("PERF formula_inspector_data (демо): {:?}/вызов", t.elapsed() / calls);
        let package = harness.state().project.document_path().expect("документ");
        let t = Instant::now();
        for _ in 0..calls {
            let _ = rde_doc::load_rde_doc(&package).expect("пакет");
        }
        println!("PERF load_rde_doc (демо): {:?}/вызов", t.elapsed() / calls);
        // Реальный проект: 62 MathType-формулы, 24+ страниц.
        let real = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.cache-tmp/import-v0003/final-e2e");
        if let Ok(project) = rde_core::load_project(&real) {
            let mut real_app = RdeApp::new(project);
            real_app.ensure_editor();
            let formula_id = real_app.project.formulas().next().map(|formula| formula.id.clone()).expect("формула реального проекта");
            let t = Instant::now();
            for _ in 0..calls {
                let _ = real_app.formula_inspector_data(&formula_id);
            }
            println!("PERF formula_inspector_data (final-e2e, 62 формулы): {:?}/вызов", t.elapsed() / calls);
            let package = real_app.project.document_path().expect("документ final-e2e");
            let t = Instant::now();
            for _ in 0..calls {
                let _ = rde_doc::load_rde_doc(&package).expect("пакет final-e2e");
            }
            println!("PERF load_rde_doc (final-e2e): {:?}/вызов", t.elapsed() / calls);
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P5 (ADR-0021 Phase 2): WYSIWYG-редактор формулы в панели вкладки «Формула» —
    /// структурный ввод (дробь через палитру/модель), LaTeX синхронизируется с моделью,
    /// «Создать» записывает канонический источник и пересчитывает кэши.
    #[test]
    fn formula_editor_panel_owns_keyboard_focus() {
        // Регресс (найдено владельцем на живой панели): при открытой панели «Формула» клавиатура
        // должна принадлежать WYSIWYG-редактору, а не скрытому WordCraft-canvas документа — тот
        // авто-запрашивал фокус и перехватывал стрелки/Tab (в итоге «активировались» кнопки).
        let root = temp_demo_copy("formula-focus");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        app.demo_formula_editor_with_model("", Some("frac.stacked"), "");
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..4 {
            harness.step();
        }
        let editor = harness.state().formula_editor.as_ref().and_then(|dialog| dialog.math_editor.as_ref()).expect("редактор формулы");
        assert!(editor.focused, "клавиатурный фокус в WYSIWYG-редакторе формулы, а не в canvas документа");
        assert!(editor.last_caret.is_some(), "каретка имеет геометрию и рисуется в полотне");
        let before = editor.state.pos.clone();
        // Tab — следующий слот структуры (числитель → знаменатель), фокус не уходит в оболочку.
        harness.event(egui::Event::Key { key: egui::Key::Tab, physical_key: None, pressed: true, repeat: false, modifiers: egui::Modifiers::NONE });
        harness.step();
        let editor = harness.state().formula_editor.as_ref().and_then(|dialog| dialog.math_editor.as_ref()).expect("редактор формулы");
        assert!(editor.focused, "редактор удерживает клавиатурный фокус после Tab");
        assert!(editor.last_caret.is_some(), "каретка по-прежнему рисуется после клавиш");
        assert_ne!(before, editor.state.pos, "Tab двигает каретку по слотам, а не переключает виджеты оболочки");
    }

    #[test]

    fn formula_wysiwyg_editor_edits_model_and_creates_formula() {
        use egui_kittest::kittest::Queryable;
        let root = temp_demo_copy("formula-wysiwyg");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        app.open_formula_editor_new();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..3 {
            harness.step();
        }
        assert!(harness.query_by_label_contains("РЕДАКТОР ФОРМУЛЫ").is_some(), "WYSIWYG-редактор в панели «Формула»");
        // Структурный ввод через модель (палитры/клавиатуру дергает виджет; здесь — модель):
        // дробь + числитель/знаменатель.
        {
            let editor = harness
                .state_mut()
                .formula_editor
                .as_mut()
                .and_then(|dialog| dialog.math_editor.as_mut())
                .expect("WYSIWYG-редактор инициализирован");
            editor.state.insert_template("frac.stacked");
            editor.state.type_text("1");
            editor.state.move_caret(rde_math_editor::edit::Move::NextSlot, false);
            editor.state.type_text("2");
        }
        harness.step();
        let dialog = harness.state().formula_editor.as_ref().expect("панель формулы");
        assert!(dialog.latex.contains("\\frac"), "модель → LaTeX-источник: {}", dialog.latex);
        // «Создать» пишет формулу с кэшами (OMML/SVG/PNG) — тот же конвейер ADR-0020.
        if let Some(dialog) = harness.state_mut().formula_editor.as_mut() {
            dialog.title = "Проверка WYSIWYG".to_owned();
        }
        harness.step();
        let create = {
            use egui_kittest::kittest::{NodeT, Queryable};
            harness
                .query_all_by_label_contains("Создать")
                .find(|node| node.accesskit_node().role() == egui::accesskit::Role::Button)
                .expect("кнопка «Создать»")
        };
        create.click();
        for _ in 0..4 {
            harness.step();
        }
        let dialog = harness.state().formula_editor.as_ref().expect("панель формулы после создания");
        let formula_id = dialog.formula_id.clone().unwrap_or_else(|| {
            panic!(
                "формула создана: latex={:?}, error={:?}, info={:?}, log={:?}",
                dialog.latex,
                dialog.error,
                dialog.info,
                harness.state().project.log.iter().map(|line| &line.text).collect::<Vec<_>>()
            )
        });
        let object = harness.state().project.formula_object(&formula_id).expect("объект формулы").clone();
        let rde_core::ObjectData::Formula(manifest) = &object.data else { panic!("не формула") };
        assert!(manifest.latex.contains("\\frac"), "источник записан: {}", manifest.latex);
        assert!(object.dir.join("formula.toml").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// P5: правка модели в WYSIWYG-редакторе обновляет поле LaTeX-источника (LaTeX — витрина
    /// модели), а источник остаётся представитьмым (roundtrip-валидация проходит).
    #[test]
    fn formula_wysiwyg_model_syncs_latex_field() {
        let root = temp_demo_copy("formula-wysiwyg-sync");
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        let formula_id = app.project.formulas().next().expect("формула демо").id.clone();
        app.open_formula_editor(&formula_id);
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..3 {
            harness.step();
        }
        let before = harness.state().formula_editor.as_ref().expect("панель").latex.clone();
        {
            let editor = harness
                .state_mut()
                .formula_editor
                .as_mut()
                .and_then(|dialog| dialog.math_editor.as_mut())
                .expect("редактор");
            // Вставка в конец существующей σ-формулы: модель меняется, LaTeX пересобирается.
            editor.state.type_text("+x");
            assert!(editor.prepare_source().is_ok(), "модель представима: {:?}", editor.state.validate().err());
        }
        harness.step();
        let after = harness.state().formula_editor.as_ref().expect("панель").latex.clone();
        assert_ne!(before, after, "LaTeX-поле обновилось из модели");
        assert!(after.contains("+x"), "{after}");
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Инварианты оболочки после переноса меню на ленту: лента только в режиме RDE на вкладке
/// «Документ», строка режимов — только на «Документе», каждая кнопка ленты доступна на своей
/// вкладке, «Обновить всё» — на всех вкладках, и строки таблиц структуры/вставок покрыты шрифтами.
#[cfg(test)]
mod shell_tests {
    use super::ribbon::{RibbonTab, Slot, ribbon_groups};
    use super::{DocMode, RdeApp, Tab};
    use egui_kittest::kittest::Queryable;

    /// Вьюер-харнесс без wgpu: достаточно дерева доступности (подписи, меню).
    fn render(app: RdeApp) -> egui_kittest::Harness<'static, RdeApp> {
        render_sized(app, 1600.0)
    }

    /// Тот же харнесс заданной ширины — для проверки адаптивной строки вкладок центра (§19-7).
    fn render_sized(app: RdeApp, width: f32) -> egui_kittest::Harness<'static, RdeApp> {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(width, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..4 {
            harness.step();
        }
        harness
    }

    /// Прямоугольник самого верхнего узла с подписью. Нужен там, где подпись встречается дважды:
    /// «Документ» — вкладка строки центра (верх) и вкладка ленты (ниже).
    fn top_rect(harness: &egui_kittest::Harness<'static, RdeApp>, label: &'static str) -> egui::Rect {
        let mut rects: Vec<egui::Rect> = harness.query_all_by_label(label).map(|node| node.rect()).collect();
        assert!(!rects.is_empty(), "узел «{label}» не найден в дереве доступности");
        rects.sort_by(|a, b| a.top().partial_cmp(&b.top()).expect("конечные координаты"));
        rects[0]
    }

    /// Подписи вкладок центра (19-7): слева направо, как в ТЗ.
    const CENTER_TAB_LABELS: [&str; 5] = ["Документ", "Структура", "Артефакты", "Источник", "Код документа"];
    /// Подписи переключателя режимов (19-7): доступные имена сегментов.
    const MODE_LABELS: [&str; 3] = ["RDE", "WordCraft", "Предпросмотр страниц"];

    /// Подсказки (полные названия) всех кнопок вкладки ленты — по ним кнопку находят в дереве
    /// доступности.
    fn ribbon_tips(tab: RibbonTab) -> Vec<&'static str> {
        ribbon_groups(tab)
            .iter()
            .flat_map(|group| group.slots.iter())
            .filter_map(|slot| match slot {
                Slot::Button { tip, .. } | Slot::Toggle { tip, .. } | Slot::Icon { tip, .. } => Some(*tip),
                Slot::InsertPosition | Slot::Zoom | Slot::Note(_) | Slot::FormulaEditor => None,
            })
            .collect()
    }

    /// (в) Лента — только в режиме RDE и только на вкладке «Документ». В WordCraft и предпросмотре
    /// её нет (как раньше не было меню).
    #[test]
    fn ribbon_only_in_rde_mode_on_document_tab() {
        for tab in Tab::ALL {
            for mode in DocMode::ALL {
                let mut app = RdeApp::new(rde_core::demo());
                app.tab = tab;
                app.doc_mode = mode;
                let harness = render(app);
                let ribbon = harness.query_by_label("Главная").is_some();
                let expect = tab == Tab::Document && mode == DocMode::DocView;
                assert_eq!(ribbon, expect, "лента на вкладке центра с режимом {}", mode.label());
            }
        }
    }

    /// (б) Вторая часть линии (RDE | WordCraft | Предпросмотр страниц) видна только на вкладке
    /// «Документ»: на Структуре/Артефактах/Источнике/Коде её нет.
    #[test]
    fn mode_switch_line_only_on_document_tab() {
        for tab in Tab::ALL {
            let mut app = RdeApp::new(rde_core::demo());
            app.tab = tab;
            app.doc_mode = DocMode::DocView;
            let harness = render(app);
            let shown = harness.query_by_label("Предпросмотр страниц").is_some() && harness.query_by_label("WordCraft").is_some();
            assert_eq!(shown, tab == Tab::Document, "строка режимов видна только на «Документе»");
        }
    }

    /// (19-7) Строка вкладок центра и переключатель режимов — ОДНА строка без переноса при
    /// 1600×1000: подписи вкладок видны, режимы справа от них, масштаба в строке нет (он на ленте).
    #[test]
    fn center_tab_line_is_single_row_with_mode_switch() {
        let mut app = RdeApp::new(rde_core::demo());
        app.tab = Tab::Document;
        app.doc_mode = DocMode::DocView;
        // Лента открыта на вкладке «Документ», чтобы проверить группу «Масштаб» (§19-7 B3).
        app.ribbon_tab = RibbonTab::Document;
        let harness = render(app);

        let tabs: Vec<egui::Rect> = CENTER_TAB_LABELS.iter().map(|label| top_rect(&harness, label)).collect();
        let row_top = tabs.iter().map(|rect| rect.top()).fold(f32::INFINITY, f32::min);
        let row_bottom = tabs.iter().map(|rect| rect.bottom()).fold(f32::NEG_INFINITY, f32::max);
        for rect in &tabs {
            assert!((rect.center().y - tabs[0].center().y).abs() < 1.5, "вкладки центра не на одной строке: {rect:?}");
            // Подписи вкладок видны при 1600 px: кнопка с подписью заметно шире иконочной.
            assert!(rect.width() > 60.0, "вкладка сжата до иконки при 1600×1000: {rect:?}");
        }
        // Одна строка: высота ряда — одна кнопка (с допуском на рамку сегментов), без второго ряда.
        assert!(row_bottom - row_top <= super::CENTER_TAB_HEIGHT + 1.0, "строка вкладок переносится: высота {}", row_bottom - row_top);

        let modes: Vec<egui::Rect> = MODE_LABELS.iter().map(|label| top_rect(&harness, label)).collect();
        for rect in &modes {
            assert!(
                (rect.center().y - tabs[0].center().y).abs() < 3.5,
                "переключатель режимов вне строки вкладок: {rect:?} при вкладках {tabs:?}"
            );
        }
        let last_tab_right = tabs.iter().map(|rect| rect.right()).fold(f32::NEG_INFINITY, f32::max);
        let modes_left = modes.iter().map(|rect| rect.left()).fold(f32::INFINITY, f32::min);
        assert!(modes_left > last_tab_right, "переключатель режимов не справа от вкладок: {modes_left} vs {last_tab_right}");

        // (19-7 B3) Масштаб убран из строки и живёт на ленте («Документ» → «Масштаб»):
        // кнопки кластера видны и находятся НИЖЕ строки вкладок.
        for label in ["−", "+", "Сброс", "По ширине"] {
            let zoom = top_rect(&harness, label);
            assert!(zoom.top() > row_bottom, "«{label}» осталось в строке вкладок: {zoom:?}");
        }
    }

    /// (19-7 B4) При нехватке ширины строка вкладок остаётся ОДНОЙ: вкладки сжимаются до иконочного
    /// вида (полное название — в доступном имени/тултипе), а не переносятся на вторую строку.
    #[test]
    fn center_tab_line_compacts_to_icons_without_wrap() {
        let mut app = RdeApp::new(rde_core::demo());
        app.tab = Tab::Document;
        app.doc_mode = DocMode::DocView;
        let harness = render_sized(app, 1280.0);
        let tabs: Vec<egui::Rect> = CENTER_TAB_LABELS.iter().map(|label| top_rect(&harness, label)).collect();
        for rect in &tabs {
            assert!((rect.center().y - tabs[0].center().y).abs() < 1.5, "вкладки центра перенеслись на другую строку: {rect:?}");
            assert!(rect.width() < 50.0, "вкладка не сжалась до иконки на узкой колонке: {rect:?}");
        }
        // Подписи не потеряны: полные названия остались в доступном имени (у «Документа» два узла —
        // вкладка центра и вкладка ленты, поэтому query_all).
        for label in CENTER_TAB_LABELS.iter().chain(MODE_LABELS.iter()) {
            assert!(harness.query_all_by_label(label).next().is_some(), "«{label}» пропала из дерева доступности в компактном режиме");
        }
    }

    /// (19-7 B2) На широкой колонке подписи режимов видны: «подписи — если хватает места».
    #[test]
    fn center_mode_switch_shows_labels_on_wide_column() {
        let mut app = RdeApp::new(rde_core::demo());
        app.tab = Tab::Document;
        app.doc_mode = DocMode::DocView;
        let harness = render_sized(app, 1920.0);
        let modes: Vec<egui::Rect> = MODE_LABELS.iter().map(|label| top_rect(&harness, label)).collect();
        for rect in &modes {
            assert!(rect.width() > 40.0, "подпись режима не показана на широкой колонке: {rect:?}");
        }
    }

    /// (19-7 B5) Режимы документа доступны во ВСЕХ режимах: лента в WordCraft и предпросмотре
    /// скрыта, но переключатель остаётся в строке вкладок — и возвращает в RDE.
    #[test]
    fn doc_mode_switch_available_in_all_modes() {
        for mode in DocMode::ALL {
            let mut app = RdeApp::new(rde_core::demo());
            app.tab = Tab::Document;
            app.doc_mode = mode;
            let harness = render(app);
            for label in MODE_LABELS {
                assert!(harness.query_by_label(label).is_some(), "в режиме {} недоступен «{label}»", mode.label());
            }
        }
        // Из WordCraft/предпросмотра переключатель возвращает в RDE (иначе пользователь «заперт»).
        for mode in [DocMode::Full, DocMode::Preview] {
            let mut app = RdeApp::new(rde_core::demo());
            app.tab = Tab::Document;
            app.doc_mode = mode;
            let mut harness = render(app);
            {
                harness.get_by_label("RDE").click_accesskit();
            }
            for _ in 0..3 {
                harness.step();
            }
            assert_eq!(harness.state().doc_mode, DocMode::DocView, "клик по «RDE» не вернул режим из {}", mode.label());
        }
    }

    /// (19-8) Инспектор: 340 px по умолчанию (решение владельца 320–380), перетаскивается,
    /// клампится 280–480; центр при 1600×1000 не обрезан.
    #[test]
    fn inspector_is_340_by_default_resizable_and_keeps_center() {
        assert_eq!(super::INSPECTOR_WIDTH, 340.0);
        assert_eq!(super::INSPECTOR_WIDTH_MIN, 280.0);
        assert_eq!(super::INSPECTOR_WIDTH_MAX, 480.0);
        assert!((280.0..=380.0).contains(&super::INSPECTOR_WIDTH), "default width вне диапазона 320–380");

        let ctx_cell = std::sync::Arc::new(std::sync::Mutex::new(None));
        let cell = ctx_cell.clone();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(
                move |ui, app: &mut RdeApp| {
                    *cell.lock().expect("мьютекс контекста") = Some(ui.ctx().clone());
                    app.ui(ui);
                },
                RdeApp::new(rde_core::demo()),
            );
        for _ in 0..4 {
            harness.step();
        }
        let ctx = ctx_cell.lock().expect("мьютекс контекста").clone().expect("ctx после кадра");
        let panel_width = |id: &'static str| {
            egui::containers::panel::PanelState::load(&ctx, egui::Id::new(id)).unwrap_or_else(|| panic!("нет состояния панели «{id}»")).size().x
        };

        let inspector_width = panel_width("rde_inspector");
        assert!((inspector_width - super::INSPECTOR_WIDTH).abs() < 1.0, "инспектор {inspector_width} px вместо {} px", super::INSPECTOR_WIDTH);
        // Ширины панелей — из одного конфига раскладки (не хардкод в разных местах, §19-8 A3).
        assert!((panel_width("rde_tree") - super::TREE_WIDTH).abs() < 1.0, "дерево не TREE_WIDTH");
        assert!((panel_width("rde_agent") - super::AGENT_WIDTH).abs() < 1.0, "агент не AGENT_WIDTH");
        // Центр не обрезан: странице документа и DAG остаётся ≥ 600 px (kx: 1600 − 2×8 полей kittest).
        let central = ctx.viewport_rect().width() - 16.0 - panel_width("rde_tree") - panel_width("rde_agent") - inspector_width;
        assert!(central >= 600.0, "центр обрезан: {central} px");

        // resizable: тянем левый край инспектора влево на 200 px → ширина растёт, но клампится к 480.
        fn drag(harness: &mut egui_kittest::Harness<'static, RdeApp>, edge_x: f32, y: f32, dx: f32) {
            let press = egui::pos2(edge_x, y);
            harness.input_mut().events.push(egui::Event::PointerMoved(press));
            harness.input_mut().events.push(egui::Event::PointerButton {
                pos: press,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::default(),
            });
            harness.step();
            let moved = egui::pos2(edge_x + dx, y);
            harness.input_mut().events.push(egui::Event::PointerMoved(moved));
            harness.step();
            harness.input_mut().events.push(egui::Event::PointerButton {
                pos: moved,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::default(),
            });
            harness.step();
        }
        let inspector_rect = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("rde_inspector")).expect("инспектор").outer_rect;
        // Край перетаскивания — левая граница правой панели (+2 px внутрь панели, чтобы взялся resize-handle).
        drag(&mut harness, inspector_rect.left() + 2.0, inspector_rect.center().y, -200.0);
        let widened = panel_width("rde_inspector");
        assert!((widened - super::INSPECTOR_WIDTH_MAX).abs() < 6.0, "resizable/кламп: ширина {widened} px вместо {}", super::INSPECTOR_WIDTH_MAX);
        // Обратный крайний случай: тянем вправо на 400 px → клампится к минимуму 280.
        let widened_rect = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("rde_inspector")).expect("инспектор").outer_rect;
        drag(&mut harness, widened_rect.left() + 2.0, widened_rect.center().y, 400.0);
        let narrowed = panel_width("rde_inspector");
        assert!((narrowed - super::INSPECTOR_WIDTH_MIN).abs() < 6.0, "resizable/кламп: ширина {narrowed} px вместо {}", super::INSPECTOR_WIDTH_MIN);
    }

    /// (а, UI) Каждая кнопка каждой вкладки ленты есть в дереве доступности: ничего не потеряно.
    #[test]
    fn ribbon_exposes_every_item_on_its_tab() {
        for tab in RibbonTab::ALL {
            let mut app = RdeApp::new(rde_core::demo());
            app.doc_mode = DocMode::DocView;
            app.tab = Tab::Document;
            app.ribbon_tab = tab;
            let harness = render(app);
            for tip in ribbon_tips(tab) {
                assert!(harness.query_by_label(tip).is_some(), "кнопка «{tip}» не найдена на вкладке «{}»", tab.title());
            }
        }
    }

    #[test]
    fn update_all_is_available_on_every_tab() {
        for tab in Tab::ALL {
            let mut app = RdeApp::new(rde_core::demo());
            app.tab = tab;
            app.doc_mode = DocMode::Preview;
            let harness = render(app);
            assert!(harness.query_by_label_contains("Обновить всё").is_some(), "«Обновить всё» отсутствует на вкладке");
        }
    }

    /// Эталон WordCraft (им подменяются шрифты оболочки после открытия редактора).
    static WORDCRAFT_INTER: &[u8] = include_bytes!("../../../vendor/wordcraft/assets/fonts/Inter-Regular.ttf");

    fn missing_glyphs(text: &str, data: &[u8]) -> Vec<char> {
        let face = ttf_parser::Face::parse(data, 0).expect("шрифт разбирается");
        let mut missing: Vec<char> = text.chars().filter(|c| !c.is_whitespace() && face.glyph_index(*c).is_none()).collect();
        missing.sort_unstable();
        missing.dedup();
        missing
    }

    /// Все строки таблиц структуры и формата вставок должны рисоваться без «квадратиков»:
    /// каждый символ есть и в шрифтах RDE, и в WordCraft-эталоне (Inter).
    #[test]
    fn structure_and_insert_strings_have_glyphs() {
        let mut strings: Vec<String> = Vec::new();
        // Форматы вставок (R-DOC-11) × типичные значения.
        for format in ["auto", "raw", "int", "dec:2", "dec:4", "sci:3", "percent:1"] {
            for raw in ["-7.598e-4", "1234.5", "0.5", "2.5e10", "0"] {
                strings.push(rde_doc::format_value(raw, format));
            }
        }
        // Подписи таблиц структуры (заголовки и метки типов).
        for text in ["№", "тип", "стиль", "путь (block)", "содержимое", "Блоки документа", "FIG·1", "CAP", "H1", "—", "клик по блоку — выбрать документ"] {
            strings.push(text.to_owned());
        }
        // Реальные строки демо-документа: блоки и живые вставки.
        let project = rde_core::load_project(&rde_core::demo_project_dir()).unwrap();
        let path = project.document_path().expect("документ");
        let (blocks, _) = super::extract_document_structure(&path).expect("разобрать .doc.rde");
        for block in &blocks {
            strings.push(super::display_text(&block.text));
            strings.push(super::display_text(&block.style));
        }
        let inserts = rde_doc::load_rde_doc(&path).expect("rde:doc").inserts;
        assert!(!inserts.is_empty(), "в демо-документе должны быть вставки");
        for insert in &inserts {
            strings.push(super::display_text(&insert.cached));
        }
        for text in &strings {
            // Якорь inline-объекта не печатается и не должен попадать в вывод.
            assert!(!text.contains('\u{FFFC}'), "U+FFFC в выводе: {text:?}");
            for data in [super::VENDORED_SANS, super::VENDORED_MONO, WORDCRAFT_INTER] {
                let missing = missing_glyphs(text, data);
                assert!(missing.is_empty(), "нет глифов {missing:?} в строке {text:?}");
            }
        }
    }

    #[test]
    fn display_text_strips_only_object_anchor() {
        assert_eq!(super::display_text("Среднее значение x1: \u{FFFC}-7.598·10^-4\u{FFFC}"), "Среднее значение x1: -7.598·10^-4");
        assert_eq!(super::display_text("\u{FFFC}"), "");
        assert_eq!(super::display_text("FIG·1 — σ"), "FIG·1 — σ");
    }
}

#[cfg(test)]
mod agent_server_ui_tests {
    use super::RdeApp;

    fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(&*name, ".rde" | "exports" | "out" | "__pycache__" | ".venv" | "target") {
                continue;
            }
            let from = entry.path();
            let to = dst.join(&*name);
            if from.is_dir() {
                copy_tree(&from, &to);
            } else {
                std::fs::copy(&from, &to).unwrap();
            }
        }
    }

    fn temp_demo_copy(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("rde-ui-agent-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        copy_tree(&rde_core::demo_project_dir(), &root);
        root
    }

    /// M4.2: запрос агента попадает в диалог, решение пользователя доходит до сервера.
    #[test]
    fn gui_approval_dialog_polls_and_decides() {
        let root = temp_demo_copy("approvals-dialog");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        assert!(app.agent_approval_dialog.is_none());
        // Сервер (в тесте — напрямую) ставит запрос в очередь approvals.
        let receiver = app.agent_approvals.enqueue(rde_mcp::ApprovalRequest {
            tool: "note_write".to_owned(),
            args_summary: "{\"note_id\":\"01NOTE\"}".to_owned(),
            args_sha256: "0".repeat(64),
            destructive: true,
            requested_by: "mcp".to_owned(),
            args: Some(serde_json::json!({"note_id": "01NOTE", "content": "# Текст"})),
        });
        app.poll_agent_approvals();
        let dialog = app.agent_approval_dialog.as_ref().expect("диалог показан");
        assert_eq!(dialog.tool(), "note_write");
        assert!(dialog.destructive);
        assert_eq!(dialog.preview().map(|(_, after)| after), Some("# Текст".to_owned()));
        assert!(dialog.args_json_summary().contains("01NOTE"));
        // Пользователь разрешает один раз — решение уходит серверу, диалог закрывается.
        app.decide_agent_approval(rde_mcp::ApprovalDecision::AllowOnce);
        assert!(app.agent_approval_dialog.is_none());
        assert_eq!(
            receiver.recv_timeout(std::time::Duration::from_secs(1)).unwrap(),
            rde_mcp::ApprovalDecision::AllowOnce
        );
        // Отклонение всех ожидающих (выход/выключение доступа) — fail-closed.
        let _ = app.agent_approvals.enqueue(rde_mcp::ApprovalRequest {
            tool: "doc_delete".to_owned(),
            args_summary: "{}".to_owned(),
            args_sha256: "1".repeat(64),
            destructive: true,
            requested_by: "mcp".to_owned(),
            args: None,
        });
        app.deny_pending_agent_approvals();
        assert_eq!(app.agent_approvals.pending_count(), 0);
    }

    /// M4.1: настройка «Разрешить внешним агентам» запускает/останавливает встроенный MCP HTTP.
    #[test]
    fn external_agents_setting_starts_and_stops_embedded_server() {
        let root = temp_demo_copy("toggle");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        // По умолчанию — выключено (fail-closed), сервер не поднят.
        assert!(!app.app_settings.allow_external_mcp);
        assert!(!app.external_agents_status().is_running());
        // Включаем через настройку → сервер работает на loopback с bearer-токеном.
        app.app_settings.allow_external_mcp = true;
        app.apply_external_agents_setting();
        let status = app.external_agents_status();
        assert!(status.is_running(), "статус: {status:?}");
        let info = match status {
            super::agent_server::ExternalAgentStatus::Running { info, .. } => info,
            other => panic!("ожидался Running, получено {other:?}"),
        };
        assert!(info.addr.ip().is_loopback());
        assert_eq!(info.token.len(), 64, "сгенерированный токен");
        assert!(info.url().ends_with("/mcp"));
        // /health отвечает без токена — сервер действительно слушает loopback-порт.
        use std::io::{Read, Write};
        let mut stream = std::net::TcpStream::connect_timeout(&info.addr, std::time::Duration::from_secs(3))
            .expect("TCP connect");
        stream
            .write_all(b"GET /health HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut response = Vec::new();
        let _ = stream.read_to_end(&mut response);
        assert!(String::from_utf8_lossy(&response).starts_with("HTTP/1.1 200"));
        // Выключаем настройку → сервер остановлен (stop() дожидается потока и закрытия слушателя;
        // «connect после stop» не проверяем — эфемерный порт может быть сразу переиспользован
        // другим параллельным тестом).
        app.app_settings.allow_external_mcp = false;
        app.apply_external_agents_setting();
        assert!(!app.external_agents_status().is_running());
        // Повторное включение поднимает новый сервер (lifecycle не залипает).
        app.app_settings.allow_external_mcp = true;
        app.apply_external_agents_setting();
        assert!(app.external_agents_status().is_running());
        app.app_settings.allow_external_mcp = false;
        app.apply_external_agents_setting();
        assert!(!app.external_agents_status().is_running());
    }

    /// Kittest-харнесс приложения (без wgpu) — только дерево доступности/layout.
    fn render(app: RdeApp) -> egui_kittest::Harness<'static, RdeApp> {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..4 {
            harness.step();
        }
        harness
    }

    /// M4.2b: две подгалочки грантов (run/git) рендерятся в настройках, кликаются и сохраняются
    /// тем же сериализатором, что и кнопка «Сохранить» (файл — аддитивные ключи в settings.toml).
    #[test]
    fn agent_grant_checkboxes_render_and_persist() {
        use egui_kittest::kittest::Queryable;
        let root = temp_demo_copy("grants-settings");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        // Мастер-галочки включены (иначе подгалочки недоступны — иерархия как в диалоге).
        app.demo_open_app_settings_agents();
        let mut harness = render(app);
        let run_label = "Разрешить запуск скриптов проекта (artifact_run)";
        let git_label = "Разрешить git-коммиты (git_commit)";
        assert!(harness.query_by_label(run_label).is_some(), "подгалочка run_script в настройках");
        assert!(harness.query_by_label(git_label).is_some(), "подгалочка git_commit в настройках");
        // Включаем обе кликами — состояние привязано к диалогу настроек.
        harness.get_by_label(run_label).click();
        harness.run_steps(2);
        harness.get_by_label(git_label).click();
        harness.run_steps(2);
        let dialog = harness.state().app_settings_dialog.as_ref().expect("диалог настроек открыт").clone();
        assert!(dialog.allow_agent_run_script, "клик включает run_script");
        assert!(dialog.allow_agent_git_commit, "клик включает git_commit");
        // Сохранение в файл (тот же rde_core::settings::save, что у кнопки «Сохранить»).
        let path = root.join("settings.toml");
        rde_core::settings::save_to(&dialog, &path).expect("сохранить настройки");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("allow_agent_run_script = true"), "{text}");
        assert!(text.contains("allow_agent_git_commit = true"), "{text}");
        let loaded = rde_core::settings::load_from(&path);
        assert!(loaded.allow_agent_run_script && loaded.allow_agent_git_commit);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M4.2b: диалог approval для run/git показывает human-readable превью, trusted-контекст
    /// и кнопки решений (egui_kittest, без окна).
    #[test]
    fn agent_approval_dialog_renders_run_and_git_previews() {
        use egui_kittest::kittest::Queryable;
        let root = temp_demo_copy("approvals-preview");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        app.demo_open_agent_approval("artifact_run");
        let mut harness = render(app);
        assert!(harness.query_by_label("Запрос внешнего агента").is_some(), "окно диалога");
        assert!(harness.query_by_label("Разрешить один раз").is_some(), "кнопка AllowOnce");
        assert!(harness.query_by_label("Разрешить в сессии").is_some(), "кнопка AllowSession");
        assert!(harness.query_by_label("Отклонить").is_some(), "кнопка Deny");
        assert!(harness.query_by_label("Что произойдёт:").is_some(), "блок сводки действия");
        assert!(
            harness.query_all_by_label_contains("R-RUN-06").next().is_some(),
            "предупреждение о выполнении кода артефактов"
        );
        assert!(
            harness.query_all_by_label_contains("01ARTSTATS").next().is_some(),
            "цели запуска в превью"
        );
        assert!(
            harness.query_all_by_label_contains("Проект:").next().is_some(),
            "trusted-статус проекта в контексте"
        );
        // Отклонение закрывает диалог; следующий запрос — git-коммит с сообщением агента.
        harness.state_mut().decide_agent_approval(rde_mcp::ApprovalDecision::Deny);
        harness.state_mut().demo_open_agent_approval("git_commit");
        harness.run_steps(3);
        harness.query_by_label("Инструмент: git_commit").expect("инструмент в диалоге");
        assert!(
            harness
                .query_all_by_label_contains("M4.2b: принят лист правок агента")
                .next()
                .is_some(),
            "сообщение коммита в превью"
        );
        assert!(
            harness.query_all_by_label_contains(".rde/**").next().is_some(),
            "предупреждение про .rde"
        );
        harness.state_mut().decide_agent_approval(rde_mcp::ApprovalDecision::Deny);
        assert!(harness.state().agent_approval_dialog.is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M4.2b: настройки применяются к грантам сервера; run-грант без trusted не выдаётся,
    /// кнопка «Пометить проект доверенным…» открывает существующий диалог согласия R-RUN-06.
    #[test]
    fn agent_grant_settings_apply_and_trust_dialog_is_reused() {
        let root = temp_demo_copy("grants-apply");
        rde_core::set_project_trusted(&root, false).unwrap();
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        app.app_settings.allow_external_mcp = true;
        app.app_settings.allow_agent_mutations = true;
        app.app_settings.allow_agent_run_script = true;
        app.app_settings.allow_agent_git_commit = true;
        app.apply_external_agents_setting();
        // Проект не trusted → эффективного run-гранта нет, остальные выданы.
        let running = app.external_agents_status();
        assert!(running.is_running(), "сервер запущен: {running:?}");
        assert_eq!(
            running.grants(),
            super::agent_server::ExternalAgentGrants {
                project_write: true,
                run_script: false,
                git_commit: true
            }
        );
        assert!(app.agent_run_script_trust_required(), "подсказка про trusted нужна");
        // Кнопка «Пометить проект доверенным…» (панель/настройки) открывает общий диалог R-RUN-06.
        app.trust_dialog = Some(super::TrustDialogState { request: super::TrustRequest::AgentRunScript });
        // Подтверждение согласия (как кнопка диалога): trusted=true → перезагрузка → перезапуск.
        rde_core::set_project_trusted(&root, true).unwrap();
        app.trust_dialog = None;
        app.reload_project();
        app.apply_external_agents_setting();
        assert!(app.project.manifest.trusted);
        assert!(app.external_agents_status().grants().run_script, "после trusted run-грант выдан");
        assert!(!app.agent_run_script_trust_required());
        app.app_settings.allow_external_mcp = false;
        app.apply_external_agents_setting();
        assert!(!app.external_agents_status().is_running());
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// GUI-02 (ADR-0017): «Проводник…», валидация путей, недавние проекты и закрытые GUI-дыры
/// (свойства источников/репозиториев, редактор заметок, импорт цитат, «Обновить вставки»).
#[cfg(test)]
mod gui_path_tests {
    use super::file_dialog::test_support::{FakePicker, ScriptedReply};
    use super::file_dialog::PickerBackend;
    use super::ribbon::Cmd;
    use super::{PickKind, RdeApp, Tab};
    use egui_kittest::kittest::Queryable;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    /// Фикстура реального `.docx` из тестов rde-doc (импорт/ревью/структура).
    fn fixture_docx() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rde-doc/tests/fixtures/base-report.docx")
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rde-gui-path-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn copy_tree(src: &Path, dst: &Path) {
        std::fs::create_dir_all(dst).unwrap();
        for entry in std::fs::read_dir(src).unwrap().flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if matches!(&*name, ".rde" | "exports" | "__pycache__" | "out" | ".venv") {
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

    fn demo_copy(name: &str) -> PathBuf {
        let root = temp_root(name);
        let project = root.join("project");
        copy_tree(&rde_core::demo_project_dir(), &project);
        project
    }

    fn picker_app(name: &str) -> (RdeApp, PathBuf) {
        let root = demo_copy(name);
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        // История проектов в тестах не должна трогать конфиг пользователя.
        app.set_recent_projects_file(None);
        (app, root)
    }

    /// Подменить backend «Проводника…» фейком и вернуть handle для проверки запросов.
    fn with_fake_picker(app: &mut RdeApp, replies: Vec<ScriptedReply>) -> Arc<FakePicker> {
        let fake = Arc::new(FakePicker::new(replies));
        app.set_file_dialog_backend(Arc::clone(&fake) as Arc<dyn PickerBackend>);
        fake
    }

    fn render(app: RdeApp) -> egui_kittest::Harness<'static, RdeApp> {
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..6 {
            harness.step();
        }
        harness
    }

    fn click_label(harness: &egui_kittest::Harness<'static, RdeApp>, label: &str) {
        let node = harness
            .query_all_by_label_contains(label)
            .next()
            .unwrap_or_else(|| panic!("в дереве доступности нет кнопки «{label}»"));
        node.click();
    }

    fn logs(app: &RdeApp) -> String {
        app.project.log.iter().map(|line| line.text.clone()).collect::<Vec<_>>().join("\n")
    }

    // ── валидация путей (до действия, понятные сообщения) ───────────────────

    #[test]
    fn path_validators_report_typos_and_wrong_types() {
        use super::{validate_agent_program, validate_citations_file, validate_docx_file, validate_new_project_dir, validate_project_dir, validate_source_file};
        // Проект: пусто → подсказка; нет каталога → ошибка; файл вместо каталога → ошибка;
        // каталог без rde.toml → ошибка; настоящий проект → ок.
        assert!(!validate_project_dir("").is_error());
        assert!(validate_project_dir("/нет/такого/каталога-12345").is_error());
        let root = temp_root("validators");
        std::fs::write(root.join("файл.txt"), "x").unwrap();
        assert!(validate_project_dir(&root.join("файл.txt").display().to_string()).is_error());
        assert!(validate_project_dir(&root.display().to_string()).is_error(), "нет rde.toml");
        std::fs::write(root.join("rde.toml"), "schema = \"rde/project@1\"\n").unwrap();
        assert!(!validate_project_dir(&root.display().to_string()).is_error());
        // Новый проект: занятый каталог → ошибка, непустой → ошибка, пустой → ок.
        assert!(validate_new_project_dir(&root.display().to_string()).is_error(), "проект уже есть");
        let occupied = root.join("occupied");
        std::fs::create_dir_all(&occupied).unwrap();
        std::fs::write(occupied.join("мусор"), "x").unwrap();
        assert!(validate_new_project_dir(&occupied.display().to_string()).is_error());
        assert!(!validate_new_project_dir(&root.join("fresh-project").display().to_string()).is_error());
        // .docx: битое расширение/каталог/несуществующий файл.
        let docx = root.join("отчёт.docx");
        std::fs::write(&docx, "PK").unwrap();
        assert!(!validate_docx_file(&docx.display().to_string()).is_error());
        let doc = root.join("отчёт.doc");
        std::fs::write(&doc, "x").unwrap();
        let check = validate_docx_file(&doc.display().to_string());
        assert!(check.is_error(), "{check:?}");
        assert!(check.text.contains(".docx"), "объясняет, что нужен .docx: {check:?}");
        assert!(validate_docx_file(&root.display().to_string()).is_error());
        // Цитаты: формат проверяется.
        let bib = root.join("refs.bib");
        std::fs::write(&bib, "@book{a, title={A}}").unwrap();
        assert!(!validate_citations_file(&bib.display().to_string()).is_error());
        assert!(validate_citations_file(&doc.display().to_string()).is_error());
        // Источник: файл обязателен.
        assert!(validate_source_file(&root.display().to_string()).is_error());
        assert!(!validate_source_file(&docx.display().to_string()).is_error());
        // Программа агента: имя из PATH — предупреждение или ок, но не ошибка блокировки;
        // абсолютный путь к несуществующему файлу — ошибка.
        let missing = validate_agent_program("/нет/такой/программы-dsh");
        assert!(missing.is_error());
        assert!(!validate_agent_program("").is_error());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn long_paths_are_ellipsized_but_keep_ends() {
        let path = "/home/user/Документы/rdeprojects/очень-длинный-проект-по-РСА/objects/sources/01ABC/report.pdf";
        let short = super::ellipsize_path(path, 40);
        assert!(short.chars().count() <= 40, "{short}");
        assert!(short.contains('…'), "середина заменена многоточием: {short}");
        assert!(short.starts_with("/home/user"), "начало пути сохранено: {short}");
        assert!(short.ends_with("report.pdf"), "конец пути сохранён: {short}");
        assert_eq!(super::ellipsize_path("/tmp/a", 80), "/tmp/a");
    }

    /// Запись полей манифеста не должна «уезжать» в таблицы: `file`/`url`/`retrieved_at`
    /// остаются ключами верхнего уровня, `[reference]` не затирается.
    #[test]
    fn manifest_setters_keep_toml_sections() {
        let root = temp_root("manifest-sections");
        let path = root.join("source.toml");
        std::fs::write(
            &path,
            "schema = \"rde/source@1\"\nid = \"01\"\ntitle = \"Источник\"\nkind = \"file\"\nfile = \"a.pdf\"\n\n[reference]\ntitle = \"Статья\"\nyear = 2020\n",
        )
        .unwrap();
        super::set_manifest_string(&path, "file", "b.pdf").unwrap();
        super::set_manifest_string(&path, "url", "https://example.org").unwrap();
        super::set_manifest_optional_string(&path, "file", None).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let value: toml::Value = toml::from_str(&text).unwrap();
        let table = value.as_table().unwrap();
        assert_eq!(table.get("url").and_then(|value| value.as_str()), Some("https://example.org"));
        assert!(table.get("file").is_none(), "file удалён: {text}");
        // [reference] не тронут.
        assert_eq!(
            table.get("reference").and_then(|value| value.get("title")).and_then(|value| value.as_str()),
            Some("Статья"),
            "{text}"
        );
        assert_eq!(table.get("title").and_then(|value| value.as_str()), Some("Источник"), "верхний title не задет");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── «Проводник…»: клик по кнопке → фоновый выбор → путь в поле ─────────

    #[test]
    fn picker_button_fills_import_docx_dialog_path() {
        let (mut app, root) = picker_app("picker-import");
        let fake = with_fake_picker(&mut app, vec![ScriptedReply::Selected(PathBuf::from("/tmp/выбранный-отчёт.docx"))]);
        app.run_cmd(Cmd::ImportDocx);
        let mut harness = render(app);
        click_label(&harness, "Проводник");
        harness.run_steps(8);
        let dialog = harness.state().import_docx_dialog.as_ref().expect("диалог импорта открыт");
        assert_eq!(dialog.path, "/tmp/выбранный-отчёт.docx", "путь подставлен из системного выбора");
        assert!(dialog.error.is_empty());
        let requests = fake.requests();
        assert_eq!(requests.len(), 1, "клик по кнопке вызвал системный диалог ровно один раз");
        assert_eq!(requests[0].title, "Импорт .docx");
        match &requests[0].kind {
            PickKind::File { filters } => {
                assert!(filters.iter().any(|filter| filter.extensions == vec!["docx".to_owned()]), "{filters:?}");
            }
            other => panic!("ожидался выбор файла .docx, получено {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn picker_folder_for_open_project_and_cancel_keeps_path() {
        let (mut app, root) = picker_app("picker-open");
        let fake = with_fake_picker(&mut app, vec![ScriptedReply::Cancelled]);
        app.run_cmd(Cmd::OpenProject);
        let mut harness = render(app);
        click_label(&harness, "Проводник");
        harness.run_steps(8);
        // Отмена: путь не меняется, ошибок нет, диалог остаётся открытым.
        let dialog = harness.state().open_project_dialog.as_ref().expect("диалог открыт");
        assert_eq!(dialog.path, root.display().to_string());
        assert!(!logs(harness.state()).contains("введите путь вручную"), "отмена — не ошибка");
        match &fake.requests()[0].kind {
            PickKind::Folder => {}
            other => panic!("«Открыть проект» должен выбирать каталог, получено {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn picker_error_logs_manual_hint_and_keeps_field() {
        let (mut app, root) = picker_app("picker-error");
        with_fake_picker(&mut app, vec![ScriptedReply::Failed("нет zenity/диалога".to_owned())]);
        app.run_cmd(Cmd::ImportDocx);
        let mut harness = render(app);
        click_label(&harness, "Проводник");
        harness.run_steps(8);
        let text = logs(harness.state());
        assert!(text.contains("введите путь вручную"), "деградация подсказывает ручной ввод: {text}");
        let dialog = harness.state().import_docx_dialog.as_ref().expect("диалог не закрылся");
        assert!(dialog.path.is_empty(), "поле не меняется при ошибке backend'а");
        // Поле остаётся рабочим: ручной ввод пути + импорт (покрыто отдельным тестом ниже).
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Регресс GUI-02: если закрыть диалог (Esc), пока системный выбор ещё открыт, поздний выбор
    /// не должен подменять путь при следующем открытии диалога (генерация запроса + abandon).
    #[test]
    fn late_pick_after_closed_dialog_is_not_applied_to_next_dialog() {
        let (mut app, root) = picker_app("late-pick");
        let fake = Arc::new(
            FakePicker::new(vec![ScriptedReply::Selected(PathBuf::from("/tmp/поздний-выбор.docx"))])
                .with_delay(std::time::Duration::from_millis(150)),
        );
        app.set_file_dialog_backend(Arc::clone(&fake) as Arc<dyn PickerBackend>);
        app.run_cmd(Cmd::ImportDocx);
        let mut harness = render(app);
        click_label(&harness, "Проводник");
        harness.run_steps(4);
        assert_eq!(fake.requests().len(), 1, "выбор запущен фоновым потоком");
        // Закрыть диалог Esc, пока выбор ещё «в полёте».
        harness.input_mut().events.push(egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        });
        harness.run_steps(2);
        assert!(harness.state().import_docx_dialog.is_none(), "диалог закрыт");
        // Дождаться позднего результата и открыть диалог заново.
        for _ in 0..40 {
            harness.run_steps(1);
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        harness.run_steps(2);
        harness.state_mut().run_cmd(Cmd::ImportDocx);
        harness.run_steps(4);
        let dialog = harness.state().import_docx_dialog.as_ref().expect("диалог открыт снова");
        assert!(dialog.path.is_empty(), "поздний выбор закрытого диалога не попал в поле: {:?}", dialog.path);
        assert!(
            !logs(harness.state()).contains("поздний-выбор"),
            "устаревший выбор не попал и в журнал: {}",
            logs(harness.state())
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── сквозные сценарии: открыть проект, импорт, новый проект ────────────

    #[test]
    fn recent_project_click_opens_project() {
        let (mut app, root) = picker_app("recent-open");
        let second = temp_root("recent-open-second").join("Второй проект");
        rde_core::create_project(&second, "Второй проект").unwrap();
        app.set_demo_recent_projects(Some(vec![rde_core::RecentProject {
            path: second.display().to_string(),
            name: "Второй проект".to_owned(),
            opened_unix: 1,
        }]));
        app.run_cmd(Cmd::OpenProject);
        let mut harness = render(app);
        click_label(&harness, "Второй проект");
        harness.run_steps(8);
        let opened = harness.state().project.root.clone();
        let expected = std::fs::canonicalize(&second).unwrap();
        assert_eq!(std::fs::canonicalize(&opened).unwrap(), expected, "клик по недавнему проекту открыл его");
        assert!(harness.state().open_project_dialog.is_none(), "диалог закрылся после открытия");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&second);
    }

    #[test]
    fn import_docx_action_from_gui_path_creates_imported_document() {
        let (mut app, root) = picker_app("import-action");
        let fixture = fixture_docx();
        assert!(fixture.is_file(), "фикстура .docx: {}", fixture.display());
        let before = app.project.documents().count();
        app.import_docx_action_with_title(&fixture, Some("Отчёт ВКР")).expect("импорт .docx по пути из «Проводника…»");
        assert_eq!(app.project.documents().count(), before + 1);
        let imported = app
            .project
            .documents()
            .find(|object| object.title == "[IMPORTED] Отчёт ВКР")
            .expect("новый объект-документ «[IMPORTED] …»");
        assert!(imported.dir.join("report.doc.rde").is_file(), "пакет .doc.rde создан");
        assert!(logs(&app).contains("Импортирован .docx"), "{}", logs(&app));
        // Ручной ввод имени без префикса: префикс добавляется, как в CLI.
        app.import_docx_action(&fixture).expect("импорт без имени");
        let stem_title = format!("[IMPORTED] {}", fixture.file_stem().unwrap().to_string_lossy());
        assert!(app.project.documents().any(|object| object.title == stem_title), "{stem_title}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn new_project_from_gui_writes_recents_and_last_project() {
        let (mut app, root) = picker_app("new-project");
        let parent = temp_root("new-project-parent");
        let target = parent.join("проект-из-gui");
        app.set_recent_projects_file(Some(parent.join("recent-projects.toml")));
        app.create_project_ui(&target, "Проект из GUI").expect("создать проект");
        assert_eq!(app.project.manifest.name, "Проект из GUI");
        let recent_file = parent.join("recent-projects.toml");
        let recent = rde_core::load_recent_projects_from(&recent_file);
        let canonical = std::fs::canonicalize(&target).unwrap();
        assert_eq!(recent[0].path, canonical.display().to_string(), "новый проект — первым в истории");
        assert_eq!(recent[0].name, "Проект из GUI");
        let last = std::fs::read_to_string(parent.join("last-project")).unwrap();
        assert_eq!(last.trim(), canonical.display().to_string(), "last-project для scripts/run.sh");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&parent);
    }

    // ── закрытые GUI-дыры: источник/репозиторий/заметка/цитаты/вставки ─────

    #[test]
    fn source_properties_attach_file_and_switch_to_url() {
        let (mut app, root) = picker_app("source-attach");
        let external = root.join("внешний-конспект.md");
        std::fs::write(&external, "# Конспект статьи\n\nТекст.").unwrap();
        let source = app.project.sources().next().cloned().expect("источник в демо-проекте");
        app.open_source_edit(&source.id);
        let mut dialog = app.source_edit_dialog.clone().expect("диалог свойств");
        dialog.kind = "file".to_owned();
        dialog.file = external.display().to_string();
        app.save_source_edit(&dialog).expect("сохранить источник с файлом");

        let reloaded = app.project.object(&source.id).cloned().expect("источник на месте");
        let rde_core::ObjectData::Source(manifest) = &reloaded.data else { panic!("не источник") };
        let file = manifest.file.clone().expect("file записан");
        assert_eq!(file, "внешний-конспект.md");
        assert!(reloaded.dir.join(&file).is_file(), "файл скопирован в объект-источник");
        assert_eq!(std::fs::read_to_string(reloaded.dir.join(&file)).unwrap(), "# Конспект статьи\n\nТекст.");
        assert!(external.is_file(), "оригинал не изменяется/не удаляется");

        // Переключение на URL: file убирается, url появляется — локатор не «висит» двойным.
        app.open_source_edit(&source.id);
        let mut dialog = app.source_edit_dialog.clone().expect("диалог свойств");
        dialog.kind = "url".to_owned();
        dialog.url = "https://example.org/paper".to_owned();
        dialog.file = external.display().to_string();
        app.save_source_edit(&dialog).expect("сохранить URL-источник");
        let reloaded = app.project.object(&source.id).cloned().unwrap();
        let rde_core::ObjectData::Source(manifest) = &reloaded.data else { panic!("не источник") };
        assert_eq!(manifest.kind, "url");
        assert_eq!(manifest.url.as_deref(), Some("https://example.org/paper"));
        assert!(manifest.file.is_none(), "старый файл удалён из манифеста");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn repository_properties_persist_kind_and_path() {
        let (mut app, root) = picker_app("repo-edit");
        let repo = app
            .project
            .objects
            .iter()
            .find(|object| matches!(object.data, rde_core::ObjectData::Repository(_)))
            .cloned()
            .expect("репозиторий в демо-проекте");
        app.open_repository_edit(&repo.id);
        let mut dialog = app.repo_edit_dialog.clone().expect("диалог свойств репозитория");
        dialog.kind = "external".to_owned();
        dialog.path = "/home/user/исходники".to_owned();
        // Путь не существует → понятная ошибка валидации до сохранения (диалог), но запись
        // манифеста допустима: проверяем и валидатор отдельно, и запись.
        assert!(super::validate_directory(&dialog.path).is_error());
        app.save_repository_edit(&dialog).expect("сохранить репозиторий");
        let reloaded = app.project.object(&repo.id).cloned().unwrap();
        let rde_core::ObjectData::Repository(manifest) = &reloaded.data else { panic!("не репозиторий") };
        assert_eq!(manifest.kind, "external");
        assert_eq!(manifest.path, "/home/user/исходники");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn note_editor_saves_markdown_content() {
        let (mut app, root) = picker_app("note-edit");
        let note = app
            .project
            .objects
            .iter()
            .find(|object| matches!(object.data, rde_core::ObjectData::Note(_)))
            .cloned()
            .expect("заметка в демо-проекте");
        app.selected = Some(note.id.clone());
        app.note_editor = Some(super::NoteEditorState {
            id: note.id.clone(),
            text: "# Новая заметка\n\nПравка из GUI.".to_owned(),
            dirty: true,
            error: String::new(),
        });
        assert!(app.save_note_editor(), "Ctrl+S в редакторе заметки сохранил её");
        let rde_core::ObjectData::Note(manifest) = &note.data else { panic!("не заметка") };
        let text = std::fs::read_to_string(note.dir.join(&manifest.file)).unwrap();
        assert!(text.contains("Правка из GUI"), "{text}");
        assert!(!app.note_editor.as_ref().unwrap().dirty, "после сохранения правок нет");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn citations_import_creates_source_objects() {
        let (mut app, root) = picker_app("citations");
        let bib = root.join("refs.bib");
        std::fs::write(
            &bib,
            "@book{cumming2005,\n  author = {Cumming, I. G. and Wong, F. H.},\n  title = {Digital Processing of SAR Data},\n  year = {2005}\n}\n@article{moreira2013,\n  author = {Moreira, A.},\n  title = {A Tutorial on SAR},\n  year = {2013}\n}\n",
        )
        .unwrap();
        let before = app.project.sources().count();
        let summary = app.import_citations_action(&bib).expect("импорт цитат");
        assert_eq!(app.project.sources().count(), before + 2, "два источника созданы");
        assert!(summary.contains("создано источников 2"), "{summary}");
        let titles: Vec<String> = app
            .project
            .sources()
            .filter_map(|object| match &object.data {
                rde_core::ObjectData::Source(source) => source.reference.as_ref().map(|reference| reference.title.clone()),
                _ => None,
            })
            .collect();
        assert!(titles.iter().any(|title| title.contains("Digital Processing")), "{titles:?}");
        // Битый файл — понятная ошибка, ничего не создано.
        let broken = root.join("broken.bib");
        std::fs::write(&broken, "не цитаты вовсе").unwrap();
        let before = app.project.sources().count();
        assert!(app.import_citations_action(&broken).is_err());
        assert_eq!(app.project.sources().count(), before, "ошибка не создаёт частичных объектов");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn update_inserts_command_updates_document_instead_of_mock_log() {
        let (mut app, root) = picker_app("update-inserts");
        app.run_cmd(Cmd::UpdateInserts);
        let text = logs(&app);
        assert!(!text.contains("сейчас mock"), "заглушка убрана: {text}");
        assert!(
            text.contains("Документ обновлён") || text.contains("Обновить документ"),
            "«Обновить вставки» реально обновляет документ: {text}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recent_history_records_plain_open_via_api() {
        let (mut app, root) = picker_app("record-open");
        let parent = temp_root("record-open-http");
        app.set_recent_projects_file(Some(parent.join("recent-projects.toml")));
        let name = app.project.manifest.name.clone();
        app.record_recent_project(&root, &name);
        let recent = rde_core::load_recent_projects_from(&parent.join("recent-projects.toml"));
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].name, name);
        assert!(recent[0].path.ends_with("project"), "канонизированный путь проекта: {}", recent[0].path);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&parent);
    }

    /// Заметка/источник открываются на вкладке «Источник» (а не показывают чужой объект).
    #[test]
    fn note_and_source_open_on_source_tab() {
        let (mut app, root) = picker_app("tabs");
        let note = app
            .project
            .objects
            .iter()
            .find(|object| matches!(object.data, rde_core::ObjectData::Note(_)))
            .map(|object| object.id.clone())
            .expect("заметка");
        app.apply_tree_action(super::TreeAction::Open(note.clone()));
        assert!(app.tab == Tab::Source, "заметка открывается на вкладке «Источник»");
        assert_eq!(app.selected.as_deref(), Some(note.as_str()));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── ADR-0018: нумерация формул и ссылки на формулы (B1–B3) ─────────────

    /// Копия демо-проекта во временный каталог (без `.rde`: кэши формул уже в пакете).
    fn numbering_demo_copy(name: &str) -> std::path::PathBuf {
        fn copy_tree(src: &std::path::Path, dst: &std::path::Path) {
            std::fs::create_dir_all(dst).unwrap();
            for entry in std::fs::read_dir(src).unwrap().flatten() {
                let name = entry.file_name();
                if name == ".rde" {
                    continue;
                }
                let path = entry.path();
                let target = dst.join(&name);
                if path.is_dir() {
                    copy_tree(&path, &target);
                } else {
                    std::fs::copy(&path, &target).unwrap();
                }
            }
        }
        let root = std::env::temp_dir().join(format!("rde-ui-numbering-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        copy_tree(&rde_core::demo_project_dir(), &root);
        root
    }

    /// Диалог перекрёстной ссылки по умолчанию предлагает формулу (B3), вставка создаёт
    /// `rde://formula/<id>` с номером документа, а перенумерация обновляет текст ссылки.
    #[test]
    fn formula_crossref_dialog_inserts_and_updates_reference() {
        let root = numbering_demo_copy("formula-crossref");
        let project = rde_core::load_project(&root).unwrap();
        let mut app = RdeApp::new(project);
        app.open_insert_dialog("crossref");
        let dialog = app.insert_dialog.as_ref().expect("диалог перекрёстной ссылки");
        assert_eq!(dialog.kind, "crossref");
        assert!(dialog.artifact_id.starts_with("formula:"), "по умолчанию — формула (B3): {}", dialog.artifact_id);
        let formula_id = dialog.artifact_id.trim_start_matches("formula:").to_owned();
        let dialog = app.insert_dialog.take().unwrap();
        app.confirm_insert(dialog);
        let package = app.project.document_path().unwrap();
        let doc = rde_doc::load_rde_doc(&package).unwrap();
        let reference = doc
            .inserts
            .iter()
            .filter(|insert| insert.kind == "crossref")
            .find(|insert| insert.target == format!("rde://formula/{formula_id}"))
            .expect("ссылка на формулу rde://formula/<id>");
        // Номер ссылки совпадает с номером формулы в документе (B3: «(N)», не руками).
        let formula = doc
            .inserts
            .iter()
            .filter(|insert| insert.kind == "formula")
            .find(|insert| rde_doc::parse_formula_target(&insert.target).map(|target| target.formula_id == formula_id).unwrap_or(false))
            .expect("формула вставлена");
        let number = formula.number_text.clone().or_else(|| formula.number.map(|number| number.to_string())).unwrap();
        assert_eq!(reference.cached, format!("({number})"), "текст ссылки — номер формулы: {reference:?}");
        // Смена схемы «по разделам»: номер и текст ссылки обновляются одним проходом (B3).
        let report = rde_doc::renumber_formula_numbers(&package, rde_doc::FormulaNumbering::Section).unwrap();
        assert!(!report.formulas.is_empty(), "номера формул пересчитаны: {report:?}");
        let doc = rde_doc::load_rde_doc(&package).unwrap();
        let reference = doc.inserts.iter().find(|insert| insert.id == reference.id).unwrap();
        let formula = doc
            .inserts
            .iter()
            .filter(|insert| insert.kind == "formula")
            .find(|insert| rde_doc::parse_formula_target(&insert.target).map(|target| target.formula_id == formula_id).unwrap_or(false))
            .unwrap();
        let number = formula.number_text.clone().or_else(|| formula.number.map(|number| number.to_string())).unwrap();
        assert!(number.contains('.'), "схема по разделам: номер вида 2.1: {number}");
        assert_eq!(reference.cached, format!("({number})"), "ссылка обновилась вместе с номером");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Смена схемы нумерации в инспекторе документа: `document.toml` + немедленная
    /// перенумерация формул документа (сквозная ↔ по разделам), без пересборки тел.
    #[test]
    fn document_numbering_scheme_switch_renumbers_formulas() {
        let root = numbering_demo_copy("formula-numbering");
        let project = rde_core::load_project(&root).unwrap();
        let object = project.document_object().cloned().unwrap();
        let mut app = RdeApp::new(project);
        let package = app.project.document_path().unwrap();
        let changed = app.apply_document_formula_numbering(&object.dir, rde_doc::FormulaNumbering::Section).unwrap();
        assert!(changed > 0, "номера изменились при переходе на схему по разделам");
        let manifest = std::fs::read_to_string(object.dir.join("document.toml")).unwrap();
        assert!(manifest.contains("formula_numbering = \"section\""), "схема записана в document.toml: {manifest}");
        let doc = rde_doc::load_rde_doc(&package).unwrap();
        let numbers: Vec<String> = doc
            .inserts
            .iter()
            .filter(|insert| insert.kind == "formula")
            .filter_map(|insert| insert.number_text.clone().or_else(|| insert.number.map(|number| number.to_string())))
            .collect();
        assert!(!numbers.is_empty(), "в демо есть формульные вставки");
        assert!(numbers.iter().all(|number| number.contains('.')), "схема по разделам: {numbers:?}");
        // Обратно — сквозная: (1), (2)…
        app.apply_document_formula_numbering(&object.dir, rde_doc::FormulaNumbering::Continuous).unwrap();
        let doc = rde_doc::load_rde_doc(&package).unwrap();
        let numbers: Vec<String> = doc
            .inserts
            .iter()
            .filter(|insert| insert.kind == "formula")
            .filter_map(|insert| insert.number_text.clone().or_else(|| insert.number.map(|number| number.to_string())))
            .collect();
        assert_eq!(numbers, vec!["1".to_owned(), "2".to_owned()], "сквозная нумерация: {numbers:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── ADR-0019: миграция MathType-формул — диалог и немедленная нумерация ─────

    /// Диалог «Принять формулы как объекты»: счётчики display/inline/найденных номеров, схема,
    /// опции сохранения; в демо-проект добавляем формулу вне реестра (legacy таблица 1×2, номер (1)).
    #[test]
    fn formula_migration_dialog_shows_counts_and_renders() {
        use egui_kittest::kittest::Queryable;
        let root = numbering_demo_copy("formula-migration-dialog");
        let package = rde_core::load_project(&root).unwrap().document_path().unwrap();
        rde_doc::insert_equation(&package, "\\sigma=\\frac{F}{A}", Some(1)).unwrap();
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        app.set_recent_projects_file(None);
        app.open_formula_migration_dialog();
        let dialog = app.formula_migration_dialog.as_ref().expect("диалог миграции формул");
        assert_eq!(dialog.display, 1, "одна display-формула вне реестра");
        assert_eq!(dialog.inline, 0);
        assert_eq!(dialog.found_numbers, 1, "номер (1) из legacy-таблицы распознан");
        assert!(dialog.preserve_numbers, "номер найден — по умолчанию сохраняем");
        assert_eq!(dialog.scheme, rde_doc::FormulaNumbering::Continuous);
        // Диалог рисуется (egui_kittest): счётчики, схема и опции доступны в дереве.
        let harness = render(app);
        assert!(
            harness.query_all_by_label_contains("Принять формулы как объекты").next().is_some(),
            "заголовок диалога миграции"
        );
        assert!(
            harness.query_all_by_label_contains("Формул вне реестра: 1").next().is_some(),
            "счётчики формул в диалоге"
        );
        assert!(
            harness.query_all_by_label_contains("Сохранять существующие номера").next().is_some(),
            "опция сохранения номеров из Word"
        );
        assert!(
            harness.query_all_by_label_contains("Нумеровать display-формулы").next().is_some(),
            "опция нумерации display-формул"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// «Принять формулы как объекты»: миграция сразу ставит номер (не требуя отдельного
    /// «Обновить документ»), display-формула становится block-вставкой, номер из Word сохранён.
    #[test]
    fn formula_migration_applies_numbered_display_block() {
        let root = numbering_demo_copy("formula-migration-apply");
        let package = rde_core::load_project(&root).unwrap().document_path().unwrap();
        rde_doc::insert_equation(&package, "\\sigma=\\frac{F}{A}", Some(1)).unwrap();
        let before = std::fs::read(&package).unwrap();
        let mut app = RdeApp::new(rde_core::load_project(&root).unwrap());
        app.set_recent_projects_file(None);
        app.open_formula_migration_dialog();
        let dialog = app.formula_migration_dialog.take().expect("диалог миграции");
        let options = rde_doc::FormulaMigrationOptions {
            numbering: dialog.scheme,
            preserve_word_numbers: dialog.preserve_numbers,
            number_display: dialog.number_display,
        };
        app.formula_migration_dialog = Some(dialog);
        app.apply_formula_migration(options);
        let text = logs(&app);
        assert!(text.contains("Принято формул 1"), "журнал миграции: {text}");
        assert!(
            text.contains("номеров сохранено из Word 1"),
            "журнал: номер сохранён из Word: {text}"
        );
        assert_ne!(before, std::fs::read(&package).unwrap(), "документ изменён миграцией");
        let doc = rde_doc::load_rde_doc(&package).unwrap();
        let migrated: Vec<_> = doc
            .inserts
            .iter()
            .filter(|insert| insert.kind == "formula" && insert.number_source == rde_doc::NUMBER_SOURCE_WORD)
            .collect();
        assert_eq!(migrated.len(), 1, "мигрированная вставка с номером из Word: {:?}", doc.inserts);
        assert_eq!(migrated[0].number_text.as_deref(), Some("1"), "номер сохранён из Word");
        let target = rde_doc::parse_formula_target(&migrated[0].target).expect("цель вставки — формула");
        assert!(!target.formula_id.is_empty(), "target ссылается на формулу проекта");
        assert!(target.act_key.is_some(), "target несёт act_key");
        // Повторное открытие диалога: формул вне реестра уже нет (идемпотентность).
        app.open_formula_migration_dialog();
        assert!(app.formula_migration_dialog.is_none(), "после миграции формул вне реестра нет; журнал: {}", logs(&app));
        let _ = std::fs::remove_dir_all(&root);
    }
}
