//! Нумерация формул в контексте документа: схемы «сквозная» и «по разделам» (ГОСТ),
//! приложения (`(А.1)`), детерминированный разбор номеров разделов из заголовков (ADR-0018).
//!
//! Схема хранится в `document.toml` проекта (`formula_numbering = "continuous" | "section"`),
//! номер — документно-локальный (ADR-0005 §3.1): одна project-level формула в разных документах
//! имеет независимые номера. Номера назначает RDE при вставке/обновлении, руками не набираются.
//!
//! Раздел определяется по заголовку 1-го уровня: уровень резолвится через `word/styles.xml`
//! (`w:style` имени style id: direct `w:outlineLvl`, имя «heading N»/«Заголовок N», цепочка
//! `w:basedOn`), прямое `w:pPr/w:outlineLvl` абзаца имеет приоритет. Номер раздела берётся из
//! ведущего числа текста заголовка («2 РСА…» → `2`, «2.1 …» → `2.1`); приложения — из буквы
//! («Приложение А» / «А.1 …» → `А`). Нет номера в заголовке — fallback: порядковый номер
//! заголовка 1-го уровня, это предупреждение (не молчаливая ошибка). Формулы до первого
//! раздела нумеруются сквозно с предупреждением; если заголовков разделов в документе нет
//! вовсе, схема «по разделам» деградирует в сквозную с предупреждением.

use std::collections::BTreeMap;
use std::fmt;

/// Схема нумерации формул документа.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum FormulaNumbering {
    /// Сквозная: `(1)`, `(2)`, … (по умолчанию).
    #[default]
    Continuous,
    /// По разделам (ГОСТ): `(2.1)`, `(2.2)`…; приложения — `(А.1)`.
    Section,
}

impl FormulaNumbering {
    /// Разобрать значение из `document.toml` (неизвестное — `None`).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "continuous" | "сквозная" | "seq" => Some(Self::Continuous),
            "section" | "по разделам" | "sections" => Some(Self::Section),
            _ => None,
        }
    }

    /// Значение для `document.toml` (каноническая строка).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Continuous => "continuous",
            Self::Section => "section",
        }
    }
}

impl fmt::Display for FormulaNumbering {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Заголовок 1-го уровня (раздел), найденный в документе.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionHeading {
    /// Байтовое смещение `<w:p` абзаца-заголовка в `word/document.xml`.
    pub offset: usize,
    /// Токен номера раздела: `"2"`, `"2.1"`, `"А"`; пустая строка — номера не нашлось.
    pub token: String,
    /// Заголовок без номера в тексте: использован порядковый fallback (нужно предупредить).
    pub fallback: bool,
}

/// Результат сканирования разделов документа.
#[derive(Clone, Debug, Default)]
pub struct SectionScan {
    pub headings: Vec<SectionHeading>,
    pub warnings: Vec<String>,
}

impl SectionScan {
    /// Токен раздела, действующий для байтовой позиции `pos` (последний заголовок до неё).
    pub fn token_at(&self, pos: usize) -> Option<&SectionHeading> {
        self.headings.iter().rev().find(|heading| heading.offset <= pos)
    }
}

/// Назначенный номер формулы: отображаемый текст без скобок (`"1"`, `"2.1"`, `"А.1"`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormulaNumber {
    pub id: String,
    pub number: String,
    pub warning: Option<String>,
}

/// Назначить номера формулам `ordered` (id, байтовая позиция тега в документе, по порядку).
///
/// Возвращает номера в порядке формул и предупреждения (fallback-разделы, сквозная нумерация
/// до первого раздела, отсутствие заголовков при схеме «по разделам»).
pub fn number_formulas(
    document_xml: &str,
    styles_xml: Option<&str>,
    scheme: FormulaNumbering,
    ordered: &[(String, usize)],
) -> (Vec<FormulaNumber>, Vec<String>) {
    let mut warnings = Vec::new();
    if scheme == FormulaNumbering::Continuous {
        let numbers = ordered
            .iter()
            .enumerate()
            .map(|(index, (id, _))| FormulaNumber { id: id.clone(), number: (index + 1).to_string(), warning: None })
            .collect();
        return (numbers, warnings);
    }
    let scan = scan_sections(document_xml, styles_xml);
    warnings.extend(scan.warnings.iter().cloned());
    if scan.headings.is_empty() {
        warnings.push(
            "схема нумерации «по разделам»: в документе нет заголовков 1-го уровня — формулы пронумерованы сквозно".to_owned(),
        );
        let numbers = ordered
            .iter()
            .enumerate()
            .map(|(index, (id, _))| FormulaNumber { id: id.clone(), number: (index + 1).to_string(), warning: None })
            .collect();
        return (numbers, warnings);
    }
    let mut counters: BTreeMap<String, u32> = BTreeMap::new();
    let mut pre_section = 0u32;
    let mut warned_pre_section = false;
    let mut numbers = Vec::with_capacity(ordered.len());
    for (id, pos) in ordered {
        match scan.token_at(*pos) {
            Some(heading) if !heading.token.is_empty() => {
                let counter = counters.entry(heading.token.clone()).or_insert(0);
                *counter += 1;
                numbers.push(FormulaNumber { id: id.clone(), number: format!("{}.{}", heading.token, counter), warning: None });
            }
            Some(heading) => {
                // Заголовок без номера в тексте: fallback-токен уже порядковый; подписываем раздел
                // номером fallback (см. ниже `scan_sections`), здесь только предупреждение.
                let counter = counters.entry(heading.token.clone()).or_insert(0);
                *counter += 1;
                let number = if heading.token.is_empty() { (numbers.len() + 1).to_string() } else { format!("{}.{}", heading.token, counter) };
                numbers.push(FormulaNumber { id: id.clone(), number, warning: Some("раздел без номера в заголовке".to_owned()) });
            }
            None => {
                pre_section += 1;
                if !warned_pre_section {
                    warned_pre_section = true;
                    warnings.push(
                        "схема «по разделам»: формулы до первого заголовка раздела пронумерованы сквозно (перенесите их в раздел или смените схему)"
                            .to_owned(),
                    );
                }
                numbers.push(FormulaNumber { id: id.clone(), number: pre_section.to_string(), warning: None });
            }
        }
    }
    (numbers, warnings)
}

/// Найти заголовки 1-го уровня и их номера в `word/document.xml`.
///
/// `styles_xml` — `word/styles.xml` пакета (резолв уровня стиля); можно `None` — тогда уровень
/// берётся только из `w:pPr/w:outlineLvl` и имени стиля по id, если id сам «Heading1»-подобный.
pub fn scan_sections(document_xml: &str, styles_xml: Option<&str>) -> SectionScan {
    let styles = styles_xml.map(StylesIndex::parse).unwrap_or_default();
    let mut scan = SectionScan::default();
    let mut auto_index = 0u32;
    for paragraph in paragraphs(document_xml) {
        let Some(level) = heading_level(paragraph.paragraph_props.as_deref(), &styles) else { continue };
        if level != 0 {
            continue;
        }
        auto_index += 1;
        let text = paragraph_text(&paragraph.xml);
        let (token, fallback) = match level1_token(&text) {
            Some(token) => (token, false),
            None => {
                scan.warnings.push(format!(
                    "заголовок раздела без номера («{}»): используется порядковый номер {auto_index}",
                    text.trim().chars().take(40).collect::<String>()
                ));
                (auto_index.to_string(), true)
            }
        };
        scan.headings.push(SectionHeading { offset: paragraph.offset, token, fallback });
    }
    scan
}

/// Абзац документа: смещение `<w:p`, содержимое `w:pPr` (если есть) и сам XML абзаца.
struct ParagraphXml {
    offset: usize,
    paragraph_props: Option<String>,
    xml: String,
}

/// Все абзацы верхнего уровня и вложенные (таблицы/контролы) — обходом открывающих тегов `<w:p`.
fn paragraphs(document_xml: &str) -> Vec<ParagraphXml> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while let Some(rel) = document_xml[pos..].find("<w:p ") {
        let start = pos + rel;
        let Some(end) = balanced_end(document_xml, start, "w:p") else { break };
        let xml = &document_xml[start..end];
        let paragraph_props = element(xml, "w:pPr").map(|(s, e)| xml[s..e].to_owned());
        out.push(ParagraphXml { offset: start, paragraph_props, xml: xml.to_owned() });
        pos = end;
    }
    // Абзац может быть записан и как `<w:p>` (без атрибутов) — включим и такие.
    let mut pos = 0usize;
    while let Some(rel) = document_xml[pos..].find("<w:p>") {
        let start = pos + rel;
        let Some(end) = balanced_end(document_xml, start, "w:p") else { break };
        let xml = &document_xml[start..end];
        let paragraph_props = element(xml, "w:pPr").map(|(s, e)| xml[s..e].to_owned());
        out.push(ParagraphXml { offset: start, paragraph_props, xml: xml.to_owned() });
        pos = end;
    }
    out.sort_by_key(|paragraph| paragraph.offset);
    out.dedup_by_key(|paragraph| paragraph.offset);
    out
}

/// Границы элемента `tag` от открывающего тега (учитываются самозакрытые и вложения того же имени).
fn balanced_end(xml: &str, start: usize, tag: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut pos = start;
    while pos < xml.len() {
        let Some(rel) = xml[pos..].find('<') else { return None };
        let at = pos + rel;
        let Some(rel_end) = xml[at..].find('>') else { return None };
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

/// Внутренние границы первого элемента `tag` внутри `xml` (без учёта вложенности того же имени —
/// для `w:pPr`/`w:pStyle` достаточно, они не вкладываются).
fn element<'a>(xml: &'a str, tag: &str) -> Option<(usize, usize)> {
    let open = format!("<{tag}");
    let start = xml.find(&open)?;
    let open_end = xml[start..].find('>')? + start;
    if xml[start..open_end].ends_with('/') {
        return Some((start, open_end + 1));
    }
    let close = format!("</{tag}>");
    let close_start = xml[open_end..].find(&close)? + open_end;
    Some((start, close_start + close.len()))
}

/// Атрибут `name` (например `w:val`) в тексте тега, начиная с `from`.
fn attribute(xml: &str, from: usize, name: &str) -> Option<(String, usize)> {
    let tag_end = xml[from..].find('>')? + from;
    let tag_text = &xml[from..tag_end];
    let marker = format!("{name}=");
    let index = tag_text.find(&marker)? + marker.len();
    let text = &tag_text[index..];
    let quote = text.chars().next()?;
    let value = if quote == '"' || quote == '\'' {
        let rest = &text[1..];
        let end = rest.find(quote)?;
        rest[..end].to_owned()
    } else {
        text.split([' ', '>', '/']).next().unwrap_or("").to_owned()
    };
    Some((value, tag_end))
}

/// Индекс стилей `word/styles.xml`: id → уровень заголовка (0 = заголовок 1-го уровня).
#[derive(Default)]
struct StylesIndex {
    /// styleId → (имя стиля, direct outlineLvl, basedOn).
    styles: BTreeMap<String, StyleEntry>,
}

#[derive(Default)]
struct StyleEntry {
    name: Option<String>,
    outline_level: Option<i64>,
    based_on: Option<String>,
}

impl StylesIndex {
    fn parse(styles_xml: &str) -> Self {
        let mut index = Self::default();
        let mut pos = 0usize;
        while let Some(rel) = styles_xml[pos..].find("<w:style ") {
            let start = pos + rel;
            let Some(end) = balanced_end(styles_xml, start, "w:style") else { break };
            let xml = &styles_xml[start..end];
            let Some((id, _)) = attribute(xml, 0, "w:styleId") else {
                pos = end;
                continue;
            };
            let name = element(xml, "w:name").and_then(|(s, _)| attribute(xml, s, "w:val")).map(|(v, _)| v);
            let outline_level = element(xml, "w:outlineLvl").and_then(|(s, _)| attribute(xml, s, "w:val")).and_then(|(v, _)| v.parse().ok());
            let based_on = element(xml, "w:basedOn").and_then(|(s, _)| attribute(xml, s, "w:val")).map(|(v, _)| v);
            index.styles.insert(id, StyleEntry { name, outline_level, based_on });
            pos = end;
        }
        index
    }

    /// Уровень заголовка стиля (0 = 1-й уровень) с проходом по `basedOn`.
    fn level_of(&self, style_id: &str) -> Option<i64> {
        let mut current = style_id.to_owned();
        for _ in 0..16 {
            let Some(entry) = self.styles.get(&current) else { return level_from_name(style_id) };
            if let Some(level) = entry.outline_level {
                return Some(level);
            }
            if let Some(name) = &entry.name
                && let Some(level) = level_from_name(name)
            {
                return Some(level);
            }
            match &entry.based_on {
                Some(parent) => current = parent.clone(),
                None => return level_from_name(&entry.name.clone().unwrap_or_default()).or_else(|| level_from_name(style_id)),
            }
        }
        None
    }
}

/// Уровень заголовка по имени стиля («heading 1» / «Heading1» / «Заголовок 1»).
fn level_from_name(name: &str) -> Option<i64> {
    let normalized = name.trim().to_ascii_lowercase().replace([' ', '_', '-'], "");
    for (prefix, _level) in [("heading", 1i64), ("заголовок", 1), ("title", 0)] {
        if let Some(rest) = normalized.strip_prefix(prefix) {
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if let Ok(number) = digits.parse::<i64>() {
                return Some((number - 1).max(0));
            }
            if prefix == "title" {
                return Some(0);
            }
        }
    }
    None
}

/// Уровень абзаца-заголовка: direct `w:outlineLvl` > стиль `w:pStyle` > имя styleId «HeadingN».
fn heading_level(paragraph_props: Option<&str>, styles: &StylesIndex) -> Option<i64> {
    let p_pr = paragraph_props?;
    if let Some((start, _)) = element(p_pr, "w:outlineLvl")
        && let Some((value, _)) = attribute(p_pr, start, "w:val")
        && let Ok(level) = value.parse::<i64>()
    {
        return Some(level);
    }
    if let Some((start, _)) = element(p_pr, "w:pStyle")
        && let Some((style_id, _)) = attribute(p_pr, start, "w:val")
    {
        return styles.level_of(&style_id);
    }
    None
}

/// Текст абзаца: содержимое всех `w:t` (в порядке документа), без полей/формул.
fn paragraph_text(paragraph_xml: &str) -> String {
    let mut out = String::new();
    let mut pos = 0usize;
    while let Some(rel) = paragraph_xml[pos..].find("<w:t") {
        let start = pos + rel;
        let open_end = paragraph_xml[start..].find('>').map(|offset| start + offset + 1);
        let Some(open_end) = open_end else { break };
        let Some(end) = paragraph_xml[open_end..].find("</w:t>").map(|offset| open_end + offset) else { break };
        out.push_str(&unescape_xml(&paragraph_xml[open_end..end]));
        pos = end + "</w:t>".len();
    }
    out
}

/// Минимальный XML-unescape (тексты заголовков; сущности не влияют на номер раздела).
fn unescape_xml(text: &str) -> String {
    if !text.contains('&') {
        return text.to_owned();
    }
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// Номер раздела из текста заголовка 1-го уровня: `"2 РСА…"` → `"2"`, `"2.1 Модель"` → `"2.1"`,
/// `"3."` → `"3"`, `"Приложение А"`/`"А.1 …"` → `"А"`. `None` — номера нет (нужен fallback).
fn level1_token(text: &str) -> Option<String> {
    let text = text.trim_start();
    let mut chars = text.chars();
    let first = chars.next()?;
    if first.is_ascii_digit() {
        let mut token = String::new();
        for ch in text.chars() {
            if ch.is_ascii_digit() || ch == '.' {
                token.push(ch);
            } else {
                break;
            }
        }
        let token = token.trim_end_matches('.').to_owned();
        return (!token.is_empty()).then_some(token);
    }
    // Приложение: «Приложение А» / «ПРИЛОЖЕНИЕ Б» / «А.» / «А.1 …» — буква, заглавная кириллица/латиница.
    let is_upper = first.is_uppercase() && (first.is_alphabetic());
    if "приложение".starts_with(&text.to_lowercase().chars().collect::<String>()) || text.to_lowercase().starts_with("приложение") {
        let rest = text.trim_start_matches(|c: char| c.is_alphabetic()).trim_start();
        return rest.chars().next().filter(|c| c.is_uppercase()).map(|c| c.to_string());
    }
    // «А», «А.», «А.1 …» — буква приложения (заглавная буква и точка/конец).
    if is_upper && first.is_alphabetic() {
        let mut chars = text.chars();
        let letter = chars.next()?;
        let rest: String = chars.collect();
        let after_letter = rest.trim_start();
        if after_letter.is_empty() || after_letter.starts_with('.') {
            return Some(letter.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(paragraphs: &str) -> String {
        format!("<w:document><w:body>{paragraphs}</w:body></w:document>")
    }

    fn heading(style: &str, text: &str) -> String {
        format!("<w:p><w:pPr><w:pStyle w:val=\"{style}\"/></w:pPr><w:r><w:t xml:space=\"preserve\">{text}</w:t></w:r></w:p>")
    }

    fn body(text: &str) -> String {
        format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
    }

    const STYLES: &str = r#"<w:styles><w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:outlineLvl w:val="0"/></w:style><w:style w:type="paragraph" w:styleId="Heading2"><w:name w:val="heading 2"/><w:outlineLvl w:val="1"/></w:style><w:style w:type="paragraph" w:styleId="Normal"><w:name w:val="Normal"/></w:style></w:styles>"#;

    #[test]
    fn scheme_strings_roundtrip() {
        assert_eq!(FormulaNumbering::parse("continuous"), Some(FormulaNumbering::Continuous));
        assert_eq!(FormulaNumbering::parse("по разделам"), Some(FormulaNumbering::Section));
        assert_eq!(FormulaNumbering::parse("section"), Some(FormulaNumbering::Section));
        assert_eq!(FormulaNumbering::parse("иное"), None);
        assert_eq!(FormulaNumbering::Section.as_str(), "section");
        assert_eq!(FormulaNumbering::default(), FormulaNumbering::Continuous);
    }

    #[test]
    fn continuous_numbers_by_order() {
        let xml = doc(&format!("{}{}{}", heading("Heading1", "1 Введение"), body("x"), heading("Heading1", "2 Метод")));
        let ordered = vec![("a".to_owned(), 400usize), ("b".to_owned(), 900usize), ("c".to_owned(), 100usize)];
        let (numbers, warnings) = number_formulas(&xml, Some(STYLES), FormulaNumbering::Continuous, &ordered);
        assert!(warnings.is_empty());
        assert_eq!(numbers.iter().map(|n| n.number.as_str()).collect::<Vec<_>>(), vec!["1", "2", "3"]);
    }

    #[test]
    fn section_numbers_restart_per_section() {
        let xml = doc(&format!(
            "{}{}{}{}{}{}",
            heading("Heading1", "1 Введение"),
            body("формула 1"),
            heading("Heading1", "2 Метод"),
            body("формула 2"),
            heading("Heading2", "2.1 Подраздел"),
            body("формула 3")
        ));
        let first = xml.find("формула 1").unwrap();
        let second = xml.find("формула 2").unwrap();
        let third = xml.find("формула 3").unwrap();
        let ordered = vec![("a".to_owned(), first), ("b".to_owned(), second), ("c".to_owned(), third)];
        let (numbers, warnings) = number_formulas(&xml, Some(STYLES), FormulaNumbering::Section, &ordered);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(numbers.iter().map(|n| n.number.as_str()).collect::<Vec<_>>(), vec!["1.1", "2.1", "2.2"]);
    }

    #[test]
    fn appendix_letter_numbering() {
        let xml = doc(&format!(
            "{}{}{}{}",
            heading("Heading1", "1 Введение"),
            heading("Heading1", "Приложение А"),
            body("формула А1"),
            body("формула А2")
        ));
        let ordered = vec![("a".to_owned(), xml.find("формула А1").unwrap()), ("b".to_owned(), xml.find("формула А2").unwrap())];
        let (numbers, warnings) = number_formulas(&xml, Some(STYLES), FormulaNumbering::Section, &ordered);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(numbers.iter().map(|n| n.number.as_str()).collect::<Vec<_>>(), vec!["А.1", "А.2"]);
    }

    #[test]
    fn heading_without_number_falls_back_with_warning() {
        let xml = doc(&format!("{}{}{}", heading("Heading1", "Введение"), body("f1"), heading("Heading1", "Метод")));
        let ordered = vec![("a".to_owned(), xml.find("f1").unwrap())];
        let (numbers, warnings) = number_formulas(&xml, Some(STYLES), FormulaNumbering::Section, &ordered);
        assert_eq!(numbers[0].number, "1.1", "fallback: порядковый номер раздела");
        assert!(warnings.iter().any(|w| w.contains("без номера")), "{warnings:?}");
    }

    #[test]
    fn no_headings_degrades_to_continuous_with_warning() {
        let xml = doc(&format!("{}{}", body("f1"), body("f2")));
        let ordered = vec![("a".to_owned(), 100usize), ("b".to_owned(), 200usize)];
        let (numbers, warnings) = number_formulas(&xml, Some(STYLES), FormulaNumbering::Section, &ordered);
        assert_eq!(numbers.iter().map(|n| n.number.as_str()).collect::<Vec<_>>(), vec!["1", "2"]);
        assert!(warnings.iter().any(|w| w.contains("нет заголовков")), "{warnings:?}");
    }

    #[test]
    fn formulas_before_first_section_are_continuous_with_warning() {
        let xml = doc(&format!("{}{}", body("до раздела"), heading("Heading1", "1 Введение")));
        let ordered = vec![("a".to_owned(), xml.find("до раздела").unwrap()), ("b".to_owned(), xml.len() - 20)];
        let (numbers, warnings) = number_formulas(&xml, Some(STYLES), FormulaNumbering::Section, &ordered);
        assert_eq!(numbers.iter().map(|n| n.number.as_str()).collect::<Vec<_>>(), vec!["1", "1.1"]);
        assert!(warnings.iter().any(|w| w.contains("до первого заголовка")), "{warnings:?}");
    }

    #[test]
    fn style_resolution_via_based_on_and_direct_outline_level() {
        // Пользовательские стили (импортированный Word): basedOn на «heading 1» и direct outlineLvl.
        let styles = r#"<w:styles>
            <w:style w:type="paragraph" w:styleId="1"><w:name w:val="heading 1"/><w:outlineLvl w:val="0"/></w:style>
            <w:style w:type="paragraph" w:styleId="Custom"><w:name w:val="Мой заголовок"/><w:basedOn w:val="1"/></w:style>
            <w:style w:type="paragraph" w:styleId="Direct"><w:name w:val="MyTitle"/><w:outlineLvl w:val="0"/></w:style>
        </w:styles>"#;
        let xml = doc(&format!(
            "{}{}{}{}{}",
            heading("Custom", "1 Первый"),
            body("f1"),
            heading("Direct", "2 Второй"),
            body("f2"),
            "<w:p><w:pPr><w:outlineLvl w:val=\"0\"/></w:pPr><w:r><w:t>3 Третий</w:t></w:r></w:p>"
        ));
        let ordered = vec![
            ("a".to_owned(), xml.find("f1").unwrap()),
            ("b".to_owned(), xml.find("f2").unwrap()),
            ("c".to_owned(), xml.len() - 30),
        ];
        let (numbers, warnings) = number_formulas(&xml, Some(styles), FormulaNumbering::Section, &ordered);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(numbers.iter().map(|n| n.number.as_str()).collect::<Vec<_>>(), vec!["1.1", "2.1", "3.1"]);
    }

    #[test]
    fn level1_token_parsing() {
        assert_eq!(level1_token("1 Введение"), Some("1".to_owned()));
        assert_eq!(level1_token("2.1 Модель"), Some("2.1".to_owned()));
        assert_eq!(level1_token("3."), Some("3".to_owned()));
        assert_eq!(level1_token("Приложение А"), Some("А".to_owned()));
        assert_eq!(level1_token("ПРИЛОЖЕНИЕ Б"), Some("Б".to_owned()));
        assert_eq!(level1_token("А.1 Схема"), Some("А".to_owned()));
        assert_eq!(level1_token("Введение"), None);
        assert_eq!(level1_token("Times New Roman"), None);
    }
}
