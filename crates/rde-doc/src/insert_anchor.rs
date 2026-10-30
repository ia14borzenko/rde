//! P9 (итерация `inline-formula-insert`): однозначный якорь вставки в `word/document.xml`.
//!
//! Проблема, которую решает модуль: GUI RDE (WordCraft) отдаёт каретку как путь блока модели
//! (`Path::top(220)` и т. п.) и смещение в тексте абзаца, а `.doc.rde` хранит разметку OOXML, где
//! абзац адресуется `w14:paraId`. Раньше якорь передавался как «индекс top-level блока» и сравнивал
//! разные системы координат: WordCraft считает блоки модели (включая блоки внутри block-level
//! `w:sdt`), а вставка искала N-й **top-level `<w:p>`** — поэтому формула уезжала в чужой абзац или
//! молча дописывалась в конец документа.
//!
//! Здесь:
//! * [`InsertAnchor`] — единый якорь: `w14:paraId` (приоритет), либо порядковый номер абзаца в
//!   порядке документа (для импортированных абзацев без `paraId`), плюс байтовое смещение в тексте
//!   абзаца для inline-вставки; `legacy_block` — совместимость со старым «индексом блока тела»;
//! * [`body_paragraph_spans`] — обход абзацев `word/document.xml` **в том же порядке, в котором их
//!   видит WordCraft** (спуск в таблицы/`w:sdt`/`w:customXml`, пропуск `w:txbxContent` и удалённых
//!   блоков, синтетические абзацы из block-level `m:oMath`); соответствие модели подтверждается
//!   сравнением числа абзацев разметки и модели — при расхождении вставка возвращает ошибку, а не
//!   «угадывает»;
//! * [`insert_inline_at`] — позиционная вставка inline-содержимого: разрыв `w:r`/`w:t` в позиции
//!   каретки (с сохранением форматирования и экранирования), либо явный `Err`/предупреждение;
//! * [`insert_block_after`] — display-вставка block-уровня после блока, содержащего абзац-якорь.
//!
//! Ограничения (осознанные, с понятной ошибкой): вставка внутрь отслеживаемой правки
//! (`w:ins`/`w:del`), вложенного content-control или поля не поддерживается; каретка внутри абзаца
//! таблицы для inline работает (абзац — обычный `w:p`), display-вставка в таблицу вставляется после
//! таблицы-блока с предупреждением.

/// Символ-заместитель inline-объекта в тексте абзаца WordCraft (`U+FFFC`).
const OBJ_CHAR: char = '\u{FFFC}';
/// Маркер `w14:paraId` в разметке.
const PARA_ID_ATTR: &str = "w14:paraId=\"";

/// Якорь вставки в документ (P9).
///
/// Приоритет разрешения: [`Self::para_id`] → [`Self::ordinal`] → [`Self::legacy_block`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InsertAnchor {
    /// Стабильный анкор Word `w14:paraId` целевого абзаца (приоритетный якорь).
    pub para_id: Option<String>,
    /// Порядковый номер абзаца в порядке документа WordCraft (0-based) — для абзацев без `paraId`.
    pub ordinal: Option<usize>,
    /// Байтовое смещение каретки в тексте абзаца (WordCraft `Pos::off`); `None` — конец абзаца.
    pub offset: Option<usize>,
    /// Текст абзаца в модели WordCraft: по нему проверяется корректность разреза разметки
    /// (позиционная вставка не «угадывает»). `None` — без проверки (только конец абзаца).
    pub text: Option<String>,
    /// Старый якорь display-вставок: индекс top-level блока тела OOXML (только совместимость).
    pub legacy_block: Option<usize>,
}

impl InsertAnchor {
    /// Якоря нет: display-формула дописывается в конец документа (прежнее поведение P3).
    pub fn end() -> Self {
        Self::default()
    }

    /// Якорь на конкретный абзац по `paraId` и позиции каретки.
    pub fn paragraph(para_id: impl Into<String>, offset: Option<usize>, text: Option<String>) -> Self {
        Self { para_id: Some(para_id.into()), ordinal: None, offset, text, legacy_block: None }
    }

    /// Якорь на абзац по порядковому номеру (абзац без `w14:paraId`).
    pub fn ordinal(ordinal: usize, offset: Option<usize>, text: Option<String>) -> Self {
        Self { para_id: None, ordinal: Some(ordinal), offset, text, legacy_block: None }
    }

    /// Совместимость: старый «индекс блока тела» для display-вставок (0-based).
    pub fn legacy_block(index: usize) -> Self {
        Self { legacy_block: Some(index), ..Self::default() }
    }

    /// Якорь пуст (ничего не задано).
    pub fn is_empty(&self) -> bool {
        self.para_id.is_none() && self.ordinal.is_none() && self.legacy_block.is_none()
    }
}

/// Абзац `word/document.xml` в порядке документа WordCraft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParagraphSpan {
    /// Начало открывающего тега `<w:p ...>`.
    pub start: usize,
    /// Позиция после открывающего тега.
    pub open_end: usize,
    /// Позиция начала закрывающего `</w:p>` (для самозакрытого — `open_end`).
    pub close_start: usize,
    /// Позиция после закрывающего тега.
    pub end: usize,
    /// `w14:paraId`, если есть в разметке.
    pub para_id: Option<String>,
    /// Порядковый номер абзаца в порядке документа (0-based).
    pub ordinal: usize,
    /// Абзац внутри ячейки таблицы.
    pub in_table: bool,
}

/// Результат позиционной inline-вставки: где именно оказалось содержимое.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InlinePlacement {
    pub para_id: Option<String>,
    pub ordinal: Option<usize>,
    /// Смещение, если вставка позиционная (`None` — дописано в конец абзаца).
    pub offset: Option<usize>,
    /// Позиция каретки соблюдена точно (разрез разметки выполнен).
    pub exact: bool,
    /// Контекст перед вставкой (хвост текста абзаца до позиции).
    pub context_before: String,
    /// Контекст после вставки (начало текста абзаца после позиции).
    pub context_after: String,
}

/// Ошибка разрешения якоря/позиции: разметка при этом не изменяется.
pub type InsertAnchorError = String;

// ── разбор XML: теги, сущности, границы элементов ───────────────────────────

fn find_tag_end(xml: &str, start: usize) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (offset, ch) in xml[start..].char_indices() {
        match quote {
            Some(open) => {
                if ch == open {
                    quote = None;
                }
            }
            None => match ch {
                '"' | '\'' => quote = Some(ch),
                '>' => return Some(start + offset),
                _ => {}
            },
        }
    }
    None
}

/// Позиция после специальной секции (`<!--…-->`, `<![CDATA[…]]>`, `<?…?>`, `<!…>`).
fn skip_special(xml: &str, start: usize) -> Option<usize> {
    let rest = &xml[start..];
    let find = |marker: &str, skip: usize| rest.find(marker).map(|at| start + at + skip);
    if rest.starts_with("<!--") {
        return find("-->", 3);
    }
    if rest.starts_with("<![CDATA[") {
        return find("]]>", 3);
    }
    if rest.starts_with("<?") {
        return find("?>", 2);
    }
    if rest.starts_with("<!") {
        return find_tag_end(xml, start).map(|end| end + 1);
    }
    None
}

/// Имя элемента из содержимого тега (без `<`/`>` и закрывающего `/`).
fn tag_name(raw: &str) -> &str {
    let raw = raw.strip_prefix('/').unwrap_or(raw);
    let end = raw.find(|ch: char| ch.is_whitespace() || ch == '/' || ch == '>').unwrap_or(raw.len());
    &raw[..end]
}

fn is_self_closing(raw: &str) -> bool {
    raw.trim_end().ends_with('/')
}

/// Конец элемента с именем `name`, открытый в `open_end`: (`close_start`, `after_close`).
fn find_matching_close(xml: &str, name: &str, open_end: usize) -> Option<(usize, usize)> {
    let limit = xml.len();
    let mut depth = 1usize;
    let mut pos = open_end;
    while pos < limit {
        let rel = xml[pos..limit].find('<')?;
        let start = pos + rel;
        if let Some(after) = skip_special(xml, start) {
            pos = after;
            continue;
        }
        let end = find_tag_end(xml, start)?;
        let raw = &xml[start + 1..end];
        let closing = raw.starts_with('/');
        let self_closing = !closing && is_self_closing(raw);
        if tag_name(raw) == name {
            if closing {
                depth -= 1;
                if depth == 0 {
                    return Some((start, end + 1));
                }
            } else if !self_closing {
                depth += 1;
            }
        }
        pos = end + 1;
    }
    None
}

/// Открывающий тег `name` в разметке: (`start`, `open_end`).
fn find_open_tag(xml: &str, name: &str, from: usize) -> Option<(usize, usize)> {
    let mut pos = from;
    while let Some(rel) = xml[pos..].find('<') {
        let start = pos + rel;
        if let Some(after) = skip_special(xml, start) {
            pos = after;
            continue;
        }
        let end = find_tag_end(xml, start)?;
        let raw = &xml[start + 1..end];
        if !raw.starts_with('/') && tag_name(raw) == name {
            return Some((start, end + 1));
        }
        pos = end + 1;
    }
    None
}

/// Декодировать XML-сущность в начале `raw` (начинается с `&`): символ и длина сущности в байтах.
fn decode_entity(raw: &str) -> Option<(char, usize)> {
    let end = raw.find(';')?;
    let body = &raw[1..end];
    let ch = match body {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        _ => {
            let number = body.strip_prefix("#x").or_else(|| body.strip_prefix("#X"))?;
            let code = u32::from_str_radix(number, 16).ok()?;
            let ch = char::from_u32(code)?;
            if ch.is_control() && !matches!(ch, '\t' | '\n' | '\r') {
                return None;
            }
            ch
        }
    };
    Some((ch, end + 1))
}

/// Текст `w:t`/`w:delText` → символы модели WordCraft с диапазонами в исходной разметке.
///
/// Повторяет `clean_text` читалки: `\n`/`\r` → пробел, `\t` → таб, служебные символы управления
/// и `U+FFFC` отбрасываются.
fn push_text_chars(out: &mut Vec<(char, usize, usize)>, raw: &str, base: usize) {
    let mut pos = 0usize;
    while pos < raw.len() {
        let rest = &raw[pos..];
        if rest.starts_with('&')
            && let Some((ch, len)) = decode_entity(rest)
        {
            push_clean_char(out, ch, base + pos, base + pos + len);
            pos += len;
            continue;
        }
        let Some(ch) = rest.chars().next() else { break };
        let len = ch.len_utf8();
        push_clean_char(out, ch, base + pos, base + pos + len);
        pos += len;
    }
}

fn push_clean_char(out: &mut Vec<(char, usize, usize)>, ch: char, start: usize, end: usize) {
    let clean = match ch {
        '\n' | '\r' => Some(' '),
        '\t' => Some('\t'),
        OBJ_CHAR => None,
        ch if (ch as u32) < 0x20 => None,
        ch => Some(ch),
    };
    if let Some(ch) = clean {
        out.push((ch, start, end));
    }
}

// ── обход абзацев в порядке WordCraft ───────────────────────────────────────

/// Блоки, содержимое которых WordCraft **не** кладёт в Body-историю: удалённые правки и текст
/// текстовых полей (они уходят в отдельные истории/части).
fn in_ignored_block(stack: &[&str]) -> bool {
    stack
        .iter()
        .rev()
        .take_while(|name| **name != "w:body")
        .any(|name| matches!(*name, "w:del" | "w:moveFrom" | "w:txbxContent"))
}

/// Прочитать `w14:paraId` из открывающего тега абзаца.
fn para_id_from_tag(tag: &str) -> Option<String> {
    let rest = tag.split(PARA_ID_ATTR).nth(1)?;
    let id = rest.split('"').next()?.trim();
    (!id.is_empty()).then(|| id.to_owned())
}

/// Абзацы тела в порядке документа WordCraft (см. `Reader::read_blocks`).
///
/// Спускается в `w:tbl`/`w:tr`/`w:tc`, block-level `w:sdt`, `w:customXml`/`w:smartTag`/`w:ins`/
/// `w:moveTo`, выбирает одну ветку `mc:AlternateContent`; не входит в `w:p` вложенных историй
/// (`w:txbxContent`), удалённые блоки и служебные свойства.
pub fn body_paragraph_spans(xml: &str) -> Result<Vec<ParagraphSpan>, InsertAnchorError> {
    let (_, body_open_end) = find_open_tag(xml, "w:body", 0).ok_or_else(|| "document.xml: нет <w:body>".to_owned())?;
    let body_close = xml.rfind("</w:body>").ok_or_else(|| "document.xml: нет </w:body>".to_owned())?;
    let mut spans: Vec<ParagraphSpan> = Vec::new();
    let mut stack: Vec<&str> = Vec::new();
    let mut pending: Option<(usize, usize, Option<String>, usize, bool)> = None;
    let mut pending_math: Option<(usize, usize)> = None;
    // Ветка `mc:AlternateContent`, которую выбрала бы читалка: (глубина, начало выбранного ребёнка).
    let mut alternate: Vec<(usize, Option<usize>)> = Vec::new();
    let mut pos = body_open_end;
    while pos < body_close {
        let Some(rel) = xml[pos..body_close].find('<') else {
            break;
        };
        let start = pos + rel;
        if let Some(after) = skip_special(xml, start) {
            pos = after;
            continue;
        }
        let end = find_tag_end(xml, start).ok_or_else(|| format!("document.xml: незакрытый тег на позиции {start}"))?;
        let raw = &xml[start + 1..end];
        let closing = raw.starts_with('/');
        let self_closing = !closing && is_self_closing(raw);
        let name = tag_name(raw);
        if closing {
            // Ошибка структуры — не «угадываем», а сообщаем (иначе можно разрезать чужой абзац).
            let Some(top) = stack.pop() else {
                return Err(format!("document.xml: лишний </{name}> на позиции {start}"));
            };
            if top != name {
                return Err(format!("document.xml: </{name}> закрывает <{top}> на позиции {start}"));
            }
            let depth = stack.len() + 1;
            if name == "w:p" && pending.as_ref().is_some_and(|entry| entry.3 == depth) {
                let (para_start, open_end, para_id, _, in_table) = pending.take().unwrap();
                spans.push(ParagraphSpan {
                    start: para_start,
                    open_end,
                    close_start: start,
                    end: end + 1,
                    para_id,
                    ordinal: spans.len(),
                    in_table,
                });
            }
            if pending_math.is_some_and(|entry| entry.1 == depth) {
                let (math_start, _) = pending_math.take().unwrap();
                spans.push(ParagraphSpan {
                    start: math_start,
                    open_end: end + 1,
                    close_start: start,
                    end: end + 1,
                    para_id: None,
                    ordinal: spans.len(),
                    in_table: false,
                });
            }
            if let Some(entry) = alternate.last()
                && entry.0 == stack.len()
            {
                alternate.pop();
            }
            pos = end + 1;
            continue;
        }
        // Открывающий тег: ветка mc:AlternateContent — только выбранная (иначе абзацы Fallback
        // посчитались бы дважды и порядок якоря разъехался бы с моделью WordCraft).
        if let Some((depth, chosen)) = alternate.last().copied()
            && depth == stack.len()
        {
            if chosen != Some(start) {
                let (_, after) = find_matching_close(xml, name, end + 1)
                    .ok_or_else(|| format!("document.xml: нет конца <{name}>"))?;
                pos = after;
                continue;
            }
            alternate.pop();
        }
        let nested_para = stack.contains(&"w:p");
        let in_table = stack.contains(&"w:tbl");
        if name == "mc:AlternateContent" && !self_closing {
            alternate.push((stack.len(), choose_alternate_branch(xml, end + 1)));
        }
        if name == "w:p" {
            let recordable = !nested_para && !in_ignored_block(&stack);
            stack.push(name);
            if self_closing {
                stack.pop();
                if recordable {
                    spans.push(ParagraphSpan {
                        start,
                        open_end: end + 1,
                        close_start: end + 1,
                        end: end + 1,
                        para_id: para_id_from_tag(raw),
                        ordinal: spans.len(),
                        in_table,
                    });
                }
            } else if recordable {
                pending = Some((start, end + 1, para_id_from_tag(raw), stack.len(), in_table));
            }
            pos = end + 1;
            continue;
        }
        // Math разрешён прямо в истории (EG_BlockLevelElts): WordCraft заводит для него абзац.
        // Вложенные oMath внутри oMathPara — объекты одного синтетического абзаца.
        let math_block = matches!(name, "m:oMathPara" | "m:oMath")
            && !nested_para
            && !in_ignored_block(&stack)
            && !stack.iter().any(|item| matches!(*item, "m:oMathPara" | "m:oMath"));
        if math_block {
            stack.push(name);
            if self_closing {
                stack.pop();
                spans.push(ParagraphSpan {
                    start,
                    open_end: end + 1,
                    close_start: end + 1,
                    end: end + 1,
                    para_id: None,
                    ordinal: spans.len(),
                    in_table: false,
                });
            } else {
                pending_math = Some((start, stack.len()));
            }
            pos = end + 1;
            continue;
        }
        if !self_closing {
            stack.push(name);
        }
        pos = end + 1;
    }
    if !stack.is_empty() {
        return Err(format!("document.xml: остались незакрытые элементы: {stack:?}"));
    }
    Ok(spans)
}

/// Прочитать `word/document.xml` из `.doc.rde`/`.docx` пакета (для резолва якоря в GUI).
pub fn document_xml(package: &std::path::Path) -> Result<String, InsertAnchorError> {
    let entries = super::read_package(package)?.entries;
    let bytes = entries.get(crate::DOCUMENT_XML).ok_or_else(|| "в пакете нет word/document.xml".to_owned())?;
    Ok(String::from_utf8_lossy(bytes).to_string())
}

/// Найти `w14:paraId` абзацев в порядке документа WordCraft (для кэша GUI).
pub fn body_paragraph_ids(document_xml: &str) -> Result<Vec<Option<String>>, InsertAnchorError> {
    Ok(body_paragraph_spans(document_xml)?.into_iter().map(|span| span.para_id).collect())
}

/// Выбранная ветка `mc:AlternateContent` (как `children_of_choice` в WordCraft): первая `mc:Choice`
/// с известными `Requires`, иначе `mc:Fallback`, иначе первая `mc:Choice`.
fn choose_alternate_branch(xml: &str, open_end: usize) -> Option<usize> {
    const KNOWN: &[&str] = &[
        "wps", "wpg", "wpc", "wpi", "w14", "w15", "wp14", "a14", "w16se", "w16cid", "w16", "w16cex", "w16sdtdh", "v",
        "m",
    ];
    let (_, ac_end) = find_matching_close(xml, "mc:AlternateContent", open_end)?;
    let mut pos = open_end;
    let mut fallback = None;
    let mut any_choice = None;
    while pos < ac_end {
        let rel = xml[pos..ac_end].find('<')?;
        let start = pos + rel;
        if let Some(after) = skip_special(xml, start) {
            pos = after;
            continue;
        }
        let end = find_tag_end(xml, start)?;
        let raw = &xml[start + 1..end];
        if raw.starts_with('/') || is_self_closing(raw) {
            pos = end + 1;
            continue;
        }
        let name = tag_name(raw);
        match name {
            "mc:Choice" => {
                any_choice.get_or_insert(start);
                let known = raw
                    .split("Requires=\"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .map(|requires| requires.split_whitespace().all(|token| KNOWN.contains(&token)))
                    .unwrap_or(false);
                if known {
                    return Some(start);
                }
            }
            "mc:Fallback" => {
                fallback.get_or_insert(start);
            }
            _ => {}
        }
        let (_, after) = find_matching_close(xml, name, end + 1)?;
        pos = after;
    }
    fallback.or(any_choice)
}

/// Разрешить `paraId` по пути каретки WordCraft: `(paraId, порядковый номер абзаца)`.
///
/// Число абзацев разметки и модели WordCraft должно совпадать, иначе соответствие неоднозначно —
/// возвращается ошибка (никакого «угадывания» по номеру).
pub fn resolve_para_id_for_path(
    package: &std::path::Path,
    doc: &wordcraft_engine::doc::Document,
    path: &wordcraft_engine::doc::Path,
) -> Result<(String, usize), InsertAnchorError> {
    let entries = super::read_package(package)?.entries;
    let bytes = entries.get(crate::DOCUMENT_XML).ok_or_else(|| "в пакете нет word/document.xml".to_owned())?;
    let xml = String::from_utf8_lossy(bytes).to_string();
    let spans = body_paragraph_spans(&xml)?;
    let paths = doc.para_paths(wordcraft_engine::doc::StoryRef::Body);
    if spans.len() != paths.len() {
        return Err(format!(
            "каретка не сопоставлена с разметкой: абзацев в document.xml {}, в модели документа {} — \
             сохраните документ («Сохранить проект») и повторите",
            spans.len(),
            paths.len()
        ));
    }
    let ordinal = paths
        .iter()
        .position(|candidate| candidate == path)
        .ok_or_else(|| format!("абзац {path} не найден в модели документа"))?;
    let span = spans.get(ordinal).ok_or_else(|| format!("абзац {ordinal} вне диапазона разметки"))?;
    let para_id = span.para_id.clone().ok_or_else(|| {
        format!("у абзаца {ordinal} ({path}) нет w14:paraId — вставка невозможна без пересохранения документа")
    })?;
    Ok((para_id, ordinal))
}

/// Разрешить якорь в конкретный абзац разметки.
pub(crate) fn locate_paragraph(xml: &str, anchor: &InsertAnchor) -> Result<ParagraphSpan, InsertAnchorError> {
    let spans = body_paragraph_spans(xml)?;
    if let Some(para_id) = &anchor.para_id {
        return spans.iter().find(|span| span.para_id.as_deref() == Some(para_id.as_str())).cloned().ok_or_else(|| {
            format!(
                "абзац с w14:paraId=\"{para_id}\" не найден в документе (документ изменился? пересохраните и повторите)"
            )
        });
    }
    if let Some(ordinal) = anchor.ordinal {
        return spans.get(ordinal).cloned().ok_or_else(|| format!("абзац #{ordinal} не найден в документе"));
    }
    Err("нет якоря абзаца: укажите w14:paraId каретки (режим RDE, каретка в тексте)".to_owned())
}

/// Top-level элемент тела, содержащий абзац: (`start`, `end`, `это w:tbl`).
pub(crate) fn top_level_element_for(
    xml: &str,
    span: &ParagraphSpan,
) -> Result<(usize, usize, bool), InsertAnchorError> {
    let (_, body_open_end) = find_open_tag(xml, "w:body", 0).ok_or_else(|| "document.xml: нет <w:body>".to_owned())?;
    let body_close = xml.rfind("</w:body>").ok_or_else(|| "document.xml: нет </w:body>".to_owned())?;
    let mut pos = body_open_end;
    while pos < body_close {
        let Some(rel) = xml[pos..body_close].find('<') else {
            break;
        };
        let start = pos + rel;
        if let Some(after) = skip_special(xml, start) {
            pos = after;
            continue;
        }
        let end = find_tag_end(xml, start).ok_or_else(|| "document.xml: незакрытый тег".to_owned())?;
        let raw = &xml[start + 1..end];
        if raw.starts_with('/') {
            pos = end + 1;
            continue;
        }
        let name = tag_name(raw);
        let (_, after) = if is_self_closing(raw) {
            (end, end + 1)
        } else {
            find_matching_close(xml, name, end + 1).ok_or_else(|| format!("document.xml: нет конца <{name}>"))?
        };
        if start <= span.start && span.end <= after {
            return Ok((start, after, name == "w:tbl"));
        }
        pos = after;
    }
    Err("абзац не найден на верхнем уровне тела документа".to_owned())
}

// ── текст абзаца в модели WordCraft и обратная карта ────────────────────────

/// Ячейка текста модели WordCraft: один символ и путь элементов разметки до него.
#[derive(Clone, Debug)]
struct StreamCell {
    /// Символ модели (для длины в UTF-8).
    ch: char,
    /// Байтовый диапазон символа в `word/document.xml` (для сущностей — весь `&…;`).
    xml_start: usize,
    xml_end: usize,
    /// Индексы элементов [`StreamElem`] от абзаца до элемента-носителя символа.
    chain: Vec<usize>,
}

#[derive(Clone, Debug)]
struct StreamElem {
    name: String,
    start: usize,
    open_end: usize,
}

struct Stream {
    text: String,
    cells: Vec<StreamCell>,
    elements: Vec<StreamElem>,
}

/// Собрать текст абзаца в модели WordCraft с обратной картой на разметку.
///
/// Правила совпадают с читалкой WordCraft для inline-содержимого `w:p`: `w:t`/`w:delText`,
/// табы/обрывы/спецсимволы, inline-объекты (`m:oMath`, `w:drawing`, поля…) — один `U+FFFC`.
fn build_stream(xml: &str, span: &ParagraphSpan) -> Result<Stream, InsertAnchorError> {
    let mut text = String::new();
    let mut cells: Vec<StreamCell> = Vec::new();
    let mut elements: Vec<StreamElem> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut alternate: Vec<(usize, Option<usize>)> = Vec::new();
    let mut fields: Vec<()> = Vec::new();
    let mut pos = span.open_end;
    while pos < span.close_start {
        let Some(rel) = xml[pos..span.close_start].find('<') else {
            break;
        };
        let start = pos + rel;
        if let Some(after) = skip_special(xml, start) {
            pos = after;
            continue;
        }
        let end = find_tag_end(xml, start).ok_or_else(|| format!("document.xml: незакрытый тег на позиции {start}"))?;
        let raw = &xml[start + 1..end];
        let closing = raw.starts_with('/');
        let self_closing = !closing && is_self_closing(raw);
        let name = tag_name(raw);
        if closing {
            let Some(index) = stack.pop() else {
                return Err(format!("document.xml: лишний </{name}> в абзаце на позиции {start}"));
            };
            if elements[index].name != name {
                return Err(format!("document.xml: нарушена вложенность </{name}> на позиции {start}"));
            }
            if let Some(entry) = alternate.last()
                && entry.0 == stack.len()
            {
                alternate.pop();
            }
            if name == "w:sdt" {
                emit_cell(&mut text, &mut cells, &stack, OBJ_CHAR, start, end + 1);
            }
            pos = end + 1;
            continue;
        }
        // Ветка mc:AlternateContent — только выбранная.
        if let Some((depth, chosen)) = alternate.last().copied()
            && depth == stack.len()
        {
            if chosen != Some(start) {
                let (_, after) = find_matching_close(xml, name, end + 1)
                    .ok_or_else(|| format!("document.xml: нет конца <{name}>"))?;
                pos = after;
                continue;
            }
            alternate.pop();
        }
        if name == "mc:AlternateContent" && !self_closing {
            alternate.push((stack.len(), choose_alternate_branch(xml, end + 1)));
        }
        // Свойства абзаца/прогона, коды полей и прочие служебные поддеревья — пропускаем целиком.
        if matches!(
            name,
            "w:pPr"
                | "w:rPr"
                | "w:rPrChange"
                | "w:pPrChange"
                | "w:sdtPr"
                | "w:sdtEndPr"
                | "w:instrText"
                | "w:delInstrText"
                | "w:proofErr"
                | "w:lastRenderedPageBreak"
                | "w:sectPr"
        ) {
            if self_closing {
                pos = end + 1;
                continue;
            }
            let (_, after) =
                find_matching_close(xml, name, end + 1).ok_or_else(|| format!("document.xml: нет конца <{name}>"))?;
            pos = after;
            continue;
        }
        // Объекты, которые WordCraft держит в тексте как один `U+FFFC` (перед разбором вложенных
        // объектов их поддеревья не нужны).
        if matches!(
            name,
            "w:drawing"
                | "w:pict"
                | "w:object"
                | "w:contentPart"
                | "w:ruby"
                | "w:pgNum"
                | "w:ptab"
                | "w:footnoteReference"
                | "w:endnoteReference"
                | "w:footnoteRef"
                | "w:endnoteRef"
                | "w:commentReference"
                | "m:oMath"
        ) {
            emit_cell(&mut text, &mut cells, &stack, OBJ_CHAR, start, end + 1);
            if self_closing {
                pos = end + 1;
                continue;
            }
            let (_, after) =
                find_matching_close(xml, name, end + 1).ok_or_else(|| format!("document.xml: нет конца <{name}>"))?;
            pos = after;
            continue;
        }
        // Текстовые элементы: содержимое + разметка в карту.
        if matches!(name, "w:t" | "w:delText") {
            let (close_start, after_close) = if self_closing {
                (end + 1, end + 1)
            } else {
                find_matching_close(xml, name, end + 1).ok_or_else(|| format!("document.xml: нет конца <{name}>"))?
            };
            if stack.is_empty() || !elements[*stack.last().unwrap()].name.eq("w:r") {
                // `w:t` вне `w:r` читалка игнорирует.
                pos = after_close;
                continue;
            }
            let mut chars = Vec::new();
            push_text_chars(&mut chars, &xml[end + 1..close_start], end + 1);
            let element = elements.len();
            elements.push(StreamElem { name: name.to_owned(), start, open_end: end + 1 });
            stack.push(element);
            for (ch, char_start, char_end) in chars {
                emit_cell(&mut text, &mut cells, &stack, ch, char_start, char_end);
            }
            stack.pop();
            pos = after_close;
            continue;
        }
        let run = stack.last().is_some_and(|index| elements[*index].name == "w:r");
        match name {
            "w:tab" if run => emit_cell(&mut text, &mut cells, &stack, '\t', start, end + 1),
            "w:br" | "w:cr" => {
                let ch = if name == "w:cr" {
                    '\n'
                } else {
                    match raw.split("w:type=\"").nth(1).and_then(|rest| rest.split('"').next()) {
                        Some("page") => '\u{000C}',
                        Some("column") => '\u{000E}',
                        _ => '\n',
                    }
                };
                emit_cell(&mut text, &mut cells, &stack, ch, start, end + 1);
            }
            "w:noBreakHyphen" if run => emit_cell(&mut text, &mut cells, &stack, '\u{2011}', start, end + 1),
            "w:softHyphen" if run => emit_cell(&mut text, &mut cells, &stack, '\u{00AD}', start, end + 1),
            "w:sym" if run => {
                let ch = raw
                    .split("w:char=\"")
                    .nth(1)
                    .and_then(|rest| rest.split('"').next())
                    .and_then(|hex| u32::from_str_radix(hex.trim(), 16).ok())
                    .and_then(char::from_u32)
                    .filter(|ch| (*ch as u32) >= 0x20);
                if let Some(ch) = ch {
                    emit_cell(&mut text, &mut cells, &stack, ch, start, end + 1);
                }
            }
            "w:fldSimple" => {
                // Простое поле — один объект с кэшированным результатом: внутрь не спускаемся.
                emit_cell(&mut text, &mut cells, &stack, OBJ_CHAR, start, end + 1);
                if self_closing {
                    pos = end + 1;
                    continue;
                }
                let (_, after) = find_matching_close(xml, name, end + 1)
                    .ok_or_else(|| "document.xml: нет конца w:fldSimple".to_owned())?;
                pos = after;
                continue;
            }
            "w:fldChar" => match raw.split("w:fldCharType=\"").nth(1).and_then(|rest| rest.split('"').next()) {
                Some("begin") => fields.push(()),
                Some("end") if fields.pop().is_some() => emit_cell(&mut text, &mut cells, &stack, OBJ_CHAR, start, end + 1),
                _ => {}
            },
            "w:sdt" => emit_cell(&mut text, &mut cells, &stack, OBJ_CHAR, start, end + 1),
            "w:bookmarkStart"
            | "w:bookmarkEnd"
            | "w:commentRangeStart"
            | "w:commentRangeEnd"
            | "w:permStart"
            | "w:permEnd" => {
                emit_cell(&mut text, &mut cells, &stack, OBJ_CHAR, start, end + 1);
            }
            name if is_transparent_inline(name) => {}
            _ => {
                // Неизвестный элемент: читалка его не моделирует — пропускаем поддерево.
                if self_closing {
                    pos = end + 1;
                    continue;
                }
                let (_, after) = find_matching_close(xml, name, end + 1)
                    .ok_or_else(|| format!("document.xml: нет конца <{name}>"))?;
                pos = after;
                continue;
            }
        }
        if self_closing {
            pos = end + 1;
            continue;
        }
        elements.push(StreamElem { name: name.to_owned(), start, open_end: end + 1 });
        stack.push(elements.len() - 1);
        pos = end + 1;
    }
    if !stack.is_empty() {
        return Err("document.xml: в абзаце остались незакрытые элементы".to_owned());
    }
    Ok(Stream { text, cells, elements })
}

/// Прозрачные inline-обёртки: читалка спускается внутрь, собственного текста не добавляют.
fn is_transparent_inline(name: &str) -> bool {
    matches!(
        name,
        "w:r"
            | "w:hyperlink"
            | "w:ins"
            | "w:del"
            | "w:moveTo"
            | "w:moveFrom"
            | "w:smartTag"
            | "w:customXml"
            | "w:dir"
            | "w:bdo"
            | "m:oMathPara"
            | "w:sdtContent"
            | "w:moveFromRangeStart"
            | "w:moveFromRangeEnd"
            | "w:moveToRangeStart"
            | "w:moveToRangeEnd"
    )
}

fn emit_cell(
    text: &mut String,
    cells: &mut Vec<StreamCell>,
    stack: &[usize],
    ch: char,
    xml_start: usize,
    xml_end: usize,
) {
    text.push(ch);
    cells.push(StreamCell { ch, xml_start, xml_end, chain: stack.to_vec() });
}

/// Диапазон элемента stream-разметки: (`start`, `content_end` = начало закрывающего тега, `end`).
fn stream_element_bounds(xml: &str, stream: &Stream, index: usize) -> Option<(usize, usize, usize)> {
    let element = stream.elements.get(index)?;
    let (close_start, end) = find_matching_close(xml, &element.name, element.open_end)?;
    Some((element.start, close_start, end))
}

fn element_name(stream: &Stream, index: usize) -> &str {
    stream.elements.get(index).map(|element| element.name.as_str()).unwrap_or("")
}

fn tail_chars(text: &str, count: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    chars[chars.len().saturating_sub(count)..].iter().collect()
}

fn head_chars(text: &str, count: usize) -> String {
    let mut out: String = text.chars().take(count).collect();
    if text.chars().count() > count {
        out.push('…');
    }
    out
}

/// Найти позицию разреза (число символов до каретки) в собранном тексте.
fn cut_index(stream: &Stream, offset: usize) -> Result<usize, String> {
    let mut consumed = 0usize;
    for (index, cell) in stream.cells.iter().enumerate() {
        if consumed == offset {
            return Ok(index);
        }
        consumed += cell.ch.len_utf8();
        if consumed > offset {
            return Err(format!(
                "позиция {offset} попала внутрь символа текста абзаца (разметка не совпала с моделью)"
            ));
        }
    }
    if consumed == offset {
        Ok(stream.cells.len())
    } else {
        Err(format!("позиция {offset} за концом текста абзаца ({consumed} байт)"))
    }
}

// ── позиционная inline-вставка ──────────────────────────────────────────────

/// Вставить inline-содержимое в позицию каретки абзаца.
///
/// Возвращает новый XML и [`InlinePlacement`]. Если позиция каретки не задана, содержимое
/// дописывается в конец **того же** абзаца с `exact = false` (вызывающий слой обязан показать
/// предупреждение). Если позиция задана, но разметка не разрезается однозначно — `Err`
/// (пакет не меняется).
pub fn insert_inline_at(
    document_xml: &str,
    anchor: &InsertAnchor,
    content: &str,
) -> Result<(String, InlinePlacement), InsertAnchorError> {
    let span = locate_paragraph(document_xml, anchor)?;
    let mut placement = InlinePlacement {
        para_id: span.para_id.clone(),
        ordinal: Some(span.ordinal),
        offset: None,
        exact: false,
        context_before: String::new(),
        context_after: String::new(),
    };
    let Some(offset) = anchor.offset else {
        return Ok((splice(document_xml, span.close_start, content), placement));
    };
    let stream = build_stream(document_xml, &span)?;
    let Some(expected) = anchor.text.as_deref() else {
        return Err("для позиционной вставки нужен текст абзаца из модели (каретка недоступна)".to_owned());
    };
    if stream.text.len() < offset
        || expected.len() < offset
        || stream.text.as_bytes()[..offset] != expected.as_bytes()[..offset]
        || !stream.text.is_char_boundary(offset)
    {
        return Err(
            "позиция каретки не совпала с разметкой абзаца (текст документа изменился) — поставьте каретку заново и повторите вставку"
                .to_owned(),
        );
    }
    let cut = cut_index(&stream, offset)?;
    placement.offset = Some(offset);
    placement.context_before = tail_chars(&expected[..offset], 32);
    placement.context_after = head_chars(&expected[offset..], 24);
    let out = insert_at_cut(document_xml, &span, &stream, cut, content)?;
    placement.exact = true;
    Ok((out, placement))
}

fn splice(xml: &str, at: usize, content: &str) -> String {
    let mut out = String::with_capacity(xml.len() + content.len());
    out.push_str(&xml[..at]);
    out.push_str(content);
    out.push_str(&xml[at..]);
    out
}

/// Открывающий тег элемента из разметки.
fn open_tag_of(xml: &str, start: usize) -> &str {
    let end = find_tag_end(xml, start).unwrap_or(start + 1);
    &xml[start..end + 1]
}

/// `w:rPr` первого прогона в диапазоне `[from, to)` (для дублирования форматирования).
fn run_properties(xml: &str, from: usize, to: usize) -> String {
    let Some((start, open_end)) = find_open_tag(xml, "w:rPr", from) else {
        return String::new();
    };
    if start >= to {
        return String::new();
    }
    match find_matching_close(xml, "w:rPr", open_end) {
        Some((_, end)) => xml[start..end].to_owned(),
        None => String::new(),
    }
}

/// Убедиться, что в открытом теге `w:t` есть `xml:space="preserve"` (иначе краевые пробелы
/// потеряются после разреза).
fn ensure_preserve(open_tag: &str) -> String {
    if open_tag.contains("xml:space=\"preserve\"") {
        return open_tag.to_owned();
    }
    let trimmed = open_tag.trim_end();
    let body = trimmed.strip_suffix('>').unwrap_or(trimmed);
    format!("{body} xml:space=\"preserve\">")
}

/// Вставка в найденную позицию разреза.
fn insert_at_cut(
    xml: &str,
    span: &ParagraphSpan,
    stream: &Stream,
    cut: usize,
    content: &str,
) -> Result<String, InsertAnchorError> {
    if cut == 0 {
        let at = if xml[span.open_end..].trim_start().starts_with("<w:pPr") {
            let tag_end = find_tag_end(xml, span.open_end).ok_or_else(|| "document.xml: битый w:pPr".to_owned())?;
            let (_, after) = find_matching_close(xml, "w:pPr", tag_end + 1)
                .ok_or_else(|| "document.xml: нет конца w:pPr".to_owned())?;
            after
        } else {
            span.open_end
        };
        return Ok(splice(xml, at, content));
    }
    if cut >= stream.cells.len() {
        return Ok(splice(xml, span.close_start, content));
    }
    let prev = &stream.cells[cut - 1];
    let next = &stream.cells[cut];
    if prev.xml_end > next.xml_start {
        return Err("позиция каретки не совпала с границей символов разметки".to_owned());
    }
    // 1) Каретка на границе элементов: вставляем после ближайшего предка предыдущего символа,
    // который заканчивается до следующего символа (ссылка/прогон/текст не разрезаются зря).
    for (depth, index) in prev.chain.iter().enumerate() {
        let Some((_, _, element_end)) = stream_element_bounds(xml, stream, *index) else {
            continue;
        };
        if element_end > next.xml_start {
            continue;
        }
        if depth == 0 {
            return Ok(splice(xml, element_end, content));
        }
        return split_chain(xml, stream, &prev.chain[..depth], element_end, content);
    }
    // 2) Каретка внутри элемента предыдущего символа: разрез цепочки.
    for (depth, index) in prev.chain.iter().enumerate().rev() {
        let Some((element_start, content_end, _)) = stream_element_bounds(xml, stream, *index) else {
            continue;
        };
        if !(element_start < prev.xml_end && prev.xml_end <= content_end) {
            continue;
        }
        return split_chain(xml, stream, &prev.chain[..=depth], prev.xml_end, content);
    }
    // 3) Символ вне прогона (маркер/объект на уровне абзаца): общий контейнер.
    let mut common = 0usize;
    while common < prev.chain.len() && common < next.chain.len() && prev.chain[common] == next.chain[common] {
        common += 1;
    }
    let container =
        if common == 0 { "w:p".to_owned() } else { element_name(stream, prev.chain[common - 1]).to_owned() };
    match container.as_str() {
        "w:p" | "w:sdtContent" => Ok(splice(xml, prev.xml_end, content)),
        other => Err(format!(
            "каретка на границе элементов <{other}> — вставка формулы сюда не поддерживается; поставьте каретку в обычный текст"
        )),
    }
}

/// Закрыть цепочку элементов (от внутреннего к внешнему) и открыть её заново вокруг `content`.
///
/// Цепочка — путь от прямого ребёнка абзаца до элемента, внутри которого лежит `cut_raw`:
/// `[w:hyperlink →] w:r [→ w:t]`. Форматирование прогона (`w:rPr`) дублируется, пробельные края
/// текста сохраняются (`xml:space="preserve"`), исходные закрывающие теги остаются в хвосте.
fn split_chain(
    xml: &str,
    stream: &Stream,
    chain: &[usize],
    cut_raw: usize,
    content: &str,
) -> Result<String, InsertAnchorError> {
    if chain.is_empty() {
        return Ok(splice(xml, cut_raw, content));
    }
    let names: Vec<&str> = chain.iter().map(|index| element_name(stream, *index)).collect();
    let shape_ok = matches!(
        names.as_slice(),
        ["w:r"]
            | ["w:r", "w:t"]
            | ["w:r", "w:delText"]
            | ["w:hyperlink"]
            | ["w:hyperlink", "w:r"]
            | ["w:hyperlink", "w:r", "w:t"]
            | ["w:hyperlink", "w:r", "w:delText"]
    );
    if !shape_ok {
        return Err(format!(
            "каретка внутри <{}> — вставка формулы сюда не поддерживается; поставьте каретку в обычный текст абзаца",
            names.first().copied().unwrap_or("?")
        ));
    }
    let run_index = chain.iter().position(|index| element_name(stream, *index) == "w:r");
    if let Some(run) = run_index {
        let (run_start, _, run_end) =
            stream_element_bounds(xml, stream, chain[run]).ok_or_else(|| "document.xml: границы w:r".to_owned())?;
        let run_raw = &xml[run_start..run_end];
        if run_raw.contains("w:fldChar") || run_raw.contains("w:instrText") {
            return Err("каретка внутри поля Word — вставка формулы туда не поддерживается".to_owned());
        }
    }
    let t_index = chain.iter().position(|index| matches!(element_name(stream, *index), "w:t" | "w:delText"));
    let mut left = String::new();
    match t_index {
        Some(t) => {
            let index = chain[t];
            let t_start = stream.elements[index].start;
            let t_open_end = stream.elements[index].open_end;
            left.push_str(&xml[..t_start]);
            left.push_str(&ensure_preserve(open_tag_of(xml, t_start)));
            left.push_str(&xml[t_open_end..cut_raw]);
        }
        None => left.push_str(&xml[..cut_raw]),
    }
    let mut closes = String::new();
    for index in chain.iter().rev() {
        closes.push_str("</");
        closes.push_str(element_name(stream, *index));
        closes.push('>');
    }
    let rpr = run_index.map(|run| run_properties(xml, stream.elements[chain[run]].start, cut_raw)).unwrap_or_default();
    let mut opens = String::new();
    for (position, index) in chain.iter().enumerate() {
        let name = element_name(stream, *index);
        let open = open_tag_of(xml, stream.elements[*index].start);
        if matches!(name, "w:t" | "w:delText") {
            opens.push_str(&ensure_preserve(open));
        } else {
            opens.push_str(open);
            if Some(position) == run_index {
                opens.push_str(&rpr);
            }
        }
    }
    let (_, _, outer_end) = stream_element_bounds(xml, stream, chain[0])
        .ok_or_else(|| "document.xml: границы внешнего элемента".to_owned())?;
    let mut out = String::with_capacity(xml.len() + content.len() + closes.len() + opens.len());
    out.push_str(&left);
    out.push_str(&closes);
    out.push_str(content);
    out.push_str(&opens);
    out.push_str(&xml[cut_raw..outer_end]);
    out.push_str(&xml[outer_end..]);
    Ok(out)
}

// ── display-вставка ─────────────────────────────────────────────────────────

/// Display-вставка: фрагмент блока после top-level элемента, содержащего абзац-якорь.
///
/// Возвращает новый XML и предупреждение (якорь в таблице — формула вставлена после таблицы).
pub fn insert_block_after(
    xml: &str,
    anchor: &InsertAnchor,
    fragment: &str,
) -> Result<(String, Option<String>), InsertAnchorError> {
    if anchor.is_empty() {
        // Прежнее поведение P3: display-вставка без якоря — в конец тела перед `w:sectPr`.
        let body_close = xml.rfind("</w:body>").unwrap_or(xml.len());
        let body = &xml[..body_close];
        let insert_at = body.rfind("<w:sectPr").unwrap_or(body_close);
        return Ok((splice(xml, insert_at, fragment), None));
    }
    if let Some(legacy) = anchor.legacy_block {
        // Совместимость: старый индекс top-level блока тела (display-вставки P3/P6).
        return Ok((insert_after_legacy_block(xml, fragment, legacy), None));
    }
    let span = locate_paragraph(xml, anchor)?;
    let (_, top_end, in_table) = top_level_element_for(xml, &span)?;
    let warning = in_table.then(|| "якорь display-формулы внутри таблицы — формула вставлена после таблицы".to_owned());
    Ok((splice(xml, top_end, fragment), warning))
}

/// Старая семантика display-вставки: после N-го top-level блока тела (совместимость).
fn insert_after_legacy_block(xml: &str, fragment: &str, index: usize) -> String {
    let Some((_, body_open_end)) = find_open_tag(xml, "w:body", 0) else {
        return format!("{xml}{fragment}");
    };
    let body_close = xml.rfind("</w:body>").unwrap_or(xml.len());
    let mut pos = body_open_end;
    let mut block = 0usize;
    while pos < body_close {
        let Some(rel) = xml[pos..body_close].find('<') else {
            break;
        };
        let start = pos + rel;
        if let Some(after) = skip_special(xml, start) {
            pos = after;
            continue;
        }
        let end = match find_tag_end(xml, start) {
            Some(end) => end,
            None => break,
        };
        let raw = &xml[start + 1..end];
        if raw.starts_with('/') {
            pos = end + 1;
            continue;
        }
        let name = tag_name(raw);
        let after = if is_self_closing(raw) {
            end + 1
        } else {
            match find_matching_close(xml, name, end + 1) {
                Some((_, after)) => after,
                None => break,
            }
        };
        if block == index {
            return splice(xml, after, fragment);
        }
        block += 1;
        pos = after;
    }
    // Прежнее поведение: дописать в конец тела перед w:sectPr.
    let body = &xml[..body_close];
    let insert_at = body.rfind("<w:sectPr").unwrap_or(body_close);
    splice(xml, insert_at, fragment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(xml: &str) -> String {
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
             <w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
             xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\" \
             xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\"><w:body>{xml}</w:body></w:document>"
        )
    }

    fn target_paragraph() -> String {
        "<w:p w14:paraId=\"200C20D9\"><w:pPr><w:ind w:firstLine=\"0\"/></w:pPr>\
         <w:r><w:rPr><w:i/></w:rPr><w:t xml:space=\"preserve\">Дифференциальная составляющая </w:t></w:r>\
         <w:r><w:t xml:space=\"preserve\"> пропорциональна удалению цели от опорной дальности , и именно она</w:t></w:r></w:p>"
            .to_owned()
    }

    const TARGET_TEXT: &str =
        "Дифференциальная составляющая  пропорциональна удалению цели от опорной дальности , и именно она";

    fn target_anchor() -> (usize, InsertAnchor) {
        let offset = TARGET_TEXT.find("опорной дальности").unwrap() + "опорной дальности".len();
        (offset, InsertAnchor::paragraph("200C20D9", Some(offset), Some(TARGET_TEXT.to_owned())))
    }

    #[test]
    fn scans_paragraphs_in_wordcraft_order() {
        let xml = body(
            "<w:p w14:paraId=\"AAAA0001\"><w:r><w:t>Первый</w:t></w:r></w:p>\
             <w:tbl><w:tr><w:tc><w:p w14:paraId=\"AAAA0002\"><w:r><w:t>В таблице</w:t></w:r></w:p></w:tc></w:tr></w:tbl>\
             <w:sdt><w:sdtPr/><w:sdtContent><w:p w14:paraId=\"AAAA0003\"><w:r><w:t>В контроле</w:t></w:r></w:p></w:sdtContent></w:sdt>",
        );
        let spans = body_paragraph_spans(&xml).unwrap();
        assert_eq!(spans.len(), 3, "{spans:?}");
        assert_eq!(spans[0].para_id.as_deref(), Some("AAAA0001"));
        assert_eq!(spans[1].para_id.as_deref(), Some("AAAA0002"));
        assert!(spans[1].in_table, "абзац таблицы помечен");
        assert_eq!(spans[2].para_id.as_deref(), Some("AAAA0003"));
    }

    #[test]
    fn skips_deleted_blocks_and_textboxes() {
        let xml = body(
            "<w:p w14:paraId=\"AAAA0001\"><w:r><w:t>Живой</w:t></w:r></w:p>\
             <w:del><w:p w14:paraId=\"AAAA0002\"><w:r><w:delText>Удалён</w:delText></w:r></w:p></w:del>\
             <w:p w14:paraId=\"AAAA0003\"><w:r><w:t>С полем</w:t></w:r><w:r><w:drawing><w:txbxContent><w:p><w:r><w:t>Текст поля</w:t></w:r></w:p></w:txbxContent></w:drawing></w:r></w:p>",
        );
        let spans = body_paragraph_spans(&xml).unwrap();
        assert_eq!(spans.len(), 2, "{spans:?}");
        assert_eq!(spans[0].para_id.as_deref(), Some("AAAA0001"));
        assert_eq!(spans[1].para_id.as_deref(), Some("AAAA0003"));
    }

    #[test]
    fn inserts_inside_run_between_chars() {
        let xml = body(&target_paragraph());
        let (offset, anchor) = target_anchor();
        let (out, placement) = insert_inline_at(&xml, &anchor, "<w:sdt>FORMULA</w:sdt>").unwrap();
        assert!(placement.exact);
        assert_eq!(placement.offset, Some(offset));
        assert_eq!(placement.para_id.as_deref(), Some("200C20D9"));
        assert_eq!(out.matches("<w:r>").count(), xml.matches("<w:r>").count() + 1, "run разорван на два: {out}");
        let left = out.find("дальности").unwrap();
        let formula = out.find("<w:sdt>FORMULA</w:sdt>").unwrap();
        assert!(left < formula, "формула после «дальности»");
        assert!(out[formula..].contains(" , и именно"), "пунктуация осталась после формулы: {out}");
        assert_eq!(out.matches(" пропорциональна удалению").count(), 1, "текст не потерян/не продублирован");
        assert_eq!(out.matches("<w:rPr><w:i/></w:rPr>").count(), 1, "чужое форматирование не дублируется");
    }

    #[test]
    fn inserts_at_paragraph_start_and_end() {
        let xml = body(&target_paragraph());
        let start = InsertAnchor::paragraph("200C20D9", Some(0), Some(TARGET_TEXT.to_owned()));
        let (out, _) = insert_inline_at(&xml, &start, "<w:sdt>F</w:sdt>").unwrap();
        assert!(!out.contains("F</w:sdt></w:pPr>"), "вставка не внутрь w:pPr");
        assert!(out.find("<w:sdt>F</w:sdt>").unwrap() > out.find("</w:pPr>").unwrap());
        let end = InsertAnchor::paragraph("200C20D9", Some(TARGET_TEXT.len()), Some(TARGET_TEXT.to_owned()));
        let (out, placement) = insert_inline_at(&xml, &end, "<w:sdt>F</w:sdt>").unwrap();
        assert!(placement.exact);
        assert!(out.find("<w:sdt>F</w:sdt>").unwrap() < out.find("</w:p>").unwrap());
    }

    #[test]
    fn unknown_anchor_errors_and_mismatched_text_errors() {
        let xml = body(&target_paragraph());
        let missing = InsertAnchor::paragraph("NOPE0000", Some(0), None);
        assert!(insert_inline_at(&xml, &missing, "x").is_err());
        let wrong = InsertAnchor::paragraph("200C20D9", Some(10), Some("совсем другой текст абзаца".to_owned()));
        assert!(insert_inline_at(&xml, &wrong, "x").is_err());
        let overflow = InsertAnchor::paragraph("200C20D9", Some(10_000), Some("т".repeat(20_000)));
        assert!(insert_inline_at(&xml, &overflow, "x").is_err());
        let no_text = InsertAnchor::paragraph("200C20D9", Some(5), None);
        assert!(insert_inline_at(&xml, &no_text, "x").is_err());
    }

    #[test]
    fn no_offset_appends_to_end_of_same_paragraph() {
        let xml = body(
            "<w:p w14:paraId=\"AAAA0001\"><w:r><w:t>Первый</w:t></w:r></w:p>\
             <w:p w14:paraId=\"AAAA0002\"><w:r><w:t>Второй</w:t></w:r></w:p>",
        );
        let anchor = InsertAnchor::paragraph("AAAA0001", None, None);
        let (out, placement) = insert_inline_at(&xml, &anchor, "<w:sdt>F</w:sdt>").unwrap();
        assert!(!placement.exact);
        assert!(out.find("<w:sdt>F</w:sdt>").unwrap() < out.find("Второй").unwrap(), "осталось в том же абзаце");
    }

    #[test]
    fn first_run_before_any_text_and_between_runs() {
        // Вставка между двумя прогонами (граница прогонов) и в самое начало абзаца.
        let xml = body(
            "<w:p w14:paraId=\"AAAA0001\"><w:r><w:t>Первый </w:t></w:r><w:r><w:rPr><w:b/></w:rPr><w:t>второй</w:t></w:r></w:p>",
        );
        let text = "Первый второй";
        let offset = "Первый ".len();
        let anchor = InsertAnchor::paragraph("AAAA0001", Some(offset), Some(text.to_owned()));
        let (out, placement) = insert_inline_at(&xml, &anchor, "<w:sdt>F</w:sdt>").unwrap();
        assert!(placement.exact);
        let formula = out.find("<w:sdt>F</w:sdt>").unwrap();
        assert!(formula > out.find("Первый ").unwrap());
        assert!(formula < out.find("второй").unwrap());
        let offset = "Первый в".len();
        let anchor = InsertAnchor::paragraph("AAAA0001", Some(offset), Some(text.to_owned()));
        let (out, placement) = insert_inline_at(&xml, &anchor, "<w:sdt>G</w:sdt>").unwrap();
        assert!(placement.exact);
        assert!(out.contains("<w:b/>"), "жирное форматирование сохранено: {out}");
        // Текст модели не изменился (кроме вставленного объекта).
        let stream =
            build_stream(&body(&target_paragraph()), &body_paragraph_spans(&body(&target_paragraph())).unwrap()[0])
                .unwrap();
        assert_eq!(stream.text, TARGET_TEXT);
    }

    #[test]
    fn display_insert_goes_after_containing_block() {
        let xml = body(
            "<w:p w14:paraId=\"AAAA0001\"><w:r><w:t>До</w:t></w:r></w:p>\
             <w:tbl><w:tr><w:tc><w:p w14:paraId=\"AAAA0002\"><w:r><w:t>В таблице</w:t></w:r></w:p></w:tc></w:tr></w:tbl>\
             <w:p w14:paraId=\"AAAA0003\"><w:r><w:t>После</w:t></w:r></w:p>",
        );
        let anchor = InsertAnchor::paragraph("AAAA0001", None, None);
        let (out, warning) = insert_block_after(&xml, &anchor, "<w:p><w:r><w:t>ФОРМУЛА</w:t></w:r></w:p>").unwrap();
        assert!(warning.is_none());
        assert!(out.find("ФОРМУЛА").unwrap() > out.find("До").unwrap());
        assert!(
            out.find("ФОРМУЛА").unwrap() < out.find("В таблице").unwrap(),
            "вставка после своего блока, а не в конце"
        );
        let in_table = InsertAnchor::paragraph("AAAA0002", None, None);
        let (out, warning) = insert_block_after(&xml, &in_table, "<w:p><w:r><w:t>ФОРМУЛА2</w:t></w:r></w:p>").unwrap();
        assert!(warning.is_some(), "предупреждение про таблицу");
        assert!(out.find("ФОРМУЛА2").unwrap() > out.find("</w:tbl>").unwrap());
        assert!(out.find("ФОРМУЛА2").unwrap() < out.find("После").unwrap());
    }

    #[test]
    fn legacy_block_anchor_keeps_old_semantics() {
        let xml = body("<w:p><w:r><w:t>A</w:t></w:r></w:p><w:p><w:r><w:t>B</w:t></w:r></w:p>");
        let (out, warning) =
            insert_block_after(&xml, &InsertAnchor::legacy_block(0), "<w:p><w:r><w:t>FORMULA</w:t></w:r></w:p>")
                .unwrap();
        assert!(warning.is_none());
        assert!(out.find(">A<").unwrap() < out.find("FORMULA").unwrap());
        assert!(out.find("FORMULA").unwrap() < out.find(">B<").unwrap());
    }

    #[test]
    fn hyperlink_is_split_and_formula_stays_outside() {
        let text = "См. ссылку и дальше";
        let xml = body(
            "<w:p w14:paraId=\"AAAA0001\"><w:r><w:t xml:space=\"preserve\">См. </w:t></w:r>\
             <w:hyperlink r:id=\"rId1\"><w:r><w:t>ссылку и дальше</w:t></w:r></w:hyperlink></w:p>",
        );
        let offset = text.find("и дальше").unwrap() + "и ".len();
        let anchor = InsertAnchor::paragraph("AAAA0001", Some(offset), Some(text.to_owned()));
        let (out, placement) = insert_inline_at(&xml, &anchor, "<w:sdt>F</w:sdt>").unwrap();
        assert!(placement.exact);
        assert_eq!(out.matches("<w:hyperlink r:id=\"rId1\">").count(), 2, "ссылка разорвана: {out}");
        assert_eq!(out.matches("</w:hyperlink>").count(), 2);
        let formula = out.find("<w:sdt>F</w:sdt>").unwrap();
        assert!(out[..formula].ends_with("</w:hyperlink>"), "формула вне ссылки: {out}");
    }

    #[test]
    fn tracked_change_layer_is_reported_not_broken() {
        let xml = body(
            "<w:p w14:paraId=\"AAAA0001\"><w:ins w:id=\"1\"><w:r><w:t>Вставленный текст</w:t></w:r></w:ins></w:p>",
        );
        let text = "Вставленный текст";
        let anchor = InsertAnchor::paragraph("AAAA0001", Some(6), Some(text.to_owned()));
        let error = insert_inline_at(&xml, &anchor, "<w:sdt>F</w:sdt>").unwrap_err();
        assert!(error.contains("w:ins"), "понятная ошибка: {error}");
    }
}
