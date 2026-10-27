//! Правка текста блоков `.doc.rde` (M2.5): замена текста абзаца с защитой от внешних правок.
//!
//! Зачем: R-AGT-05 / R-DOC-32 требуют, чтобы правки агента были **предложениями**, а не молчаливой
//! перезаписью пользовательского текста. Домен «suggested edits» ([`super::suggestions`]) хранит
//! changeset (block_id + ожидаемый исходный текст + предложенный текст + автор + базовый
//! `body_hash`), а эта функция — единственный низкоуровневый способ применить замену к пакету.
//!
//! Правила (fail-closed):
//! - документ должен быть валидным `.doc.rde` с актуальным `rde:doc` (иначе — [`BlockTextEditError::Conflict`]:
//!   `body_hash` не совпадает, документ правили вне RDE — перечитывать и повторять);
//! - правится **целиком абзац** по `block_id` (а не пользовательский диапазон); `expected_text`
//!   обязан совпадать с текущим текстом блока (`Stale` иначе — R-DOC-13-семантика);
//! - блок обязан быть «простым абзацем»: без `w:sdt`-вставок RDE, полей Word, рисунков, формул,
//!   track changes, вложенных абзацев (иначе — [`BlockTextEditError::Unsupported`]);
//! - новый текст непустой и без переводов строк (M2.5: удаление абзаца/вставка новых абзацев —
//!   отдельная операция M2.6, чтобы не сдвигать позиционные `block_id` b1..bN);
//! - форматирование абзаца (`w:pPr`) и первого run (`w:rPr`) сохраняются; меняется только текст.
//!
//! Функция не делает бэкапов/транзакций — это обязанность вызывающего слоя (`rde-host`):
//! чекпоинт (R-AGT-05) + версия документа в store ([`super::commit_document`]).

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::{document_blocks, load_rde_doc_from_entries, read_package, update_document_package,
            write_entries_atomic, PackageInfo, RdeDoc, DOCUMENT_XML};

/// Запрос замены текста блока (предложение агента или прямая правка с approval).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockTextEdit {
    /// Стабильный id блока (`b1`, `b2`, …) из `doc_outline`/`doc_read`.
    pub block_id: String,
    /// Ожидаемый текущий текст блока целиком (защита от устаревания, R-DOC-13).
    pub expected_text: String,
    /// Новый текст абзаца (непустой, без переводов строк).
    pub new_text: String,
}

/// Ошибка правки блока (машиночитаемые категории; `rde-host` маппит их в `RDE_*`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BlockTextEditError {
    /// Документ/блок не найден.
    NotFound(String),
    /// Блок не является простым абзацем либо операция не поддерживается M2.5.
    Unsupported(String),
    /// Текст блока уже не тот, что ожидался (устаревшее чтение/внешняя правка) — `RDE_CONFLICT`.
    Stale {
        /// Ожидавшийся текст.
        expected: String,
        /// Фактический текст блока.
        actual: String,
    },
    /// Конфликт состояния: документ изменён вне RDE (`body_hash`), статус предложения и т.п.
    Conflict(String),
    /// Внутренняя ошибка чтения/записи пакета.
    Internal(String),
}

impl fmt::Display for BlockTextEditError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockTextEditError::NotFound(message) => write!(f, "{message}"),
            BlockTextEditError::Unsupported(message) => write!(f, "{message}"),
            BlockTextEditError::Stale { expected, actual } => write!(
                f,
                "текст блока изменился: ожидался {:?}, сейчас {:?}",
                expected, actual
            ),
            BlockTextEditError::Conflict(message) => write!(f, "{message}"),
            BlockTextEditError::Internal(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for BlockTextEditError {}

/// Результат замены текста блока.
#[derive(Clone, Debug)]
pub struct BlockTextEditReport {
    /// Id блока.
    pub block_id: String,
    /// `w14:paraId` абзаца (анкор блока).
    pub para_id: String,
    /// Текст до правки (фактический).
    pub before: String,
    /// Текст после правки (фактический, как его извлёк WordCraft).
    pub after: String,
    /// Новое состояние пакета (body_hash/части).
    pub package: PackageInfo,
    /// Обновлённый реестр `rde:doc`.
    pub rde_doc: RdeDoc,
}

/// Проверить, что блок можно править этой операцией (без записи файла).
///
/// # Errors
///
/// [`BlockTextEditError::NotFound`]/[`Unsupported`]/[`Conflict`].
pub fn block_edit_support(package: &Path, block_id: &str) -> Result<(), BlockTextEditError> {
    resolve_editable_block(package, block_id).map(|_| ())
}

/// Заменить текст абзаца в `.doc.rde` (см. правила модуля).
///
/// # Errors
///
/// [`BlockTextEditError`]: `NotFound`/`Unsupported`/`Stale`/`Conflict`/`Internal`.
pub fn replace_block_text(
    package: &Path,
    edit: &BlockTextEdit,
) -> Result<BlockTextEditReport, BlockTextEditError> {
    validate_new_text(&edit.new_text)?;
    if !package.is_file() {
        return Err(BlockTextEditError::NotFound(format!(
            "пакет документа не найден: {}",
            package.display()
        )));
    }
    let mut resolved = resolve_editable_block(package, &edit.block_id)?;
    if resolved.full_text != edit.expected_text {
        return Err(BlockTextEditError::Stale {
            expected: edit.expected_text.clone(),
            actual: resolved.full_text.clone(),
        });
    }
    // Новый абзац: тот же открывающий тег (с paraId/атрибутами), pPr + первый rPr сохраняются.
    let text = escape_xml(&edit.new_text);
    let run = format!("<w:r>{}<w:t xml:space=\"preserve\">{text}</w:t></w:r>", resolved.run_props);
    let new_paragraph = format!("{}{}{}</w:p>", resolved.open_tag, resolved.paragraph_props, run);
    let mut document_xml = std::mem::take(&mut resolved.document_xml);
    let tail = document_xml.split_off(resolved.end);
    document_xml.truncate(resolved.start);
    document_xml.push_str(&new_paragraph);
    document_xml.push_str(&tail);

    let mut entries = resolved.entries;
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    // Пакет с новым document.xml, но старым rde:doc: временное состояние — сразу пересобираем
    // реестр (blocks/body_hash/paraId) штатным путём `update_document_package` (как UI «Обновить»).
    write_entries_atomic(&entries, package).map_err(BlockTextEditError::Internal)?;
    let update = update_document_package(package).map_err(BlockTextEditError::Internal)?;
    let rde_doc = update.rde_doc.ok_or_else(|| {
        BlockTextEditError::Internal("после правки не удалось перечитать rde:doc".to_owned())
    })?;
    let blocks = document_blocks(package).map_err(BlockTextEditError::Internal)?;
    let after = blocks
        .iter()
        .find(|block| block.id == edit.block_id)
        .map(|block| block.text.clone())
        .ok_or_else(|| {
            BlockTextEditError::Internal(format!("блок {} исчез после правки", edit.block_id))
        })?;
    Ok(BlockTextEditReport {
        block_id: edit.block_id.clone(),
        para_id: resolved.para_id,
        before: resolved.full_text,
        after,
        package: update.package.unwrap_or_default(),
        rde_doc,
    })
}

// ── M2.6: вставка и удаление абзацев (block_id сдвигаются у последующих блоков) ──

/// Куда вставить новый абзац относительно блока-якоря.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsertPosition {
    /// После якорного блока.
    After,
    /// Перед якорным блоком.
    Before,
}

impl InsertPosition {
    /// Разобрать `after|before` (без учёта регистра).
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "after" => Some(InsertPosition::After),
            "before" => Some(InsertPosition::Before),
            _ => None,
        }
    }

    /// Строковое значение (`after`/`before`).
    pub fn as_str(self) -> &'static str {
        match self {
            InsertPosition::After => "after",
            InsertPosition::Before => "before",
        }
    }
}

/// Запрос вставки нового абзаца (M2.6, R-DOC-32): один абзац, без переводов строк.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParagraphInsert {
    /// Блок-якорь (`None` — в конец документа перед `w:sectPr`).
    pub anchor_block: Option<String>,
    /// Позиция относительно якоря.
    pub position: InsertPosition,
    /// Текст нового абзаца (непустой, без переводов строк).
    pub text: String,
    /// Стиль абзаца (`w:pStyle`); пусто — форматирование якоря.
    pub style: String,
}

/// Результат вставки абзаца.
#[derive(Clone, Debug)]
pub struct ParagraphInsertReport {
    /// Id нового блока (позиционный `bN`; последующие блоки сдвинулись).
    pub block_id: String,
    /// Стабильный `w14:paraId` нового абзаца.
    pub para_id: String,
    /// Текст нового блока (как извлечён WordCraft).
    pub text: String,
    /// Якорь (если был).
    pub anchor_block: Option<String>,
    /// Новое состояние пакета.
    pub package: PackageInfo,
    /// Обновлённый реестр `rde:doc`.
    pub rde_doc: RdeDoc,
}

/// Запрос удаления абзаца (M2.6).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockDelete {
    /// Id удаляемого блока.
    pub block_id: String,
    /// Ожидаемый текст блока (защита от устаревшего чтения, R-DOC-13).
    pub expected_text: String,
}

/// Результат удаления абзаца.
#[derive(Clone, Debug)]
pub struct BlockDeleteReport {
    /// Id удалённого блока (в документе он уже не существует).
    pub block_id: String,
    /// `w14:paraId` удалённого абзаца.
    pub para_id: String,
    /// Текст удалённого блока.
    pub before: String,
    /// Новое состояние пакета.
    pub package: PackageInfo,
    /// Обновлённый реестр `rde:doc`.
    pub rde_doc: RdeDoc,
}

/// Вставить новый абзац в `.doc.rde` (M2.6).
///
/// Абзац получает стабильный `w14:paraId`; форматирование копируется у якоря (или последнего
/// редактируемого абзаца при `anchor_block = None`), стиль можно переопределить. Id блоков
/// позиционные (`bN`): блоки после места вставки сдвигаются; подписи/формулы/вставки RDE не
/// перезаписываются (новый абзац пустой по смыслу, без sdt).
///
/// # Errors
///
/// [`BlockTextEditError`]: `NotFound` (нет якоря), `Unsupported` (непустой/переводы строк,
/// якорь не простой абзац), `Conflict` (внешняя правка), `Internal`.
pub fn insert_paragraph(
    package: &Path,
    request: &ParagraphInsert,
) -> Result<ParagraphInsertReport, BlockTextEditError> {
    validate_new_text(&request.text)?;
    let (mut entries, document_xml, rde_doc) = load_fresh_document(package)?;
    let before_blocks = document_blocks(package)
        .map_err(|error| BlockTextEditError::Internal(format!("блоки документа: {error}")))?;
    // Индекс нового блока в видимом списке (id позиционные: `b{index+1}`; после engine-сохранения
    // paraId переинжектируются позиционно, поэтому id нового абзаца вычисляем по позиции).
    let mut expected_index = before_blocks.len();
    let mut anchor_block_id: Option<String> = None;
    // Форматирование и место вставки: у якоря, иначе — последний редактируемый абзац и конец тела.
    let (paragraph_props, run_props, insert_at) = match &request.anchor_block {
        Some(anchor) => {
            let anchor_index = before_blocks
                .iter()
                .position(|block| &block.id == anchor)
                .ok_or_else(|| {
                    BlockTextEditError::NotFound(format!("блок `{anchor}` не найден в документе"))
                })?;
            let resolved = resolve_editable_block(package, anchor)?;
            let insert_at = match request.position {
                InsertPosition::After => resolved.end,
                InsertPosition::Before => resolved.start,
            };
            expected_index = match request.position {
                InsertPosition::After => anchor_index + 1,
                InsertPosition::Before => anchor_index,
            };
            anchor_block_id = Some(anchor.clone());
            (resolved.paragraph_props, resolved.run_props, insert_at)
        }
        None => {
            let mut formatting = (String::new(), String::new());
            for block in before_blocks.iter().rev() {
                if let Ok(resolved) = resolve_editable_block(package, &block.id) {
                    formatting = (resolved.paragraph_props, resolved.run_props);
                    break;
                }
            }
            let insert_at = append_position(&document_xml);
            (formatting.0, formatting.1, insert_at)
        }
    };
    let paragraph_props = apply_paragraph_style(&paragraph_props, &request.style);
    let para_id = fresh_para_id(&document_xml, &rde_doc, &request.text);
    let text = escape_xml(request.text.trim());
    let paragraph = format!(
        "<w:p w14:paraId=\"{para_id}\">{paragraph_props}<w:r>{run_props}<w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>"
    );
    let mut document_xml = document_xml;
    document_xml.insert_str(insert_at, &paragraph);
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    write_entries_atomic(&entries, package).map_err(BlockTextEditError::Internal)?;
    let update = update_document_package(package).map_err(BlockTextEditError::Internal)?;
    let rde_doc = update.rde_doc.ok_or_else(|| {
        BlockTextEditError::Internal("после вставки не удалось перечитать rde:doc".to_owned())
    })?;
    let new_block = rde_doc.blocks.get(expected_index).ok_or_else(|| {
        BlockTextEditError::Internal("вставленный абзац не появился в реестре rde:doc".to_owned())
    })?;
    let expected_text = request.text.trim();
    if new_block.text != expected_text {
        return Err(BlockTextEditError::Internal(format!(
            "после вставки текст блока `{}` отличается: ожидался {:?}, получен {:?}",
            new_block.id, expected_text, new_block.text
        )));
    }
    Ok(ParagraphInsertReport {
        block_id: new_block.id.clone(),
        para_id: new_block.para_id.clone().unwrap_or(para_id),
        text: new_block.text.clone(),
        anchor_block: anchor_block_id,
        package: update.package.unwrap_or_default(),
        rde_doc,
    })
}

/// Удалить абзац из `.doc.rde` (M2.6).
///
/// Нельзя удалить последний непустой блок; блоки с вставками RDE/полями/рисунками/формулами/
/// track changes не удаляются (`Unsupported`). Id блоков позиционные — последующие сдвигаются.
///
/// # Errors
///
/// [`BlockTextEditError`]: `NotFound`, `Unsupported`, `Stale` (текст не совпал), `Conflict`,
/// `Internal`.
pub fn delete_block(
    package: &Path,
    request: &BlockDelete,
) -> Result<BlockDeleteReport, BlockTextEditError> {
    let resolved = resolve_editable_block(package, &request.block_id)?;
    if resolved.full_text != request.expected_text {
        return Err(BlockTextEditError::Stale {
            expected: request.expected_text.clone(),
            actual: resolved.full_text.clone(),
        });
    }
    if resolved.visible_blocks <= 1 {
        return Err(BlockTextEditError::Unsupported(
            "нельзя удалить единственный блок документа".to_owned(),
        ));
    }
    let mut document_xml = resolved.document_xml;
    document_xml.replace_range(resolved.start..resolved.end, "");
    let mut entries = resolved.entries;
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    write_entries_atomic(&entries, package).map_err(BlockTextEditError::Internal)?;
    let update = update_document_package(package).map_err(BlockTextEditError::Internal)?;
    let rde_doc = update.rde_doc.ok_or_else(|| {
        BlockTextEditError::Internal("после удаления не удалось перечитать rde:doc".to_owned())
    })?;
    Ok(BlockDeleteReport {
        block_id: request.block_id.clone(),
        para_id: resolved.para_id,
        before: resolved.full_text,
        package: update.package.unwrap_or_default(),
        rde_doc,
    })
}

/// Позиция для вставки в конец тела: перед `w:sectPr` (он обязан быть последним в `w:body`).
fn append_position(document_xml: &str) -> usize {
    let Some(body_end) = document_xml.rfind("</w:body>") else {
        return document_xml.len();
    };
    let body = &document_xml[..body_end];
    body.rfind("<w:sectPr").unwrap_or(body_end)
}

/// Подставить стиль абзаца в копию `w:pPr` (пустой стиль — не менять).
fn apply_paragraph_style(paragraph_props: &str, style: &str) -> String {
    let style = style.trim();
    if style.is_empty() {
        return paragraph_props.to_owned();
    }
    let style_attr = escape_xml(style);
    if paragraph_props.contains("<w:pStyle ") {
        // Заменяем значение существующего стиля (атрибут `w:val`).
        let Some(start) = paragraph_props.find("<w:pStyle ") else { return paragraph_props.to_owned() };
        let Some(val_start) = paragraph_props[start..].find("w:val=\"") else {
            return paragraph_props.to_owned();
        };
        let val_start = start + val_start + "w:val=\"".len();
        let Some(val_end) = paragraph_props[val_start..].find('"') else {
            return paragraph_props.to_owned();
        };
        let mut out = String::with_capacity(paragraph_props.len() + style_attr.len());
        out.push_str(&paragraph_props[..val_start]);
        out.push_str(&style_attr);
        out.push_str(&paragraph_props[val_start + val_end..]);
        return out;
    }
    if let Some(open) = paragraph_props.find("<w:pPr>") {
        let insert_at = open + "<w:pPr>".len();
        let mut out = String::with_capacity(paragraph_props.len() + style_attr.len() + 32);
        out.push_str(&paragraph_props[..insert_at]);
        out.push_str(&format!("<w:pStyle w:val=\"{style_attr}\"/>"));
        out.push_str(&paragraph_props[insert_at..]);
        out
    } else {
        format!("<w:pPr><w:pStyle w:val=\"{style_attr}\"/></w:pPr>")
    }
}

/// Свежий `w14:paraId` (8 hex, верхний регистр), не встречающийся в документе и реестре.
fn fresh_para_id(document_xml: &str, rde_doc: &RdeDoc, text: &str) -> String {
    for index in 0..1000usize {
        let hash = rde_hash::sha256(
            format!("{}:para-insert:{index}:{}", rde_doc.doc_id, text).as_bytes(),
        )
        .hex();
        let candidate: String = hash.chars().take(8).collect::<String>().to_uppercase();
        let taken = document_xml.contains(&format!("w14:paraId=\"{candidate}\""))
            || rde_doc
                .blocks
                .iter()
                .any(|block| block.para_id.as_deref() == Some(candidate.as_str()));
        if !taken {
            return candidate;
        }
    }
    // Практически недостижимо; детерминированный fallback (всё равно может совпасть).
    "7DE0A1A6".to_owned()
}

fn validate_new_text(new_text: &str) -> Result<(), BlockTextEditError> {
    if new_text.trim().is_empty() {
        return Err(BlockTextEditError::Unsupported(
            "пустой текст недопустим при замене: для удаления абзаца используйте `doc_delete` (M2.6)"
                .to_owned(),
        ));
    }
    if new_text.contains('\n') || new_text.contains('\r') {
        return Err(BlockTextEditError::Unsupported(
            "переводы строк внутри абзаца не поддерживаются: вставка нового абзаца — `doc_insert` (M2.6)"
                .to_owned(),
        ));
    }
    Ok(())
}

/// Разрешённый к правке блок: всё, что нужно для пересборки `w:p` байт-в-байт по форматированию.
struct ResolvedBlock {
    para_id: String,
    full_text: String,
    open_tag: String,
    paragraph_props: String,
    run_props: String,
    /// Начало `<w:p …>` в `word/document.xml`.
    start: usize,
    /// Позиция сразу после `</w:p>`.
    end: usize,
    /// Видимых блоков в документе (для запрета удалить последний блок, M2.6).
    visible_blocks: usize,
    document_xml: String,
    entries: std::collections::BTreeMap<String, Vec<u8>>,
}

fn load_fresh_document(
    package: &Path,
) -> Result<(std::collections::BTreeMap<String, Vec<u8>>, String, RdeDoc), BlockTextEditError> {
    if !package.is_file() {
        return Err(BlockTextEditError::NotFound(format!(
            "пакет документа не найден: {}",
            package.display()
        )));
    }
    let entries = read_package(package)
        .map_err(|error| BlockTextEditError::Internal(format!("пакет .doc.rde: {error}")))?
        .entries;
    let document_xml = entries
        .get(DOCUMENT_XML)
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .ok_or_else(|| BlockTextEditError::Internal("в пакете нет word/document.xml".to_owned()))?;
    let rde_doc = load_rde_doc_from_entries(&entries)
        .map_err(|error| BlockTextEditError::Internal(format!("rde:doc: {error}")))?;
    // R-DOC-13: документ, изменённый вне RDE, править нельзя — сначала перечитать/переимпортировать.
    let actual_hash = rde_hash::sha256(document_xml.as_bytes()).hex();
    if !rde_doc.body_hash.is_empty() && rde_doc.body_hash != actual_hash {
        return Err(BlockTextEditError::Conflict(
            "документ изменён вне RDE (body_hash не совпадает с rde:doc) — перечитайте doc_list/doc_read и повторите"
                .to_owned(),
        ));
    }
    Ok((entries, document_xml, rde_doc))
}

fn resolve_editable_block(
    package: &Path,
    block_id: &str,
) -> Result<ResolvedBlock, BlockTextEditError> {
    let (entries, document_xml, rde_doc) = load_fresh_document(package)?;
    let blocks = document_blocks(package)
        .map_err(|error| BlockTextEditError::Internal(format!("блоки документа: {error}")))?;
    let full_text = blocks
        .iter()
        .find(|block| block.id == block_id)
        .map(|block| block.text.clone())
        .ok_or_else(|| {
            BlockTextEditError::NotFound(format!("блок `{block_id}` не найден в документе"))
        })?;
    let para_id = rde_doc
        .blocks
        .iter()
        .find(|block| block.id == block_id)
        .and_then(|block| block.para_id.clone())
        .ok_or_else(|| {
            BlockTextEditError::Unsupported(format!(
                "у блока `{block_id}` нет w14:paraId — документ не в актуальном формате RDE"
            ))
        })?;
    let (start, content_end, end, open_tag) = paragraph_bounds(&document_xml, &para_id)?;
    let inner = &document_xml[start + open_tag.len()..content_end];
    check_editable_paragraph(inner, block_id)?;
    let paragraph_props = extract_element(inner, "w:pPr").unwrap_or_default();
    let after_ppr = if paragraph_props.is_empty() {
        inner
    } else {
        match inner.find(&paragraph_props) {
            Some(index) => &inner[index + paragraph_props.len()..],
            None => inner,
        }
    };
    let run_props = first_run_props(after_ppr);
    Ok(ResolvedBlock {
        para_id,
        full_text,
        open_tag,
        paragraph_props,
        run_props,
        start,
        end,
        visible_blocks: rde_doc.blocks.len(),
        document_xml,
        entries,
    })
}

/// Границы абзаца: `(start, content_end, end, open_tag)` — `end` сразу после `</w:p>`.
fn paragraph_bounds(
    document_xml: &str,
    para_id: &str,
) -> Result<(usize, usize, usize, String), BlockTextEditError> {
    let needle = format!("w14:paraId=\"{para_id}\"");
    let id_pos = document_xml.find(&needle).ok_or_else(|| {
        BlockTextEditError::NotFound(format!(
            "абзац с w14:paraId `{para_id}` не найден в word/document.xml"
        ))
    })?;
    let open_start = document_xml[..id_pos]
        .rfind("<w:p")
        .ok_or_else(|| BlockTextEditError::Internal("не найден открывающий <w:p>".to_owned()))?;
    let tag_rest = &document_xml[open_start + "<w:p".len()..];
    if !(tag_rest.starts_with(' ') || tag_rest.starts_with('>') || tag_rest.starts_with('\t') || tag_rest.starts_with('\n')) {
        return Err(BlockTextEditError::Internal(
            "w14:paraId найден не в элементе <w:p>".to_owned(),
        ));
    }
    let open_end = open_start
        + document_xml[open_start..]
            .find('>')
            .ok_or_else(|| BlockTextEditError::Internal("открывающий <w:p> не закрыт".to_owned()))?;
    let open_tag = document_xml[open_start..=open_end].to_owned();
    if open_tag.trim_end().ends_with("/>") {
        return Err(BlockTextEditError::Unsupported(
            "пустой абзац (self-closing <w:p/>) не содержит текста".to_owned(),
        ));
    }
    let close_rel = document_xml[open_end + 1..]
        .find("</w:p>")
        .ok_or_else(|| BlockTextEditError::Internal("не найден </w:p>".to_owned()))?;
    let close_start = open_end + 1 + close_rel;
    Ok((open_start, close_start, close_start + "</w:p>".len(), open_tag))
}

fn check_editable_paragraph(inner: &str, block_id: &str) -> Result<(), BlockTextEditError> {
    // Вложенные абзацы (например, в `w:txbxContent` текстового поля) — не правим построчно.
    if inner.contains("<w:p ") || inner.contains("<w:p>") {
        return Err(BlockTextEditError::Unsupported(format!(
            "блок `{block_id}` содержит вложенные абзацы — правка не поддерживается"
        )));
    }
    let unsupported: [(&str, &str); 8] = [
        ("<w:sdt", "вставка RDE (w:sdt)"),
        ("<w:fldChar", "поле Word"),
        ("<w:instrText", "поле Word"),
        ("<w:drawing", "рисунок"),
        ("<w:object", "встроенный объект"),
        ("<m:oMath", "формула OMML"),
        ("<w:ins", "track changes (w:ins)"),
        ("<w:del", "track changes (w:del)"),
    ];
    for (marker, label) in unsupported {
        if inner.contains(marker) {
            return Err(BlockTextEditError::Unsupported(format!(
                "блок `{block_id}` содержит {label} — правка текста этим инструментом недоступна"
            )));
        }
    }
    Ok(())
}

/// Границы первого элемента с данным именем (включая открывающий/закрывающий тег либо self-closing).
fn element_bounds(text: &str, name: &str) -> Option<(usize, usize)> {
    let open_prefix = format!("<{name}");
    let start = text.find(&open_prefix)?;
    let after = &text[start + open_prefix.len()..];
    let first = after.chars().next()?;
    if !(first == ' ' || first == '>' || first == '\t' || first == '\n' || first == '/') {
        return None;
    }
    let gt = text[start..].find('>')? + start;
    if text[start..=gt].trim_end().ends_with("/>") {
        return Some((start, gt + 1));
    }
    let close = format!("</{name}>");
    let close_rel = text[gt + 1..].find(&close)?;
    Some((start, gt + 1 + close_rel + close.len()))
}

fn extract_element(text: &str, name: &str) -> Option<String> {
    element_bounds(text, name).map(|(start, end)| text[start..end].to_owned())
}

/// `w:rPr` первого run абзаца (для сохранения форматирования текста).
fn first_run_props(inner: &str) -> String {
    let run_start = match inner.find("<w:r>") {
        Some(index) => index,
        None => match inner.find("<w:r ") {
            Some(index) => index,
            None => return String::new(),
        },
    };
    let run = &inner[run_start..];
    let Some(gt) = run.find('>') else { return String::new() };
    let after = &run[gt + 1..];
    // Пропускаем текст перед rPr (в обычном OOXML rPr идёт первым, но не полагаемся на это).
    let search_end = after.find("</w:r>").unwrap_or(after.len());
    let head = &after[..search_end];
    extract_element(head, "w:rPr").unwrap_or_default()
}

/// XML-экранирование текста (без переводов строк — они запрещены выше).
fn escape_xml(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}
