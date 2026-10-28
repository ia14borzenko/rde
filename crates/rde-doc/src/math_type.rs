//! R-DOC-44: импорт MathType-вставок (OLE `Equation.DSMT4`) как нативных формул RDE.
//!
//! Почему: в чужих `.docx` формулы MathType — это OLE-объекты с приватным бинарным форматом MTEF;
//! в документе видна только preview-картинка (WMF). WordCraft preview не воспроизводит как формулу
//! (см. отдельный vendor-патч WMF-текста), а RDE умеет хранить формулы нативно (OMML).
//!
//! Что делает модуль (пост-обработка готового `.doc.rde` после [`crate::import_docx`]):
//! 1. находит в `word/document.xml` объекты `w:object` с `o:OLEObject ProgID="Equation.DSMT4"`;
//! 2. читает OLE-часть `word/embeddings/oleObjectN.bin` и достаёт стрим `Equation Native`;
//! 3. разбирает MTEF v5 → LaTeX ([`crate::mtef`]);
//! 4. компилирует формулу существующим пайплайном [`crate::formula::compile_formula`] (OMML+SVG+PNG);
//! 5. заменяет OLE-объект (вместе с его preview) на inline-OMML-формулу в том же абзаце;
//! 6. сохраняет **скрытые объекты RDE** в `rde:doc` (`<rde:formulas>`): LaTeX, sha256 MTEF, исходный
//!    OLE и preview (hex) — формулы и данные MathType не теряются, их можно восстановить/пересобрать;
//! 7. возвращает отчёт: сколько найдено/сконвертировано/не удалось, и предупреждения.
//!
//! Конверсия — «best effort»: если MTEF не разобрался или LaTeX не принят движком, объект остаётся
//! как есть (preview-картинка), а в отчёт импорта попадает честное предупреждение.

use std::collections::BTreeMap;
use std::path::Path;

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};

use crate::{DOCUMENT_XML, RDE_DOC_PART, RdeDoc, SCHEMA_VERSION, parse_rde_doc, read_package, render_rde_doc, write_package_entries};

/// ProgID объекта MathType в OOXML.
pub const MATHTYPE_PROG_ID: &str = "Equation.DSMT4";
/// Rel-тип OLE-объекта OOXML.
const OLE_REL: &str = "/oleObject";
/// Rel-тип картинки OOXML.
const IMAGE_REL: &str = "/image";

/// Скрытый объект RDE: MathType-вставка, переведённая в LaTeX (+ исходные данные).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdeFormula {
    /// Стабильный id: `mt-<12 hex sha256 MTEF-стрима>`.
    pub id: String,
    /// Эффективный LaTeX (результат MTEF v5 → LaTeX).
    pub latex: String,
    /// Display-формула (отдельный абзац MathType) или inline.
    #[serde(default)]
    pub display: bool,
    /// Часть пакета, из которой взят OLE: например `word/embeddings/oleObject1.bin`.
    #[serde(default)]
    pub source: String,
    /// Часть пакета с preview-картинкой (WMF): `word/media/image4.wmf`.
    #[serde(default)]
    pub preview: String,
    /// sha256 MTEF-стрима (`Equation Native`) — идентичность формулы, не зависит от OLE-обёртки.
    #[serde(default)]
    pub mtef_sha256: String,
    /// Исходный OLE-объект MathType (hex) — скрытое сохранение для будущего восстановления.
    #[serde(default)]
    pub ole_hex: String,
    /// Preview-картинка MathType (hex) — на случай восстановления исходного вида.
    #[serde(default)]
    pub preview_hex: String,
}

/// Отчёт конверсии MathType-объектов пакета.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MathTypeImportReport {
    /// Найдено объектов `Equation.DSMT4` в `word/document.xml`.
    pub found: usize,
    /// Успешно заменено на нативные формулы.
    pub converted: usize,
    /// Из них display-формул (отдельный абзац).
    pub display: usize,
    /// Не удалось (объект оставлен как preview) — причины в `warnings`.
    pub failed: usize,
    /// Скрытые объекты для `rde:doc`.
    pub formulas: Vec<RdeFormula>,
    pub warnings: Vec<String>,
}

impl MathTypeImportReport {
    /// Строка для журнала/отчёта импорта.
    pub fn summary(&self) -> String {
        if self.found == 0 {
            return "MathType-формул не найдено".to_owned();
        }
        format!(
            "MathType: {} из {} формул переведены в нативные (display: {}), не удалось: {}",
            self.converted, self.found, self.display, self.failed
        )
    }
}

/// Найти и сконвертировать все MathType-объекты `.doc.rde`-пакета (in place).
///
/// Пакет должен быть уже созданным `.doc.rde` (`word/document.xml` + части OLE). После успешной
/// конверсии в `word/document.xml` вместо OLE-объекта стоит OMML-формула, а в `rde:doc` добавлены
/// скрытые объекты [`RdeFormula`] и обновлён `body_hash`.
pub fn convert_math_type_objects(package: &Path) -> Result<MathTypeImportReport, String> {
    let mut entries = read_package(package)?.entries;
    let document_xml = entries.get(DOCUMENT_XML).cloned().ok_or_else(|| format!("в пакете нет `{DOCUMENT_XML}`"))?;
    let document_xml = String::from_utf8_lossy(&document_xml).to_string();
    // MathType вставляет OMML-фрагменты с префиксом `m:` — корень должен объявить пространство имён.
    let document_xml = ensure_math_namespace(document_xml);
    let rels_xml = entries.get("word/_rels/document.xml.rels").cloned().unwrap_or_default();
    let rels = parse_rels(&String::from_utf8_lossy(&rels_xml));
    let mut report = MathTypeImportReport::default();
    let mut replacements: Vec<(usize, usize, String)> = Vec::new();
    for (span_start, span_end) in object_spans(&document_xml) {
        let object = document_xml.get(span_start..span_end).unwrap_or_default();
        if !object.contains(MATHTYPE_PROG_ID) {
            continue;
        }
        report.found += 1;
        match convert_object(&document_xml, span_start, span_end, &rels, &entries) {
            Ok((run_start, run_end, omml, formula)) => {
                if formula.display {
                    report.display += 1;
                }
                replacements.push((run_start, run_end, omml));
                report.formulas.push(formula);
                report.converted += 1;
            }
            Err(error) => {
                report.failed += 1;
                report.warnings.push(format!("MathType: объект не переведён ({error}); оставлен preview"));
            }
        }
    }
    if replacements.is_empty() {
        return Ok(report);
    }
    // Заменяем с конца, чтобы смещения предыдущих объектов остались валидными.
    let mut updated = document_xml;
    replacements.sort_by_key(|(start, _, _)| std::cmp::Reverse(*start));
    for (start, end, omml) in &replacements {
        if updated.get(*start..*end).is_none() {
            return Err("MathType: границы замены вышли за документ".to_owned());
        }
        updated.replace_range(*start..*end, omml);
    }
    entries.insert(DOCUMENT_XML.to_owned(), updated.into_bytes());
    // rde:doc: добавить скрытые объекты, обновить версию схемы и подпись тела.
    let rde_xml = entries.get(RDE_DOC_PART).cloned().unwrap_or_default();
    let mut rde_doc: RdeDoc = parse_rde_doc(&String::from_utf8_lossy(&rde_xml))?;
    rde_doc.formulas.extend(report.formulas.clone());
    rde_doc.schema_version = SCHEMA_VERSION.to_owned();
    rde_doc.body_hash = rde_hash::sha256(&entries[DOCUMENT_XML]).hex();
    rde_doc.updated_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
    write_package_entries(&entries, package)?;
    Ok(report)
}

/// Сконвертировать один объект: заменить несущий его `w:r` на OMML-формулу.
fn convert_object(
    document_xml: &str,
    span_start: usize,
    span_end: usize,
    rels: &BTreeMap<String, Rel>,
    entries: &BTreeMap<String, Vec<u8>>,
) -> Result<(usize, usize, String, RdeFormula), String> {
    let object = document_xml.get(span_start..span_end).ok_or("объект вне документа")?;
    // r:id OLE-объекта и preview-картинки.
    let ole_rid = tag_attr(object, "<o:OLEObject", "r:id").ok_or("у OLEObject нет r:id")?;
    let ole_rel = rels.get(&ole_rid).ok_or_else(|| format!("нет relationship `{ole_rid}`"))?;
    if !ole_rel.kind.ends_with(OLE_REL) {
        return Err(format!("relationship `{ole_rid}` не OLE-объект ({})", ole_rel.kind));
    }
    let ole_part = normalized_part(&ole_rel.target);
    let ole = entries.get(&ole_part).ok_or_else(|| format!("в пакете нет OLE-части `{ole_part}`"))?;
    let image = tag_attr(object, "<v:imagedata", "r:id")
        .and_then(|rid| rels.get(&rid).cloned())
        .filter(|rel| rel.kind.ends_with(IMAGE_REL))
        .map(|rel| normalized_part(&rel.target));
    // OLE → MTEF → LaTeX.
    let mtef = crate::mtef::equation_native(ole).map_err(|error| format!("OLE: {error}"))?;
    let latex = crate::mtef::latex_from_mtef(&mtef).map_err(|error| format!("MTEF: {error}"))?;
    let mtef_sha256 = rde_hash::sha256(&mtef).hex();
    let id = format!("mt-{}", &mtef_sha256[..12]);
    // Заменить несущий абзац/run: display-формула — отдельный абзац MathType.
    let (run_start, run_end) = enclosing_run(document_xml, span_start, span_end).ok_or("не найден несущий w:r")?;
    let display = paragraph_is_display(document_xml, run_start);
    // Компиляция: LaTeX → OMML (штатный пайплайн RDE).
    let caches = crate::formula::compile_formula(&latex, display).map_err(|error| format!("LaTeX: {error}"))?;
    let omml = String::from_utf8_lossy(&caches.omml).to_string();
    if omml.trim().is_empty() {
        return Err("пустой OMML-фрагмент".to_owned());
    }
    let preview_hex = image
        .as_ref()
        .and_then(|part| entries.get(part))
        .map(|bytes| hex(bytes))
        .unwrap_or_default();
    let formula = RdeFormula {
        id,
        latex,
        display,
        source: ole_part,
        preview: image.unwrap_or_default(),
        mtef_sha256,
        ole_hex: hex(ole),
        preview_hex,
    };
    Ok((run_start, run_end, omml, formula))
}

/// Один relationship пакета.
#[derive(Clone, Debug)]
struct Rel {
    kind: String,
    target: String,
}

/// Target relationship → путь части пакета (`word/` + target, без ведущего `/`).
fn normalized_part(target: &str) -> String {
    let target = target.trim_start_matches('/');
    if target.starts_with("word/") { target.to_owned() } else { format!("word/{target}") }
}

/// Разобрать `word/_rels/document.xml.rels`.
fn parse_rels(xml: &str) -> BTreeMap<String, Rel> {
    let mut rels = BTreeMap::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                if local_name(event.name().as_ref()) != "Relationship" {
                    continue;
                }
                let mut id = String::new();
                let mut kind = String::new();
                let mut target = String::new();
                for attr in event.attributes().flatten() {
                    let key = local_name(attr.key.as_ref());
                    let value = attr.value.as_ref().to_string();
                    match key.as_str() {
                        "Id" => id = value,
                        "Type" => kind = value,
                        "Target" => target = value,
                        _ => {}
                    }
                }
                if !id.is_empty() {
                    rels.insert(id, Rel { kind, target });
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    rels
}

fn local_name(raw: &str) -> String {
    match raw.rsplit_once(':') {
        Some((_, local)) => local.to_owned(),
        None => raw.to_owned(),
    }
}

/// Найти спаны `<w:object ...>...</w:object>` в `word/document.xml`.
fn object_spans(xml: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = xml.get(from..).and_then(|rest| rest.find("<w:object")) {
        let start = from + rel;
        let Some(close_rel) = xml.get(start..).and_then(|rest| rest.find("</w:object>")) else { break };
        let end = start + close_rel + "</w:object>".len();
        spans.push((start, end));
        from = end;
    }
    spans
}

/// Значение атрибута в теге, начинающемся с `tag_prefix` (например `<o:OLEObject`).
fn tag_attr(xml: &str, tag_prefix: &str, name: &str) -> Option<String> {
    let start = xml.find(tag_prefix)?;
    let tag = xml.get(start..)?;
    let end = tag.find('>')?;
    let tag = tag.get(..=end)?;
    let needle = format!("{name}=\"");
    let at = tag.find(&needle)? + needle.len();
    let rest = tag.get(at..)?;
    let value_end = rest.find('"')?;
    Some(rest.get(..value_end)?.to_owned())
}

/// Найти `w:r` (несущий run), в котором лежит объект: от `<w:r` перед объектом до `</w:r>` после.
fn enclosing_run(xml: &str, object_start: usize, object_end: usize) -> Option<(usize, usize)> {
    let before = xml.get(..object_start)?;
    let run_start = rfind_element_start(before, "<w:r")?;
    // Между началом run и объектом не должно быть закрытия run.
    let between = xml.get(run_start..object_start)?;
    if between.contains("</w:r>") {
        return None;
    }
    let after = xml.get(object_end..)?;
    let close_rel = after.find("</w:r>")?;
    let run_end = object_end + close_rel + "</w:r>".len();
    Some((run_start, run_end))
}

/// Найти ближайший к концу `text` открывающий тег `prefix` (`<w:r`, `<w:p`…): после префикса идёт
/// пробел, `>` или `/`. Так `<w:r>` (без атрибутов) не путается с `<w:rPr>`/`<w:rFonts>` и с более
/// ранним `<w:r …>`.
fn rfind_element_start(text: &str, prefix: &str) -> Option<usize> {
    let mut from = text.len();
    while let Some(relative) = text.get(..from)?.rfind(prefix) {
        let after = text.as_bytes().get(relative + prefix.len()).copied();
        if matches!(after, Some(b' ' | b'>' | b'/')) {
            return Some(relative);
        }
        from = relative;
    }
    None
}

/// Абзац, в котором лежит позиция `pos`, — display-формула? (MathType пишет display-абзацам стиль
/// `MTDisplayEquation`/`MTDisplayEquationCenter` и т.п.)
fn paragraph_is_display(xml: &str, pos: usize) -> bool {
    let Some(before) = xml.get(..pos) else { return false };
    let Some(paragraph_start) = rfind_element_start(before, "<w:p") else { return false };
    let Some(after) = xml.get(paragraph_start..) else { return false };
    let Some(paragraph_end) = after.find("</w:p>") else { return false };
    let paragraph = after.get(..paragraph_end).unwrap_or_default();
    let style = tag_attr(paragraph, "<w:pStyle", "w:val")
        .or_else(|| tag_attr(paragraph, "<w:pStyle", "val"))
        .unwrap_or_default()
        .to_ascii_lowercase();
    style.contains("display") || style.contains("equation")
}

/// Убедиться, что корень документа объявляет пространство имён `m:` (OMML).
fn ensure_math_namespace(xml: String) -> String {
    if xml.contains("xmlns:m=") {
        return xml;
    }
    let needle = "<w:document ";
    match xml.find(needle) {
        Some(at) => {
            let insert_at = at + needle.len();
            let mut out = xml;
            out.insert_str(insert_at, "xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\" ");
            out
        }
        None => xml,
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(char::from_digit(u32::from(byte >> 4), 16).unwrap_or('0'));
        out.push(char::from_digit(u32::from(byte & 0x0F), 16).unwrap_or('0'));
    }
    out
}

#[cfg(test)]
mod tests;
