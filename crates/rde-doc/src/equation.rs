//! Формулы документа: OMML-фундамент R-DOC-21/22 (фаза 1, без UI).
//!
//! Хранение — нативные MS Equations (OOXML OMML: `m:oMath` для inline, `m:oMathPara` для
//! display). Вставка идёт через модель WordCraft (`InlineObject::Equation`), сохранение — через
//! [`crate::save_engine_document`] (RDE владеет записью пакета), поэтому формула остаётся
//! редактируемой в Word, а не превращается в картинку.
//!
//! Нумерация (§9.4) — **таблица 1×2**: левая ячейка с display-уравнением, правая — номер
//! `(n)` с выравниванием по правому краю. Это канонический для ГОСТ способ, он одинаково
//! рендерится в Word, LibreOffice и WordCraft. Альтернатива WordCraft — автономер через `#`
//! (`m:eqArr` + `resolve_numbers`) — отклонена на фазе 1: OOXML-writer пишет сам маркер `#`
//! в `m:t` (`vendor/wordcraft/crates/docx/src/write/math.rs:96`), и номер в Word/LO выглядит как
//! `E=mc² # (1)` вместо правого поля (проверено LO-рендером в отчёте сессии); `SEQ Eq` — задел
//! на фазу 2 вместе с полями Word (R-DOC-14).
//!
//! Реестр формул в `rde:doc` не ведётся: формула — содержимое документа (как текст), источник
//! истины — сам OMML; id/позиция вычисляются по модели.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use wordcraft_engine::doc::{
    para_block, Align, Block, Cell, CharProps, Document, InlineObject, Paragraph, Path as ParaPath, Pos, StoryRef, Table,
};
use wordcraft_engine::doc::math::{parse_linear, to_linear, MNode, Math};
use wordcraft_engine::doc::math_edit::each_run_mut;
use wordcraft_engine::doc::math_latex::{parse_latex, to_latex};
use wordcraft_engine::doc::props::{Border, BorderStyle, Borders, TableProps};

use crate::{extract_docx, load_rde_doc, open_engine, package_info, save_engine_document, temp_docx_path, PackageInfo};

/// Ширина колонки номера формулы, точки (1.5 см — номер у правого поля по ГОСТ).
const NUMBER_COLUMN_WIDTH: f32 = 42.5;

/// Синтаксис ввода формулы.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EquationSyntax {
    /// LaTeX (`\frac{a+b}{c}`).
    Latex,
    /// Linear-формат UnicodeMath, он же «линейный» вид Word (`(a+b)/c`).
    Linear,
}

impl EquationSyntax {
    /// Автоопределение: LaTeX, если во вводе есть `\`, иначе linear.
    pub fn detect(input: &str) -> EquationSyntax {
        if input.contains('\\') { EquationSyntax::Latex } else { EquationSyntax::Linear }
    }

    /// Разобрать формулу в math-дерево WordCraft.
    pub fn parse(self, input: &str) -> Vec<MNode> {
        match self {
            EquationSyntax::Latex => parse_latex(input),
            EquationSyntax::Linear => parse_linear(input),
        }
    }
}

/// Запрос на вставку формулы.
#[derive(Clone, Debug)]
pub struct EquationRequest {
    /// Формула: LaTeX (`\frac{a}{b}`) или linear (`a/b`) — определяется по наличию `\`.
    pub formula: String,
    /// Номер для отображения справа в скобках; `None` — без номера.
    pub number: Option<u32>,
    /// `true` — отдельная строка (display); `false` — inline в конец абзаца `anchor_block`.
    pub display: bool,
    /// Индекс верхнеуровневого блока тела (0-based): display вставляется после него, inline — в
    /// его конец. `None` — display в конец документа (для inline обязателен).
    pub anchor_block: Option<usize>,
}

impl EquationRequest {
    /// Display-уравнение в конец документа.
    pub fn display(formula: impl Into<String>) -> EquationRequest {
        EquationRequest { formula: formula.into(), number: None, display: true, anchor_block: None }
    }
}

/// Результат вставки формулы.
#[derive(Clone, Debug)]
pub struct EquationReport {
    /// Идентификатор в порядке документа (`e1`, `e2`, …) — вычисляется по модели, не хранится.
    pub id: String,
    /// Путь блока WordCraft (`16` или `16/0/0/0` для формулы в таблице 1×2).
    pub block: String,
    /// Анкор `w14:paraId` абзаца формулы (R-DOC-04).
    pub para_id: Option<String>,
    /// Linear-формат (UnicodeMath).
    pub linear: String,
    /// LaTeX-представление.
    pub latex: String,
    pub display: bool,
    pub number: Option<u32>,
    pub package: PackageInfo,
}

/// Формула в документе (для UI/агента, R-AGT-07).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EquationInfo {
    pub id: String,
    pub block: String,
    pub linear: String,
    pub latex: String,
    pub display: bool,
    pub number: Option<u32>,
}

/// Изменение номера формулы при перенумерации.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EquationNumberChange {
    pub block: String,
    pub old_number: u32,
    pub new_number: u32,
}

/// Вставить display-уравнение в конец документа. Простой вариант API (R-DOC-21):
/// `insert_equation(package, latex_or_linear, number)`.
pub fn insert_equation(package: &Path, formula: &str, number: Option<u32>) -> Result<EquationReport, String> {
    add_equation(package, EquationRequest { formula: formula.to_owned(), number, display: true, anchor_block: None })
}

/// Вставить уравнение в `.doc.rde` через модель WordCraft.
///
/// Display: отдельная строка. С номером — таблица 1×2 (формула по центру, номер справа в
/// скобках); без номера — центрированный абзац с display-уравнением. Inline: уравнение в конец
/// абзаца `anchor_block`. Пакет пересобирается штатным путём RDE (`save_engine_document`).
pub fn add_equation(package: &Path, request: EquationRequest) -> Result<EquationReport, String> {
    let formula = request.formula.trim().to_owned();
    if formula.is_empty() {
        return Err("пустая формула".to_owned());
    }
    if request.number == Some(0) {
        return Err("номер формулы начинается с 1".to_owned());
    }
    let syntax = EquationSyntax::detect(&formula);
    let mut nodes = syntax.parse(&formula);
    if nodes.is_empty() {
        return Err(format!("формула не разобрана ({syntax:?}): {formula}"));
    }
    // Пакет должен быть валидным .doc.rde (идентичность сохраняет save_engine_document).
    let _ = load_rde_doc(package)?;
    let temp_docx = temp_docx_path("equation");
    let result = (|| -> Result<EquationReport, String> {
        extract_docx(package, &temp_docx)?;
        let mut document = open_engine(&temp_docx)?;
        let existing = count_equations(&document);
        let size = body_font_size(&document);
        if let Some(size) = size {
            each_run_mut(&mut nodes, &mut |run| {
                if run.size.is_none() {
                    run.size = Some(size);
                }
            });
        }
        let linear = to_linear(&nodes);
        let latex = to_latex(&nodes);
        let block = if request.display {
            let paragraph = display_paragraph(&nodes, size)?;
            match request.number {
                Some(number) => {
                    let table = numbered_table(&document, paragraph, number);
                    let index = insert_block(&mut document, Block::Table(table), request.anchor_block)?;
                    ensure_body_ends_with_paragraph(&mut document);
                    format!("{index}/0/0/0")
                }
                None => format!("{}", insert_block(&mut document, Block::Para(paragraph), request.anchor_block)?),
            }
        } else {
            insert_inline(&mut document, &nodes, request.anchor_block)?
        };
        wordcraft_engine::io::save_path(&temp_docx, &document).map_err(|e| format!("сохранить .docx с формулой: {e}"))?;
        let saved = save_engine_document(&temp_docx, package, "", "")?;
        let para_id = saved.blocks.iter().find(|entry| entry.path == block).and_then(|entry| entry.para_id.clone());
        Ok(EquationReport {
            id: format!("e{}", existing + 1),
            block,
            para_id,
            linear,
            latex,
            display: request.display,
            number: request.number,
            package: package_info(package)?,
        })
    })();
    let _ = fs::remove_file(&temp_docx);
    result
}

/// Список формул документа в порядке следования.
pub fn list_equations(package: &Path) -> Result<Vec<EquationInfo>, String> {
    let temp_docx = temp_docx_path("equations");
    let result = (|| -> Result<Vec<EquationInfo>, String> {
        extract_docx(package, &temp_docx)?;
        let document = open_engine(&temp_docx)?;
        let numbers = numbered_equation_map(&document);
        let mut out = Vec::new();
        each_equation(&document, |path, _off, object| {
            let InlineObject::Equation { linear, display, math } = object else { return };
            let nodes = resolved_nodes(linear, math);
            out.push(EquationInfo {
                id: format!("e{}", out.len() + 1),
                block: path.to_owned(),
                linear: linear.clone(),
                latex: to_latex(&nodes),
                display: *display,
                number: numbers.get(path).copied(),
            });
        });
        Ok(out)
    })();
    let _ = fs::remove_file(&temp_docx);
    result
}

/// Перенумеровать нумерованные формулы по порядку документа: `(1)`, `(2)`, … (сквозная
/// нумерация). Возвращает только изменившиеся номера; пакет пересобирается через движок.
pub fn renumber_equations(package: &Path) -> Result<Vec<EquationNumberChange>, String> {
    let temp_docx = temp_docx_path("renumber-equations");
    let result = (|| -> Result<Vec<EquationNumberChange>, String> {
        extract_docx(package, &temp_docx)?;
        let mut document = open_engine(&temp_docx)?;
        let mut targets: Vec<(ParaPath, u32)> = Vec::new();
        for index in 0..document.body.len() {
            if let Block::Table(table) = &*document.body[index] {
                if let Some(row) = numbered_row(table, index) {
                    targets.push((row.number_path, row.number));
                }
            }
        }
        let mut changes = Vec::new();
        for (index, (path, old)) in targets.iter().enumerate() {
            let new_number = index as u32 + 1;
            if *old == new_number {
                continue;
            }
            set_paragraph_text(&mut document, path, &format!("({new_number})"))?;
            changes.push(EquationNumberChange { block: path.to_string(), old_number: *old, new_number });
        }
        if changes.is_empty() {
            return Ok(changes);
        }
        wordcraft_engine::io::save_path(&temp_docx, &document).map_err(|e| format!("сохранить .docx после перенумерации формул: {e}"))?;
        save_engine_document(&temp_docx, package, "", "")?;
        Ok(changes)
    })();
    let _ = fs::remove_file(&temp_docx);
    result
}

// ── построение блоков формулы ────────────────────────────────────────────────

fn equation_object(nodes: &[MNode], display: bool) -> InlineObject {
    InlineObject::Equation {
        linear: to_linear(nodes),
        display,
        math: Math { nodes: nodes.to_vec(), ..Math::default() },
    }
}

/// Абзац display-уравнения: по центру, без отступов и лишних интервалов.
fn display_paragraph(nodes: &[MNode], size: Option<f32>) -> Result<Paragraph, String> {
    let mut paragraph = Paragraph::new();
    paragraph.props.align = Some(Align::Center);
    paragraph.props.space_before = Some(0.0);
    paragraph.props.space_after = Some(0.0);
    paragraph.props.indent_left = Some(0.0);
    paragraph.props.indent_right = Some(0.0);
    paragraph.props.indent_first = Some(0.0);
    let props = CharProps { size, ..CharProps::default() };
    paragraph.insert_object(0, equation_object(nodes, true), &props).map_err(|e| format!("уравнение в абзац: {e}"))?;
    Ok(paragraph)
}

/// Таблица 1×2: формула по центру слева, номер `(n)` справа (§9.4).
fn numbered_table(document: &Document, formula: Paragraph, number: u32) -> Table {
    let section = &document.last_section;
    let text_width = (section.page_w - section.margin_left - section.margin_right - section.gutter).max(120.0);
    let number_width = NUMBER_COLUMN_WIDTH.min((text_width * 0.2).max(18.0));
    let formula_width = (text_width - number_width).max(60.0);
    let mut table = Table::new(1, 2, text_width);
    table.props = TableProps {
        style: None,
        width: Some(text_width),
        borders: Some(no_borders()),
        cell_margins: Some([0.0, 0.0, 0.0, 0.0]),
        fixed: true,
        ..TableProps::default()
    };
    table.grid = vec![formula_width, number_width];
    table.rows[0].cells[0].props.width = Some(formula_width);
    table.rows[0].cells[1].props.width = Some(number_width);
    table.rows[0].cells[0].blocks = vec![para_block(formula)];
    let mut number_paragraph = Paragraph::with_text(&format!("({number})"), CharProps::default());
    number_paragraph.props.align = Some(Align::Right);
    number_paragraph.props.space_before = Some(0.0);
    number_paragraph.props.space_after = Some(0.0);
    number_paragraph.props.indent_left = Some(0.0);
    number_paragraph.props.indent_right = Some(0.0);
    number_paragraph.props.indent_first = Some(0.0);
    table.rows[0].cells[1].blocks = vec![para_block(number_paragraph)];
    table
}

fn no_borders() -> Borders {
    let none = Some(Border { style: BorderStyle::None, width: 0.0, color: None, space: 0.0 });
    Borders { top: none, left: none, bottom: none, right: none, between: none, inside_v: none }
}

/// Вставить блок после `anchor` (0-based индекс тела) или в конец. Возвращает индекс блока.
fn insert_block(document: &mut Document, block: Block, anchor: Option<usize>) -> Result<usize, String> {
    let story = StoryRef::Body;
    match anchor {
        Some(index) => {
            if index >= document.body.len() {
                return Err(format!("anchor_block {index} вне тела документа (блоков {})", document.body.len()));
            }
            document
                .insert_block(story, &ParaPath::top(index + 1), block)
                .map_err(|e| format!("вставить блок формулы: {e}"))?;
            Ok(index + 1)
        }
        None => {
            let index = document.body.len();
            document.body.push(Arc::new(block));
            Ok(index)
        }
    }
}

/// Word ожидает абзац после таблицы в конце тела — иначе документ переоткрывается с правкой.
fn ensure_body_ends_with_paragraph(document: &mut Document) {
    if matches!(document.body.last().map(|block| &**block), Some(Block::Table(_))) {
        document.body.push(para_block(Paragraph::new()));
    }
}

/// Inline-уравнение в конец абзаца `anchor`; возвращает путь абзаца.
fn insert_inline(document: &mut Document, nodes: &[MNode], anchor: Option<usize>) -> Result<String, String> {
    let index = anchor.ok_or_else(|| "inline-формуле нужен anchor_block (индекс абзаца)".to_owned())?;
    let story = StoryRef::Body;
    let path = ParaPath::top(index);
    let paragraph = document
        .para(story, &path)
        .ok_or_else(|| format!("блок {index} не абзац: inline-формулу можно вставить только в абзац"))?;
    let off = paragraph.len();
    let props = paragraph.props_of_char(off.saturating_sub(1)).clone();
    document
        .insert_object(&Pos { story, path: path.clone(), off }, equation_object(nodes, false), &props)
        .map_err(|e| format!("inline-уравнение в абзац: {e}"))?;
    Ok(path.to_string())
}

// ── обход формул в модели ────────────────────────────────────────────────────

/// Node-дерево формулы: у импортированных уравнений оно может быть пустым — тогда linear.
fn resolved_nodes(linear: &str, math: &Math) -> Vec<MNode> {
    if math.nodes.is_empty() && !math.linear { parse_linear(linear) } else { math.nodes.clone() }
}

/// Обойти все формулы тела в порядке документа: (путь абзаца, offset, объект).
fn each_equation(document: &Document, mut visit: impl FnMut(&str, usize, &InlineObject)) {
    let story = StoryRef::Body;
    for path in document.para_paths(story) {
        let Some(paragraph) = document.para(story, &path) else { continue };
        for off in paragraph.object_offsets() {
            if let Some(object @ InlineObject::Equation { .. }) = paragraph.object_at(off) {
                visit(&path.to_string(), off, object);
            }
        }
    }
}

fn count_equations(document: &Document) -> usize {
    let mut count = 0;
    each_equation(document, |_, _, _| count += 1);
    count
}

/// Найденная наша нумерованная таблица: формула в нулевой ячейке, номер в первой.
struct NumberedRow {
    number: u32,
    number_path: ParaPath,
}

/// Карта «путь абзаца формулы → номер» для нумерованных таблиц 1×2.
fn numbered_equation_map(document: &Document) -> BTreeMap<String, u32> {
    let mut map = BTreeMap::new();
    for index in 0..document.body.len() {
        let Block::Table(table) = &*document.body[index] else { continue };
        if let Some(number) = table_number(table, index) {
            map.insert(format!("{index}/0/0/0"), number);
        }
    }
    map
}

/// Номер нумерованной таблицы 1×2 (для `list_equations`/`renumber_equations`).
fn numbered_row(table: &Table, top_index: usize) -> Option<NumberedRow> {
    let row = table.rows.first()?;
    if row.cells.len() != 2 || !cell_has_equation(&row.cells[0]) {
        return None;
    }
    for (block_index, block) in row.cells[1].blocks.iter().enumerate() {
        let Block::Para(paragraph) = &**block else { continue };
        let text = paragraph.text.trim();
        let number = text.strip_prefix('(')?.strip_suffix(')')?.trim().parse::<u32>().ok()?;
        return Some(NumberedRow { number, number_path: ParaPath(vec![top_index as u32, 0, 1, block_index as u32]) });
    }
    None
}

fn table_number(table: &Table, top_index: usize) -> Option<u32> {
    numbered_row(table, top_index).map(|row| row.number)
}

fn cell_has_equation(cell: &Cell) -> bool {
    cell.blocks.iter().any(|block| block_has_equation(block))
}

fn block_has_equation(block: &Block) -> bool {
    match block {
        Block::Para(paragraph) => paragraph.objects.iter().any(|object| matches!(object, InlineObject::Equation { .. })),
        Block::Table(table) => table.rows.iter().any(|row| row.cells.iter().any(cell_has_equation)),
    }
}

/// Размер шрифта основного текста (первый непустой абзац тела), иначе — docDefaults.
fn body_font_size(document: &Document) -> Option<f32> {
    let story = StoryRef::Body;
    for path in document.para_paths(story) {
        let Some(paragraph) = document.para(story, &path) else { continue };
        if paragraph.text.trim().is_empty() {
            continue;
        }
        if let Some(size) = paragraph.props_of_char(0).size {
            return Some(size);
        }
    }
    document.styles.default_chr.size
}

fn set_paragraph_text(document: &mut Document, path: &ParaPath, text: &str) -> Result<(), String> {
    let story = StoryRef::Body;
    let paragraph = document.para_mut(story, path).map_err(|e| format!("абзац номера формулы: {e}"))?;
    let len = paragraph.len();
    let props = paragraph.props_of_char(0).clone();
    if len > 0 {
        paragraph.delete_except(0, len, &[]).map_err(|e| format!("очистить номер формулы: {e}"))?;
    }
    paragraph.insert_text(0, text, &props).map_err(|e| format!("записать номер формулы: {e}"))?;
    Ok(())
}
