//! P3 (ADR-0005 §3): вставки project-level формул в документ — `kind="formula"`.
//!
//! Формула вставляется как content-control `w:sdt` с тегом `rde:formula:<insert-id>` (механизм
//! of record §8.5, как остальные живые вставки):
//!
//! * **display** — блок-уровневый `w:sdt` вокруг содержимого: нумерованная таблица 1×2 (§9.4:
//!   формула по центру, номер `(n)` справа) или центрированный абзац;
//! * **inline** — inline-`w:sdt` с `m:oMath`/`w:drawing` в абзаце;
//! * внутри — OMML из кэша формулы (`omml`, редактируется в Word) или `w:drawing` с
//!   `asvg:svgBlip` + PNG-fallback (`svg`, MS-ODRAWXML; writer WordCraft сохраняет blip-extLst —
//!   P2.5) вместе с медиа-частями, связями и Content_Types пакета.
//!
//! Обновление ([`update_formula_inserts`]) пересобирает содержимое sdt из кэшей formula-объекта в
//! документно-локальной нумерации; тело, изменённое вручную (`body_hash` ≠ текущая подпись), не
//! перезаписывается молча (R-DOC-13). Статусы — [`formula_insert_statuses`], обратные ссылки «где
//! вставлено» — [`formula_backlinks`]. Миграция существующего OMML вне реестра — явной командой
//! ([`migrate_formulas`]), автоизменений при открытии нет.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use wordcraft_engine::doc::math::parse_linear;
use wordcraft_engine::doc::math_latex::to_latex;
use wordcraft_engine::doc::{InlineObject, StoryRef};

use crate::{InsertReport, RdeDoc, RdeInsert, DOCUMENT_XML, RDE_DOC_PART};

/// Префикс тега sdt формульной вставки (`rde:formula:<insert-id>`).
pub const FORMULA_TAG_PREFIX: &str = "rde:formula:";
/// `a:ext` URI для `asvg:svgBlip` (MS-ODRAWXML §2.3.1) — векторный источник картинки Word 2016+.
pub const SVG_BLIP_EXT: &str = "{96DAC541-7B7A-43D3-8B79-37D633B846F1}";
/// Пространство имён Office 2016 SVG (`asvg`).
pub const ASVG_NS: &str = "http://schemas.microsoft.com/office/drawing/2016/SVG/main";
/// Relationship-тип картинки.
const IMAGE_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";

// ── модель запроса/статусов ─────────────────────────────────────────────────

/// Бандл кэшей формулы для вставки/обновления (из CAS formula-объекта, ADR-0005 §2.4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct FormulaBody {
    pub formula_id: String,
    /// `act_key` формулы, под который скомпилированы кэши.
    pub act_key: String,
    /// Эффективный LaTeX (кэш для diff/«где вставлено»).
    pub latex: String,
    /// Display-формула (`false` — inline).
    pub display: bool,
    /// Нумеровать `(n)` при вставке/обновлении (политика `numbering.mode == "auto"`).
    pub numbered: bool,
    /// OMML-фрагмент: `m:oMathPara` (display) или `m:oMath` (inline).
    pub omml: Vec<u8>,
    /// Векторный SVG (рендер формулы).
    pub svg: Option<Vec<u8>>,
    /// PNG-fallback для WordCraft/старых Word.
    pub png: Option<Vec<u8>>,
}

/// Запрос на вставку формулы в документ.
#[derive(Clone, Debug)]
pub struct FormulaInsertRequest {
    pub body: FormulaBody,
    /// `omml` — редактируемая формула, `svg` — векторная картинка + PNG-fallback.
    pub render_mode: String,
    /// Блок-якорь (0-based индекс блока тела); `None` — в конец документа.
    pub anchor_block: Option<usize>,
}

/// Разобранная ссылка на формулу: `rde://formula/<id>[@<act_key>]`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormulaTarget {
    pub formula_id: String,
    pub act_key: Option<String>,
}

/// Разобрать `rde://formula/<id>[@<act_key>]`.
pub fn parse_formula_target(uri: &str) -> Option<FormulaTarget> {
    let rest = uri.trim().strip_prefix("rde://formula/")?;
    let rest = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    let (formula_id, act_key) = match rest.split_once('@') {
        Some((id, key)) => (id, Some(key.to_owned())),
        None => (rest, None),
    };
    if formula_id.is_empty() {
        return None;
    }
    Some(FormulaTarget { formula_id: formula_id.to_owned(), act_key })
}

/// Доступность формулы для статуса вставки (резолв делает слой `rde-dag`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormulaAvailability {
    /// Формула активирована: текущий `act_key` (может отличаться от вставленного).
    Current(String),
    /// Формула удалена/не найдена в проекте.
    Missing,
    /// Формулу не вычислить (входы/фрагменты/источник); текст — причина.
    NotComputed(String),
}

/// Состояние формульной вставки в документе.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormulaInsertState {
    /// `act_key` вставки = текущему `act_key` формулы, тело не правили вручную.
    Fresh,
    /// Формула изменилась (`act_key` вставки ≠ текущему) — нужно «Обновить документ».
    Stale,
    /// Формулы-цели нет (удалена/не найдена) — вставка остаётся со статусом.
    MissingTarget,
    /// Формула не вычислена/ошибка входа (`NeverComputed`/`NotComputable`).
    NotComputed,
    /// Тело sdt правили вручную (R-DOC-13) — молча не перезаписываем.
    BodyModified,
}

impl FormulaInsertState {
    pub fn label_ru(self) -> &'static str {
        match self {
            FormulaInsertState::Fresh => "актуальна",
            FormulaInsertState::Stale => "устарела",
            FormulaInsertState::MissingTarget => "цель отсутствует",
            FormulaInsertState::NotComputed => "не вычислена",
            FormulaInsertState::BodyModified => "ручная правка",
        }
    }
}

/// Статус одной формульной вставки для UI/дерева.
#[derive(Clone, Debug)]
pub struct FormulaInsertStatus {
    pub insert_id: String,
    pub formula_id: String,
    pub number: Option<u32>,
    pub render_mode: String,
    pub state: FormulaInsertState,
    /// Причина для NotComputed/устаревания (для инспектора).
    pub detail: String,
}

/// Обратная ссылка «где вставлено» (R-DOC-19).
#[derive(Clone, Debug)]
pub struct FormulaBacklink {
    pub document_id: String,
    pub document_title: String,
    pub document_path: PathBuf,
    pub insert_id: String,
    pub render_mode: String,
    pub number: Option<u32>,
    pub stale: bool,
}

/// Отчёт обновления формульных вставок.
#[derive(Clone, Debug, Default)]
pub struct FormulaUpdateReport {
    pub updated: Vec<String>,
    pub conflicts: Vec<String>,
    pub missing: Vec<String>,
}

/// Отчёт миграции формул документа.
#[derive(Clone, Debug, Default)]
pub struct FormulaMigrationReport {
    pub insert_ids: Vec<String>,
    pub replaced_tables: usize,
    pub replaced_blocks: usize,
    pub replaced_inline: usize,
}

/// Формула документа вне реестра (для явной миграции/подсказки «формул вне реестра: N»).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnregisteredFormula {
    pub block: String,
    pub latex: String,
    pub display: bool,
}

/// План миграции одной формулы документа в project-level объект.
#[derive(Clone, Debug)]
pub struct FormulaMigrationPlan {
    pub formula_id: String,
    pub act_key: String,
    pub latex: String,
    /// OMML-фрагмент из кэша формулы (`m:oMathPara` для display).
    pub omml: Vec<u8>,
}

// ── вставка ─────────────────────────────────────────────────────────────────

/// Вставить формулу в `.doc.rde`: block-sdt (display) или inline-sdt (inline) + запись в
/// `rde:doc`. Кэши приходят из CAS формулы; движок для вставки не запускается.
pub fn add_formula_insert(package: &Path, request: FormulaInsertRequest) -> Result<InsertReport, String> {
    let body = request.body;
    if body.formula_id.is_empty() || body.act_key.is_empty() {
        return Err("формула без formula_id/act_key".to_owned());
    }
    if body.omml.is_empty() {
        return Err("нет OMML-кэша формулы (сначала вычислите формулу)".to_owned());
    }
    let render_mode = normalize_render_mode(&request.render_mode).to_owned();
    if render_mode == "svg" && (body.svg.is_none() || body.png.is_none()) {
        return Err("svg-режим: у формулы нет SVG/PNG-кэшей (пересчитайте формулу)".to_owned());
    }
    let mut entries = super::read_package(package)?.entries;
    let mut rde_doc = super::load_rde_doc_from_entries(&entries)?;
    let document_xml = entries.get(DOCUMENT_XML).map(|bytes| String::from_utf8_lossy(bytes).to_string()).unwrap_or_default();
    let id = super::next_insert_id(&rde_doc.inserts, &document_xml);
    let para_id = super::para_id_for(&format!("{}:formula:{id}", rde_doc.doc_id), rde_doc.inserts.len());
    let number = if body.display && body.numbered { next_formula_number(&rde_doc) } else { None };
    let mut insert = RdeInsert {
        id: id.clone(),
        block: String::new(),
        kind: "formula".to_owned(),
        target: format!("rde://formula/{}@{}", body.formula_id, body.act_key),
        caption: String::new(),
        number,
        hash: Some(body.act_key.clone()),
        cached: body.latex.clone(),
        selector: String::new(),
        value_kind: "formula".to_owned(),
        para_id: Some(para_id.clone()),
        style: String::new(),
        format: String::new(),
        run_style: String::new(),
        render_mode: render_mode.clone(),
        body_hash: None,
    };
    let signature = if render_mode == "svg" { svg_signature(&body) } else { omml_signature(&body.omml)? };
    insert.body_hash = Some(signature);
    let content = if body.display {
        let media = if render_mode == "svg" { Some(ensure_formula_media(&mut entries, &insert.id, &body)?) } else { None };
        build_block_content(&insert, &body, number, &para_id, media)?
    } else {
        let media = if render_mode == "svg" { Some(ensure_formula_media(&mut entries, &insert.id, &body)?) } else { None };
        build_inline_content(&insert, &body, &para_id, media)?
    };
    let document_xml = if body.display {
        super::insert_fragment(&document_xml, &content, request.anchor_block)
    } else {
        match request.anchor_block.and_then(|index| append_inline_to_block(&document_xml, &content, index)) {
            Some(xml) => xml,
            None => super::append_paragraph(&document_xml, &inline_paragraph(&content, &para_id)),
        }
    };
    rde_doc.inserts.push(insert.clone());
    rde_doc.updated_unix = super::now_unix();
    rde_doc.body_hash = rde_hash::sha256(document_xml.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), super::render_rde_doc(&rde_doc).into_bytes());
    super::upsert_custom_props(&mut entries, &rde_doc);
    let info = super::info_from_entries(&entries);
    super::write_entries_atomic(&entries, package)?;
    Ok(InsertReport { insert, package: info })
}

fn normalize_render_mode(mode: &str) -> &'static str {
    if mode.eq_ignore_ascii_case("svg") { "svg" } else { "omml" }
}

/// Следующий документно-локальный номер формулы: максимальный занятый + 1.
fn next_formula_number(rde_doc: &RdeDoc) -> Option<u32> {
    let max = rde_doc.inserts.iter().filter(|insert| insert.kind == "formula").filter_map(|insert| insert.number).max().unwrap_or(0);
    Some(max + 1)
}

/// Содержимое block-sdt: `w:sdt` вокруг нумерованной таблицы 1×2 или центрированного абзаца.
fn build_block_content(
    insert: &RdeInsert,
    body: &FormulaBody,
    number: Option<u32>,
    para_id: &str,
    media: Option<(String, String)>,
) -> Result<String, String> {
    let inner = if insert.render_mode == "svg" {
        let (png_rel, svg_rel) = media.ok_or("svg-режим: нет медиа-связей")?;
        let (cx, cy) = formula_extent_emu(body);
        let drawing = drawing_run_xml(&png_rel, &svg_rel, cx, cy, para_id);
        match number {
            Some(number) => numbered_table_xml(&paragraph_centered(para_id, &drawing), number),
            None => paragraph_centered(para_id, &drawing),
        }
    } else {
        let omml = omml_fragment(&body.omml, body.display)?;
        let math = String::from_utf8_lossy(&omml);
        match number {
            Some(number) => numbered_table_xml(&paragraph_centered(para_id, &math), number),
            None => paragraph_centered(para_id, &math),
        }
    };
    Ok(format!(
        "<w:sdt><w:sdtPr><w:tag w:val=\"{FORMULA_TAG_PREFIX}{}\"/><w:alias w:val=\"формула\"/></w:sdtPr><w:sdtContent>{inner}</w:sdtContent></w:sdt>",
        crate::escape_attr(&insert.id)
    ))
}

/// Содержимое inline-формулы: inline-`w:sdt` с `m:oMath`/`w:drawing` (вставляется в абзац).
fn build_inline_content(
    insert: &RdeInsert,
    body: &FormulaBody,
    para_id: &str,
    media: Option<(String, String)>,
) -> Result<String, String> {
    let inner = if insert.render_mode == "svg" {
        let (png_rel, svg_rel) = media.ok_or("svg-режим: нет медиа-связей")?;
        let (cx, cy) = formula_extent_emu(body);
        drawing_run_xml(&png_rel, &svg_rel, cx, cy, para_id)
    } else {
        String::from_utf8_lossy(&omml_fragment(&body.omml, false)?).to_string()
    };
    Ok(format!(
        "<w:sdt><w:sdtPr><w:tag w:val=\"{FORMULA_TAG_PREFIX}{}\"/><w:alias w:val=\"формула\"/></w:sdtPr><w:sdtContent>{inner}</w:sdtContent></w:sdt>",
        crate::escape_attr(&insert.id)
    ))
}

/// Абзац display-формулы: по центру, без отступов и интервалов, с содержимым (OMML/drawing).
fn paragraph_centered(para_id: &str, content: &str) -> String {
    format!(
        "<w:p w14:paraId=\"{para_id}\"><w:pPr><w:spacing w:before=\"120\" w:after=\"120\"/><w:ind w:left=\"0\" w:right=\"0\" w:firstLine=\"0\"/><w:jc w:val=\"center\"/></w:pPr>{content}</w:p>"
    )
}

/// Абзац для inline-формулы, когда у вставки нет абзаца-якоря.
fn inline_paragraph(content: &str, para_id: &str) -> String {
    format!(
        "<w:p w14:paraId=\"{para_id}\"><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/><w:ind w:left=\"0\" w:right=\"0\" w:firstLine=\"0\"/></w:pPr>{content}</w:p>"
    )
}

/// Дописать inline-sdt в конец абзаца-якоря (индекс блока тела).
fn append_inline_to_block(xml: &str, content: &str, block_index: usize) -> Option<String> {
    let p_start = nth_body_paragraph_start(xml, block_index)?;
    let p_end = xml[p_start..].find("</w:p>")? + p_start;
    let mut out = String::with_capacity(xml.len() + content.len());
    out.push_str(&xml[..p_end]);
    out.push_str(content);
    out.push_str(&xml[p_end..]);
    Some(out)
}

/// Начало N-го абзаца верхнего уровня тела.
fn nth_body_paragraph_start(xml: &str, index: usize) -> Option<usize> {
    let body_open = xml.find("<w:body>")?;
    let mut depth = 1usize;
    let mut block = 0usize;
    let mut pos = body_open + "<w:body>".len();
    while pos < xml.len() {
        let rel = xml[pos..].find('<')?;
        let start = pos + rel;
        let rel_end = xml[start..].find('>')?;
        let end = start + rel_end;
        let tag = &xml[start + 1..end];
        if tag.starts_with('/') {
            depth = depth.saturating_sub(1);
            pos = end + 1;
            continue;
        }
        let self_closing = tag.ends_with('/');
        let declaration = tag.starts_with('?') || tag.starts_with('!');
        let name = tag.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("");
        if depth == 1 && !self_closing && !declaration && name == "w:p" {
            if block == index {
                return Some(start);
            }
            block += 1;
        }
        if !self_closing && !declaration {
            depth += 1;
        }
        pos = end + 1;
    }
    None
}

/// Нумерованная таблица 1×2: формула в левой ячейке, `(n)` справа (§9.4).
fn numbered_table_xml(left: &str, number: u32) -> String {
    let nil = "<w:top w:val=\"nil\" w:sz=\"0\" w:space=\"0\" w:color=\"auto\"/><w:left w:val=\"nil\" w:sz=\"0\" w:space=\"0\" w:color=\"auto\"/>\
               <w:bottom w:val=\"nil\" w:sz=\"0\" w:space=\"0\" w:color=\"auto\"/><w:right w:val=\"nil\" w:sz=\"0\" w:space=\"0\" w:color=\"auto\"/>\
               <w:insideH w:val=\"nil\" w:sz=\"0\" w:space=\"0\" w:color=\"auto\"/><w:insideV w:val=\"nil\" w:sz=\"0\" w:space=\"0\" w:color=\"auto\"/>";
    format!(
        "<w:tbl><w:tblPr><w:tblW w:w=\"9355\" w:type=\"dxa\"/><w:tblBorders>{nil}</w:tblBorders>\
         <w:tblLayout w:type=\"fixed\"/><w:tblCellMar><w:top w:w=\"0\" w:type=\"dxa\"/><w:left w:w=\"0\" w:type=\"dxa\"/>\
         <w:bottom w:w=\"0\" w:type=\"dxa\"/><w:right w:w=\"0\" w:type=\"dxa\"/></w:tblCellMar>\
         <w:tblLook w:val=\"04A0\" w:firstRow=\"1\" w:lastRow=\"0\" w:firstColumn=\"1\" w:lastColumn=\"0\" w:noHBand=\"0\" w:noVBand=\"1\"/></w:tblPr>\
         <w:tblGrid><w:gridCol w:w=\"8505\"/><w:gridCol w:w=\"850\"/></w:tblGrid>\
         <w:tr><w:tc><w:tcPr><w:tcW w:w=\"8505\" w:type=\"dxa\"/></w:tcPr>{left}</w:tc>\
         <w:tc><w:tcPr><w:tcW w:w=\"850\" w:type=\"dxa\"/></w:tcPr><w:p><w:pPr><w:spacing w:before=\"0\" w:after=\"0\"/>\
         <w:ind w:left=\"0\" w:right=\"0\" w:firstLine=\"0\"/><w:jc w:val=\"right\"/></w:pPr>\
         <w:r><w:t xml:space=\"preserve\">({number})</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"
    )
}

/// `w:drawing` с PNG-preview и `asvg:svgBlip` (вектор) в `a:blip/a:extLst` + PNG-fallback.
fn drawing_run_xml(png_rel: &str, svg_rel: &str, cx: u64, cy: u64, para_id: &str) -> String {
    let doc_pr = para_id.bytes().fold(1u32, |acc, b| acc.wrapping_mul(31).saturating_add(b as u32)).max(1);
    format!(
        "<w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\"><wp:extent cx=\"{cx}\" cy=\"{cy}\"/>\
         <wp:docPr id=\"{doc_pr}\" name=\"Формула\"/><wp:cNvGraphicFramePr/>\
         <a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">\
         <a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
         <pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
         <pic:nvPicPr><pic:cNvPr id=\"0\" name=\"formula.svg\"/><pic:cNvPicPr/></pic:nvPicPr>\
         <pic:blipFill><a:blip r:embed=\"{png_rel}\"><a:extLst><a:ext uri=\"{SVG_BLIP_EXT}\">\
         <asvg:svgBlip xmlns:asvg=\"{ASVG_NS}\" r:embed=\"{svg_rel}\"/></a:ext></a:extLst></a:blip>\
         <a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
         <pic:spPr bwMode=\"auto\"><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm>\
         <a:prstGeom prst=\"rect\"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic>\
         </wp:inline></w:drawing></w:r>"
    )
}

/// Размер формулы в EMU: из PNG (IHDR), иначе из SVG (`width`/`height`, допускается `pt`).
fn formula_extent_emu(body: &FormulaBody) -> (u64, u64) {
    if let Some(png) = &body.png {
        if png.starts_with(b"\x89PNG") {
            let (cx, cy) = super::image_extent_emu(png, "png");
            if cx > 0 && cy > 0 {
                return (cx, cy);
            }
        }
    }
    if let Some(svg) = &body.svg {
        let text = String::from_utf8_lossy(svg);
        let attr = |name: &str| -> Option<f64> {
            let index = text.find(&format!("{name}=\""))?;
            let rest = &text[index + name.len() + 2..];
            let end = rest.find('"')?;
            let value = rest[..end].trim();
            let value = value.strip_suffix("pt").or_else(|| value.strip_suffix("px")).unwrap_or(value);
            value.parse().ok()
        };
        if let (Some(w), Some(h)) = (attr("width"), attr("height")) {
            if w > 0.0 && h > 0.0 {
                return ((w * 9525.0) as u64, (h * 9525.0) as u64);
            }
        }
    }
    (400 * 9525, 300 * 9525)
}

/// PNG/SVG-кэши формулы как части пакета: медиа-файлы, связи и Content_Types.
/// Возвращает id связей `(png, svg)` для записи в `w:drawing`.
fn ensure_formula_media(
    entries: &mut BTreeMap<String, Vec<u8>>,
    insert_id: &str,
    body: &FormulaBody,
) -> Result<(String, String), String> {
    let png = body.png.as_ref().ok_or("svg-режим: нет PNG-кэша формулы")?;
    let svg = body.svg.as_ref().ok_or("svg-режим: нет SVG-кэша формулы")?;
    let png_part = format!("word/media/rde-formula-{insert_id}.png");
    let svg_part = format!("word/media/rde-formula-{insert_id}.svg");
    entries.insert(png_part.clone(), png.clone());
    entries.insert(svg_part.clone(), svg.clone());
    super::ensure_default_content_type(entries, "png");
    super::ensure_default_content_type(entries, "svg");
    let mut rels = entries
        .get(super::rels_path_for_document())
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .unwrap_or_default();
    let png_rel = ensure_relationship(&mut rels, png_part.trim_start_matches("word/"));
    let svg_rel = ensure_relationship(&mut rels, svg_part.trim_start_matches("word/"));
    entries.insert(super::rels_path_for_document().to_owned(), rels.into_bytes());
    Ok((png_rel, svg_rel))
}

/// Связь картинки на `target`: существующая или новая (`rIdRdeN`).
fn ensure_relationship(rels: &mut String, target: &str) -> String {
    if let Some(id) = relationship_id_for_target(rels, target) {
        return id;
    }
    let id = super::next_relationship_id(rels);
    let relationship = format!("<Relationship Id=\"{id}\" Type=\"{IMAGE_REL}\" Target=\"{target}\"/>");
    match rels.rfind("</Relationships>") {
        Some(index) => rels.insert_str(index, &relationship),
        None => rels.push_str(&relationship),
    }
    id
}

fn relationship_id_for_target(rels: &str, target: &str) -> Option<String> {
    let marker = format!("Target=\"{target}\"");
    let index = rels.find(&marker)?;
    let before = &rels[..index];
    let id_at = before.rfind("Id=\"")? + "Id=\"".len();
    let id = &before[id_at..];
    Some(id[..id.find('"')?].to_owned())
}

fn relationship_target(rels: &str, id: &str) -> Option<String> {
    let marker = format!("Id=\"{id}\"");
    let index = rels.find(&marker)?;
    let rest = &rels[index + marker.len()..];
    let target_marker = "Target=\"";
    let at = rest.find(target_marker)? + target_marker.len();
    let after = &rest[at..];
    let end = after.find('"')?;
    Some(after[..end].to_owned())
}

// ── подписи тела (R-DOC-13) ─────────────────────────────────────────────────

fn omml_signature(omml: &[u8]) -> Result<String, String> {
    if omml.is_empty() {
        return Err("пустой OMML-фрагмент".to_owned());
    }
    Ok(rde_hash::sha256(omml).hex())
}

fn svg_signature(body: &FormulaBody) -> String {
    let mut bytes = Vec::with_capacity(body.png.as_ref().map_or(0, Vec::len) + body.svg.as_ref().map_or(0, Vec::len));
    bytes.extend_from_slice(body.png.as_deref().unwrap_or_default());
    bytes.extend_from_slice(body.svg.as_deref().unwrap_or_default());
    rde_hash::sha256(&bytes).hex()
}

/// OMML-фрагмент кэша: `m:oMathPara` (display) или `m:oMath` (inline); для display допустим
/// inline-фрагмент (оборачиваем в `m:oMathPara`).
fn omml_fragment(omml: &[u8], display: bool) -> Result<Vec<u8>, String> {
    let text = String::from_utf8_lossy(omml).to_string();
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err("пустой OMML-фрагмент".to_owned());
    }
    if display && !trimmed.starts_with("<m:oMathPara") && trimmed.starts_with("<m:oMath") {
        Ok(format!("<m:oMathPara>{trimmed}</m:oMathPara>").into_bytes())
    } else {
        Ok(trimmed.as_bytes().to_vec())
    }
}

/// Подпись текущего тела вставки в пакете: OMML-фрагмент (omml) или пара PNG+SVG (svg).
pub(crate) fn body_signature(entries: &BTreeMap<String, Vec<u8>>, insert_id: &str) -> Option<String> {
    let document = entries.get(DOCUMENT_XML)?;
    let xml = String::from_utf8_lossy(document);
    let sdt = find_formula_sdt(&xml, insert_id)?;
    let content = &xml[sdt.content_start..sdt.content_end];
    if content.contains("svgBlip") || content.contains("w:drawing") {
        let rels = entries
            .get(super::rels_path_for_document())
            .map(|bytes| String::from_utf8_lossy(bytes).to_string())
            .unwrap_or_default();
        let mut png: Option<Vec<u8>> = None;
        let mut svg: Option<Vec<u8>> = None;
        for id in embed_ids(content) {
            let Some(target) = relationship_target(&rels, &id) else { continue };
            let part = format!("word/{target}");
            let Some(bytes) = entries.get(&part) else { continue };
            if target.to_ascii_lowercase().ends_with(".svg") {
                svg = Some(bytes.clone());
            } else {
                png = Some(bytes.clone());
            }
        }
        let mut bytes = png?;
        bytes.extend_from_slice(&svg?);
        return Some(rde_hash::sha256(&bytes).hex());
    }
    let omml = extract_omml_from_content(content)?;
    Some(rde_hash::sha256(omml.as_bytes()).hex())
}

/// `r:embed`-идентификаторы содержимого (в порядке появления).
fn embed_ids(content: &str) -> Vec<String> {
    let marker = "r:embed=\"";
    let mut ids = Vec::new();
    let mut rest = content;
    while let Some(index) = rest.find(marker) {
        let after = &rest[index + marker.len()..];
        let Some(end) = after.find('"') else { break };
        ids.push(after[..end].to_owned());
        rest = &after[end..];
    }
    ids
}

/// OMML-фрагмент внутри содержимого sdt (для подписи/сравнения).
fn extract_omml_from_content(content: &str) -> Option<String> {
    let (start, close) = match content.find("<m:oMathPara") {
        Some(start) => (start, "</m:oMathPara>"),
        None => (content.find("<m:oMath")?, "</m:oMath>"),
    };
    let end = content[start..].find(close)? + start + close.len();
    Some(content[start..end].to_owned())
}

// ── обновление и статусы ────────────────────────────────────────────────────

/// Пересобрать формульные вставки документа из кэшей formula-объектов: заменить содержимое sdt,
/// обновить `hash`/`cached`/`number`/`body_hash` в `rde:doc`, перенумеровать документно-локальные
/// номера. Тело, изменённое вручную (подпись ≠ `body_hash`), не перезаписывается — конфликт.
pub fn update_formula_inserts(package: &Path, bodies: &BTreeMap<String, FormulaBody>) -> Result<FormulaUpdateReport, String> {
    let mut entries = super::read_package(package)?.entries;
    let mut rde_doc = super::load_rde_doc_from_entries(&entries)?;
    let Some(document) = entries.get(DOCUMENT_XML) else { return Ok(FormulaUpdateReport::default()) };
    let mut document_xml = String::from_utf8_lossy(document).to_string();
    let mut report = FormulaUpdateReport::default();
    let mut formulas: Vec<RdeInsert> = rde_doc.inserts.iter().filter(|insert| insert.kind == "formula").cloned().collect();
    if formulas.is_empty() {
        return Ok(report);
    }
    formulas.sort_by_key(|insert| formula_tag_position(&document_xml, &insert.id).unwrap_or(usize::MAX));
    // Документно-локальная нумерация (ADR-0005 §3.1): по порядку в документе.
    let mut number = 0u32;
    let mut numbers: BTreeMap<String, u32> = BTreeMap::new();
    for insert in &formulas {
        let body = parse_formula_target(&insert.target).and_then(|target| bodies.get(&target.formula_id));
        let numbered = body.map(|body| body.display && body.numbered).unwrap_or(insert.number.is_some());
        if numbered {
            number += 1;
            numbers.insert(insert.id.clone(), number);
        }
    }
    let mut changed = false;
    for index in 0..rde_doc.inserts.len() {
        if rde_doc.inserts[index].kind != "formula" {
            continue;
        }
        let insert_snapshot = rde_doc.inserts[index].clone();
        let Some(target) = parse_formula_target(&insert_snapshot.target) else { continue };
        let Some(body) = bodies.get(&target.formula_id) else {
            report.missing.push(insert_snapshot.id.clone());
            continue;
        };
        let current_signature = body_signature(&entries, &insert_snapshot.id);
        if insert_snapshot.body_hash.is_some() && current_signature.as_deref() != insert_snapshot.body_hash.as_deref() {
            // R-DOC-13: ручная правка в теле — молча не перезаписываем.
            report.conflicts.push(insert_snapshot.id.clone());
            continue;
        }
        let new_number = numbers.get(&insert_snapshot.id).copied();
        let expected_signature = if insert_snapshot.render_mode == "svg" { svg_signature(body) } else { omml_signature(&body.omml)? };
        let unchanged = insert_snapshot.hash.as_deref() == Some(body.act_key.as_str())
            && insert_snapshot.number == new_number
            && current_signature.as_deref() == Some(expected_signature.as_str());
        if unchanged {
            continue;
        }
        let para_id = insert_snapshot
            .para_id
            .clone()
            .unwrap_or_else(|| super::para_id_for(&format!("{}:formula:{}", rde_doc.doc_id, insert_snapshot.id), 0));
        let media = if insert_snapshot.render_mode == "svg" { Some(ensure_formula_media(&mut entries, &insert_snapshot.id, body)?) } else { None };
        let content = if is_block_sdt(&document_xml, &insert_snapshot.id) {
            build_block_content(&insert_snapshot, body, new_number, &para_id, media)?
        } else {
            build_inline_content(&insert_snapshot, body, &para_id, media)?
        };
        let Some(sdt) = find_formula_sdt(&document_xml, &insert_snapshot.id) else { continue };
        let mut out = String::with_capacity(document_xml.len() + content.len());
        out.push_str(&document_xml[..sdt.content_start]);
        out.push_str(&content);
        out.push_str(&document_xml[sdt.content_end..]);
        document_xml = out;
        let insert = &mut rde_doc.inserts[index];
        insert.hash = Some(body.act_key.clone());
        insert.cached = body.latex.clone();
        insert.number = new_number;
        insert.body_hash = Some(expected_signature);
        report.updated.push(insert_snapshot.id.clone());
        changed = true;
    }
    if !changed {
        return Ok(report);
    }
    rde_doc.updated_unix = super::now_unix();
    rde_doc.body_hash = rde_hash::sha256(document_xml.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), super::render_rde_doc(&rde_doc).into_bytes());
    super::upsert_custom_props(&mut entries, &rde_doc);
    super::write_entries_atomic(&entries, package)?;
    Ok(report)
}

/// Статусы формульных вставок документа (R-DOC-13/19): доступность формулы задаёт вызывающий
/// слой (`rde-dag`), тело сравнивается с подписью, сохранённой при последней записи RDE.
pub fn formula_insert_statuses(
    package: &Path,
    availability: &BTreeMap<String, FormulaAvailability>,
) -> Result<Vec<FormulaInsertStatus>, String> {
    let entries = super::read_package(package)?.entries;
    let rde_doc = super::load_rde_doc_from_entries(&entries)?;
    let mut out = Vec::new();
    for insert in rde_doc.inserts.iter().filter(|insert| insert.kind == "formula") {
        let Some(target) = parse_formula_target(&insert.target) else {
            out.push(FormulaInsertStatus {
                insert_id: insert.id.clone(),
                formula_id: String::new(),
                number: insert.number,
                render_mode: insert.render_mode.clone(),
                state: FormulaInsertState::NotComputed,
                detail: "некорректная ссылка на формулу".to_owned(),
            });
            continue;
        };
        let (state, detail) = match availability.get(&target.formula_id) {
            None | Some(FormulaAvailability::Missing) => (FormulaInsertState::MissingTarget, "формула не найдена в проекте".to_owned()),
            Some(FormulaAvailability::NotComputed(reason)) => (FormulaInsertState::NotComputed, reason.clone()),
            Some(FormulaAvailability::Current(act_key)) => {
                let current_signature = body_signature(&entries, &insert.id);
                if insert.body_hash.is_some() && current_signature.as_deref() != insert.body_hash.as_deref() {
                    (FormulaInsertState::BodyModified, "тело формулы правили вручную".to_owned())
                } else if insert.hash.as_deref() != Some(act_key.as_str()) {
                    (FormulaInsertState::Stale, "формула изменилась — обновите документ".to_owned())
                } else {
                    (FormulaInsertState::Fresh, String::new())
                }
            }
        };
        out.push(FormulaInsertStatus {
            insert_id: insert.id.clone(),
            formula_id: target.formula_id,
            number: insert.number,
            render_mode: insert.render_mode.clone(),
            state,
            detail,
        });
    }
    Ok(out)
}

/// Обратные ссылки «где вставлено» (R-DOC-19): документы проекта → вставки формулы.
/// `documents` — (id, заголовок, путь пакета) из проекта.
pub fn formula_backlinks(documents: &[(String, String, PathBuf)], formula_id: &str) -> Result<Vec<FormulaBacklink>, String> {
    let mut out = Vec::new();
    for (document_id, document_title, path) in documents {
        let Ok(rde_doc) = super::load_rde_doc(path) else { continue };
        for insert in rde_doc.inserts.iter().filter(|insert| insert.kind == "formula") {
            let Some(target) = parse_formula_target(&insert.target) else { continue };
            if target.formula_id != formula_id {
                continue;
            }
            let stale = target.act_key.as_deref() != insert.hash.as_deref();
            out.push(FormulaBacklink {
                document_id: document_id.clone(),
                document_title: document_title.clone(),
                document_path: path.clone(),
                insert_id: insert.id.clone(),
                render_mode: insert.render_mode.clone(),
                number: insert.number,
                stale,
            });
        }
    }
    Ok(out)
}

// ── миграция существующего OMML вне реестра (явная команда) ──────────────────

/// Формулы документа вне реестра (`m:oMath`/`m:oMathPara` не внутри `rde:formula:*` sdt), в
/// порядке документа: LaTeX из модели WordCraft (автоизменений при открытии нет).
pub fn unregistered_formulas(package: &Path) -> Result<Vec<UnregisteredFormula>, String> {
    let equations = model_equations(package)?;
    let Some(document) = super::read_package(package)?.entries.get(DOCUMENT_XML).cloned() else {
        return Ok(Vec::new());
    };
    let xml = String::from_utf8_lossy(&document).to_string();
    let raw = raw_equations(&xml);
    if raw.len() != equations.len() {
        return Err(format!("не удалось сопоставить формулы документа: OMML ×{}, модель ×{}", raw.len(), equations.len()));
    }
    Ok(raw
        .iter()
        .zip(equations)
        .filter(|(raw, _)| !raw.registered)
        .map(|(_, equation)| UnregisteredFormula { block: equation.block, latex: equation.latex, display: equation.display })
        .collect())
}

/// Явная миграция (ADR-0005 §7): заменить OMML вне реестра вставками-ссылками на formula-объекты.
/// Идемпотентна по построению: переносит только формулы, ещё не обёрнутые в `rde:formula:*`.
/// Нумерованная таблица 1×2 заменяется канонической формульной таблицей с тем же номером.
pub fn migrate_formulas(package: &Path, plans: &[FormulaMigrationPlan]) -> Result<FormulaMigrationReport, String> {
    let mut entries = super::read_package(package)?.entries;
    let mut rde_doc = super::load_rde_doc_from_entries(&entries)?;
    let document = entries.get(DOCUMENT_XML).cloned().ok_or("в пакете нет word/document.xml")?;
    let xml = String::from_utf8_lossy(&document).to_string();
    let equations = model_equations(package)?;
    let raw = raw_equations(&xml);
    if raw.len() != equations.len() {
        return Err(format!("не удалось сопоставить формулы: OMML ×{}, модель ×{}", raw.len(), equations.len()));
    }
    let candidates: Vec<(RawEquation, ModelEquation)> = raw.into_iter().zip(equations).filter(|(raw, _)| !raw.registered).collect();
    if candidates.len() != plans.len() {
        return Err(format!("миграция: формул вне реестра {}, планов {}", candidates.len(), plans.len()));
    }
    let mut report = FormulaMigrationReport::default();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for ((raw, _model), plan) in candidates.iter().zip(plans) {
        let id = super::next_insert_id(&rde_doc.inserts, &xml);
        let para_id = super::para_id_for(&format!("{}:formula:{id}", rde_doc.doc_id), rde_doc.inserts.len());
        let body = FormulaBody {
            formula_id: plan.formula_id.clone(),
            act_key: plan.act_key.clone(),
            latex: plan.latex.clone(),
            display: raw.display,
            numbered: raw.number.is_some(),
            omml: plan.omml.clone(),
            svg: None,
            png: None,
        };
        let mut insert = RdeInsert {
            id: id.clone(),
            block: String::new(),
            kind: "formula".to_owned(),
            target: format!("rde://formula/{}@{}", plan.formula_id, plan.act_key),
            caption: String::new(),
            number: raw.number,
            hash: Some(plan.act_key.clone()),
            cached: plan.latex.clone(),
            selector: String::new(),
            value_kind: "formula".to_owned(),
            para_id: None,
            style: String::new(),
            format: String::new(),
            run_style: String::new(),
            render_mode: "omml".to_owned(),
            body_hash: Some(omml_signature(&plan.omml)?),
        };
        let content = if raw.display {
            insert.para_id = Some(para_id.clone());
            build_block_content(&insert, &body, raw.number, &para_id, None)?
        } else {
            build_inline_content(&insert, &body, &para_id, None)?
        };
        if raw.display {
            if let Some((start, end, _)) = raw.numbered_table {
                report.replaced_tables += 1;
                edits.push((start, end, content));
            } else if raw.paragraph_only {
                report.replaced_blocks += 1;
                let start = raw.paragraph_start.ok_or("миграция: у формулы нет абзаца")?;
                let end = raw.paragraph_end.ok_or("миграция: абзац формулы не закрыт")?;
                edits.push((start, end, content));
            } else {
                report.replaced_inline += 1;
                edits.push((raw.start, raw.end, content));
            }
        } else {
            report.replaced_inline += 1;
            edits.push((raw.start, raw.end, content));
        }
        rde_doc.inserts.push(insert);
        report.insert_ids.push(id);
    }
    edits.sort_by_key(|(start, _, _)| *start);
    let mut out = xml;
    for (start, end, replacement) in edits.iter().rev() {
        if *start > out.len() || *end > out.len() || start > end {
            return Err("миграция: диапазон замены вне документа".to_owned());
        }
        out.replace_range(*start..*end, replacement);
    }
    rde_doc.updated_unix = super::now_unix();
    rde_doc.body_hash = rde_hash::sha256(out.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), out.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), super::render_rde_doc(&rde_doc).into_bytes());
    super::upsert_custom_props(&mut entries, &rde_doc);
    super::write_entries_atomic(&entries, package)?;
    Ok(report)
}

/// Формула в модели документа (порядок документа).
struct ModelEquation {
    block: String,
    latex: String,
    display: bool,
}

/// Все формулы тела в порядке документа (включая уже зарегистрированные вставки) — через модель
/// WordCraft, чтобы не разбирать OMML вручную.
fn model_equations(package: &Path) -> Result<Vec<ModelEquation>, String> {
    let temp = super::temp_docx_path("formula-equations");
    let result = (|| -> Result<Vec<ModelEquation>, String> {
        super::extract_docx(package, &temp)?;
        let doc = super::open_engine(&temp)?;
        let story = StoryRef::Body;
        let mut out = Vec::new();
        for path in doc.para_paths(story) {
            let Some(paragraph) = doc.para(story, &path) else { continue };
            for object in &paragraph.objects {
                if let InlineObject::Equation { linear, display, math } = object {
                    let nodes = if math.nodes.is_empty() { parse_linear(linear) } else { math.nodes.clone() };
                    out.push(ModelEquation { block: path.to_string(), latex: to_latex(&nodes), display: *display });
                }
            }
        }
        Ok(out)
    })();
    let _ = fs::remove_file(&temp);
    result
}

/// Сырое OMML-вхождение в `word/document.xml`: границы, макет, регистрация в реестре.
#[derive(Clone, Debug)]
struct RawEquation {
    start: usize,
    end: usize,
    display: bool,
    registered: bool,
    number: Option<u32>,
    numbered_table: Option<(usize, usize, u32)>,
    paragraph_start: Option<usize>,
    paragraph_end: Option<usize>,
    paragraph_only: bool,
}

#[derive(Clone)]
struct OpenTag {
    name: String,
    start: usize,
}

/// Найти все `m:oMath`/`m:oMathPara` в документе (документный порядок) с контекстом.
fn raw_equations(xml: &str) -> Vec<RawEquation> {
    let mut stack: Vec<OpenTag> = Vec::new();
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < xml.len() {
        let Some(rel) = xml[pos..].find('<') else { break };
        let start = pos + rel;
        let Some(rel_end) = xml[start..].find('>') else { break };
        let end = start + rel_end;
        let tag = &xml[start + 1..end];
        let closing = tag.starts_with('/');
        let declaration = tag.starts_with('?') || tag.starts_with('!');
        let self_closing = tag.ends_with('/');
        let name = tag.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("").to_string();
        if closing {
            if let Some(open) = stack.pop() {
                let equation = matches!(open.name.as_str(), "m:oMathPara" | "m:oMath");
                // `m:oMath` внутри `m:oMathPara` — та же display-формула (модель читает её одной).
                let nested_in_para = open.name == "m:oMath" && stack.iter().any(|open| open.name == "m:oMathPara");
                if equation && open.name == name && !nested_in_para {
                    let display = name == "m:oMathPara";
                    let registered = stack.iter().any(|open| {
                        open.name == "w:sdt" && xml[open.start..end].contains("w:tag w:val=\"rde:formula:")
                    });
                    let paragraph = stack.iter().rev().find(|open| open.name == "w:p");
                    let paragraph_start = paragraph.map(|open| open.start);
                    let paragraph_end = paragraph.map(|_| end + 1);
                    let numbered_table = ancestor_numbered_table(xml, &stack, end + 1);
                    let number = numbered_table.map(|(_, _, number)| number);
                    let paragraph_only = paragraph_start.is_some_and(|p_start| paragraph_is_only_math(xml, p_start, end + 1, open.start));
                    out.push(RawEquation {
                        start: open.start,
                        end: end + 1,
                        display,
                        registered,
                        number,
                        numbered_table,
                        paragraph_start,
                        paragraph_end,
                        paragraph_only,
                    });
                }
            }
            pos = end + 1;
            continue;
        }
        if !self_closing && !declaration {
            stack.push(OpenTag { name, start });
        }
        pos = end + 1;
    }
    out
}

/// Нумерованная таблица 1×2 вокруг формулы: (границы таблицы, номер).
fn ancestor_numbered_table(xml: &str, stack: &[OpenTag], equation_end: usize) -> Option<(usize, usize, u32)> {
    let table = stack.iter().rev().find(|open| open.name == "w:tbl")?;
    let start = table.start;
    let close = "</w:tbl>";
    let end = xml[start..].find(close)? + start + close.len();
    let after = &xml[equation_end..end];
    let mut rest = after;
    while let Some(index) = next_text_run(rest) {
        let Some(open_end) = rest[index..].find('>') else { break };
        let inner_start = index + open_end + 1;
        let Some(close_rel) = rest[inner_start..].find("</w:t>") else { break };
        if let Some(number) = parse_paren_number(rest[inner_start..inner_start + close_rel].trim()) {
            return Some((start, end, number));
        }
        rest = &rest[inner_start + close_rel..];
    }
    None
}

/// Начало следующего текстового run (`<w:t>`/`<w:t ...>`, не `<w:tc>`/`<w:tbl>`).
fn next_text_run(text: &str) -> Option<usize> {
    let plain = text.find("<w:t>");
    let with_attrs = text.find("<w:t ");
    match (plain, with_attrs) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    }
}

/// Разобрать «(n)» → n.
fn parse_paren_number(text: &str) -> Option<u32> {
    let inner = text.strip_prefix('(')?.strip_suffix(')')?;
    inner.trim().parse().ok()
}

/// Абзац содержит только формулу (и `w:pPr`)? — миграция заменит абзац целиком.
fn paragraph_is_only_math(xml: &str, paragraph_start: usize, paragraph_end: usize, math_start: usize) -> bool {
    let paragraph = &xml[paragraph_start..paragraph_end];
    let rel_math = math_start.saturating_sub(paragraph_start);
    let paragraph_tail = &xml[math_start..paragraph_end];
    let math_len = if let Some(at) = paragraph_tail.find("</m:oMathPara>") {
        at + "</m:oMathPara>".len()
    } else if let Some(at) = paragraph_tail.find("</m:oMath>") {
        at + "</m:oMath>".len()
    } else {
        return false;
    };
    let before = &paragraph[..rel_math];
    let after = &paragraph_tail[math_len..];
    let rest = format!("{before}{after}");
    let stripped = strip_between(&rest, "<w:pPr>", "</w:pPr>");
    !stripped.contains("<w:t") && !stripped.contains("<w:drawing")
}

/// Убрать из строки все фрагменты между `open` и `close` (включительно).
fn strip_between(text: &str, open: &str, close: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(index) = rest.find(open) {
        out.push_str(&rest[..index]);
        match rest[index..].find(close) {
            Some(end) => rest = &rest[index + end + close.len()..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

// ── поиск sdt формулы в document.xml ─────────────────────────────────────────

/// Границы sdt формулы в `word/document.xml`.
struct FormulaSdt {
    content_start: usize,
    content_end: usize,
}

/// Найти sdt формулы по тегу `rde:formula:<id>`.
fn find_formula_sdt(xml: &str, insert_id: &str) -> Option<FormulaSdt> {
    let tag = format!("<w:tag w:val=\"{FORMULA_TAG_PREFIX}{insert_id}\"");
    let tag_index = xml.find(&tag)?;
    let sdt_start = xml[..tag_index].rfind("<w:sdt>")?;
    let content_marker = "<w:sdtContent>";
    let content_start = xml[sdt_start..].find(content_marker)? + sdt_start + content_marker.len();
    let content_end = xml[content_start..].find("</w:sdtContent>")? + content_start;
    Some(FormulaSdt { content_start, content_end })
}

/// Позиция тега формулы в документе (для документного порядка нумерации).
fn formula_tag_position(xml: &str, insert_id: &str) -> Option<usize> {
    xml.find(&format!("<w:tag w:val=\"{FORMULA_TAG_PREFIX}{insert_id}\""))
}

/// Блок-уровневый ли sdt формулы (прямой потомок `w:body`)?
fn is_block_sdt(xml: &str, insert_id: &str) -> bool {
    let tag = format!("<w:tag w:val=\"{FORMULA_TAG_PREFIX}{insert_id}\"");
    let Some(tag_index) = xml.find(&tag) else { return false };
    let Some(sdt_start) = xml[..tag_index].rfind("<w:sdt>") else { return false };
    let Some(body_open) = xml.find("<w:body>") else { return false };
    let mut depth = 1usize;
    let mut pos = body_open + "<w:body>".len();
    while pos < xml.len() {
        let Some(rel) = xml[pos..].find('<') else { break };
        let start = pos + rel;
        if start >= sdt_start {
            return depth == 1;
        }
        let Some(rel_end) = xml[start..].find('>') else { break };
        let end = start + rel_end;
        let tag = &xml[start + 1..end];
        if tag.starts_with('/') {
            depth = depth.saturating_sub(1);
            pos = end + 1;
            continue;
        }
        let self_closing = tag.ends_with('/');
        let declaration = tag.starts_with('?') || tag.starts_with('!');
        if !self_closing && !declaration {
            depth += 1;
        }
        pos = end + 1;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_formula_targets() {
        let target = parse_formula_target("rde://formula/01ABC@deadbeef").unwrap();
        assert_eq!(target.formula_id, "01ABC");
        assert_eq!(target.act_key.as_deref(), Some("deadbeef"));
        let plain = parse_formula_target("rde://formula/01ABC").unwrap();
        assert_eq!(plain.formula_id, "01ABC");
        assert!(plain.act_key.is_none());
        assert!(parse_formula_target("rde://artifact/01/x").is_none());
        assert!(parse_formula_target("rde://formula/").is_none());
    }

    #[test]
    fn parses_paren_numbers() {
        assert_eq!(parse_paren_number("(12)"), Some(12));
        assert_eq!(parse_paren_number("( 3 )"), Some(3));
        assert!(parse_paren_number("рис. 3").is_none());
    }

    #[test]
    fn raw_equations_reads_numbered_table_and_ignores_nested_math() {
        let xml = "<w:document><w:body><w:tbl><w:tr><w:tc><w:p><m:oMathPara><m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></m:oMathPara></w:p></w:tc><w:tc><w:p><w:r><w:t>(7)</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>";
        let raw = raw_equations(xml);
        assert_eq!(raw.len(), 1, "nested m:oMath не считается отдельно: {raw:?}");
        assert!(raw[0].display);
        assert_eq!(raw[0].number, Some(7));
        assert!(raw[0].numbered_table.is_some(), "таблица 1×2 распознана: {raw:?}");
        // В левой ячейке нумерованной таблицы — только формула (абзац «чистый»), но миграция
        // выбирает замену таблицы целиком (numbered_table имеет приоритет над paragraph_only).
        assert!(raw[0].paragraph_only);
    }
}
