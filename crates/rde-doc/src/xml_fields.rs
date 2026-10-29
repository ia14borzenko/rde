//! Текст WordprocessingML с учётом полей (`w:fldChar`/`w:instrText`) — для импорта и миграции
//! формул (ADR-0019).
//!
//! Зачем отдельный разбор: реальные MathType-документы с нумерацией формул справа хранят номер не
//! обычным `w:t`, а кэшированным результатом полей Word:
//!
//! ```text
//! <w:r><w:fldChar w:fldCharType="begin"/></w:r>
//! <w:r><w:instrText> MACROBUTTON MTPlaceRef \* MERGEFORMAT </w:instrText></w:r>
//! <w:r><w:fldChar w:fldCharType="begin"/></w:r>
//! <w:r><w:instrText> SEQ MTEqn \h \* MERGEFORMAT </w:instrText></w:r>
//! <w:r><w:fldChar w:fldCharType="end"/></w:r>
//! <w:r><w:instrText>(</w:instrText></w:r>
//! <w:r><w:fldChar w:fldCharType="begin"/></w:r>
//! <w:r><w:instrText> SEQ MTEqn \c \* Arabic \* MERGEFORMAT </w:instrText></w:r>
//! <w:r><w:fldChar w:fldCharType="separate"/></w:r>
//! <w:r><w:instrText>3</w:instrText></w:r>   <!-- кэш результата номера -->
//! <w:r><w:fldChar w:fldCharType="end"/></w:r>
//! <w:r><w:instrText>)</w:instrText></w:r>
//! <w:r><w:fldChar w:fldCharType="end"/></w:r>
//! ```
//!
//! Без разбора полей номер `(3)` невидим: в `w:t` его нет, а наивный сбор всего текста подряд
//! выдаёт код поля (`11\* MERGEFORMAT ()`), как это делает LibreOffice. Помощники модуля:
//! [`visible_text`] (видимый текст с учётом полей), [`literal_text`] (только `w:t` — пунктуация
//! обычных run'ов), [`mteqn_cached_number`] (кэш номера `SEQ MTEqn \c`) и
//! [`paragraph_bounds`] (настоящие границы абзаца формульного абзаца).

/// Текст фрагмента с учётом полей Word: результат поля (после `separate`) виден, код поля — нет.
/// `w:tab`/`w:ptab` дают `\t`; содержимое `w:pPr` пропускается. Второй элемент результата —
/// признак нетекстового содержимого (`w:drawing`/`w:object`/`w:pict`/`w:sym`/`w:fldSimple`):
/// для классификации «абзац только с формулой» это делает абзац смешанным.
pub(crate) fn visible_text(xml: &str) -> (String, bool) {
    let mut out = String::new();
    let mut non_text = false;
    let mut fields: Vec<FieldState> = Vec::new();
    let mut ppr_depth = 0usize;
    let mut pos = 0usize;
    while pos < xml.len() {
        let Some(rel) = xml[pos..].find('<') else { break };
        let start = pos + rel;
        let Some(rel_end) = xml[start..].find('>') else { break };
        let end = start + rel_end;
        let tag = &xml[start + 1..end];
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');
        let declaration = tag.starts_with('?') || tag.starts_with('!');
        let name = tag.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("").to_owned();
        if declaration {
            pos = end + 1;
            continue;
        }
        if closing {
            if ppr_depth > 0 {
                ppr_depth = ppr_depth.saturating_sub(1);
            } else if name == "w:fldSimple" {
                fields.pop();
            }
            // Текст `w:t`/`w:instrText` читается на открывающем теге; закрытие — просто конец.
            pos = end + 1;
            continue;
        }
        if ppr_depth > 0 {
            if !self_closing {
                ppr_depth += 1;
            }
            pos = end + 1;
            continue;
        }
        match name.as_str() {
            "w:pPr" => {
                if !self_closing {
                    ppr_depth = 1;
                }
                pos = end + 1;
            }
            "w:fldChar" => {
                let kind = attribute(tag, "w:fldCharType").unwrap_or_default();
                match kind.as_str() {
                    "begin" => fields.push(FieldState::default()),
                    "separate" => {
                        if let Some(field) = fields.last_mut() {
                            field.in_result = true;
                        }
                    }
                    "end" => {
                        fields.pop();
                    }
                    _ => {}
                }
                pos = end + 1;
            }
            "w:instrText" => {
                let Some((content, next)) = element_text(xml, end, "w:instrText") else { break };
                // Внутри результата поля `w:instrText` — кэш значения (реальный MathType-документ),
                // в коде поля — инструкция (невидима).
                if fields.last().is_some_and(|field| field.in_result) {
                    out.push_str(&decode_entities(&content));
                }
                pos = next;
            }
            "w:t" => {
                let Some((content, next)) = element_text(xml, end, "w:t") else { break };
                let visible = fields.is_empty() || fields.last().is_some_and(|field| field.in_result);
                if visible {
                    out.push_str(&decode_entities(&content));
                }
                pos = next;
            }
            "w:tab" | "w:ptab" => {
                let visible = fields.is_empty() || fields.last().is_some_and(|field| field.in_result);
                if visible {
                    out.push('\t');
                }
                pos = end + 1;
            }
            // `w:fldSimple` — поле Word в форме «инструкция в атрибуте, результат — содержимое»;
            // результат видим (в т.ч. когда кэш номера лежит в `w:instrText` внутри, как у MathType).
            "w:fldSimple" => {
                let instruction = attribute(tag, "w:instr").unwrap_or_default();
                if !self_closing {
                    fields.push(FieldState { in_result: true, code: instruction, result: String::new() });
                }
                pos = end + 1;
            }
            "w:drawing" | "w:object" | "w:pict" | "w:sym" => {
                if !fields.last().is_some_and(|field| field.in_result) {
                    non_text = true;
                }
                pos = end + 1;
            }
            _ => pos = end + 1,
        }
    }
    (out, non_text)
}

/// Только литеральный текст `w:t` (без полей и тегов `<w:tbl>`): пунктуация обычных run'ов.
pub(crate) fn literal_text(xml: &str) -> String {
    let mut out = String::new();
    let mut ppr_depth = 0usize;
    let mut pos = 0usize;
    while pos < xml.len() {
        let Some(rel) = xml[pos..].find('<') else { break };
        let start = pos + rel;
        let Some(rel_end) = xml[start..].find('>') else { break };
        let end = start + rel_end;
        let tag = &xml[start + 1..end];
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');
        let name = tag.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("").to_owned();
        if closing {
            if ppr_depth > 0 {
                ppr_depth = ppr_depth.saturating_sub(1);
            }
            pos = end + 1;
            continue;
        }
        if ppr_depth > 0 {
            if !self_closing {
                ppr_depth += 1;
            }
            pos = end + 1;
            continue;
        }
        if name == "w:pPr" {
            if !self_closing {
                ppr_depth = 1;
            }
            pos = end + 1;
            continue;
        }
        if name == "w:t" {
            let Some((content, next)) = element_text(xml, end, "w:t") else { break };
            out.push_str(&decode_entities(&content));
            pos = next;
            continue;
        }
        pos = end + 1;
    }
    out
}

/// Кэш номера формулы MathType: результат поля `SEQ MTEqn \c` (текст без скобок). Так Word и
/// MathType хранят номер `(N)` справа от display-формулы; без полей — `None`.
pub(crate) fn mteqn_cached_number(xml: &str) -> Option<String> {
    let mut fields: Vec<FieldState> = Vec::new();
    let mut pos = 0usize;
    let mut found: Option<String> = None;
    while pos < xml.len() {
        let Some(rel) = xml[pos..].find('<') else { break };
        let start = pos + rel;
        let Some(rel_end) = xml[start..].find('>') else { break };
        let end = start + rel_end;
        let tag = &xml[start + 1..end];
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');
        let name = tag.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("").to_owned();
        if closing {
            pos = end + 1;
            continue;
        }
        match name.as_str() {
            "w:fldChar" => {
                let kind = attribute(tag, "w:fldCharType").unwrap_or_default();
                match kind.as_str() {
                    "begin" => fields.push(FieldState::default()),
                    "separate" => {
                        if let Some(field) = fields.last_mut() {
                            field.in_result = true;
                        }
                    }
                    "end" => {
                        if let Some(field) = fields.pop()
                            && found.is_none()
                            && field.result_number()
                        {
                            found = Some(field.result.trim().to_owned());
                        }
                    }
                    _ => {}
                }
                pos = end + 1;
            }
            "w:instrText" => {
                let Some((content, next)) = element_text(xml, end, "w:instrText") else { break };
                if let Some(field) = fields.last_mut() {
                    if field.in_result {
                        field.result.push_str(&decode_entities(&content));
                    } else {
                        field.code.push_str(&decode_entities(&content));
                    }
                }
                pos = next;
            }
            "w:t" => {
                let Some((content, next)) = element_text(xml, end, "w:t") else { break };
                if let Some(field) = fields.last_mut().filter(|field| field.in_result) {
                    field.result.push_str(&decode_entities(&content));
                }
                pos = next;
            }
            // `w:fldSimple` — результат целиком внутри элемента (WordCraft/Word пишут так часть
            // полей MathType); инструкция — в атрибуте `w:instr`.
            "w:fldSimple" => {
                let instruction = attribute(tag, "w:instr").unwrap_or_default();
                let simple = !self_closing;
                if simple
                    && instruction.contains("SEQ MTEqn")
                    && instruction.contains("\\c")
                    && found.is_none()
                    && let Some(close_rel) = xml[end..].find("</w:fldSimple>")
                {
                    let content = &xml[end + 1..end + close_rel];
                    let text = collect_plain_text(content);
                    let text = text.trim();
                    if !text.is_empty() {
                        found = Some(text.to_owned());
                    }
                }
                if simple
                    && let Some(close_rel) = xml[end..].find("</w:fldSimple>")
                {
                    pos = end + close_rel + "</w:fldSimple>".len();
                    continue;
                }
                pos = end + 1;
            }
            _ => {
                if !self_closing {
                    // прочие элементы полей не несут; маркеры и текст уже обработаны выше.
                }
                pos = end + 1;
            }
        }
    }
    found.filter(|number| !number.is_empty())
}

/// Кроме формулы в абзаце допустима только пунктуация/пробелы?
pub(crate) fn is_punctuation_only(text: &str) -> bool {
    text.chars().all(|ch| {
        ch.is_whitespace()
            || matches!(
                ch,
                ',' | '.' | ';' | ':' | '!' | '?' | '-' | '–' | '—' | '―' | '…' | '«' | '»' | '"' | '\'' | '(' | ')' | '[' | ']' | '/' | '\\' | '*' | '|' | '№'
            )
    })
}

/// Есть ли в тексте осмысленный текст (буквы): цифры/пунктуация/табы — «не текст». Нужен для
/// классификации display/inline формул при импорте (номер `(3)`/поле — не текст, слова — текст).
pub(crate) fn has_meaningful_text(text: &str) -> bool {
    text.chars().any(char::is_alphabetic)
}

/// Убрать из текста токены номеров формул `(...)`: `(3)`, `(2.1)`, `(А.1)` — чтобы номер не
/// считался «содержательным текстом» при классификации абзаца.
pub(crate) fn strip_number_tokens(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        out.push_str(&rest[..open]);
        let after = &rest[open..];
        let Some(close) = after.find(')') else {
            out.push_str(after);
            return out;
        };
        let inner = &after[1..close];
        let valid = !inner.is_empty()
            && inner.chars().next().is_some_and(|first| first.is_ascii_digit() || first.is_uppercase())
            && inner.chars().all(|ch| ch.is_ascii_digit() || ch == '.' || ch.is_uppercase())
            && !inner.starts_with('.')
            && !inner.ends_with('.')
            && !inner.contains("..");
        if valid {
            // Токен номера вырезан целиком.
            rest = &after[close + 1..];
        } else {
            out.push('(');
            rest = &after[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Границы абзаца `w:p`, содержащего позицию `pos`: `(start, end)` включая открывающий и
/// закрывающий теги. `None` — позиция вне абзаца (например, в `w:body`-обёртках).
pub(crate) fn paragraph_bounds(xml: &str, pos: usize) -> Option<(usize, usize)> {
    let before = xml.get(..pos)?;
    let start = rfind_element(before, "<w:p")?;
    let mut depth = 0usize;
    let mut at = start;
    while at < xml.len() {
        let rel = xml[at..].find('<')?;
        let tag_start = at + rel;
        let rel_end = xml[tag_start..].find('>')?;
        let tag_end = tag_start + rel_end;
        let tag = &xml[tag_start + 1..tag_end];
        let closing = tag.starts_with('/');
        let self_closing = tag.ends_with('/');
        let name = tag.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("");
        if name == "w:p" {
            if closing {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some((start, tag_end + 1));
                }
            } else if self_closing {
                if depth == 0 {
                    return Some((start, tag_end + 1));
                }
            } else {
                depth += 1;
            }
        }
        at = tag_end + 1;
    }
    None
}

/// Найти последний открывающий тег `prefix` (`<w:p`, `<w:tbl`…) до конца `text`: после префикса
/// должен идти пробел, `>` или `/` — чтобы `<w:p` не совпал с `<w:pPr>`, а `<w:tbl` — с `<w:tblPr>`.
fn rfind_element(text: &str, prefix: &str) -> Option<usize> {
    let mut from = text.len();
    while let Some(relative) = text[..from].rfind(prefix) {
        let after = text.as_bytes().get(relative + prefix.len()).copied();
        if matches!(after, Some(b' ' | b'>' | b'/')) {
            return Some(relative);
        }
        from = relative;
    }
    None
}

/// Содержимое элемента `tag`, открытый тег которого закончился на `open_end`; `(content, next)`,
/// где `next` — позиция сразу после закрывающего тега (или после `/>`).
fn element_text(xml: &str, open_end: usize, tag: &str) -> Option<(String, usize)> {
    if xml.as_bytes().get(open_end.wrapping_sub(1)) == Some(&b'/') {
        return Some((String::new(), open_end + 1));
    }
    let close = format!("</{tag}>");
    let close_rel = xml[open_end..].find(&close)?;
    let content_start = open_end + 1;
    let content_end = open_end + close_rel;
    Some((xml[content_start..content_end].to_owned(), content_end + close.len()))
}

/// Текст содержимого элемента: все `w:t`/`w:instrText` по порядку (результат `w:fldSimple`).
fn collect_plain_text(xml: &str) -> String {
    let mut out = String::new();
    let mut pos = 0usize;
    while pos < xml.len() {
        let Some(rel) = xml[pos..].find('<') else { break };
        let start = pos + rel;
        let Some(rel_end) = xml[start..].find('>') else { break };
        let end = start + rel_end;
        let tag = &xml[start + 1..end];
        let name = tag.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("").to_owned();
        if (name == "w:t" || name == "w:instrText")
            && let Some((content, next)) = element_text(xml, end, &name)
        {
            out.push_str(&decode_entities(&content));
            pos = next;
            continue;
        }
        pos = end + 1;
    }
    out
}

/// Значение атрибута `name` в тексте открывающего тега (атрибуты в кавычках).
fn attribute(tag: &str, name: &str) -> Option<String> {
    let marker = format!("{name}=");
    let index = tag.find(&marker)? + marker.len();
    let rest = tag[index..].trim_start();
    let quote = rest.chars().next()?;
    if quote == '"' || quote == '\'' {
        let rest = &rest[1..];
        let end = rest.find(quote)?;
        Some(rest[..end].to_owned())
    } else {
        Some(rest.split([' ', '/', '>']).next().unwrap_or("").to_owned())
    }
}

fn decode_entities(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

#[derive(Default)]
struct FieldState {
    in_result: bool,
    code: String,
    result: String,
}

impl FieldState {
    /// Поле — номер формулы MathType (`SEQ MTEqn \c`): результат поля и есть номер.
    fn result_number(&self) -> bool {
        let code = self.code.trim();
        code.contains("SEQ MTEqn") && code.contains("\\c") && !self.result.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Параметр номера MathType как в реальном документе (MACROBUTTON MTPlaceRef + SEQ MTEqn \h/\c).
    fn math_type_number(number: &str) -> String {
        format!(
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
        )
    }

    #[test]
    fn visible_text_skips_field_codes_and_reads_cached_results() {
        let xml = format!("<w:r><w:t>.</w:t></w:r>{}", math_type_number("3"));
        let (text, non_text) = visible_text(&xml);
        assert_eq!(text, ".\t3", "видимый текст: пунктуация, табы и кэш номера: {text:?}");
        assert!(!non_text);
        assert_eq!(mteqn_cached_number(&xml).as_deref(), Some("3"));
        assert_eq!(literal_text(&xml), ".", "литеральный текст — только w:t");
    }

    #[test]
    fn plain_field_result_in_text_run_is_visible() {
        let xml = "<w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>\
                   <w:r><w:instrText> PAGE </w:instrText></w:r>\
                   <w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>\
                   <w:r><w:t>7</w:t></w:r>\
                   <w:r><w:fldChar w:fldCharType=\"end\"/></w:r>";
        let (text, _) = visible_text(xml);
        assert_eq!(text, "7");
        assert!(mteqn_cached_number(xml).is_none());
    }

    #[test]
    fn simple_field_number_is_read_from_cached_result() {
        // WordCraft/Word пишут часть полей MathType как `w:fldSimple` (инструкция в атрибуте,
        // кэш номера — `w:instrText` внутри).
        let xml = "<w:r><w:t>.</w:t></w:r><w:r><w:tab/></w:r><w:r><w:instrText>(</w:instrText></w:r>\
             <w:fldSimple w:instr=\" SEQ MTEqn \\c \\* Arabic \\* MERGEFORMAT \"><w:r><w:rPr><w:noProof/></w:rPr><w:instrText>7</w:instrText></w:r></w:fldSimple>\
             <w:r><w:instrText>)</w:instrText></w:r>";
        assert_eq!(mteqn_cached_number(xml).as_deref(), Some("7"));
        let (text, _) = visible_text(xml);
        assert!(text.contains('7'), "кэш результата виден: {text:?}");
        assert_eq!(literal_text(xml), ".");
    }

    #[test]
    fn paragraph_props_are_skipped_and_tabs_are_not_text() {
        let xml = "<w:pPr><w:tabs><w:tab w:val=\"center\" w:pos=\"4677\"/></w:tabs></w:pPr><w:r><w:tab/></w:r><w:r><w:t xml:space=\"preserve\">,</w:t></w:r>";
        let (text, non_text) = visible_text(xml);
        assert_eq!(text, "\t,");
        assert!(!non_text);
        assert_eq!(literal_text(xml), ",");
    }

    #[test]
    fn drawings_are_non_text_content() {
        let xml = "<w:r><w:drawing><wp:inline/></w:drawing></w:r>";
        let (text, non_text) = visible_text(xml);
        assert!(text.is_empty());
        assert!(non_text);
    }

    #[test]
    fn meaningful_text_ignores_numbers_and_punctuation() {
        assert!(!has_meaningful_text(&strip_number_tokens(".\t3\t(2.1)")));
        assert!(has_meaningful_text(&strip_number_tokens("где x — постоянная")));
        assert!(has_meaningful_text(&strip_number_tokens("см. (3)")));
        assert_eq!(strip_number_tokens("a (3) b (А.1)"), "a  b ");
        assert_eq!(strip_number_tokens("f(x) (2)"), "f(x) ");
    }

    #[test]
    fn paragraph_bounds_balances_nested_paragraphs() {
        let xml = "<w:body><w:p w14:paraId=\"A\"><w:r><w:t>x</w:t></w:r><w:pPr/></w:p><w:p><w:r><w:t>y</w:t></w:r></w:p></w:body>";
        let (start, end) = paragraph_bounds(xml, xml.find("x</w:t>").unwrap()).unwrap();
        assert_eq!(&xml[start..end], "<w:p w14:paraId=\"A\"><w:r><w:t>x</w:t></w:r><w:pPr/></w:p>");
        let second = paragraph_bounds(xml, xml.find("y</w:t>").unwrap()).unwrap();
        assert_eq!(&xml[second.0..second.1], "<w:p><w:r><w:t>y</w:t></w:r></w:p>");
    }
}
