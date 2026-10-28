//! Импорт чужих `.docx` в проект RDE (R-DOC-39, минимальная честная версия 2026-10-28).
//!
//! Задача: пользователь приносит обычный `.docx` (ВКР, отчёт, чужая работа) и хочет открыть его
//! в RDE как **новый объект-документ**. Импорт:
//! - никогда не перезаписывает исходник (только читает);
//! - создаёт **новый** `.doc.rde`-пакет (WordCraft-модель → OOXML + `rde:doc` + `w14:paraId`),
//!   исходное имя/происхождение фиксируются в `import.json` рядом с пакетом;
//! - возвращает **отчёт**: части исходного пакета, число блоков, предупреждения о конструкциях,
//!   которые RDE не поддерживает/может потерять при последующем сохранении (R-DOC-39: «импорт без
//!   потерь молча» — потерять данные можно, но пользователь обязан увидеть предупреждение);
//! - маркировка `[IMPORTED]` в заголовке объекта — делает вызывающий слой (UI/CLI).
//!
//! Осознанные ограничения этой версии: только `.docx` (старый `.doc`, `.odt/.rtf/.html/.md/.tex`
//! — по возможностям WordCraft, отдельная работа); полный отчёт «что не поддержано» по каждому
//! элементу (сравнение фиделити) и merge правок — задачи полного R-DOC-39 в v0.2. Здесь —
//! конвертация + эвристические предупреждения по частям пакета и известным потерям WordCraft.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{DOCUMENT_XML, package_info, read_package, save_engine_document};

/// Сводка по MathType-формулам в отчёте импорта (R-DOC-44). Сами скрытые объекты — в `rde:doc`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MathTypeSummary {
    pub found: usize,
    pub converted: usize,
    pub display: usize,
    pub failed: usize,
}

/// Отчёт импорта `.docx` (пишется в `import.json` рядом с пакетом и показывается пользователю).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct DocImportReport {
    /// Абсолютный путь исходного `.docx`.
    pub source: PathBuf,
    /// SHA-256 исходника (фиксация происхождения; исходник не изменяется).
    pub source_sha256: String,
    /// Размер исходника, байт.
    pub source_bytes: u64,
    /// Путь созданного `.doc.rde`.
    pub package: PathBuf,
    /// Размер созданного пакета, байт.
    pub package_bytes: u64,
    /// Id документа в `rde:doc` (совпадает с `objects/documents/<id>/document.toml`).
    pub doc_id: String,
    /// Шаблон оформления, применённый к пакету.
    pub template: String,
    /// Части исходного `.docx` (для прозрачности/диагностики).
    pub source_parts: Vec<String>,
    /// Сколько частей в созданном `.doc.rde`.
    pub imported_parts: usize,
    /// Число блоков (абзацев/рисунков/подписей) в импортированном документе.
    pub blocks: usize,
    /// Unix-время импорта.
    pub imported_unix: u64,
    /// R-DOC-44: MathType-формулы, переведённые в нативные (LaTeX извлечён, OLE сохранён скрыто).
    #[serde(default)]
    pub math_type: MathTypeSummary,
    /// Предупреждения: что RDE не поддерживает и что может потеряться при последующем сохранении.
    pub warnings: Vec<String>,
}

impl DocImportReport {
    /// Короткая строка для журнала: «N частей, M блоков, K предупреждений».
    pub fn summary(&self) -> String {
        let math = if self.math_type.found > 0 {
            format!(
                ", MathType-формул: {}/{} нативных",
                self.math_type.converted, self.math_type.found
            )
        } else {
            String::new()
        };
        format!(
            "импортирован .docx ({} частей, {} блоков{math}, предупреждений: {})",
            self.source_parts.len(),
            self.blocks,
            self.warnings.len()
        )
    }
}

/// Импортировать `.docx` в новый `.doc.rde`-пакет.
///
/// `package` — путь будущего пакета (`objects/documents/<id>/report.doc.rde`); `doc_id_hint` —
/// id документа из `document.toml`; `template` — шаблон оформления (например,
/// `gost-7.32-2017`). Исходник только читается. Рядом с пакетом создаётся `import.json`.
///
/// # Errors
///
/// Не `.docx`/нет файла/битый zip/нет `word/document.xml`/ошибка конвертации WordCraft.
pub fn import_docx(
    source: &Path,
    package: &Path,
    doc_id_hint: &str,
    template: &str,
) -> Result<DocImportReport, String> {
    if !source.is_file() {
        return Err(format!("файл импорта не найден: {}", source.display()));
    }
    let extension = source
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "docx" | "docm" => {}
        "doc" => {
            return Err(
                "старый бинарный `.doc` не импортируется этой версией: сохраните документ как `.docx` в Word/LibreOffice и повторите"
                    .to_owned(),
            );
        }
        other => {
            return Err(format!(
                "импорт этой версии поддерживает `.docx` (получено `.{other}`); `.odt/.rtf/.html/.md/.tex/.txt` — по возможностям WordCraft, отдельная задача"
            ));
        }
    }
    if package == source {
        return Err("пакет назначения не может совпадать с исходным .docx".to_owned());
    }
    let bytes = std::fs::read(source)
        .map_err(|error| format!("прочитать {}: {error}", source.display()))?;
    let source_bytes = bytes.len() as u64;
    let source_sha256 = rde_hash::sha256(&bytes).hex();
    // Разбор исходного zip: нужен для списка частей и эвристик «что может потеряться».
    let source_entries = read_package(source)?;
    let Some(document_xml_bytes) = source_entries.entries.get(DOCUMENT_XML) else {
        return Err(format!(
            "в {} нет `{DOCUMENT_XML}` — это не WordprocessingML-документ",
            source.display()
        ));
    };
    let document_xml = String::from_utf8_lossy(document_xml_bytes);
    let mut source_parts: Vec<String> = source_entries.entries.keys().cloned().collect();
    source_parts.sort();
    let warnings = analyze_source(&source_parts, &document_xml);
    if let Some(parent) = package.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("создать каталог документа {}: {error}", parent.display()))?;
    }
    // Конвертация: WordCraft-модель → OOXML + rde:doc + w14:paraId (штатный путь RDE).
    let rde_doc = save_engine_document(source, package, doc_id_hint, template)?;
    // R-DOC-44: MathType-объекты → нативные формулы (LaTeX из MTEF) + скрытые объекты в rde:doc.
    let (math_type, math_warnings) = convert_math_type_in_package(package);
    let mut warnings = warnings;
    warnings.extend(math_warnings);
    let info = package_info(package)?;
    let blocks = crate::document_blocks(package).map_err(|error| {
        format!("проверить импортированный документ {}: {error}", package.display())
    })?;
    let package_bytes = std::fs::metadata(package).map(|meta| meta.len()).unwrap_or(0);
    let report = DocImportReport {
        source: source.to_path_buf(),
        source_sha256,
        source_bytes,
        package: package.to_path_buf(),
        package_bytes,
        doc_id: rde_doc.doc_id,
        template: rde_doc.template,
        source_parts,
        imported_parts: info.parts.len(),
        blocks: blocks.len(),
        imported_unix: now_unix(),
        math_type,
        warnings,
    };
    write_provenance(package, &report)?;
    Ok(report)
}

/// Post-processing импортированного пакета: MathType-объекты → нативные формулы (R-DOC-44).
///
/// Ошибка конверсии **не** проваливает импорт: пакет остаётся как есть (формулы видны preview-
/// картинкой благодаря vendor-патчу WMF-текста), а в отчёт попадает предупреждение.
fn convert_math_type_in_package(package: &Path) -> (MathTypeSummary, Vec<String>) {
    match crate::math_type::convert_math_type_objects(package) {
        Ok(report) => {
            let summary = MathTypeSummary {
                found: report.found,
                converted: report.converted,
                display: report.display,
                failed: report.failed,
            };
            let mut warnings = report.warnings;
            if report.converted > 0 {
                warnings.push(format!(
                    "MathType-формулы: {} из {} переведены в нативные (LaTeX извлечён из MTEF, OLE сохранён скрыто в rde:doc)",
                    report.converted, report.found
                ));
            }
            (summary, warnings)
        }
        Err(error) => (
            MathTypeSummary::default(),
            vec![format!("MathType-формулы не переведены в нативные: {error}")],
        ),
    }
}

/// Записать `import.json` рядом с пакетом (происхождение импорта, R-DOC-39).
fn write_provenance(package: &Path, report: &DocImportReport) -> Result<(), String> {
    let dir = package.parent().ok_or_else(|| "у пакета нет родительского каталога".to_owned())?;
    let json = serde_json::to_string_pretty(report)
        .map_err(|error| format!("сериализовать отчёт импорта: {error}"))?;
    std::fs::write(dir.join("import.json"), json)
        .map_err(|error| format!("записать {}: {error}", dir.join("import.json").display()))
}

/// Эвристики «что RDE не поддерживает / может потерять при сохранении» (R-DOC-39).
///
/// Это предупреждения, а не отказ: пользователь видит их в журнале/CLI-отчёте и решает сам.
fn analyze_source(parts: &[String], document_xml: &str) -> Vec<String> {
    let mut warnings = Vec::new();
    let has_part = |prefix: &str| parts.iter().any(|part| part.starts_with(prefix));
    if has_part("word/charts/") {
        warnings.push(
            "диаграммы Word (`word/charts/*`): RDE их не редактирует; после open/save в RDE сохранность зависит от WordCraft — проверьте документ"
                .to_owned(),
        );
    }
    if has_part("word/diagrams/") {
        warnings.push(
            "SmartArt (`word/diagrams/*`): не поддерживается RDE; может быть потерян при сохранении"
                .to_owned(),
        );
    }
    if has_part("word/embeddings/") {
        warnings.push(
            "OLE/встроенные объекты (`word/embeddings/*`): MathType-формулы (`Equation.DSMT4`) при импорте переводятся в нативные формулы RDE с сохранением исходника; остальные OLE могут быть потеряны при сохранении из RDE"
                .to_owned(),
        );
    }
    if has_part("word/vbaProject.bin") {
        warnings.push("макросы VBA: не поддерживаются RDE и не сохраняются в `.doc.rde`".to_owned());
    }
    if has_part("word/glossary/") {
        warnings.push("глоссарий Word: не поддерживается RDE".to_owned());
    }
    if document_xml.contains("<w:ins ") || document_xml.contains("<w:del ") {
        warnings.push(
            "track changes (`w:ins`/`w:del`): RDE не поддерживает их; примите/отклоните изменения в Word до импорта, иначе они будут потеряны"
                .to_owned(),
        );
    }
    if document_xml.contains("<w:txbxContent") {
        warnings.push("надписи/текстовые поля (text boxes): поддерживаются ограниченно".to_owned());
    }
    if document_xml.contains("<w:fldChar") || document_xml.contains("<w:instrText") {
        warnings.push(
            "поля Word (TOC/PAGE/REF/SEQ и др.): RDE не пересчитывает поля (R-DOC-14 — v0.2); в редакторе они могут отображаться как текст"
                .to_owned(),
        );
    }
    if document_xml.contains("<w:sdt") {
        warnings.push(
            "content controls (`w:sdt`): сохраняются как элементы Word, но не превращаются во вставки RDE"
                .to_owned(),
        );
    }
    if parts.iter().any(|part| part.starts_with("customXml/") && !part.contains("itemProps")) {
        warnings.push(
            "чужие `customXml`-части (data binding): RDE добавляет свою `rde:doc`-часть; смешивание с Word Online не поддерживается"
                .to_owned(),
        );
    }
    warnings
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::analyze_source;

    fn parts(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn warnings_cover_known_lossy_constructions() {
        let source_parts = parts(&[
            "word/document.xml",
            "word/charts/chart1.xml",
            "word/diagrams/data1.xml",
            "word/embeddings/oleObject1.bin",
            "word/vbaProject.bin",
            "word/glossary/document.xml",
            "customXml/item1.xml",
        ]);
        let xml = "<w:document><w:body><w:p><w:ins w:id=\"1\"><w:r><w:t>x</w:t></w:r></w:ins>\
                   <w:del w:id=\"2\"><w:r><w:delText>y</w:delText></w:r></w:del>\
                   <w:txbxContent/><w:fldChar w:fldCharType=\"begin\"/><w:instrText>TOC</w:instrText>\
                   <w:sdt/></w:p></w:body></w:document>";
        let warnings = analyze_source(&source_parts, xml);
        for marker in [
            "диаграммы Word",
            "SmartArt",
            "OLE/встроенные",
            "макросы VBA",
            "глоссарий",
            "track changes",
            "надписи/текстовые поля",
            "поля Word",
            "content controls",
            "customXml",
        ] {
            assert!(
                warnings.iter().any(|warning| warning.contains(marker)),
                "нет предупреждения `{marker}`: {warnings:?}"
            );
        }
    }

    #[test]
    fn clean_document_has_no_warnings() {
        let warnings = analyze_source(&parts(&["word/document.xml", "word/styles.xml"]), "<w:document/>");
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}
