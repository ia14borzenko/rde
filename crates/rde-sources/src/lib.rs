//! RDE sources (M5): импорт библиографии, работа с PDF и OCR, веб-архив.
//!
//! Реализовано в прототипе:
//! - импорт цитат из BibTeX / RIS / CSL-JSON → объекты-источники проекта (`source.toml` + `[reference]`);
//! - PDF: текстовый слой (`pdftotext`), число страниц (`pdfinfo`), рендер страницы в PNG (`pdftoppm`);
//! - OCR: `tesseract` (rus+eng) для изображений и отсканированных страниц PDF;
//! - веб-архив: `monolith` (если установлен; иначе явная ошибка — M5-ограничение).
//!
//! Бэкенд PDF/OCR — внешние CLI poppler/tesseract (Linux-first, см. ADR-0001 §3). Интеграция
//! `pdfium-render` (библиотека уже лежит в `refs/bin/pdfium`) — следующий шаг: тот же трейт
//! `PdfBackend`, без изменения вызывающего кода. Это осознанное отклонение прототипа от ТЗ
//! (в ТЗ pdfium), зафиксировано в `docs/milestones.md` (M5).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Импортированная библиографическая запись (до сохранения в проект).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImportedReference {
    pub authors: Vec<String>,
    pub title: String,
    pub year: Option<u16>,
    /// `book|article|standard|thesis|web|other`.
    pub kind: String,
    pub publisher: String,
    pub designation: String,
    /// `bibtex|ris|csl-json`.
    pub origin: String,
}

impl Default for ImportedReference {
    fn default() -> Self {
        Self {
            authors: Vec::new(),
            title: String::new(),
            year: None,
            kind: "other".to_owned(),
            publisher: String::new(),
            designation: String::new(),
            origin: String::new(),
        }
    }
}

/// Формат импорта цитат.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CitationFormat {
    Bibtex,
    Ris,
    CslJson,
}

impl CitationFormat {
    /// Определить формат по содержимому (эвристика; для CLI-импорта).
    pub fn detect(text: &str) -> Self {
        let trimmed = text.trim_start();
        if trimmed.starts_with('[') || trimmed.starts_with('{') {
            CitationFormat::CslJson
        } else if trimmed.starts_with('@') {
            CitationFormat::Bibtex
        } else {
            CitationFormat::Ris
        }
    }

    pub fn parse(self, text: &str) -> Result<Vec<ImportedReference>, String> {
        match self {
            CitationFormat::Bibtex => parse_bibtex(text),
            CitationFormat::Ris => parse_ris(text),
            CitationFormat::CslJson => parse_csl_json(text),
        }
    }
}

fn map_bib_kind(kind: &str) -> &'static str {
    match kind.to_ascii_lowercase().as_str() {
        "book" | "inbook" | "booklet" => "book",
        "article" | "inproceedings" | "conference" => "article",
        "techreport" | "report" => "report",
        "phdthesis" | "mastersthesis" => "thesis",
        "misc" | "online" | "webpage" => "web",
        "standard" => "standard",
        _ => "other",
    }
}

fn map_ris_kind(kind: &str) -> &'static str {
    match kind.to_ascii_uppercase().as_str() {
        "BOOK" | "EBK" => "book",
        "JOUR" | "JFULL" | "MGZN" => "article",
        "RPRT" => "report",
        "THES" => "thesis",
        "ELEC" | "WEB" | "GEN" => "web",
        "STAND" => "standard",
        _ => "other",
    }
}

fn map_csl_kind(kind: &str) -> &'static str {
    match kind.to_ascii_lowercase().as_str() {
        "book" => "book",
        "article-journal" | "article-magazine" | "article-newspaper" | "article" => "article",
        "report" => "report",
        "thesis" => "thesis",
        "webpage" | "post-weblog" => "web",
        "standard" | "legislation" | "regulation" => "standard",
        _ => "other",
    }
}

fn clean_bib_value(raw: &str) -> String {
    let mut text = raw.trim();
    if (text.starts_with('{') && text.ends_with('}')) || (text.starts_with('"') && text.ends_with('"')) {
        text = &text[1..text.len() - 1];
    }
    // Минимальная очистка LaTeX-экранирования.
    text.replace("\\&", "&")
        .replace("\\%", "%")
        .replace("---", "—")
        .replace("--", "–")
        .replace('{', "")
        .replace('}', "")
        .replace("\\ ", " ")
}

fn split_bib_authors(raw: &str) -> Vec<String> {
    raw.split(" and ")
        .map(|author| clean_bib_value(author))
        .filter(|author| !author.is_empty())
        .collect()
}

fn parse_year(raw: &str) -> Option<u16> {
    let digits: String = raw.chars().filter(|ch| ch.is_ascii_digit()).collect();
    if digits.len() >= 4 {
        digits[..4].parse().ok()
    } else {
        None
    }
}

/// Разобрать BibTeX (подмножество: @type{key, field = value, ...}).
pub fn parse_bibtex(text: &str) -> Result<Vec<ImportedReference>, String> {
    let mut references = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find('@') {
        rest = &rest[at + 1..];
        let Some(open) = rest.find('{') else { break };
        let entry_type = rest[..open].trim();
        // Найти закрывающую скобку с учётом вложенности.
        let mut depth = 1usize;
        let mut end = None;
        for (index, ch) in rest[open + 1..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + 1 + index);
                        break;
                    }
                }
                _ => {}
            }
        }
        let Some(end) = end else { return Err("BibTeX: незакрытая запись".to_owned()) };
        let body = &rest[open + 1..end];
        rest = &rest[end + 1..];
        // Поля: разбиваем по запятым верхнего уровня.
        let mut fields = Vec::new();
        let mut current = String::new();
        let mut level = 0i32;
        let mut in_quotes = false;
        for ch in body.chars() {
            match ch {
                '{' => {
                    level += 1;
                    current.push(ch);
                }
                '}' => {
                    level -= 1;
                    current.push(ch);
                }
                '"' => {
                    in_quotes = !in_quotes;
                    current.push(ch);
                }
                ',' if level == 0 && !in_quotes => {
                    fields.push(std::mem::take(&mut current));
                }
                _ => current.push(ch),
            }
        }
        if !current.trim().is_empty() {
            fields.push(current);
        }
        let mut reference = ImportedReference { kind: map_bib_kind(entry_type).to_owned(), origin: "bibtex".to_owned(), ..ImportedReference::default() };
        for field in fields {
            let Some((key, value)) = field.split_once('=') else { continue };
            let key = key.trim().to_ascii_lowercase();
            let value = value.trim();
            match key.as_str() {
                "author" => reference.authors = split_bib_authors(value),
                "title" => reference.title = clean_bib_value(value),
                "year" | "date" => reference.year = parse_year(value),
                "publisher" | "school" | "institution" | "organization" => reference.publisher = clean_bib_value(value),
                "number" | "isbn" | "doi" => {
                    if reference.designation.is_empty() {
                        reference.designation = clean_bib_value(value);
                    }
                }
                "journal" | "booktitle" => {
                    if reference.publisher.is_empty() {
                        reference.publisher = clean_bib_value(value);
                    }
                }
                _ => {}
            }
        }
        if reference.title.is_empty() && reference.designation.is_empty() {
            continue;
        }
        references.push(reference);
    }
    Ok(references)
}

/// Разобрать RIS (TY/AU/TI/PY/PB/ER и др.).
pub fn parse_ris(text: &str) -> Result<Vec<ImportedReference>, String> {
    let mut references = Vec::new();
    let mut current: Option<ImportedReference> = None;
    for line in text.lines() {
        let line = line.trim_end();
        if line.len() < 5 {
            continue;
        }
        let tag = &line[..2];
        let value = line[5..].trim();
        match tag {
            "TY" => {
                current = Some(ImportedReference { kind: map_ris_kind(value).to_owned(), origin: "ris".to_owned(), ..ImportedReference::default() });
            }
            "AU" | "A1" | "A2" => {
                if let Some(reference) = current.as_mut() {
                    if !value.is_empty() {
                        reference.authors.push(value.to_owned());
                    }
                }
            }
            "TI" | "T1" | "BT" => {
                if let Some(reference) = current.as_mut() {
                    reference.title = value.to_owned();
                }
            }
            "PY" | "Y1" => {
                if let Some(reference) = current.as_mut() {
                    reference.year = parse_year(value);
                }
            }
            "PB" => {
                if let Some(reference) = current.as_mut() {
                    reference.publisher = value.to_owned();
                }
            }
            "ER" => {
                if let Some(reference) = current.take() {
                    if !reference.title.is_empty() {
                        references.push(reference);
                    }
                }
            }
            _ => {}
        }
    }
    Ok(references)
}

fn csl_author(author: &serde_json::Value) -> Option<String> {
    if let Some(literal) = author.get("literal").and_then(|value| value.as_str()) {
        return Some(literal.to_owned());
    }
    let family = author.get("family").and_then(|value| value.as_str()).unwrap_or("");
    let given = author.get("given").and_then(|value| value.as_str()).unwrap_or("");
    match (family.is_empty(), given.is_empty()) {
        (false, false) => Some(format!("{family}, {given}")),
        (false, true) => Some(family.to_owned()),
        (true, false) => Some(given.to_owned()),
        _ => None,
    }
}

/// Разобрать CSL-JSON (массив элементов).
pub fn parse_csl_json(text: &str) -> Result<Vec<ImportedReference>, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("CSL-JSON: {e}"))?;
    let items = match value {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(_) => vec![value],
        _ => return Err("CSL-JSON: ожидался массив или объект".to_owned()),
    };
    let mut references = Vec::new();
    for item in items {
        let mut reference = ImportedReference { origin: "csl-json".to_owned(), ..ImportedReference::default() };
        if let Some(kind) = item.get("type").and_then(|value| value.as_str()) {
            reference.kind = map_csl_kind(kind).to_owned();
        }
        if let Some(authors) = item.get("author").and_then(|value| value.as_array()) {
            reference.authors = authors.iter().filter_map(csl_author).collect();
        }
        if let Some(title) = item.get("title").and_then(|value| value.as_str()) {
            reference.title = title.to_owned();
        }
        if let Some(year) = item
            .get("issued")
            .and_then(|value| value.get("date-parts"))
            .and_then(|value| value.get(0))
            .and_then(|value| value.get(0))
            .and_then(|value| value.as_u64())
        {
            reference.year = u16::try_from(year).ok();
        }
        if let Some(publisher) = item.get("publisher").and_then(|value| value.as_str()) {
            reference.publisher = publisher.to_owned();
        }
        if let Some(number) = item.get("number").and_then(|value| value.as_str()) {
            reference.designation = number.to_owned();
        }
        if !reference.title.is_empty() {
            references.push(reference);
        }
    }
    Ok(references)
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Импортировать записи в проект: создаёт объекты-источники с `[reference]`.
/// Возвращает каталоги созданных источников.
pub fn import_citations(project_root: &Path, references: &[ImportedReference]) -> Result<Vec<PathBuf>, String> {
    let mut created = Vec::new();
    for reference in references {
        let title = if !reference.title.is_empty() {
            reference.title.clone()
        } else if !reference.designation.is_empty() {
            reference.designation.clone()
        } else {
            "Импортированный источник".to_owned()
        };
        let dir = rde_core::create_object(
            project_root,
            &rde_core::CreateRequest { kind: rde_core::NewObjectKind::Source, title: title.clone() },
        )?;
        // Дописываем [reference] в source.toml и заметку с цитатой.
        let manifest_path = dir.join("source.toml");
        let mut manifest = fs::read_to_string(&manifest_path).map_err(|e| e.to_string())?;
        manifest.push_str(&format!(
            "\n[reference]\nauthors = [{}]\ntitle = {}\nkind = {}\n",
            reference.authors.iter().map(|author| toml_string(author)).collect::<Vec<_>>().join(", "),
            toml_string(&title),
            toml_string(&reference.kind)
        ));
        if let Some(year) = reference.year {
            manifest.push_str(&format!("year = {year}\n"));
        }
        if !reference.publisher.is_empty() {
            manifest.push_str(&format!("publisher = {}\n", toml_string(&reference.publisher)));
        }
        if !reference.designation.is_empty() {
            manifest.push_str(&format!("designation = {}\n", toml_string(&reference.designation)));
        }
        fs::write(&manifest_path, manifest).map_err(|e| e.to_string())?;
        let citation = format!(
            "Импортировано ({origin}): {authors} {title}{year}.\n",
            origin = reference.origin,
            authors = reference.authors.join(", "),
            title = title,
            year = reference.year.map(|year| format!(" — {year}")).unwrap_or_default()
        );
        fs::write(dir.join("source.md"), format!("# {title}\n\n{citation}")).map_err(|e| e.to_string())?;
        created.push(dir);
    }
    Ok(created)
}

fn run_tool(program: &str, args: &[&str]) -> Result<Vec<u8>, String> {
    let output = Command::new(program).args(args).output().map_err(|e| format!("не удалось запустить {program}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "{program} завершился с {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(output.stdout)
}

fn tool_available(program: &str) -> bool {
    Command::new(program).arg("--version").output().map(|output| output.status.success()).unwrap_or(false)
}

/// Текстовый слой PDF (`pdftotext`).
pub fn pdf_text(path: &Path) -> Result<String, String> {
    let path_str = path.to_string_lossy().to_string();
    let bytes = run_tool("pdftotext", &["-layout", "-enc", "UTF-8", &path_str, "-"])?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

/// Число страниц PDF (`pdfinfo`).
pub fn pdf_page_count(path: &Path) -> Result<u32, String> {
    let path_str = path.to_string_lossy().to_string();
    let bytes = run_tool("pdfinfo", &[&path_str])?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Pages:") {
            return rest.trim().parse().map_err(|e| format!("pdfinfo Pages: {e}"));
        }
    }
    Err("pdfinfo: не найдено число страниц".to_owned())
}

/// Текстовый слой одной страницы PDF (`pdftotext -f/-l`); страницы нумеруются с 1.
///
/// Нужно контекст-ссылкам RDE (M4.4, §8.7.2): чип `rde://source/<id>?page=N` резолвится в
/// текст конкретной страницы без чтения всего PDF.
pub fn pdf_page_text(path: &Path, page: u32) -> Result<String, String> {
    if page == 0 {
        return Err("страницы нумеруются с 1".to_owned());
    }
    let page_arg = page.to_string();
    let path_str = path.to_string_lossy().to_string();
    let bytes = run_tool(
        "pdftotext",
        &["-layout", "-enc", "UTF-8", "-f", &page_arg, "-l", &page_arg, &path_str, "-"],
    )?;
    Ok(String::from_utf8_lossy(&bytes).to_string())
}

/// Размер страницы PDF в точках (`pdfinfo -f N -l N` → `Page size: W x H pts`); страницы с 1.
///
/// Нужен для пересчёта региона (bbox) между растром и PDF-координатами в контекст-ссылках
/// (M4.4): `rde://source/<id>?page=N&bbox=…&pageSize=W,H`.
pub fn pdf_page_size(path: &Path, page: u32) -> Result<(f32, f32), String> {
    if page == 0 {
        return Err("страницы нумеруются с 1".to_owned());
    }
    let page_arg = page.to_string();
    let path_str = path.to_string_lossy().to_string();
    let bytes = run_tool("pdfinfo", &["-f", &page_arg, "-l", &page_arg, &path_str])?;
    let text = String::from_utf8_lossy(&bytes);
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("Page size:") {
            // Пример: `Page size: 595.276 x 841.89 pts (A4)`.
            let values = rest.split('(').next().unwrap_or(rest).trim();
            let mut parts = values.split(" x ");
            let width = parts.next().and_then(|value| value.trim().parse::<f32>().ok());
            let height = parts
                .next()
                .map(|value| value.trim().trim_end_matches("pts").trim())
                .and_then(|value| value.parse::<f32>().ok());
            if let (Some(width), Some(height)) = (width, height) {
                return Ok((width, height));
            }
            return Err(format!("pdfinfo Page size: не разобран `{rest}`"));
        }
    }
    Err("pdfinfo: не найден размер страницы".to_owned())
}

/// Отрисовать страницу PDF в PNG (`pdftoppm`, 150 dpi по умолчанию).
pub fn render_pdf_page(path: &Path, page: u32, out_png: &Path) -> Result<PathBuf, String> {
    if page == 0 {
        return Err("страницы нумеруются с 1".to_owned());
    }
    let out_base = out_png.with_extension("");
    let page_arg = page.to_string();
    let path_str = path.to_string_lossy().to_string();
    let out_str = out_base.to_string_lossy().to_string();
    run_tool("pdftoppm", &["-png", "-r", "150", "-f", &page_arg, "-l", &page_arg, "-singlefile", &path_str, &out_str])?;
    let produced = out_base.with_extension("png");
    if produced.is_file() {
        Ok(produced)
    } else {
        Err(format!("pdftoppm не создал {}", produced.display()))
    }
}

/// OCR изображения (`tesseract`, rus+eng).
pub fn ocr_image(path: &Path) -> Result<String, String> {
    let path_str = path.to_string_lossy().to_string();
    let bytes = run_tool("tesseract", &[&path_str, "stdout", "-l", "rus+eng", "--psm", "3"])?;
    Ok(String::from_utf8_lossy(&bytes).trim().to_owned())
}

/// OCR страницы PDF: рендер в PNG → tesseract.
pub fn ocr_pdf_page(path: &Path, page: u32) -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("rde-ocr-{}-{page}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let png = render_pdf_page(path, page, &dir.join("page.png"))?;
    let text = ocr_image(&png);
    let _ = fs::remove_dir_all(&dir);
    text
}

/// Веб-архив страницы (`monolith`, если установлен).
pub fn archive_url(url: &str, out_html: &Path) -> Result<(), String> {
    if !tool_available("monolith") {
        return Err("веб-архив: monolith не установлен (M5-ограничение; сохраните страницу вручную)".to_owned());
    }
    let out_str = out_html.to_string_lossy().to_string();
    run_tool("monolith", &[url, "-o", &out_str])?;
    Ok(())
}

/// Доступность внешних инструментов (для UI/диагностики).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ToolAvailability {
    pub pdftotext: bool,
    pub pdfinfo: bool,
    pub pdftoppm: bool,
    pub tesseract: bool,
    pub monolith: bool,
}

pub fn tool_availability() -> ToolAvailability {
    ToolAvailability {
        pdftotext: tool_available("pdftotext"),
        pdfinfo: tool_available("pdfinfo"),
        pdftoppm: tool_available("pdftoppm"),
        tesseract: tool_available("tesseract"),
        monolith: tool_available("monolith"),
    }
}
