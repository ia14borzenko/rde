//! RDE templates (M4): профили оформления (ГОСТ 7.32-2017 и др.), применение к документу
//! WordCraft и библиография по ГОСТ 7.0.100-2018 (упрощённое подмножество).
//!
//! Шаблон — декларативный TOML (`rde/template@1`): страница (размер/поля), стили семейств
//! (основной текст, заголовки 1/2 уровня, подписи) и правило библиографии. Применение ставит
//! прямые свойства абзацев/символов в модели WordCraft; именованные стили документа не
//! переопределяются (стили шаблона — следующий шаг после v0.2).
//!
//! Единицы: TOML в миллиметрах/сантиметрах/пунктах, модель WordCraft — в пунктах (1 мм = 72/25.4 pt).

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};
use wordcraft_engine::doc::props::LineSpacing;
use wordcraft_engine::doc::{Align, Block, CharProps, Document, InlineObject, Paragraph, StoryRef};
use wordcraft_engine::doc::Path;

const MM_TO_PT: f32 = 72.0 / 25.4;
const CM_TO_PT: f32 = 72.0 / 2.54;

/// Правила страницы.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageRule {
    pub page_w_mm: f32,
    pub page_h_mm: f32,
    pub margin_left_mm: f32,
    pub margin_right_mm: f32,
    pub margin_top_mm: f32,
    pub margin_bottom_mm: f32,
    #[serde(default = "default_header")]
    pub header_mm: f32,
    #[serde(default = "default_header")]
    pub footer_mm: f32,
}

fn default_header() -> f32 {
    10.0
}

/// Стиль семейства абзацев.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StyleRule {
    pub font: String,
    pub size_pt: f32,
    #[serde(default)]
    pub bold: bool,
    #[serde(default)]
    pub italic: bool,
    /// `left|center|right|justify`.
    #[serde(default = "default_align")]
    pub align: String,
    #[serde(default)]
    pub first_line_indent_cm: f32,
    #[serde(default = "default_line_spacing")]
    pub line_spacing: f32,
    #[serde(default)]
    pub space_before_pt: f32,
    #[serde(default)]
    pub space_after_pt: f32,
}

fn default_align() -> String {
    "justify".to_owned()
}

fn default_line_spacing() -> f32 {
    1.5
}

/// Правило библиографии.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BibliographyRule {
    pub heading: String,
    /// Формат: пока только `gost-7.0.100-2018`.
    #[serde(default = "default_bib_format")]
    pub format: String,
}

fn default_bib_format() -> String {
    "gost-7.0.100-2018".to_owned()
}

/// Шаблон оформления.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Template {
    pub schema: String,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    pub page: PageRule,
    pub body: StyleRule,
    pub heading1: StyleRule,
    pub heading2: StyleRule,
    pub caption: StyleRule,
    pub bibliography: BibliographyRule,
}

const GOST_732: &str = include_str!("../templates/gost-7.32-2017.toml");
const LAB_REPORT: &str = include_str!("../templates/lab-report.toml");
const FREE: &str = include_str!("../templates/free.toml");

fn parse_builtin(source: &str) -> Template {
    toml::from_str(source).expect("встроенный шаблон должен быть валиден")
}

/// Встроенные шаблоны.
pub fn builtin_templates() -> &'static [Template] {
    static TEMPLATES: OnceLock<Vec<Template>> = OnceLock::new();
    TEMPLATES.get_or_init(|| vec![parse_builtin(GOST_732), parse_builtin(LAB_REPORT), parse_builtin(FREE)])
}

/// Найти встроенный шаблон по id.
pub fn template_by_id(id: &str) -> Option<&'static Template> {
    builtin_templates().iter().find(|template| template.id == id)
}

/// Разобрать пользовательский шаблон из TOML.
pub fn parse_template(toml_text: &str) -> Result<Template, String> {
    let template: Template = toml::from_str(toml_text).map_err(|e| format!("шаблон TOML: {e}"))?;
    if template.schema != "rde/template@1" {
        return Err(format!("несовместимая schema шаблона: {}", template.schema));
    }
    if template.id.is_empty() {
        return Err("у шаблона пустой id".to_owned());
    }
    Ok(template)
}

fn align_of(rule: &StyleRule) -> Align {
    match rule.align.as_str() {
        "center" => Align::Center,
        "right" => Align::Right,
        "justify" => Align::Justify,
        _ => Align::Left,
    }
}

fn char_props(rule: &StyleRule) -> CharProps {
    CharProps {
        font: Some(rule.font.clone()),
        size: Some(rule.size_pt),
        bold: rule.bold.then_some(true),
        italic: rule.italic.then_some(true),
        ..CharProps::default()
    }
}

fn para_props(rule: &StyleRule) -> wordcraft_engine::doc::ParaProps {
    let mut props = wordcraft_engine::doc::ParaProps::default();
    props.align = Some(align_of(rule));
    props.indent_first = Some(rule.first_line_indent_cm * CM_TO_PT);
    props.line_spacing = Some(LineSpacing::Multiple(rule.line_spacing));
    props.space_before = Some(rule.space_before_pt);
    props.space_after = Some(rule.space_after_pt);
    props
}

/// Семейство абзаца: основной текст, заголовок 1/2, подпись/рисунок.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Body,
    Heading1,
    Heading2,
    Caption,
}

impl Template {
    pub fn rule(&self, family: Family) -> &StyleRule {
        match family {
            Family::Body => &self.body,
            Family::Heading1 => &self.heading1,
            Family::Heading2 => &self.heading2,
            Family::Caption => &self.caption,
        }
    }
}

fn classify(paragraph: &Paragraph) -> Family {
    let style = paragraph.props.style.clone().unwrap_or_default().to_ascii_lowercase();
    let has_image = paragraph
        .objects
        .iter()
        .any(|object| matches!(object, InlineObject::Image { .. }));
    let text = paragraph.text.trim_start();
    if style == "1" || style == "heading1" || style.starts_with("heading 1") {
        Family::Heading1
    } else if style == "2" || style == "heading2" || style.starts_with("heading 2") {
        Family::Heading2
    } else if has_image || text.starts_with("Рисунок") || text.starts_with("Таблица") {
        Family::Caption
    } else {
        Family::Body
    }
}

/// Применить шаблон к документу: страница/поля + прямые стили абзацев и символов.
/// Возвращает журнал применённых правил (для UI/лога).
pub fn apply(template: &Template, doc: &mut Document) -> Vec<String> {
    let mut log = Vec::new();

    // 1. Страница и поля (все секции документа).
    let section_blocks: Vec<usize> = doc.sections().iter().map(|(block, _)| *block).collect();
    for block in section_blocks {
        let section = doc.section_mut(block);
        section.page_w = template.page.page_w_mm * MM_TO_PT;
        section.page_h = template.page.page_h_mm * MM_TO_PT;
        section.margin_left = template.page.margin_left_mm * MM_TO_PT;
        section.margin_right = template.page.margin_right_mm * MM_TO_PT;
        section.margin_top = template.page.margin_top_mm * MM_TO_PT;
        section.margin_bottom = template.page.margin_bottom_mm * MM_TO_PT;
        section.header = template.page.header_mm * MM_TO_PT;
        section.footer = template.page.footer_mm * MM_TO_PT;
    }
    log.push(format!(
        "страница: {:.0}×{:.0} мм, поля слева/справа/сверху/снизу {:.0}/{:.0}/{:.0}/{:.0} мм",
        template.page.page_w_mm,
        template.page.page_h_mm,
        template.page.margin_left_mm,
        template.page.margin_right_mm,
        template.page.margin_top_mm,
        template.page.margin_bottom_mm
    ));

    // 2. Абзацы и символы по семействам.
    let story = StoryRef::Body;
    let paths = doc.para_paths(story);
    let mut counts = [0usize; 4];
    for path in paths {
        let Some(paragraph) = doc.para(story, &path) else { continue };
        let family = classify(paragraph);
        let rule = template.rule(family);
        let props = para_props(rule);
        let chars = char_props(rule);
        if let Ok(paragraph) = doc.para_mut(story, &path) {
            paragraph.props.align = props.align;
            paragraph.props.indent_first = props.indent_first;
            paragraph.props.line_spacing = props.line_spacing;
            paragraph.props.space_before = props.space_before;
            paragraph.props.space_after = props.space_after;
            paragraph.mark.font = chars.font.clone();
            paragraph.mark.size = chars.size;
            paragraph.mark.bold = chars.bold;
            paragraph.mark.italic = chars.italic;
            for run in &mut paragraph.runs {
                run.props.font = chars.font.clone();
                run.props.size = chars.size;
                run.props.bold = chars.bold;
                run.props.italic = chars.italic;
            }
            paragraph.rev = paragraph.rev.wrapping_add(1);
            counts[family as usize] += 1;
        }
    }
    log.push(format!(
        "основной текст: {} ({} абзацев)",
        describe_rule(&template.body),
        counts[Family::Body as usize]
    ));
    log.push(format!(
        "заголовки 1 уровня: {} ({}), 2 уровня: {} ({})",
        describe_rule(&template.heading1),
        counts[Family::Heading1 as usize],
        describe_rule(&template.heading2),
        counts[Family::Heading2 as usize]
    ));
    log.push(format!(
        "подписи/рисунки: {} ({})",
        describe_rule(&template.caption),
        counts[Family::Caption as usize]
    ));
    log.push(format!(
        "библиография: «{}» (формат {})",
        template.bibliography.heading, template.bibliography.format
    ));
    log
}

fn describe_rule(rule: &StyleRule) -> String {
    let mut text = format!("{} {:.0} pt", rule.font, rule.size_pt);
    if rule.bold {
        text.push_str(", полужирный");
    }
    text.push_str(&format!(", интервал {}, {}", rule.line_spacing, rule.align));
    if rule.first_line_indent_cm > 0.0 {
        text.push_str(&format!(", отступ {:.2} см", rule.first_line_indent_cm));
    }
    text
}

/// Сводка шаблона для UI/CLI.
pub fn summary(template: &Template) -> String {
    format!("{} — {}", template.id, template.title)
}

// ── библиография ─────────────────────────────────────────────────────────────

/// Отформатировать библиографическую запись источника по ГОСТ 7.0.100-2018 (упрощённо).
/// `fallback_title` — название источника, если в `[reference]` нет title (например, для стандарта).
pub fn format_reference(reference: &rde_core::ReferenceManifest, fallback_title: &str) -> String {
    let authors = reference.authors.join(", ");
    let title = if !reference.title.is_empty() { reference.title.clone() } else { fallback_title.to_owned() };
    let year = reference.year.map(|year| year.to_string()).unwrap_or_default();
    let publisher = reference.publisher.trim();
    match reference.kind.as_str() {
        "standard" => {
            let mut text = if reference.designation.is_empty() { title } else { reference.designation.clone() };
            if !reference.title.is_empty() {
                text.push_str(&format!(". {}", reference.title));
            }
            if !year.is_empty() {
                text.push_str(&format!(". — {year}"));
            }
            text
        }
        _ => {
            let mut text = String::new();
            if !authors.is_empty() {
                text.push_str(&authors);
                text.push_str(". ");
            }
            text.push_str(&title);
            if !publisher.is_empty() {
                text.push_str(&format!(". — {publisher}"));
                if !year.is_empty() {
                    text.push_str(&format!(", {year}"));
                }
            } else if !year.is_empty() {
                text.push_str(&format!(". — {year}"));
            }
            if !reference.designation.is_empty() {
                text.push_str(&format!(". — {}", reference.designation));
            }
            text
        }
    }
}

/// Собрать библиографию проекта из источников (сортировка по алфавиту, без дублей).
pub fn bibliography_entries(project: &rde_core::Project) -> Vec<String> {
    let mut entries: Vec<String> = project
        .sources()
        .filter_map(|object| match &object.data {
            rde_core::ObjectData::Source(source) => source.reference.as_ref().map(|reference| format_reference(reference, &source.title)),
            _ => None,
        })
        .filter(|entry| !entry.trim().is_empty())
        .collect();
    entries.sort_by_key(|entry| entry.to_ascii_lowercase());
    entries.dedup();
    entries
}

/// Вставить список источников в конец документа.
/// Возвращает число вставленных абзацев (0 — библиография уже есть).
pub fn insert_bibliography(template: &Template, doc: &mut Document, entries: &[String]) -> Result<usize, String> {
    let story = StoryRef::Body;
    let heading = &template.bibliography.heading;
    let existing = doc.plain_text(story);
    if existing.contains(heading) {
        return Ok(0);
    }
    let body_rule = &template.body;
    let heading_rule = &template.heading1;
    let mut next_index = doc
        .para_paths(story)
        .iter()
        .map(|path| path.0.first().copied().unwrap_or(0))
        .max()
        .map(|index| index as usize + 1)
        .unwrap_or(0);

    // Заголовок списка.
    let mut heading_para = Paragraph::new();
    heading_para.insert_text(0, heading, &char_props(heading_rule)).map_err(|e| e.to_string())?;
    heading_para.props = para_props(heading_rule);
    heading_para.props.indent_first = Some(0.0);
    doc.insert_block(story, &Path::top(next_index), Block::Para(heading_para)).map_err(|e| e.to_string())?;
    next_index += 1;

    let mut inserted = 1;
    for (index, entry) in entries.iter().enumerate() {
        let mut paragraph = Paragraph::new();
        let text = format!("{}. {}", index + 1, entry);
        paragraph.insert_text(0, &text, &char_props(body_rule)).map_err(|e| e.to_string())?;
        paragraph.props = para_props(body_rule);
        paragraph.props.indent_first = Some(0.0);
        doc.insert_block(story, &Path::top(next_index), Block::Para(paragraph)).map_err(|e| e.to_string())?;
        next_index += 1;
        inserted += 1;
    }
    Ok(inserted)
}

/// Применить шаблон и вставить библиографию проекта (сценарий кнопки «Оформить по ГОСТ»).
pub fn apply_with_bibliography(template: &Template, doc: &mut Document, project: &rde_core::Project) -> Result<Vec<String>, String> {
    let mut log = apply(template, doc);
    let entries = bibliography_entries(project);
    let inserted = insert_bibliography(template, doc, &entries)?;
    if inserted == 0 {
        log.push("библиография уже присутствует в документе".to_owned());
    } else {
        log.push(format!("библиография: добавлено {} абзацев ({} источников)", inserted, entries.len()));
    }
    Ok(log)
}
