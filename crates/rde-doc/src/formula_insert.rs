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

use crate::formula_numbering::{FormulaNumbering, number_formulas};
use crate::xml_fields::{is_punctuation_only, literal_text, mteqn_cached_number, paragraph_bounds, visible_text};
use crate::{InsertReport, RdeDoc, RdeInsert, DOCUMENT_XML, RDE_DOC_PART};

/// Префикс тега sdt формульной вставки (`rde:formula:<insert-id>`).
pub const FORMULA_TAG_PREFIX: &str = "rde:formula:";
/// Источник номера вставки (ADR-0019): номер сохранён из Word при миграции — «Обновить документ»
/// его не перенумеровывает (перенумеровка — явная смена схемы в `renumber_formula_numbers`).
pub const NUMBER_SOURCE_WORD: &str = "word";
/// Источник номера вставки: номер задан вручную (резерв для UI; тоже не перенумеровывается).
pub const NUMBER_SOURCE_MANUAL: &str = "manual";
/// Источник номера вставки: формула сознательно не нумеруется (нумерация display отключена).
pub const NUMBER_SOURCE_OFF: &str = "off";

/// Номер вставки зафиксирован внешним источником и не пересчитывается автоматически.
fn number_locked(insert: &RdeInsert) -> bool {
    matches!(insert.number_source.as_str(), NUMBER_SOURCE_WORD | NUMBER_SOURCE_MANUAL)
}
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
    /// Куда вставлять (P9): `w14:paraId` абзаца + позиция каретки для inline; для display —
    /// после блока, содержащего абзац. [`InsertAnchor::end`] — в конец документа (прежнее
    /// поведение P3); `legacy_block` — совместимость со старым индексом блока тела.
    pub anchor: crate::InsertAnchor,
    /// Схема нумерации документа (ADR-0018): сквозная (по умолчанию) или по разделам. Номер
    /// назначается документно-локально и пересчитывается после вставки (см.
    /// [`renumber_formula_numbers`]).
    pub numbering: FormulaNumbering,
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
    /// Отображаемый номер без скобок (`"2.1"`, `"А.1"`); `None` — отображается `number`.
    pub number_text: Option<String>,
    /// Источник номера (ADR-0019): пусто — схема документа, `word` — сохранён из Word,
    /// `manual` — задан вручную, `off` — формула не нумеруется.
    pub number_source: String,
    pub stale: bool,
}

/// Отчёт обновления формульных вставок.
#[derive(Clone, Debug, Default)]
pub struct FormulaUpdateReport {
    pub updated: Vec<String>,
    pub conflicts: Vec<String>,
    pub missing: Vec<String>,
    /// Предупреждения нумерации (раздел без номера, формулы до первого раздела, нет заголовков).
    pub warnings: Vec<String>,
}

/// Отчёт миграции формул документа.
#[derive(Clone, Debug, Default)]
pub struct FormulaMigrationReport {
    pub insert_ids: Vec<String>,
    pub replaced_tables: usize,
    pub replaced_blocks: usize,
    pub replaced_inline: usize,
    /// display/inline среди принятых формул.
    pub display: usize,
    pub inline: usize,
    /// display-формулы в смешанных абзацах/таблицах — оставлены inline, номер не назначался.
    pub mixed: usize,
    /// Сколько номеров сохранено из Word и сколько назначено по схеме документа.
    pub numbers_preserved: usize,
    pub numbers_assigned: usize,
    /// Схема, по которой назначались номера (когда сохранять было нечего).
    pub numbering: FormulaNumbering,
    /// Предупреждения: смешанные абзацы, автономера Word, частичные номера, схема.
    pub warnings: Vec<String>,
}

/// Политика миграции формул (ADR-0019): что делать с display-формулами и их номерами.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FormulaMigrationOptions {
    /// Схема нумерации для назначаемых номеров (`document.toml: formula_numbering`).
    pub numbering: FormulaNumbering,
    /// Сохранять номера, найденные в Word (если найдены у всех display-формул).
    pub preserve_word_numbers: bool,
    /// Назначать номера display-формулам по схеме документа (когда сохранять нечего).
    pub number_display: bool,
}

impl Default for FormulaMigrationOptions {
    fn default() -> Self {
        Self { numbering: FormulaNumbering::Continuous, preserve_word_numbers: true, number_display: true }
    }
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
    add_formula_insert_with(package, request, |entries, numbering| {
        renumber_entries(entries, numbering).map(|report| report.warnings)
    })
}

/// Тестовая точка (итерация inline-formula-insert): вставка с намеренной ошибкой перенумерации —
/// проверяет транзакционность (пакет/store не меняются, `.bak` не создаётся). Не для продакшена.
#[doc(hidden)]
pub fn add_formula_insert_with_renumber_error(package: &Path, request: FormulaInsertRequest) -> Result<InsertReport, String> {
    add_formula_insert_with(package, request, |_, _| Err("инъекция ошибки перенумерации (тест)".to_owned()))
}

/// Транзакционная вставка: всё, включая перенумерацию, считается в памяти; пакет записывается
/// одним `write_entries_atomic` в самом конце. Ошибка на любой стадии (якорь/позиция/ренумерация)
/// не меняет пакет — вызывающий слой не создаёт бэкапов и не плодит дубликатов.
fn add_formula_insert_with(
    package: &Path,
    request: FormulaInsertRequest,
    renumber: impl FnOnce(&mut BTreeMap<String, Vec<u8>>, FormulaNumbering) -> Result<Vec<String>, String>,
) -> Result<InsertReport, String> {
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
    let mut document_xml = entries.get(DOCUMENT_XML).map(|bytes| String::from_utf8_lossy(bytes).to_string()).unwrap_or_default();
    let id = super::next_insert_id(&rde_doc.inserts, &document_xml);
    let generated_para_id = super::para_id_for(&format!("{}:formula:{id}", rde_doc.doc_id), rde_doc.inserts.len());
    let numbering = request.numbering;
    let number = if body.display && body.numbered { next_formula_number(&rde_doc) } else { None };
    let mut insert = RdeInsert {
        id: id.clone(),
        block: String::new(),
        kind: "formula".to_owned(),
        target: format!("rde://formula/{}@{}", body.formula_id, body.act_key),
        caption: String::new(),
        number,
        number_text: None,
        suffix: String::new(),
        number_source: String::new(),
        hash: Some(body.act_key.clone()),
        cached: body.latex.clone(),
        selector: String::new(),
        value_kind: "formula".to_owned(),
        para_id: Some(generated_para_id.clone()),
        style: String::new(),
        format: String::new(),
        run_style: String::new(),
        render_mode: render_mode.clone(),
        body_hash: None,
    };
    let signature = if render_mode == "svg" { svg_signature(&body) } else { omml_signature(&body.omml)? };
    insert.body_hash = Some(signature);
    let number_text_provisional = number.map(|number| number.to_string());
    let content = if body.display {
        let media = if render_mode == "svg" { Some(ensure_formula_media(&mut entries, &id, &body)?) } else { None };
        wrap_block_sdt(&id, &block_content(&insert, &body, number_text_provisional.as_deref(), &insert.suffix, &generated_para_id, media, &entries)?)
    } else {
        let media = if render_mode == "svg" { Some(ensure_formula_media(&mut entries, &id, &body)?) } else { None };
        wrap_inline_sdt(&id, &inline_content(&insert, &body, &generated_para_id, media)?)
    };
    let mut warnings: Vec<String> = Vec::new();
    let placement = if body.display {
        let (updated, warning) = crate::insert_block_after(&document_xml, &request.anchor, &content)?;
        if let Some(warning) = warning {
            warnings.push(warning);
        }
        document_xml = updated;
        None
    } else {
        // Inline: без якоря — явная ошибка (прежний «тихий» append в конец документа убран).
        let (updated, placement) = crate::insert_inline_at(&document_xml, &request.anchor, &content)?;
        if !placement.exact {
            warnings.push("позиция каретки недоступна — inline-формула вставлена в конец абзаца-якоря".to_owned());
        }
        document_xml = updated;
        Some(placement)
    };
    if !body.display {
        // Анкор inline-вставки — хост-абзац (для диагностики/«где вставлено»); своего абзаца у
        // inline content-control нет.
        insert.para_id = placement.as_ref().and_then(|placement| placement.para_id.clone()).or(Some(generated_para_id));
        if let Some(para_id) = &insert.para_id
            && let Some(block) = rde_doc.blocks.iter().find(|block| block.para_id.as_deref() == Some(para_id.as_str()))
        {
            insert.block = block.id.clone();
        }
    }
    rde_doc.inserts.push(insert.clone());
    rde_doc.updated_unix = super::now_unix();
    rde_doc.body_hash = rde_hash::sha256(document_xml.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), super::render_rde_doc(&rde_doc).into_bytes());
    super::upsert_custom_props(&mut entries, &rde_doc);
    // Схемная нумерация (ADR-0018) — ДО записи пакета: ошибка не оставляет полу-вставленный пакет.
    let renumber_warnings = renumber(&mut entries, numbering)?;
    warnings.extend(renumber_warnings);
    super::write_entries_atomic(&entries, package)?;
    // Итоговая вставка — после перенумерации (номера/текст могли обновиться в rde:doc).
    let final_doc = super::load_rde_doc_from_entries(&entries)?;
    let insert = final_doc.inserts.iter().find(|candidate| candidate.id == id).cloned().unwrap_or(insert);
    let info = super::info_from_entries(&entries);
    Ok(InsertReport { insert, package: info, placement, warnings })
}

fn normalize_render_mode(mode: &str) -> &'static str {
    if mode.eq_ignore_ascii_case("svg") { "svg" } else { "omml" }
}

/// Предварительный номер следующей формулы в документе (для диалога вставки, ADR-0018):
/// формула ставится последней в порядке документа; возвращает текст без скобок (`"3"`, `"2.2"`,
/// `"А.1"`) и предупреждения схемы. Ошибки чтения пакета — `None` (превью необязательно).
pub fn next_formula_number_text(package: &Path, numbering: FormulaNumbering) -> Option<(String, Vec<String>)> {
    let entries = super::read_package(package).ok()?.entries;
    let rde_doc = super::load_rde_doc_from_entries(&entries).ok()?;
    let document_xml = entries.get(DOCUMENT_XML).map(|bytes| String::from_utf8_lossy(bytes).to_string())?;
    let styles_xml = entries.get("word/styles.xml").map(|bytes| String::from_utf8_lossy(bytes).to_string());
    let mut ordered: Vec<(String, usize)> = rde_doc
        .inserts
        .iter()
        .filter(|insert| insert.kind == "formula" && insert.number.is_some())
        .map(|insert| (insert.id.clone(), formula_tag_position(&document_xml, &insert.id).unwrap_or(usize::MAX)))
        .collect();
    ordered.push(("__next__".to_owned(), usize::MAX));
    let (values, warnings) = number_formulas(&document_xml, styles_xml.as_deref(), numbering, &ordered);
    let text = values.into_iter().find(|value| value.id == "__next__")?.number;
    Some((text, warnings))
}

/// Следующий документно-локальный номер формулы: максимальный занятый + 1.
fn next_formula_number(rde_doc: &RdeDoc) -> Option<u32> {
    let max = rde_doc.inserts.iter().filter(|insert| insert.kind == "formula").filter_map(|insert| insert.number).max().unwrap_or(0);
    Some(max + 1)
}

/// Содержимое block-sdt display-формулы.
///
/// Нумерованная формула — **как «Right-numbered» в Word**: один абзац с табуляциями (центр/право
/// по ширине текста), display-формула по центру и номер `(n)` у правого поля; видимой таблицы нет
/// (ADR-0018). Ненумерованная — центрированный абзац. `entries` нужны для ширины текста из
/// `w:sectPr` (позиции табов).
fn block_content(
    insert: &RdeInsert,
    body: &FormulaBody,
    number_text: Option<&str>,
    suffix: &str,
    para_id: &str,
    media: Option<(String, String)>,
    entries: &BTreeMap<String, Vec<u8>>,
) -> Result<String, String> {
    let inner = if insert.render_mode == "svg" {
        let (png_rel, svg_rel) = media.ok_or("svg-режим: нет медиа-связей")?;
        let (cx, cy) = formula_extent_emu(body);
        let drawing = drawing_run_xml(&png_rel, &svg_rel, cx, cy, para_id);
        // P6 (ADR-0022): низ картинки садится на базовую линию абзаца — номер `(N)` поднимается
        // на глубину формулы + отступ кропа, чтобы его базовая линия совпала с формулой.
        let raise_pt = number_raise_pt(body);
        match number_text {
            Some(number) => numbered_tabs_paragraph(para_id, &drawing, number, suffix, entries, raise_pt),
            None => paragraph_centered(para_id, &drawing, suffix),
        }
    } else {
        // Display-фрагмент: `m:oMathPara` (движок/Word считают его display-формулой — пределы
        // сумм/интегралов сверху/снизу, как в Word); табы-соседи по абзацу — по ECMA-376
        // math — run-level элемент (EG_RunLevelElts), поэтому абзац валиден.
        let omml = omml_fragment(&body.omml, body.display)?;
        let math = String::from_utf8_lossy(&omml);
        match number_text {
            // OMML-формула живёт на базовой линии абзаца — номер выравнивается сам, подъём не нужен.
            Some(number) => numbered_tabs_paragraph(para_id, &math, number, suffix, entries, 0.0),
            None => paragraph_centered(para_id, &math, suffix),
        }
    };
    Ok(inner)
}

/// Обернуть блок-содержимое формулы в `w:sdt` с тегом реестра `rde:formula:<id>`.
fn wrap_block_sdt(insert_id: &str, inner: &str) -> String {
    format!(
        "<w:sdt><w:sdtPr><w:tag w:val=\"{FORMULA_TAG_PREFIX}{}\"/><w:alias w:val=\"формула\"/></w:sdtPr><w:sdtContent>{inner}</w:sdtContent></w:sdt>",
        crate::escape_attr(insert_id)
    )
}

/// Inline-содержимое формулы: `m:oMath` для редактируемой или `w:drawing` (SVG+PNG).
fn inline_content(
    insert: &RdeInsert,
    body: &FormulaBody,
    para_id: &str,
    media: Option<(String, String)>,
) -> Result<String, String> {
    if insert.render_mode == "svg" {
        let (png_rel, svg_rel) = media.ok_or("svg-режим: нет медиа-связей")?;
        let (cx, cy) = formula_extent_emu(body);
        Ok(drawing_run_xml(&png_rel, &svg_rel, cx, cy, para_id))
    } else {
        Ok(String::from_utf8_lossy(&omml_fragment(&body.omml, false)?).to_string())
    }
}

/// Обернуть inline-содержимое формулы в inline-`w:sdt` (вставляется в абзац).
fn wrap_inline_sdt(insert_id: &str, inner: &str) -> String {
    format!(
        "<w:sdt><w:sdtPr><w:tag w:val=\"{FORMULA_TAG_PREFIX}{}\"/><w:alias w:val=\"формула\"/></w:sdtPr><w:sdtContent>{inner}</w:sdtContent></w:sdt>",
        crate::escape_attr(insert_id)
    )
}

/// Подъём номера `(N)` в SVG-вставке, pt (P6, ADR-0022 §3.3).
///
/// `w:drawing` в WordprocessingML — inline-объект: его **низ** ставится на базовую линию строки
/// (проверено LO-оракулом P6; WordCraft делает так же). Значит базовая линия формулы находится
/// выше низа картинки на `descent + отступ кропа`; номер `(N)` — обычный текст на базовой линии
/// абзаца, поэтому его надо поднять на эту величину (`w:position`, half-points), иначе номер
/// визуально «висит» ниже формулы. Метрики — из той же вёрстки WordCraft, что рисует документ.
fn number_raise_pt(body: &FormulaBody) -> f64 {
    crate::formula::formula_metrics(&body.latex, body.display)
        .map(|metrics| metrics.descent_pt + crate::formula::SVG_CROP_PAD_PT)
        .unwrap_or(0.0)
}

/// Абзац display-формулы: по центру, без отступов и интервалов, с содержимым (OMML/drawing) и
/// пунктуацией после формулы (`suffix` — `,`/`.`; ADR-0019: не теряется при миграции/обновлении).
fn paragraph_centered(para_id: &str, content: &str, suffix: &str) -> String {
    format!(
        "<w:p w14:paraId=\"{para_id}\"><w:pPr><w:spacing w:before=\"120\" w:after=\"120\"/><w:ind w:left=\"0\" w:right=\"0\" w:firstLine=\"0\"/><w:jc w:val=\"center\"/></w:pPr>{content}{}</w:p>",
        suffix_run(suffix)
    )
}

/// Run с пунктуацией после формулы (пустой `suffix` — без run'а); текст экранируется.
fn suffix_run(suffix: &str) -> String {
    if suffix.is_empty() {
        return String::new();
    }
    format!("<w:r><w:t xml:space=\"preserve\">{}</w:t></w:r>", crate::escape_attr(suffix))
}

/// Абзац «Right-numbered» формулы: таб-стопы центр/право по ширине текста, формула по центру,
/// номер `(n)` у правого поля. Схема OOXML: табы + display-формула (`m:oMathPara`) в одном абзаце —
/// `EG_RunLevelElts` разрешает math внутри `w:p`; Word и LibreOffice ставят формулу на таб-центр,
/// а номер — на правый таб (LO-оракул в retest'ах). Видимой таблицы нет.
fn numbered_tabs_paragraph(
    para_id: &str,
    content: &str,
    number: &str,
    suffix: &str,
    entries: &BTreeMap<String, Vec<u8>>,
    number_raise_pt: f64,
) -> String {
    let (center, right) = tab_stops(entries);
    // Подъём номера: `w:position` в half-points (0 — номер на базовой линии, как у OMML).
    let number_run = if number_raise_pt > 0.05 {
        let half_points = (number_raise_pt * 2.0).round().max(1.0) as i64;
        format!("<w:r><w:rPr><w:position w:val=\"{half_points}\"/></w:rPr><w:t xml:space=\"preserve\">({number})</w:t></w:r>")
    } else {
        format!("<w:r><w:t xml:space=\"preserve\">({number})</w:t></w:r>")
    };
    // Пунктуация — сразу после формулы (до таба к номеру): «α = λ/d, … (1)» (ADR-0019).
    format!(
        "<w:p w14:paraId=\"{para_id}\"><w:pPr>\
         <w:tabs><w:tab w:val=\"center\" w:pos=\"{center}\"/><w:tab w:val=\"right\" w:pos=\"{right}\"/></w:tabs>\
         <w:spacing w:before=\"120\" w:after=\"120\"/>\
         <w:ind w:left=\"0\" w:right=\"0\" w:firstLine=\"0\"/></w:pPr>\
         <w:r><w:tab/></w:r>{content}{suffix_run}<w:r><w:tab/></w:r>{number_run}</w:p>",
        suffix_run = suffix_run(suffix)
    )
}

/// Позиции таб-стопов (twips от левого поля) из `w:sectPr`: (центр, право) по ширине текста.
/// Не удалось разобрать `pgSz`/`pgMar` — типовая A4 с полями ГОСТ (текст 9355 twips = 16.5 см).
fn tab_stops(entries: &BTreeMap<String, Vec<u8>>) -> (i32, i32) {
    const DEFAULT_TEXT_WIDTH: i32 = 9355;
    let width = entries
        .get(DOCUMENT_XML)
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .and_then(|xml| text_width_twips(&xml))
        .unwrap_or(DEFAULT_TEXT_WIDTH);
    let width = width.clamp(3000, 30_000);
    (width / 2, width)
}

/// Ширина текстового поля страницы в twips из первого `w:sectPr` (pgSz − поля pgMar).
fn text_width_twips(document_xml: &str) -> Option<i32> {
    let attr = |xml: &str, name: &str| -> Option<i32> {
        let marker = format!("{name}=\"");
        let index = xml.find(&marker)? + marker.len();
        let rest = &xml[index..];
        let end = rest.find('"')?;
        rest[..end].trim().parse::<i32>().ok()
    };
    let (page_start, page_end) = element_bounds(document_xml, "<w:pgSz", "</w:pgSz>")?;
    let page = &document_xml[page_start..page_end];
    let page_width = attr(page, "w:w")?;
    let margin = element_bounds(document_xml, "<w:pgMar", "</w:pgMar>").map(|(start, end)| &document_xml[start..end]).unwrap_or("");
    let left = attr(margin, "w:left").unwrap_or(0);
    let right = attr(margin, "w:right").unwrap_or(0);
    let width = page_width - left - right;
    (width > 0).then_some(width)
}

/// Границы элемента по открывающему фрагменту и закрывающему тегу (для самозакрытых —
/// `<w:pgSz .../>` без явного `</w:pgSz>`: границы до `>`).
fn element_bounds(xml: &str, open: &str, close: &str) -> Option<(usize, usize)> {
    let start = xml.find(open)?;
    let open_end = xml[start..].find('>')? + start;
    if xml[start..open_end].ends_with('/') {
        return Some((start, open_end + 1));
    }
    let end = xml[open_end..].find(close)? + open_end + close.len();
    Some((start, end))
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
    } else if !display && trimmed.starts_with("<m:oMathPara") {
        // Display-кэш внутри inline-вставки (смешанный абзац): `m:oMathPara` — block-level элемент
        // внутри `w:p`, в inline-`w:sdt` он невалиден; снимаем обёртку до `m:oMath`.
        Ok(unwrap_math_para(trimmed).into_bytes())
    } else {
        Ok(trimmed.as_bytes().to_vec())
    }
}

/// Снять обёртку `m:oMathPara` (и `m:oMathParaPr`): содержимое для inline-вставки.
fn unwrap_math_para(omml: &str) -> String {
    let Some(open) = omml.find("<m:oMathPara") else { return omml.to_owned() };
    let Some(open_end) = omml[open..].find('>').map(|offset| open + offset + 1) else { return omml.to_owned() };
    let Some(close) = omml.rfind("</m:oMathPara>") else { return omml.to_owned() };
    let inner = omml[open_end..close].trim();
    let Some(start) = inner.find("<m:oMathParaPr") else { return inner.to_owned() };
    let Some(end) = inner[start..].find("</m:oMathParaPr>").map(|offset| start + offset) else { return inner.to_owned() };
    let after = end + "</m:oMathParaPr>".len();
    format!("{}{}", &inner[..start], &inner[after..]).trim().to_owned()
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
/// Обновить формульные вставки документа (совместимость): схема нумерации — сквозная.
pub fn update_formula_inserts(package: &Path, bodies: &BTreeMap<String, FormulaBody>) -> Result<FormulaUpdateReport, String> {
    update_formula_inserts_with(package, bodies, FormulaNumbering::Continuous)
}

/// Обновить формульные вставки документа: пересобрать содержимое `w:sdt` из кэшей формулы и
/// перенумеровать документно-локальные номера по схеме `numbering` (ADR-0005 §3.1, ADR-0018).
pub fn update_formula_inserts_with(
    package: &Path,
    bodies: &BTreeMap<String, FormulaBody>,
    numbering: FormulaNumbering,
) -> Result<FormulaUpdateReport, String> {
    let mut entries = super::read_package(package)?.entries;
    let mut rde_doc = super::load_rde_doc_from_entries(&entries)?;
    let Some(document) = entries.get(DOCUMENT_XML) else { return Ok(FormulaUpdateReport::default()) };
    let mut document_xml = String::from_utf8_lossy(document).to_string();
    let styles_xml = entries.get("word/styles.xml").map(|bytes| String::from_utf8_lossy(bytes).to_string());
    let mut report = FormulaUpdateReport::default();
    let mut formulas: Vec<RdeInsert> = rde_doc.inserts.iter().filter(|insert| insert.kind == "formula").cloned().collect();
    if formulas.is_empty() {
        return Ok(report);
    }
    formulas.sort_by_key(|insert| formula_tag_position(&document_xml, &insert.id).unwrap_or(usize::MAX));
    // Документно-локальная нумерация (ADR-0005 §3.1) по схеме документа (ADR-0018): нумеруются
    // только block-вставки display-формул с политикой «нумеровать», в порядке документа. Номера,
    // сохранённые из Word (`number_source = word/manual`), не пересчитываются (ADR-0019);
    // `number_source = off` — формула сознательно не нумеруется.
    let locked = formulas.iter().filter(|insert| number_locked(insert)).count();
    let numbered_ids: Vec<(String, usize)> = formulas
        .iter()
        .filter(|insert| !number_locked(insert) && insert.number_source != NUMBER_SOURCE_OFF)
        .filter(|insert| is_block_sdt(&document_xml, &insert.id))
        .filter(|insert| {
            let body = parse_formula_target(&insert.target).and_then(|target| bodies.get(&target.formula_id));
            body.map(|body| body.display && body.numbered).unwrap_or(insert.number.is_some())
        })
        .filter_map(|insert| formula_tag_position(&document_xml, &insert.id).map(|position| (insert.id.clone(), position)))
        .collect();
    let (number_values, numbering_warnings) =
        number_formulas(&document_xml, styles_xml.as_deref(), numbering, numbered_ids.as_slice());
    report.warnings = numbering_warnings;
    if locked > 0 && !numbered_ids.is_empty() {
        report.warnings.push(format!(
            "часть номеров сохранена из Word ({locked}), новые/прочие формулы нумеруются по схеме — номера могут повторяться"
        ));
    }
    let numbers: BTreeMap<String, (u32, String)> = numbered_ids
        .iter()
        .enumerate()
        .map(|(index, (id, _))| {
            let text = number_values
                .iter()
                .find(|value| &value.id == id)
                .map(|value| value.number.clone())
                .unwrap_or_else(|| (index + 1).to_string());
            (id.clone(), ((index + 1) as u32, text))
        })
        .collect();
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
        let (new_number, new_number_text) = if number_locked(&insert_snapshot) {
            // Номер из Word/вручную: сохраняем как есть (ADR-0019).
            (insert_snapshot.number, insert_snapshot.number_text.clone())
        } else if insert_snapshot.number_source == NUMBER_SOURCE_OFF {
            (None, None)
        } else {
            (
                numbers.get(&insert_snapshot.id).map(|(number, _)| *number),
                numbers.get(&insert_snapshot.id).map(|(_, text)| text.clone()),
            )
        };
        // Подпись тела — по фактической вёрстке: display-кэш в inline-вставке (смешанный абзац)
        // хранится без обёртки `m:oMathPara`, иначе «Обновить документ» видел бы ложную ручную
        // правку (R-DOC-13).
        let is_block = is_block_sdt(&document_xml, &insert_snapshot.id);
        let expected_signature = if insert_snapshot.render_mode == "svg" {
            svg_signature(body)
        } else {
            omml_signature(&omml_fragment(&body.omml, is_block)?)?
        };
        let expected_target = format!("rde://formula/{}@{}", body.formula_id, body.act_key);
        let unchanged = insert_snapshot.hash.as_deref() == Some(body.act_key.as_str())
            && insert_snapshot.target == expected_target
            && insert_snapshot.number == new_number
            && insert_snapshot.number_text == new_number_text
            && current_signature.as_deref() == Some(expected_signature.as_str());
        if unchanged {
            continue;
        }
        let para_id = insert_snapshot
            .para_id
            .clone()
            .unwrap_or_else(|| super::para_id_for(&format!("{}:formula:{}", rde_doc.doc_id, insert_snapshot.id), 0));
        let media = if insert_snapshot.render_mode == "svg" { Some(ensure_formula_media(&mut entries, &insert_snapshot.id, body)?) } else { None };
        // ВАЖНО: заменяем содержимое существующего `w:sdt` (его inner), а не сам sdt — иначе
        // формула оборачивалась бы в новый sdt при каждом обновлении (вложенные content-controls).
        let content = if is_block {
            block_content(&insert_snapshot, body, new_number_text.as_deref(), &insert_snapshot.suffix, &para_id, media, &entries)?
        } else {
            inline_content(&insert_snapshot, body, &para_id, media)?
        };
        let Some((block_start, block_end)) = find_formula_sdt_block(&document_xml, &insert_snapshot.id) else { continue };
        // Заменяем блок `w:sdt` целиком: чинит исторические вложения sdt (после прежних версий
        // обновления формула была обёрнута в новый sdt при каждом проходе — XML становился
        // невалидным для LibreOffice/Word).
        let block = if is_block {
            wrap_block_sdt(&insert_snapshot.id, &content)
        } else {
            wrap_inline_sdt(&insert_snapshot.id, &content)
        };
        let mut out = String::with_capacity(document_xml.len() + block.len());
        out.push_str(&document_xml[..block_start]);
        out.push_str(&block);
        out.push_str(&document_xml[block_end..]);
        document_xml = out;
        let insert = &mut rde_doc.inserts[index];
        insert.hash = Some(body.act_key.clone());
        // Целевой URI несёт act_key: обновляем вместе с телом, иначе «где вставлено» показывает
        // формулу устаревшей, хотя тело пересобрано из её текущего кэша.
        insert.target = format!("rde://formula/{}@{}", body.formula_id, body.act_key);
        insert.cached = body.latex.clone();
        insert.number = new_number;
        insert.number_text = new_number_text;
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

/// Отчёт пересчёта номеров формул и ссылок на них (ADR-0018).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FormulaRenumberReport {
    /// Формулы с изменившимся отображаемым номером: (insert-id, новый номер без скобок).
    pub formulas: Vec<(String, String)>,
    /// Обновлённые перекрёстные ссылки: (insert-id ссылки, новый текст в документе).
    pub crossrefs: Vec<(String, String)>,
    /// Предупреждения (раздел без номера, цель ссылки отсутствует и т. п.).
    pub warnings: Vec<String>,
}

/// Пересчитать номера формул и перекрёстные ссылки на них **без пересборки тел** (меняется только
/// текст номера в `w:sdt`): один проход, документно-локально, по схеме `numbering` (ADR-0018).
///
/// Вызывается после вставки/удаления/перестановки формулы, чтобы номера и ссылки в тексте
/// обновлялись автоматически (главное требование B3). Тело формулы не перезаписывается — правки
/// `number_text` не входят в `body_hash`, поэтому ручные правки тела не конфликтуют (R-DOC-13).
pub fn renumber_formula_numbers(package: &Path, numbering: FormulaNumbering) -> Result<FormulaRenumberReport, String> {
    let mut entries = super::read_package(package)?.entries;
    let report = renumber_entries(&mut entries, numbering)?;
    if report.changed() {
        super::write_entries_atomic(&entries, package)?;
    }
    Ok(report)
}

impl FormulaRenumberReport {
    /// Были ли реальные изменения (тогда нужно записать пакет).
    pub fn changed(&self) -> bool {
        !self.formulas.is_empty() || !self.crossrefs.is_empty()
    }
}

/// Пересчитать номера/ссылки в уже прочитанных частях пакета (без записи на диск): общая часть
/// [`renumber_formula_numbers`] и транзакционной вставки формулы.
fn renumber_entries(entries: &mut BTreeMap<String, Vec<u8>>, numbering: FormulaNumbering) -> Result<FormulaRenumberReport, String> {
    let mut rde_doc = super::load_rde_doc_from_entries(entries)?;
    let Some(document) = entries.get(DOCUMENT_XML) else { return Ok(FormulaRenumberReport::default()) };
    let mut document_xml = String::from_utf8_lossy(document).to_string();
    let styles_xml = entries.get("word/styles.xml").map(|bytes| String::from_utf8_lossy(bytes).to_string());
    let mut report = FormulaRenumberReport::default();
    let mut ordered: Vec<(String, usize)> = rde_doc
        .inserts
        .iter()
        .filter(|insert| insert.kind == "formula" && insert.number.is_some())
        .filter_map(|insert| formula_tag_position(&document_xml, &insert.id).map(|position| (insert.id.clone(), position)))
        .collect();
    ordered.sort_by_key(|(_, position)| *position);
    if ordered.is_empty() {
        return Ok(report);
    }
    let (values, warnings) = number_formulas(&document_xml, styles_xml.as_deref(), numbering, &ordered);
    report.warnings = warnings;
    let mut changed = false;
    for (index, (id, _)) in ordered.iter().enumerate() {
        let Some(value) = values.iter().find(|value| &value.id == id) else { continue };
        let previous = rde_doc
            .inserts
            .iter()
            .find(|insert| &insert.id == id)
            .and_then(|insert| insert.number_text.clone().or_else(|| insert.number.map(|number| number.to_string())));
        if previous.as_deref() != Some(value.number.as_str())
            && let Some(updated) = replace_formula_number_text(&document_xml, id, &value.number)
        {
            document_xml = updated;
            changed = true;
            report.formulas.push((id.clone(), value.number.clone()));
        }
        if let Some(insert) = rde_doc.inserts.iter_mut().find(|insert| &insert.id == id) {
            insert.number = Some((index + 1) as u32);
            insert.number_text = Some(value.number.clone());
            // Явный пересчёт схемы снимает фиксацию «номер из Word» (ADR-0019).
            insert.number_source = String::new();
        }
    }
    // Перекрёстные ссылки на формулы — после того, как известны все номера.
    changed |= refresh_crossrefs(&mut document_xml, &mut rde_doc, &mut report);
    if !changed {
        return Ok(report);
    }
    rde_doc.updated_unix = super::now_unix();
    rde_doc.body_hash = rde_hash::sha256(document_xml.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), super::render_rde_doc(&rde_doc).into_bytes());
    super::upsert_custom_props(entries, &rde_doc);
    Ok(report)
}

/// Обновить текст перекрёстных ссылок `rde://formula/<id>` по **текущим** номерам формул документа,
/// не трогая сами номера (ADR-0019: вызывается после миграции, когда номера уже назначены или
/// сохранены из Word, и явный пересчёт схемы не нужен).
pub fn refresh_formula_crossrefs(package: &Path) -> Result<FormulaRenumberReport, String> {
    let mut entries = super::read_package(package)?.entries;
    let mut rde_doc = super::load_rde_doc_from_entries(&entries)?;
    let Some(document) = entries.get(DOCUMENT_XML) else { return Ok(FormulaRenumberReport::default()) };
    let mut document_xml = String::from_utf8_lossy(document).to_string();
    let mut report = FormulaRenumberReport::default();
    if !refresh_crossrefs(&mut document_xml, &mut rde_doc, &mut report) {
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

/// Синхронизировать текст перекрёстных ссылок на формулы с номерами вставок в `rde:doc`.
/// Возвращает `true`, если документ изменился; обновлённые ссылки/предупреждения — в `report`.
fn refresh_crossrefs(document_xml: &mut String, rde_doc: &mut RdeDoc, report: &mut FormulaRenumberReport) -> bool {
    let mut formula_numbers: BTreeMap<String, String> = BTreeMap::new();
    for insert in rde_doc.inserts.iter().filter(|insert| insert.kind == "formula") {
        let Some(target) = parse_formula_target(&insert.target) else { continue };
        let Some(number) = insert.number_text.clone().or_else(|| insert.number.map(|number| number.to_string())) else { continue };
        // Первое вхождение формулы в документе — «основное»: ссылки указывают на него.
        formula_numbers.entry(target.formula_id).or_insert_with(|| format!("({number})"));
    }
    let mut missing_targets = Vec::new();
    let crossrefs: Vec<(String, String)> = rde_doc
        .inserts
        .iter()
        .filter(|insert| insert.kind == "crossref")
        .filter_map(|insert| {
            let formula_id = insert.target.strip_prefix("rde://formula/")?;
            let formula_id = formula_id.split('@').next().unwrap_or(formula_id);
            let text = match formula_numbers.get(formula_id) {
                Some(text) => text.clone(),
                None => {
                    missing_targets.push((insert.id.clone(), formula_id.to_owned()));
                    "(?)".to_owned()
                }
            };
            (insert.cached != text).then(|| (insert.id.clone(), text))
        })
        .collect();
    for (id, formula_id) in &missing_targets {
        report.warnings.push(format!(
            "перекрёстная ссылка {id}: формула {formula_id} не вставлена в документ — в тексте placeholder «(?)»"
        ));
    }
    let mut changed = false;
    for (id, text) in crossrefs {
        if let Some(insert) = rde_doc.inserts.iter().find(|insert| insert.id == id).cloned()
            && let Some(updated) = super::replace_sdt_text(document_xml, &insert, &text)
        {
            *document_xml = updated;
            changed = true;
            report.crossrefs.push((id.clone(), text.clone()));
        }
        if let Some(insert) = rde_doc.inserts.iter_mut().find(|insert| insert.id == id) {
            insert.cached = text;
        }
    }
    changed
}

/// Заменить текст номера `(…)` в block-sdt формулы (последний `w:t` внутри sdtContent).
fn replace_formula_number_text(document_xml: &str, insert_id: &str, number: &str) -> Option<String> {
    let (start, end) = find_formula_sdt_block(document_xml, insert_id)?;
    let block = &document_xml[start..end];
    let text_open = block.rfind("<w:t")?;
    let open_end = block[text_open..].find('>')? + text_open;
    let close = block[open_end..].find("</w:t>")? + open_end;
    let mut out = String::with_capacity(document_xml.len() + 16);
    out.push_str(&document_xml[..start]);
    out.push_str(&block[..open_end + 1]);
    out.push('(');
    out.push_str(&crate::escape_attr(number));
    out.push(')');
    out.push_str(&block[close..]);
    out.push_str(&document_xml[end..]);
    Some(out)
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
                number_text: insert.number_text.clone(),
                number_source: insert.number_source.clone(),
                stale,
            });
        }
    }
    Ok(out)
}

// ── миграция существующего OMML вне реестра (явная команда) ──────────────────

/// Формула документа вне реестра (для явной миграции/подсказки «формул вне реестра: N»).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnregisteredFormula {
    pub block: String,
    pub latex: String,
    pub display: bool,
    /// Номер, найденный в Word (`"3"`, `"2.1"`, `"А.1"`): поле `SEQ MTEqn`, таб-абзац или
    /// legacy-таблица 1×2. `None` — номера нет (ADR-0019).
    pub number_text: Option<String>,
    /// display-формула в смешанном абзаце/таблице — останется inline, номер не назначается.
    pub mixed: bool,
    /// Автономер Word (`#`/`eqArr`) без литерального значения: восстановить нельзя.
    pub auto_number: bool,
    /// Пунктуация сразу после формулы (`,`, `.`), переносится в канонический абзац.
    pub suffix: String,
}

impl UnregisteredFormula {
    /// Короткая метка номера для журнала/диалога: `(3)`, `(2.1)`, `(А.1)` или `—`.
    pub fn number_label(&self) -> String {
        match &self.number_text {
            Some(number) => format!("({number})"),
            None => "—".to_owned(),
        }
    }
}

/// Формулы документа вне реестра (`m:oMath`/`m:oMathPara` не внутри `rde:formula:*` sdt), в
/// порядке документа: LaTeX из модели WordCraft + контекст абзаца (номер/пунктуация/смешанность).
/// Автоизменений при открытии нет.
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
        .map(|(raw, equation)| UnregisteredFormula {
            block: equation.block,
            latex: equation.latex,
            display: equation.display,
            number_text: raw.number_text.clone(),
            mixed: raw.mixed_display(),
            auto_number: raw.auto_number,
            suffix: if raw.mixed_display() { String::new() } else { raw.suffix.clone() },
        })
        .collect())
}

/// Явная миграция (ADR-0005 §7, ADR-0019): заменить OMML вне реестра вставками-ссылками на
/// formula-объекты с сохранением/назначением номеров и пунктуации.
///
/// Политика по умолчанию — «сохранить номера Word, если они найдены у **всех** display-формул,
/// иначе назначить по схеме документа (сквозная)»; display-формулы становятся block-вставками,
/// inline остаются inline. Идемпотентна: переносит только формулы, ещё не обёрнутые в
/// `rde:formula:*`.
pub fn migrate_formulas(package: &Path, plans: &[FormulaMigrationPlan]) -> Result<FormulaMigrationReport, String> {
    migrate_formulas_with(package, plans, FormulaMigrationOptions::default())
}

/// Миграция формул с явной политикой номеров (ADR-0019): см. [`FormulaMigrationOptions`].
pub fn migrate_formulas_with(
    package: &Path,
    plans: &[FormulaMigrationPlan],
    options: FormulaMigrationOptions,
) -> Result<FormulaMigrationReport, String> {
    let mut entries = super::read_package(package)?.entries;
    let mut rde_doc = super::load_rde_doc_from_entries(&entries)?;
    let document = entries.get(DOCUMENT_XML).cloned().ok_or("в пакете нет word/document.xml")?;
    let xml = String::from_utf8_lossy(&document).to_string();
    let styles_xml = entries.get("word/styles.xml").map(|bytes| String::from_utf8_lossy(bytes).to_string());
    let equations = model_equations(package)?;
    let raw = raw_equations(&xml);
    if raw.len() != equations.len() {
        return Err(format!("не удалось сопоставить формулы: OMML ×{}, модель ×{}", raw.len(), equations.len()));
    }
    let candidates: Vec<(RawEquation, ModelEquation)> = raw.into_iter().zip(equations).filter(|(raw, _)| !raw.registered).collect();
    if candidates.len() != plans.len() {
        return Err(format!("миграция: формул вне реестра {}, планов {}", candidates.len(), plans.len()));
    }
    let mut report = FormulaMigrationReport { numbering: options.numbering, ..FormulaMigrationReport::default() };
    if candidates.is_empty() {
        return Ok(report);
    }

    // ── Шаг 1. Классификация и политика номеров (ADR-0019 §3) ────────────────────────────────
    let display_indices: Vec<usize> = candidates
        .iter()
        .enumerate()
        .filter(|(_, (raw, _))| raw.display)
        .map(|(index, _)| index)
        .collect();
    let block_indices: Vec<usize> = display_indices
        .iter()
        .copied()
        .filter(|index| candidates[*index].0.block_formula())
        .collect();
    let mixed_indices: Vec<usize> = display_indices.iter().copied().filter(|index| !block_indices.contains(index)).collect();
    report.display = display_indices.len();
    report.inline = candidates.iter().filter(|(raw, _)| !raw.display).count();
    report.mixed = mixed_indices.len();
    let found = block_indices.iter().filter(|index| candidates[**index].0.number_text.is_some()).count();
    // Сохранять номера Word можно только если они есть у всех display-формул и согласованы:
    // разрывы/повторы/нелитеральные автономера делают сохранение нечестным.
    let preserved_all = options.preserve_word_numbers
        && !block_indices.is_empty()
        && mixed_indices.is_empty()
        && found == block_indices.len()
        && numbers_are_consistent(&block_indices, &candidates);
    let mut number_texts: Vec<Option<String>> = vec![None; candidates.len()];
    let mut number_indices: Vec<Option<u32>> = vec![None; candidates.len()];
    if preserved_all {
        for (order, index) in block_indices.iter().enumerate() {
            number_texts[*index] = candidates[*index].0.number_text.clone();
            number_indices[*index] = u32::try_from(order + 1).ok();
        }
        report.numbers_preserved = block_indices.len();
    } else if options.number_display && !block_indices.is_empty() {
        // Позициями для схемы «по разделам» служат исходные смещения абзацев: заголовки заменой
        // формульных абзацев не двигаются, принадлежность разделу сохраняется.
        let ordered: Vec<(String, usize)> = block_indices
            .iter()
            .enumerate()
            .map(|(order, index)| (format!("candidate-{order}"), candidates[*index].0.paragraph_start.unwrap_or(usize::MAX)))
            .collect();
        let (values, warnings) = number_formulas(&xml, styles_xml.as_deref(), options.numbering, &ordered);
        report.warnings.extend(warnings);
        for (order, index) in block_indices.iter().enumerate() {
            if let Some(value) = values.get(order) {
                number_texts[*index] = Some(value.number.clone());
            }
            number_indices[*index] = u32::try_from(order + 1).ok();
        }
        report.numbers_assigned = block_indices.len();
        if found > 0 {
            let reason = if options.preserve_word_numbers {
                "номера найдены не у всех display-формул/несогласованы"
            } else {
                "выбрана перенумерация по схеме проекта"
            };
            report.warnings.push(format!(
                "в Word найдены номера у {found} из {} display-формул ({reason}) — назначены по схеме «{}»",
                block_indices.len(),
                options.numbering
            ));
        }
    } else if !block_indices.is_empty() {
        report.warnings.push("нумерация display-формул отключена: номера не назначены".to_owned());
    }
    for index in &mixed_indices {
        let raw = &candidates[*index].0;
        let order = display_indices.iter().position(|candidate| candidate == index).map(|order| order + 1).unwrap_or(0);
        let reason = if raw.in_table { "формула в таблице" } else { "в абзаце есть текст" };
        report.warnings.push(format!("display-формула #{order}: {reason} — оставлена inline, номер не назначен"));
    }
    for (order, index) in display_indices.iter().enumerate() {
        if candidates[*index].0.auto_number {
            report.warnings.push(format!(
                "display-формула #{}: автономер Word (`#`/eqArr) не хранится литерально — {}",
                order + 1,
                if number_texts[*index].is_some() { "назначен по схеме" } else { "номер не назначен" }
            ));
        }
    }

    // ── Шаг 2. Вставки и замены фрагментов ───────────────────────────────────────────────────
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for (index, ((raw, _model), plan)) in candidates.iter().zip(plans.iter()).enumerate() {
        let id = super::next_insert_id(&rde_doc.inserts, &xml);
        let para_id = super::para_id_for(&format!("{}:formula:{id}", rde_doc.doc_id), rde_doc.inserts.len());
        let number_text = number_texts[index].clone();
        let number = number_indices[index];
        let block_formula = raw.block_formula();
        let numbered = block_formula && number_text.is_some();
        let number_source = if preserved_all && number.is_some() {
            NUMBER_SOURCE_WORD.to_owned()
        } else if block_formula && number.is_none() {
            NUMBER_SOURCE_OFF.to_owned()
        } else {
            String::new()
        };
        let suffix = if block_formula { raw.suffix.clone() } else { String::new() };
        // Подпись тела — по фрагменту, который реально попадёт в sdt (inline-вставка display-кэша
        // хранится без `m:oMathPara`, см. `omml_fragment`).
        let embedded_omml = omml_fragment(&plan.omml, block_formula)?;
        let body = FormulaBody {
            formula_id: plan.formula_id.clone(),
            act_key: plan.act_key.clone(),
            latex: plan.latex.clone(),
            display: raw.display,
            numbered,
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
            number,
            number_text: number_text.clone(),
            suffix: suffix.clone(),
            number_source,
            hash: Some(plan.act_key.clone()),
            cached: plan.latex.clone(),
            selector: String::new(),
            value_kind: "formula".to_owned(),
            para_id: if block_formula { Some(para_id.clone()) } else { None },
            style: String::new(),
            format: String::new(),
            run_style: String::new(),
            render_mode: "omml".to_owned(),
            body_hash: Some(omml_signature(&embedded_omml)?),
        };
        if raw.numbered_table_block() {
            let (start, end, _) = raw.numbered_table.clone().ok_or("миграция: нет границ нумерованной таблицы")?;
            let content = wrap_block_sdt(&id, &block_content(&insert, &body, number_text.as_deref(), &suffix, &para_id, None, &entries)?);
            edits.push((start, end, content));
            report.replaced_tables += 1;
            insert.number = number;
        } else if block_formula {
            let (start, end) = match (raw.paragraph_start, raw.paragraph_end) {
                (Some(start), Some(end)) => (start, end),
                _ => return Err("миграция: у формульного абзаца нет границ".to_owned()),
            };
            let content = wrap_block_sdt(&id, &block_content(&insert, &body, number_text.as_deref(), &suffix, &para_id, None, &entries)?);
            edits.push((start, end, content));
            report.replaced_blocks += 1;
        } else {
            // Смешанный display-абзац или inline-формула: заменяем только сам OMML-фрагмент
            // inline-вставкой (block-`w:sdt` внутрь `w:p` не вставляем — невалидный OOXML).
            let content = wrap_inline_sdt(&id, &inline_content(&insert, &body, &para_id, None)?);
            edits.push((raw.start, raw.end, content));
            report.replaced_inline += 1;
        }
        rde_doc.inserts.push(insert);
        report.insert_ids.push(id);
    }
    // Диапазоны замен не должны пересекаться: иначе замены «съедят» друг друга (например,
    // нумерованная таблица с формулами и отдельная формула в её ячейке).
    edits.sort_by_key(|(start, _, _)| *start);
    for pair in edits.windows(2) {
        if pair[0].1 > pair[1].0 {
            return Err("миграция: пересекающиеся диапазоны замен (вложенные формулы в таблице)".to_owned());
        }
    }
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

/// Согласованы ли номера Word для сохранения: все есть, уникальны и (если все числовые) растут.
fn numbers_are_consistent(indices: &[usize], candidates: &[(RawEquation, ModelEquation)]) -> bool {
    let numbers: Vec<&str> = indices
        .iter()
        .filter_map(|index| candidates[*index].0.number_text.as_deref())
        .collect();
    if numbers.len() != indices.len() {
        return false;
    }
    let mut unique = std::collections::BTreeSet::new();
    if !numbers.iter().all(|number| unique.insert((*number).to_owned())) {
        return false;
    }
    let numeric: Option<Vec<u32>> = numbers.iter().map(|number| number.parse::<u32>().ok()).collect();
    match numeric {
        Some(values) => values.windows(2).all(|pair| pair[0] < pair[1]),
        // Буквенные номера (приложения): достаточно полного набора уникальных значений.
        None => true,
    }
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

/// Сырое OMML-вхождение в `word/document.xml`: границы, макет, регистрация в реестре, номер и
/// пунктуация формульного абзаца (ADR-0019).
#[derive(Clone, Debug)]
struct RawEquation {
    start: usize,
    end: usize,
    display: bool,
    registered: bool,
    /// Номер из Word без скобок: `"3"`, `"2.1"`, `"А.1"`; `None` — номера нет.
    number_text: Option<String>,
    /// Нумерованная legacy-таблица 1×2 вокруг формулы: (границы таблицы, номер без скобок).
    numbered_table: Option<(usize, usize, String)>,
    paragraph_start: Option<usize>,
    paragraph_end: Option<usize>,
    /// Кроме формулы в абзаце только табы/пробелы/пунктуация/номер.
    paragraph_only: bool,
    /// Пунктуация сразу после формулы (`,`, `.`, `;`).
    suffix: String,
    /// Формула внутри таблицы, но не в нумерованной 1×2: не нумеруем (ограничение).
    in_table: bool,
    /// Автономер Word (`#`/eqArr) без литерального значения.
    auto_number: bool,
}

impl RawEquation {
    /// display-формула отдельного абзаца — можно заменить block-вставкой.
    fn block_paragraph(&self) -> bool {
        self.display && !self.in_table && self.paragraph_only
    }

    /// display-формула в legacy-нумерованной таблице 1×2 — заменяется таблица целиком.
    fn numbered_table_block(&self) -> bool {
        self.display && self.numbered_table.is_some()
    }

    /// display-формула, которую нельзя вынести в block (смешанный абзац/таблица): останется inline.
    fn mixed_display(&self) -> bool {
        self.display && !self.block_paragraph() && !self.numbered_table_block()
    }

    /// Формула станет block-вставкой в миграции и может получить номер.
    fn block_formula(&self) -> bool {
        self.block_paragraph() || self.numbered_table_block()
    }
}

#[derive(Clone)]
struct OpenTag {
    name: String,
    start: usize,
}

/// Есть ли в тексте открывающий тег абзаца (`<w:p `, `<w:p>`, `<w:p/`; не `<w:pPr>`).
fn contains_paragraph_tag(text: &str) -> bool {
    let mut from = 0usize;
    while let Some(relative) = text[from..].find("<w:p") {
        let at = from + relative;
        let after = text.as_bytes().get(at + "<w:p".len()).copied();
        if matches!(after, Some(b' ' | b'>' | b'/')) {
            return true;
        }
        from = at + "<w:p".len();
    }
    false
}

/// Найти все `m:oMath`/`m:oMathPara` в документе (документный порядок) с контекстом абзаца.
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
                    let math_end = end + 1;
                    // Границы настоящего абзаца: формула должна быть внутри него, а не в следующем.
                    let paragraph = paragraph_bounds(xml, open.start).filter(|(paragraph_start, _)| {
                        // Между найденным абзацем и формулой не должно быть других абзацев
                        // (`<w:pPr>` и прочие теги с префиксом `w:p` не считаются абзацем).
                        let inside = &xml[paragraph_start + 4..open.start];
                        !contains_paragraph_tag(inside) && !inside.contains("</w:p>")
                    });
                    let paragraph_start = paragraph.map(|(start, _)| start);
                    let paragraph_end = paragraph.map(|(_, end)| end);
                    let (number_text, suffix, paragraph_only, auto_number) = match (paragraph_start, paragraph_end) {
                        (Some(paragraph_start), Some(paragraph_end)) => {
                            let parts = paragraph_parts(xml, paragraph_start, paragraph_end, open.start, math_end);
                            (parts.number_text, parts.suffix, parts.paragraph_only, parts.auto_number)
                        }
                        _ => (None, String::new(), false, false),
                    };
                    let numbered_table = if display { ancestor_numbered_table(xml, &stack, math_end) } else { None };
                    let number_text = numbered_table.as_ref().map(|(_, _, number)| number.clone()).or(number_text);
                    let in_table = stack.iter().any(|open| open.name == "w:tbl") && numbered_table.is_none();
                    out.push(RawEquation {
                        start: open.start,
                        end: math_end,
                        display,
                        registered,
                        number_text,
                        numbered_table,
                        paragraph_start,
                        paragraph_end,
                        paragraph_only,
                        suffix,
                        in_table,
                        auto_number,
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

/// Контекст формулы внутри абзаца: номер, пунктуация, «чистота» абзаца, автономер.
struct ParagraphParts {
    number_text: Option<String>,
    suffix: String,
    paragraph_only: bool,
    auto_number: bool,
}

/// Разобрать абзац вокруг формулы: допустимые соседи display-формулы — табы/пробелы, пунктуация
/// (`, . ; : — – -` и т.п.) и номер `(N)`/`(N.M)`/`(А.N)`, в т.ч. в полях Word (`SEQ MTEqn`).
fn paragraph_parts(xml: &str, paragraph_start: usize, paragraph_end: usize, math_start: usize, math_end: usize) -> ParagraphParts {
    let before = &xml[paragraph_start..math_start];
    let after = &xml[math_end..paragraph_end];
    let (visible_before, non_text_before) = visible_text(before);
    let (visible_after, non_text_after) = visible_text(after);
    let literal_before = literal_text(before);
    let literal_after = literal_text(after);
    // Номер: кэш поля `SEQ MTEqn \c` (MathType/Word) или литеральный `(N)` в конце абзаца.
    let number_text = mteqn_cached_number(after).or_else(|| trailing_paren_number(&literal_after));
    let suffix = suffix_punctuation(&literal_after, number_text.as_deref());
    let prefix_clean = visible_before.chars().all(char::is_whitespace) && literal_before.trim().is_empty();
    let suffix_clean = is_punctuation_only(&suffix);
    // Видимый текст после формулы помимо номера не должен содержать слов: поле Word могло
    // оставить в результате не только номер (например, «, (3)» — ок, «, см. (3)» — смешанный).
    let visible_after_words = visible_after
        .chars()
        .filter(|ch| !ch.is_whitespace())
        .collect::<String>();
    let after_clean = is_punctuation_only(&visible_after_words)
        || number_text.as_deref().is_some_and(|number| visible_after_words.is_empty() || visible_after_words.contains(number));
    let (auto_number, auto_number_text) = eqarr_number(&xml[math_start..math_end]);
    ParagraphParts {
        number_text: number_text.or(auto_number_text),
        suffix,
        paragraph_only: prefix_clean && suffix_clean && after_clean && !non_text_before && !non_text_after,
        auto_number,
    }
}

/// Номер в конце литерального текста абзаца: `(3)`, `(2.1)`, `(А.1)`.
fn trailing_paren_number(text: &str) -> Option<String> {
    let trimmed = text.trim_matches(|ch: char| ch.is_whitespace());
    let open = trimmed.rfind('(')?;
    parse_paren_number_text(&trimmed[open..])
}

/// Пунктуация после формулы: литеральный текст абзаца после формулы без номера и пробелов.
fn suffix_punctuation(literal_after: &str, number_text: Option<&str>) -> String {
    let mut text = literal_after.trim_matches(char::is_whitespace).to_owned();
    if let Some(number) = number_text
        && let Some(open) = text.rfind('(')
        && parse_paren_number_text(&text[open..]).as_deref() == Some(number)
    {
        text.truncate(open);
    }
    text.trim_matches(char::is_whitespace).to_owned()
}

/// Номер `(3)`/`(2.1)`/`(А.1)` из строки (без пробелов по краям).
fn parse_paren_number_text(text: &str) -> Option<String> {
    let trimmed = text.trim();
    let inner = trimmed.strip_prefix('(')?.strip_suffix(')')?.trim();
    if inner.is_empty() {
        return None;
    }
    let first = inner.chars().next()?;
    if !(first.is_ascii_digit() || first.is_uppercase()) {
        return None;
    }
    if !inner.chars().all(|ch| ch.is_ascii_digit() || ch == '.' || ch.is_uppercase()) {
        return None;
    }
    if inner.starts_with('.') || inner.ends_with('.') || inner.contains("..") {
        return None;
    }
    Some(inner.to_owned())
}

/// Автономер Word: литеральный `#`/`#(N)` в конце формулы (eqArr-нумерация).
///
/// Word при вводе `#(N)` в линейном формате превращает формулу в `m:eqArr`, а номер хранит
/// отдельной ячейкой/текстом; литеральное значение есть не всегда. Если значение найдено — оно
/// используется как номер, иначе номер будет назначен по схеме (с предупреждением).
fn eqarr_number(math_xml: &str) -> (bool, Option<String>) {
    let mut number = None;
    let mut marker = false;
    let mut pos = 0usize;
    while let Some(rel) = math_xml[pos..].find("<m:t") {
        let start = pos + rel;
        let Some(open_end) = math_xml[start..].find('>').map(|offset| start + offset + 1) else { break };
        let Some(close) = math_xml[open_end..].find("</m:t>") else { break };
        let text = &math_xml[open_end..open_end + close];
        if let Some(at) = text.find('#') {
            marker = true;
            let rest = text[at + 1..].trim();
            if let Some(found) = trailing_paren_number(rest).or_else(|| parse_paren_number_text(rest)) {
                number = Some(found);
            }
        }
        pos = open_end + close + "</m:t>".len();
    }
    (marker, number)
}

/// Нумерованная legacy-таблица 1×2 вокруг формулы: (границы таблицы, номер без скобок).
fn ancestor_numbered_table(xml: &str, stack: &[OpenTag], equation_end: usize) -> Option<(usize, usize, String)> {
    let table = stack.iter().rev().find(|open| open.name == "w:tbl")?;
    let start = table.start;
    let end = balanced_end(xml, start, "w:tbl")?;
    let table_xml = xml.get(start..end)?;
    // Строго 1×2 (как пишет legacy `equation.rs`): иначе это обычная таблица данных.
    if table_xml.matches("</w:tr>").count() != 1 || table_xml.matches("</w:tc>").count() != 2 {
        return None;
    }
    let after = xml.get(equation_end.min(end)..end)?;
    let mut rest = after;
    while let Some(index) = next_text_run(rest) {
        let Some(open_end) = rest[index..].find('>') else { break };
        let inner_start = index + open_end + 1;
        let Some(close_rel) = rest[inner_start..].find("</w:t>") else { break };
        if let Some(number) = parse_paren_number_text(rest[inner_start..inner_start + close_rel].trim()) {
            return Some((start, end, number));
        }
        rest = &rest[inner_start + close_rel..];
    }
    None
}

/// Границы элемента `tag` от открывающего тега, с балансировкой вложений того же имени.
fn balanced_end(xml: &str, start: usize, tag: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut pos = start;
    while pos < xml.len() {
        let rel = xml[pos..].find('<')?;
        let at = pos + rel;
        let rel_end = xml[at..].find('>')?;
        let end = at + rel_end;
        let body = &xml[at + 1..end];
        let closing = body.starts_with('/');
        let self_closing = body.ends_with('/');
        let name = body.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("");
        if name == tag {
            if closing {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(end + 1);
                }
            } else if self_closing {
                if depth == 0 {
                    return Some(end + 1);
                }
            } else {
                depth += 1;
            }
        }
        pos = end + 1;
    }
    None
}

/// Начало следующего текстового run (`<w:t>`/`<w:t ...>`, не `<w:tc>`/`<w:tbl>`/`<w:tabs>`).
fn next_text_run(text: &str) -> Option<usize> {
    let mut from = 0usize;
    while let Some(relative) = text[from..].find("<w:t") {
        let at = from + relative;
        let after = text.as_bytes().get(at + "<w:t".len()).copied();
        if matches!(after, Some(b' ' | b'>' | b'/')) {
            return Some(at);
        }
        from = at + "<w:t".len();
    }
    None
}

// ── поиск sdt формулы в document.xml ─────────────────────────────────────────

/// Границы sdt формулы в `word/document.xml`.
struct FormulaSdt {
    content_start: usize,
    content_end: usize,
}

/// Границы **целого** `w:sdt` формулы (внешнего, если исторические вложения): замена блока
/// целиком лечит вложенные content-controls от прежних обновлений (они ломали XML-документ).
fn find_formula_sdt_block(xml: &str, insert_id: &str) -> Option<(usize, usize)> {
    let tag = format!("<w:tag w:val=\"{FORMULA_TAG_PREFIX}{insert_id}\"");
    let tag_index = xml.find(&tag)?;
    // Самый внешний `w:sdt` вокруг тега: идём по открывающим тегам до позиции тега.
    let mut outer: Option<usize> = None;
    let mut stack: Vec<usize> = Vec::new();
    let mut pos = 0usize;
    while pos < tag_index {
        let Some(rel) = xml[pos..].find('<') else { break };
        let start = pos + rel;
        let Some(rel_end) = xml[start..].find('>') else { break };
        let end = start + rel_end;
        let body = &xml[start + 1..end];
        let closing = body.starts_with('/');
        let self_closing = body.ends_with('/');
        let name = body.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("");
        if name == "w:sdt" {
            if closing {
                stack.pop();
            } else if !self_closing {
                stack.push(start);
            }
        }
        pos = end + 1;
    }
    outer = stack.first().copied().or(outer);
    let start = outer?;
    // Конец блока: балансировка `<w:sdt>`/`</w:sdt>` (точное имя, без sdtPr/sdtContent).
    let mut depth = 0usize;
    let mut pos = start;
    while pos < xml.len() {
        let Some(rel) = xml[pos..].find('<') else { break };
        let at = pos + rel;
        let Some(rel_end) = xml[at..].find('>') else { break };
        let end = at + rel_end;
        let body = &xml[at + 1..end];
        let closing = body.starts_with('/');
        let self_closing = body.ends_with('/');
        let name = body.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("");
        if name == "w:sdt" {
            if closing {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((start, end + 1));
                }
            } else if !self_closing {
                depth += 1;
            } else if depth == 1 {
                return Some((start, end + 1));
            }
        }
        pos = end + 1;
    }
    None
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
    fn parses_paren_number_texts() {
        assert_eq!(parse_paren_number_text("(12)").as_deref(), Some("12"));
        assert_eq!(parse_paren_number_text("( 3 )").as_deref(), Some("3"));
        assert_eq!(parse_paren_number_text("(2.1)").as_deref(), Some("2.1"));
        assert_eq!(parse_paren_number_text("(А.1)").as_deref(), Some("А.1"));
        assert_eq!(parse_paren_number_text("(Б.12)").as_deref(), Some("Б.12"));
        assert!(parse_paren_number_text("рис. 3").is_none());
        assert!(parse_paren_number_text("(рис. 3)").is_none());
        assert!(parse_paren_number_text("()").is_none());
        assert!(parse_paren_number_text("(1.)").is_none());
    }

    /// MathType-абзац как в реальном Word-документе: таб, формула, пунктуация, таб и поля номера.
    fn display_paragraph(omml: &str, punctuation: &str, number: Option<&str>) -> String {
        let number_xml = match number {
            Some(number) => format!(
                "<w:r><w:tab/></w:r>\
                 <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>\
                 <w:r><w:instrText xml:space=\"preserve\"> MACROBUTTON MTPlaceRef \\* MERGEFORMAT </w:instrText></w:r>\
                 <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>\
                 <w:r><w:instrText xml:space=\"preserve\"> SEQ MTEqn \\h \\* MERGEFORMAT </w:instrText></w:r>\
                 <w:r><w:fldChar w:fldCharType=\"end\"/></w:r>\
                 <w:r><w:instrText>(</w:instrText></w:r>\
                 <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>\
                 <w:r><w:instrText xml:space=\"preserve\"> SEQ MTEqn \\c \\* Arabic \\* MERGEFORMAT </w:instrText></w:r>\
                 <w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>\
                 <w:r><w:rPr><w:noProof/></w:rPr><w:instrText>{number}</w:instrText></w:r>\
                 <w:r><w:fldChar w:fldCharType=\"end\"/></w:r>\
                 <w:r><w:instrText>)</w:instrText></w:r>\
                 <w:r><w:fldChar w:fldCharType=\"end\"/></w:r>"
            ),
            None => String::new(),
        };
        format!(
            "<w:p w14:paraId=\"1A2B3C4D\"><w:pPr><w:pStyle w:val=\"MTDisplayEquation\"/></w:pPr>\
             <w:r><w:tab/></w:r>{omml}<w:r><w:t xml:space=\"preserve\">{punctuation}</w:t></w:r>{number_xml}</w:p>"
        )
    }

    fn paragraph_doc(paragraphs: &str) -> String {
        format!("<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\"><w:body>{paragraphs}</w:body></w:document>")
    }

    fn display_omml(text: &str) -> String {
        format!("<m:oMathPara><m:oMath><m:r><m:t>{text}</m:t></m:r></m:oMath></m:oMathPara>")
    }

    #[test]
    fn raw_equations_reads_numbered_table_and_ignores_nested_math() {
        let xml = "<w:document><w:body><w:tbl><w:tr><w:tc><w:p><m:oMathPara><m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></m:oMathPara></w:p></w:tc><w:tc><w:p><w:r><w:t>(7)</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>";
        let raw = raw_equations(xml);
        assert_eq!(raw.len(), 1, "nested m:oMath не считается отдельно: {raw:?}");
        assert!(raw[0].display);
        assert_eq!(raw[0].number_text.as_deref(), Some("7"));
        assert!(raw[0].numbered_table.is_some(), "таблица 1×2 распознана: {raw:?}");
        // В левой ячейке нумерованной таблицы — только формула (абзац «чистый»), но миграция
        // выбирает замену таблицы целиком (numbered_table имеет приоритет над paragraph_only).
        assert!(raw[0].paragraph_only);
        assert!(raw[0].block_formula());
    }

    #[test]
    fn raw_equations_reads_display_field_number_and_punctuation() {
        let xml = paragraph_doc(&display_paragraph(&display_omml("Δr>ΔR"), ".", Some("3")));
        let raw = raw_equations(&xml);
        assert_eq!(raw.len(), 1, "{raw:?}");
        assert!(raw[0].display);
        assert_eq!(raw[0].number_text.as_deref(), Some("3"), "номер из поля SEQ MTEqn: {raw:?}");
        assert_eq!(raw[0].suffix, ".");
        assert!(raw[0].paragraph_only, "табы/пунктуация/номер не делают абзац смешанным: {raw:?}");
        assert!(raw[0].block_formula());
        assert!(!raw[0].mixed_display());
    }

    #[test]
    fn raw_equations_reads_literal_paren_numbers() {
        for (punctuation, number, expected) in [
            (",", Some("(5)"), Some("5")),
            ("", Some("(2.1)"), Some("2.1")),
            ("", Some("(А.1)"), Some("А.1")),
            (".", None, None),
        ] {
            let paragraph = format!(
                "<w:p><w:r><w:tab/></w:r>{}{}<w:r><w:tab/></w:r>{}</w:p>",
                display_omml("x"),
                if punctuation.is_empty() { String::new() } else { format!("<w:r><w:t>{punctuation}</w:t></w:r>") },
                number.map(|number| format!("<w:r><w:t>{number}</w:t></w:r>")).unwrap_or_default()
            );
            let raw = raw_equations(&paragraph_doc(&paragraph));
            assert_eq!(raw.len(), 1);
            assert_eq!(raw[0].number_text.as_deref(), expected, "номер {number:?}: {raw:?}");
            assert_eq!(raw[0].suffix, punctuation, "пунктуация {punctuation:?}");
            assert!(raw[0].paragraph_only);
        }
    }

    #[test]
    fn raw_equations_keeps_display_in_mixed_paragraph_inline() {
        let paragraph = format!("<w:p><w:r><w:t>где </w:t></w:r>{}<w:r><w:t> — постоянная.</w:t></w:r></w:p>", display_omml("x"));
        let raw = raw_equations(&paragraph_doc(&paragraph));
        assert_eq!(raw.len(), 1);
        assert!(raw[0].display);
        assert!(!raw[0].paragraph_only);
        assert!(raw[0].mixed_display(), "содержательный текст → inline: {raw:?}");
    }

    #[test]
    fn raw_equations_ignores_data_tables_and_flags_them() {
        let table = "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc></w:tr>\
                     <w:tr><w:tc><w:p><m:oMath><m:r><m:t>y</m:t></m:r></m:oMath></w:p></w:tc><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr></w:tbl>";
        let raw = raw_equations(&paragraph_doc(table));
        assert_eq!(raw.len(), 1);
        assert!(raw[0].in_table, "обычная таблица — не нумерованная: {raw:?}");
        assert!(raw[0].numbered_table.is_none());
        assert!(!raw[0].display);
    }

    #[test]
    fn raw_equations_detects_eqarr_auto_number() {
        let omml = "<m:oMathPara><m:oMath><m:eqArr><m:e><m:r><m:t>x=1</m:t></m:r></m:e><m:e><m:r><m:t>#(4)</m:t></m:r></m:e></m:eqArr></m:oMath></m:oMathPara>";
        let paragraph = format!("<w:p>{omml}</w:p>");
        let raw = raw_equations(&paragraph_doc(&paragraph));
        assert_eq!(raw.len(), 1);
        assert!(raw[0].auto_number, "маркер # распознан: {raw:?}");
        assert_eq!(raw[0].number_text.as_deref(), Some("4"));
    }

    #[test]
    fn paragraph_bounds_cover_whole_formula_paragraph() {
        let paragraph = display_paragraph(&display_omml("x"), ",", Some("1"));
        let xml = paragraph_doc(&paragraph);
        let math = xml.find("<m:oMathPara").unwrap();
        let (start, end) = paragraph_bounds(&xml, math).unwrap();
        assert!(xml[start..end].starts_with("<w:p "));
        assert!(xml[start..end].ends_with("</w:p>"));
        assert!(xml[start..end].contains("SEQ MTEqn"), "поля номера внутри границ абзаца: {}", &xml[start..end]);
    }
}
