//! RDE document package (M3): `.doc.rde` = OOXML-пакет WordCraft + RDE-часть `rde:doc` (customXml).
//!
//! Зачем: документ должен оставаться настоящим `.docx` для Word/LibreOffice, но RDE нужно хранить
//! своё — стабильные блоки (paraId/пути), вставки (рисунки/подписи/ссылки на артефакты), шаблон,
//! сводку ревью и «могилу» для детекта внешнего редактирования.
//!
//! Раскладка `.doc.rde` (zip):
//! ```text
//! [Content_Types].xml              # + Override для customXml/itemProps1.xml
//! word/document.xml, …             # обычный OOXML (Word открывает после очистки)
//! customXml/item1.xml              # rde:doc (наша схема)
//! customXml/itemProps1.xml         # datastoreItem со schemaRef на https://rde.dev/ns/doc
//! customXml/_rels/item1.xml.rels   # связь item1 → itemProps1
//! ```
//! Word может удалить неизвестные customXml при сохранении — это и есть сигнал внешнего
//! редактирования: `check_external_edit` видит отсутствие `rde:doc` (или расхождение `body_hash`).
//!
//! Инженерное решение M3: части rde:doc **не** referenced из `word/document.xml.rels` (Word не
//! ругается на лишние parts; при пересохранении Word их удаляет — детект). RDE-экспорт `.docx`
//! всегда вырезает свои customXml, поэтому получается чистый OOXML.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use quick_xml::Reader;
use quick_xml::events::Event;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;
use zip::ZipWriter;
use zip::write::SimpleFileOptions;

pub const RDE_NS: &str = "https://rde.dev/ns/doc";
pub const SCHEMA_VERSION: &str = "1";
pub const RDE_DOC_PART: &str = "customXml/item1.xml";
pub const RDE_ITEM_PROPS: &str = "customXml/itemProps1.xml";
pub const RDE_ITEM_RELS: &str = "customXml/_rels/item1.xml.rels";
pub const CONTENT_TYPES: &str = "[Content_Types].xml";
pub const DOCUMENT_XML: &str = "word/document.xml";

/// Блок документа, которым оперирует RDE (стабильный id + путь в WordCraft-модели).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdeBlock {
    pub id: String,
    /// Путь блока в модели WordCraft (`Path::to_string`, например `5` или `3/0/0/1`).
    pub path: String,
    /// `heading1|heading2|heading|paragraph|figure|caption|table`.
    pub kind: String,
    pub text: String,
}

/// Вставка RDE (рисунок/подпись/таблица), привязанная к блоку.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdeInsert {
    pub id: String,
    pub block: String,
    /// `figure|caption|table|note`.
    pub kind: String,
    /// URI цели: `rde://artifact/<id>@<out_hash>/<selector>` или `rde://source/<id>`.
    pub target: String,
    pub caption: String,
    #[serde(default)]
    pub number: Option<u32>,
    #[serde(default)]
    pub hash: Option<String>,
}

/// Сводка ревью (комментарии/правки; полноценно — M5).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdeReview {
    pub open: u32,
    pub resolved: u32,
}

/// RDE-часть пакета (`customXml/item1.xml`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdeDoc {
    pub schema_version: String,
    pub doc_id: String,
    pub template: String,
    /// sha256 байтов `word/document.xml` на момент последнего сохранения RDE.
    pub body_hash: String,
    pub blocks: Vec<RdeBlock>,
    pub inserts: Vec<RdeInsert>,
    pub review: RdeReview,
    pub updated_unix: u64,
}

impl Default for RdeDoc {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION.to_owned(),
            doc_id: ulid_like_id(),
            template: String::new(),
            body_hash: String::new(),
            blocks: Vec::new(),
            inserts: Vec::new(),
            review: RdeReview::default(),
            updated_unix: 0,
        }
    }
}

fn ulid_like_id() -> String {
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{nanos:032x}")
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Метаданные части пакета.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartInfo {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// Информация о пакете (docx или doc.rde).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PackageInfo {
    pub parts: Vec<PartInfo>,
    pub has_rde_doc: bool,
    pub body_hash: String,
}

impl PackageInfo {
    pub fn has_part(&self, name: &str) -> bool {
        self.parts.iter().any(|part| part.name == name)
    }

    pub fn part(&self, name: &str) -> Option<&PartInfo> {
        self.parts.iter().find(|part| part.name == name)
    }

    pub fn part_names(&self) -> Vec<&str> {
        self.parts.iter().map(|part| part.name.as_str()).collect()
    }
}

#[derive(Debug)]
struct PackageEntries {
    entries: BTreeMap<String, Vec<u8>>,
}

fn read_package(path: &Path) -> Result<PackageEntries, String> {
    let file = File::open(path).map_err(|e| format!("открыть пакет {}: {e}", path.display()))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("zip {}: {e}", path.display()))?;
    let mut entries = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| format!("часть zip #{index}: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_owned();
        let mut data = Vec::with_capacity(entry.size() as usize);
        entry.read_to_end(&mut data).map_err(|e| format!("прочитать {name}: {e}"))?;
        entries.insert(name, data);
    }
    Ok(PackageEntries { entries })
}

fn write_package_entries(entries: &BTreeMap<String, Vec<u8>>, out: &Path) -> Result<(), String> {
    let file = File::create(out).map_err(|e| format!("создать {}: {e}", out.display()))?;
    let mut writer = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (name, data) in entries {
        writer.start_file(name, options).map_err(|e| format!("zip start {name}: {e}"))?;
        writer.write_all(data).map_err(|e| format!("zip write {name}: {e}"))?;
    }
    writer.finish().map_err(|e| format!("zip finish: {e}"))?;
    Ok(())
}

fn info_from_entries(entries: &BTreeMap<String, Vec<u8>>) -> PackageInfo {
    let parts = entries
        .iter()
        .map(|(name, data)| PartInfo { name: name.clone(), size: data.len() as u64, sha256: rde_hash::sha256(data).hex() })
        .collect();
    let body_hash = entries
        .get(DOCUMENT_XML)
        .map(|data| rde_hash::sha256(data).hex())
        .unwrap_or_default();
    PackageInfo { parts, has_rde_doc: entries.contains_key(RDE_DOC_PART), body_hash }
}

/// Информация о пакете без его изменения.
pub fn package_info(path: &Path) -> Result<PackageInfo, String> {
    Ok(info_from_entries(&read_package(path)?.entries))
}

/// Есть ли в пакете RDE-часть.
pub fn has_rde_doc(path: &Path) -> Result<bool, String> {
    Ok(read_package(path)?.entries.contains_key(RDE_DOC_PART))
}

/// Прочитать `rde:doc` из `.doc.rde`.
pub fn load_rde_doc(path: &Path) -> Result<RdeDoc, String> {
    let entries = read_package(path)?.entries;
    let xml = entries
        .get(RDE_DOC_PART)
        .ok_or_else(|| format!("в пакете {} нет RDE-части ({RDE_DOC_PART}) — документ редактировался вне RDE?", path.display()))?;
    parse_rde_doc(&String::from_utf8_lossy(xml))
}

/// Отрендерить `rde:doc` в XML.
pub fn render_rde_doc(doc: &RdeDoc) -> String {
    let mut out = String::with_capacity(1024 + doc.blocks.len() * 120);
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n");
    out.push_str(&format!(
        "<rde:doc xmlns:rde=\"{RDE_NS}\" schema_version=\"{}\" doc_id=\"{}\" template=\"{}\" body_hash=\"{}\" updated_unix=\"{}\">\n",
        escape_attr(&doc.schema_version),
        escape_attr(&doc.doc_id),
        escape_attr(&doc.template),
        escape_attr(&doc.body_hash),
        doc.updated_unix
    ));
    out.push_str("  <rde:blocks>\n");
    for block in &doc.blocks {
        out.push_str(&format!(
            "    <rde:block id=\"{}\" path=\"{}\" kind=\"{}\" text=\"{}\"/>\n",
            escape_attr(&block.id),
            escape_attr(&block.path),
            escape_attr(&block.kind),
            escape_attr(&block.text)
        ));
    }
    out.push_str("  </rde:blocks>\n");
    out.push_str("  <rde:inserts>\n");
    for insert in &doc.inserts {
        out.push_str(&format!(
            "    <rde:insert id=\"{}\" block=\"{}\" kind=\"{}\" target=\"{}\" caption=\"{}\"",
            escape_attr(&insert.id),
            escape_attr(&insert.block),
            escape_attr(&insert.kind),
            escape_attr(&insert.target),
            escape_attr(&insert.caption)
        ));
        if let Some(number) = insert.number {
            out.push_str(&format!(" number=\"{number}\""));
        }
        if let Some(hash) = &insert.hash {
            out.push_str(&format!(" hash=\"{}\"", escape_attr(hash)));
        }
        out.push_str("/>\n");
    }
    out.push_str("  </rde:inserts>\n");
    out.push_str(&format!("  <rde:review open=\"{}\" resolved=\"{}\"/>\n", doc.review.open, doc.review.resolved));
    out.push_str("</rde:doc>\n");
    out
}

fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' => out.push(' '),
            '\r' => {}
            other => out.push(other),
        }
    }
    out
}

fn unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Разобрать `rde:doc` XML (терпимо к префиксам пространств имён).
pub fn parse_rde_doc(xml: &str) -> Result<RdeDoc, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut doc = RdeDoc::default();
    let mut seen_doc = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) | Ok(Event::Empty(event)) => {
                let name = local_name(event.name().as_ref());
                match name.as_str() {
                    "doc" => {
                        seen_doc = true;
                        for attr in event.attributes().flatten() {
                            let key = local_name(attr.key.as_ref());
                            let value = unescape(attr.value.as_ref());
                            match key.as_str() {
                                "schema_version" => doc.schema_version = value,
                                "doc_id" => doc.doc_id = value,
                                "template" => doc.template = value,
                                "body_hash" => doc.body_hash = value,
                                "updated_unix" => doc.updated_unix = value.parse().unwrap_or(0),
                                _ => {}
                            }
                        }
                    }
                    "block" => {
                        let mut block = RdeBlock { id: String::new(), path: String::new(), kind: "paragraph".into(), text: String::new() };
                        for attr in event.attributes().flatten() {
                            let key = local_name(attr.key.as_ref());
                            let value = unescape(attr.value.as_ref());
                            match key.as_str() {
                                "id" => block.id = value,
                                "path" => block.path = value,
                                "kind" => block.kind = value,
                                "text" => block.text = value,
                                _ => {}
                            }
                        }
                        doc.blocks.push(block);
                    }
                    "insert" => {
                        let mut insert = RdeInsert { id: String::new(), block: String::new(), kind: "figure".into(), target: String::new(), caption: String::new(), number: None, hash: None };
                        for attr in event.attributes().flatten() {
                            let key = local_name(attr.key.as_ref());
                            let value = unescape(attr.value.as_ref());
                            match key.as_str() {
                                "id" => insert.id = value,
                                "block" => insert.block = value,
                                "kind" => insert.kind = value,
                                "target" => insert.target = value,
                                "caption" => insert.caption = value,
                                "number" => insert.number = value.parse().ok(),
                                "hash" => insert.hash = Some(value),
                                _ => {}
                            }
                        }
                        doc.inserts.push(insert);
                    }
                    "review" => {
                        for attr in event.attributes().flatten() {
                            let key = local_name(attr.key.as_ref());
                            let value = unescape(attr.value.as_ref());
                            match key.as_str() {
                                "open" => doc.review.open = value.parse().unwrap_or(0),
                                "resolved" => doc.review.resolved = value.parse().unwrap_or(0),
                                _ => {}
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("rde:doc XML: {error}")),
            _ => {}
        }
    }
    if !seen_doc {
        return Err("в XML нет корневого элемента rde:doc".to_owned());
    }
    if doc.schema_version.is_empty() {
        doc.schema_version = SCHEMA_VERSION.to_owned();
    }
    Ok(doc)
}

fn local_name(raw: &str) -> String {
    match raw.rsplit_once(':') {
        Some((_, local)) => local.to_owned(),
        None => raw.to_owned(),
    }
}

fn strip_content_type_overrides(xml: &str, part_name: &str) -> String {
    // Удаляем <Override PartName="/customXml/..." .../> точечно, не трогая остальные части.
    let needle = format!("PartName=\"/{part_name}\"");
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<Override") {
        let Some(end) = rest[start..].find("/>") else {
            out.push_str(rest);
            return out;
        };
        let element = &rest[start..start + end + 2];
        if !element.contains(&needle) {
            out.push_str(&rest[..start + end + 2]);
        }
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    out
}

fn ensure_override(xml: &str, part_name: &str, content_type: &str) -> String {
    if xml.contains(&format!("PartName=\"/{part_name}\"")) {
        return xml.to_owned();
    }
    let override_tag = format!("<Override PartName=\"/{part_name}\" ContentType=\"{content_type}\"/>");
    match xml.rfind("</Types>") {
        Some(index) => {
            let mut out = String::with_capacity(xml.len() + override_tag.len());
            out.push_str(&xml[..index]);
            out.push_str(&override_tag);
            out.push_str(&xml[index..]);
            out
        }
        None => format!("{xml}{override_tag}"),
    }
}

/// Части, которые RDE добавляет/вырезает при работе с пакетом.
fn is_rde_part(name: &str) -> bool {
    name == RDE_DOC_PART || name == RDE_ITEM_PROPS || name == RDE_ITEM_RELS
}

fn item_props_xml(doc_id: &str) -> String {
    let item_id = rde_hash::sha256(doc_id.as_bytes()).hex();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <ds:datastoreItem xmlns:ds=\"http://schemas.openxmlformats.org/officeDocument/2006/customXml\" \
         ds:itemID=\"{{{item_id}}}\"><ds:schemaRefs><ds:schemaRef ds:uri=\"{RDE_NS}\"/></ds:schemaRefs></ds:datastoreItem>\n"
    )
}

fn item_rels_xml() -> String {
    "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
     <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\
     <Relationship Id=\"rId1\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXmlProps\" \
     Target=\"itemProps1.xml\"/></Relationships>\n"
        .to_owned()
}

/// Извлечь чистый `.docx` из `.doc.rde` (вырезав RDE-части).
pub fn extract_docx(package: &Path, out_docx: &Path) -> Result<PackageInfo, String> {
    let mut entries = read_package(package)?.entries;
    for name in entries.keys().filter(|name| is_rde_part(name.as_str())).cloned().collect::<Vec<_>>() {
        entries.remove(&name);
    }
    for removed in [RDE_DOC_PART, RDE_ITEM_PROPS] {
        if let Some(content_types) = entries.get_mut(CONTENT_TYPES) {
            let text = String::from_utf8_lossy(content_types).to_string();
            *content_types = strip_content_type_overrides(&text, removed).into_bytes();
        }
    }
    if !entries.contains_key(DOCUMENT_XML) {
        return Err("в пакете нет word/document.xml".to_owned());
    }
    let info = info_from_entries(&entries);
    write_package_entries(&entries, out_docx)?;
    Ok(info)
}

/// Собрать `.doc.rde` из чистого `.docx` + `rde:doc`.
pub fn write_package(clean_docx: &Path, rde_doc: &RdeDoc, out_package: &Path) -> Result<PackageInfo, String> {
    let mut entries = read_package(clean_docx)?.entries;
    if !entries.contains_key(DOCUMENT_XML) {
        return Err(format!("{} не похож на .docx: нет word/document.xml", clean_docx.display()));
    }
    for name in entries.keys().filter(|name| is_rde_part(name.as_str())).cloned().collect::<Vec<_>>() {
        entries.remove(&name);
    }
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(rde_doc).into_bytes());
    entries.insert(RDE_ITEM_PROPS.to_owned(), item_props_xml(&rde_doc.doc_id).into_bytes());
    entries.insert(RDE_ITEM_RELS.to_owned(), item_rels_xml().into_bytes());
    if let Some(content_types) = entries.get_mut(CONTENT_TYPES) {
        let text = String::from_utf8_lossy(content_types).to_string();
        let text = ensure_override(
            &text,
            RDE_ITEM_PROPS,
            "application/vnd.openxmlformats-officedocument.customXmlProperties+xml",
        );
        *content_types = text.into_bytes();
    }
    let info = info_from_entries(&entries);
    // Атомарная подмена: temp рядом → rename.
    let tmp = out_package.with_extension("doc.rde.tmp");
    write_package_entries(&entries, &tmp)?;
    fs::rename(&tmp, out_package).map_err(|e| format!("заменить {}: {e}", out_package.display()))?;
    Ok(info)
}

/// Результат проверки «документ редактировали вне RDE?».
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditVerdict {
    /// RDE-часть на месте и body_hash совпадает.
    NotModified,
    /// RDE-часть есть, но документ менялся (body_hash не тот) — правка, вероятно, внешняя.
    ExternalEdit,
    /// RDE-часть исчезла — Word/LibreOffice пересохранили документ и потеряли customXml.
    MissingRdeDoc,
}

#[derive(Clone, Debug)]
pub struct ExternalEditCheck {
    pub verdict: EditVerdict,
    pub body_hash_matches: Option<bool>,
    pub rde_doc: Option<RdeDoc>,
    pub current_body_hash: String,
}

/// Проверить, менялся ли документ вне RDE (по наличию rde:doc и body_hash).
pub fn check_external_edit(package: &Path) -> Result<ExternalEditCheck, String> {
    let entries = read_package(package)?.entries;
    let current_body_hash = entries
        .get(DOCUMENT_XML)
        .map(|data| rde_hash::sha256(data).hex())
        .unwrap_or_default();
    let Some(xml) = entries.get(RDE_DOC_PART) else {
        return Ok(ExternalEditCheck {
            verdict: EditVerdict::MissingRdeDoc,
            body_hash_matches: None,
            rde_doc: None,
            current_body_hash,
        });
    };
    let rde_doc = parse_rde_doc(&String::from_utf8_lossy(xml))?;
    let matches = !rde_doc.body_hash.is_empty() && rde_doc.body_hash == current_body_hash;
    Ok(ExternalEditCheck {
        verdict: if matches { EditVerdict::NotModified } else { EditVerdict::ExternalEdit },
        body_hash_matches: Some(matches),
        rde_doc: Some(rde_doc),
        current_body_hash,
    })
}

// ── работа с WordCraft-моделью ────────────────────────────────────────────────

fn open_engine(path: &Path) -> Result<wordcraft_engine::doc::Document, String> {
    let region = wordcraft_engine::Region::from_tag("ru-RU");
    wordcraft_engine::io::open_path_in(path, None, &region).map(|(doc, _encrypted)| doc)
}

/// Открыть документ (чистый `.docx` или `.doc.rde`) в модели WordCraft.
/// Для `.doc.rde` предварительно извлекается чистый `.docx` во временный файл.
pub fn open_document(path: &Path) -> Result<(wordcraft_engine::doc::Document, bool), String> {
    if is_rde_package(path) {
        let temp = temp_docx_path("open");
        extract_docx(path, &temp)?;
        let doc = open_engine(&temp)?;
        let _ = fs::remove_file(&temp);
        Ok((doc, true))
    } else {
        open_engine(path).map(|doc| (doc, false))
    }
}

/// Является ли файл `.doc.rde`-пакетом.
pub fn is_rde_package(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some("rde")
}

fn temp_docx_path(tag: &str) -> PathBuf {
    let unique = format!("rde-doc-{tag}-{}-{}.docx", std::process::id(), now_unix());
    std::env::temp_dir().join(unique)
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        format!("{}…", text.chars().take(max).collect::<String>())
    }
}

/// Извлечь блоки из модели WordCraft (то же представление, что хранится в rde:doc).
pub fn extract_blocks(doc: &wordcraft_engine::doc::Document) -> Vec<RdeBlock> {
    let story = wordcraft_engine::doc::StoryRef::Body;
    let mut blocks = Vec::new();
    for (index, path) in doc.para_paths(story).iter().enumerate() {
        let Some(paragraph) = doc.para(story, path) else { continue };
        let images = paragraph
            .objects
            .iter()
            .filter(|object| matches!(object, wordcraft_engine::doc::InlineObject::Image { .. }))
            .count();
        if paragraph.text.trim().is_empty() && images == 0 {
            continue;
        }
        let style = paragraph.props.style.clone().unwrap_or_default().to_ascii_lowercase();
        let kind = if style == "1" || style == "heading1" || style.starts_with("heading 1") {
            "heading1"
        } else if style == "2" || style == "heading2" || style.starts_with("heading 2") {
            "heading2"
        } else if style.starts_with("heading") {
            "heading"
        } else if images > 0 {
            "figure"
        } else if paragraph.text.trim_start().starts_with("Рисунок") || paragraph.text.trim_start().starts_with("Таблица") {
            "caption"
        } else {
            "paragraph"
        };
        blocks.push(RdeBlock {
            id: format!("b{}", index + 1),
            path: path.to_string(),
            kind: kind.to_owned(),
            text: truncate(paragraph.text.trim(), 400),
        });
    }
    blocks
}

/// Номер рисунка/таблицы из подписи: «Рисунок 3 — ...» → 3.
pub fn caption_number(text: &str) -> Option<u32> {
    let rest = text.trim_start().strip_prefix("Рисунок")?;
    let rest = rest.trim_start();
    let digits: String = rest.chars().take_while(|ch| ch.is_ascii_digit()).collect();
    digits.parse().ok()
}

fn caption_suffix(text: &str) -> String {
    let rest = text.trim_start().strip_prefix("Рисунок").unwrap_or(text);
    let rest = rest.trim_start();
    let after_digits: String = rest.chars().skip_while(|ch| ch.is_ascii_digit()).collect();
    after_digits.trim_start_matches([' ', '—', '–', '-', ':', '.']).trim().to_owned()
}

/// Собрать `rde:doc` из чистого `.docx`: блоки, рисунки-вставки, подписи, body_hash.
pub fn build_rde_doc(clean_docx: &Path, doc_id: &str, template: &str) -> Result<RdeDoc, String> {
    let entries = read_package(clean_docx)?.entries;
    let body_hash = entries
        .get(DOCUMENT_XML)
        .map(|data| rde_hash::sha256(data).hex())
        .ok_or_else(|| "в .docx нет word/document.xml".to_owned())?;
    let doc = open_engine(clean_docx)?;
    let blocks = extract_blocks(&doc);
    let inserts = build_inserts(&blocks);
    Ok(RdeDoc {
        schema_version: SCHEMA_VERSION.to_owned(),
        doc_id: doc_id.to_owned(),
        template: template.to_owned(),
        body_hash,
        blocks,
        inserts,
        review: RdeReview::default(),
        updated_unix: now_unix(),
    })
}

/// Автоматические вставки: для каждого рисунка — figure, для подписи — caption с номером.
fn build_inserts(blocks: &[RdeBlock]) -> Vec<RdeInsert> {
    let mut inserts = Vec::new();
    let mut counter = 0u32;
    for (index, block) in blocks.iter().enumerate() {
        if block.kind == "figure" {
            counter += 1;
            let caption_block = blocks.get(index + 1).filter(|next| next.kind == "caption");
            let caption = caption_block.map(|next| next.text.clone()).unwrap_or_default();
            inserts.push(RdeInsert {
                id: format!("i{}", inserts.len() + 1),
                block: block.id.clone(),
                kind: "figure".to_owned(),
                target: String::new(),
                caption: caption.clone(),
                number: caption_number(&caption).or(Some(counter)),
                hash: None,
            });
        } else if block.kind == "caption" {
            inserts.push(RdeInsert {
                id: format!("i{}", inserts.len() + 1),
                block: block.id.clone(),
                kind: "caption".to_owned(),
                target: String::new(),
                caption: block.text.clone(),
                number: caption_number(&block.text),
                hash: None,
            });
        }
    }
    inserts
}

/// Изменение подписи при обновлении документа.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptionChange {
    pub block_path: String,
    pub old_text: String,
    pub new_text: String,
}

/// Перенумеровать подписи «Рисунок N — …» в порядке документа (в модели WordCraft).
pub fn renumber_captions(doc: &mut wordcraft_engine::doc::Document) -> Vec<CaptionChange> {
    let story = wordcraft_engine::doc::StoryRef::Body;
    let paths = doc.para_paths(story);
    let mut changes = Vec::new();
    let mut number = 0u32;
    for path in paths {
        let Some(paragraph) = doc.para(story, &path) else { continue };
        if !paragraph.text.trim_start().starts_with("Рисунок") {
            continue;
        }
        number += 1;
        let old_text = paragraph.text.clone();
        let new_text = format!("Рисунок {number} — {}", caption_suffix(&old_text));
        if old_text != new_text {
            // Меняем текст через редактирующее API: правка `text` напрямую не сериализуется в OOXML.
            if let Ok(paragraph) = doc.para_mut(story, &path) {
                let len = paragraph.len();
                let props = paragraph.props_of_char(0).clone();
                if paragraph.delete_except(0, len, &[]).is_ok() {
                    let _ = paragraph.insert_text(0, &new_text, &props);
                }
            }
            changes.push(CaptionChange { block_path: path.to_string(), old_text, new_text });
        }
    }
    changes
}

/// Обновить `rde:doc` по актуальному состоянию `.docx`: body_hash, блоки, подписи вставок.
pub fn refresh_rde_doc(clean_docx: &Path, rde_doc: &mut RdeDoc) -> Result<(), String> {
    let entries = read_package(clean_docx)?.entries;
    let body_hash = entries
        .get(DOCUMENT_XML)
        .map(|data| rde_hash::sha256(data).hex())
        .ok_or_else(|| "в .docx нет word/document.xml".to_owned())?;
    let doc = open_engine(clean_docx)?;
    rde_doc.blocks = extract_blocks(&doc);
    rde_doc.body_hash = body_hash;
    rde_doc.updated_unix = now_unix();
    // Обновляем кэш подписей у вставок по свежим блокам.
    let captions: BTreeMap<&str, &str> = rde_doc.blocks.iter().map(|b| (b.id.as_str(), b.text.as_str())).collect();
    for insert in &mut rde_doc.inserts {
        if let Some(text) = captions.get(insert.block.as_str()) {
            if insert.kind == "caption" || insert.kind == "figure" {
                insert.caption = (*text).to_owned();
                insert.number = caption_number(text).or(insert.number);
            }
        }
    }
    Ok(())
}

/// Отчёт «Обновить документ» (кнопка в UI): перенумерация подписей + пересохранение пакета.
#[derive(Clone, Debug, Default)]
pub struct DocumentUpdateReport {
    pub changes: Vec<CaptionChange>,
    pub package: Option<PackageInfo>,
    pub rde_doc: Option<RdeDoc>,
}

/// Обновить `.doc.rde`: извлечь `.docx`, перенумеровать подписи, сохранить обратно в пакет.
pub fn update_document_package(package: &Path) -> Result<DocumentUpdateReport, String> {
    let mut rde_doc = load_rde_doc(package)?;
    let temp_docx = temp_docx_path("update");
    extract_docx(package, &temp_docx)?;
    let opened = open_engine(&temp_docx);
    let mut doc = match opened {
        Ok(doc) => doc,
        Err(error) => {
            let _ = fs::remove_file(&temp_docx);
            return Err(error);
        }
    };
    let changes = renumber_captions(&mut doc);
    let save_result = wordcraft_engine::io::save_path(&temp_docx, &doc);
    if let Err(error) = save_result {
        let _ = fs::remove_file(&temp_docx);
        return Err(format!("сохранить обновлённый .docx: {error}"));
    }
    refresh_rde_doc(&temp_docx, &mut rde_doc)?;
    let info = write_package(&temp_docx, &rde_doc, package)?;
    let _ = fs::remove_file(&temp_docx);
    Ok(DocumentUpdateReport { changes, package: Some(info), rde_doc: Some(rde_doc) })
}

/// Сохранить отредактированный WordCraft-документ как `.doc.rde` (RDE — владелец сохранения).
///
/// Сохраняет OOXML в `editor_docx_path`, затем пересобирает `rde:doc` (сохраняя doc_id/шаблон/
/// вставки/ревью) и атомарно заменяет пакет.
pub fn save_engine_document(editor_docx: &Path, package: &Path, doc_id_hint: &str, template: &str) -> Result<RdeDoc, String> {
    let mut rde_doc = match load_rde_doc(package) {
        Ok(doc) => doc,
        Err(_) => RdeDoc {
            schema_version: SCHEMA_VERSION.to_owned(),
            doc_id: if doc_id_hint.is_empty() { ulid_like_id() } else { doc_id_hint.to_owned() },
            template: template.to_owned(),
            ..RdeDoc::default()
        },
    };
    if !template.is_empty() {
        // Явно заданный шаблон (например, «Оформить по ГОСТ») важнее сохранённого в пакете.
        rde_doc.template = template.to_owned();
    }
    refresh_rde_doc(editor_docx, &mut rde_doc)?;
    write_package(editor_docx, &rde_doc, package)?;
    Ok(rde_doc)
}

// ── M5: ревью (комментарии Word, импорт внешнего ревью) ────────────────────────

/// Комментарий Word из `word/comments.xml`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommentInfo {
    pub author: String,
    pub initials: String,
    pub date: String,
    pub text: String,
    pub resolved: bool,
}

/// Прочитать комментарии Word из `.docx` или `.doc.rde`.
pub fn comments(path: &Path) -> Result<Vec<CommentInfo>, String> {
    let (docx_path, temporary) = if is_rde_package(path) {
        let temp = temp_docx_path("comments");
        extract_docx(path, &temp)?;
        (temp, true)
    } else {
        (path.to_path_buf(), false)
    };
    let result = (|| {
        let entries = read_package(&docx_path)?.entries;
        let Some(xml) = entries.get("word/comments.xml") else {
            return Ok(Vec::new());
        };
        parse_comments_xml(&String::from_utf8_lossy(xml))
    })();
    if temporary {
        let _ = fs::remove_file(&docx_path);
    }
    result
}

fn parse_comments_xml(xml: &str) -> Result<Vec<CommentInfo>, String> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut comments = Vec::new();
    let mut current: Option<CommentInfo> = None;
    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "comment" {
                    let mut info = CommentInfo { author: String::new(), initials: String::new(), date: String::new(), text: String::new(), resolved: false };
                    for attr in event.attributes().flatten() {
                        let key = local_name(attr.key.as_ref());
                        let value = unescape(attr.value.as_ref());
                        match key.as_str() {
                            "author" => info.author = value,
                            "initials" => info.initials = value,
                            "date" => info.date = value,
                            "done" | "resolved" => info.resolved = value == "1" || value.eq_ignore_ascii_case("true"),
                            _ => {}
                        }
                    }
                    current = Some(info);
                }
            }
            Ok(Event::Text(text)) => {
                if let Some(info) = current.as_mut() {
                    let value = text.xml10_content().to_string();
                    if !value.trim().is_empty() {
                        if !info.text.is_empty() {
                            info.text.push(' ');
                        }
                        info.text.push_str(value.trim());
                    }
                }
            }
            Ok(Event::End(event)) => {
                if local_name(event.name().as_ref()) == "comment" {
                    if let Some(info) = current.take() {
                        comments.push(info);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(format!("comments.xml: {error}")),
            _ => {}
        }
    }
    Ok(comments)
}

/// Отчёт импорта внешнего ревью.
#[derive(Clone, Debug)]
pub struct ReviewImportReport {
    pub comments_before: usize,
    pub comments_after: usize,
    pub external_edit: EditVerdict,
    pub doc_id: String,
    pub package: PackageInfo,
}

/// Импорт внешнего ревью: принять OOXML-файл (например, после правок в Word) как новую версию
/// документа, сохранив RDE-метаданные (doc_id/шаблон/вставки) и обновив сводку ревью.
pub fn import_external_review(package: &Path, external_docx: &Path) -> Result<ReviewImportReport, String> {
    let mut rde_doc = load_rde_doc(package)?;
    let comments_before = comments(package)?.len();
    let external_info = package_info(external_docx)?;
    if !external_info.has_part(DOCUMENT_XML) {
        return Err(format!("{} не является .docx (нет word/document.xml)", external_docx.display()));
    }
    let comments_after = comments(external_docx)?.len();
    let external_edit = check_external_edit(external_docx)
        .map(|check| check.verdict)
        .unwrap_or(EditVerdict::ExternalEdit);
    refresh_rde_doc(external_docx, &mut rde_doc)?;
    rde_doc.review = RdeReview { open: comments_after as u32, resolved: rde_doc.review.resolved };
    let package_info = write_package(external_docx, &rde_doc, package)?;
    Ok(ReviewImportReport {
        comments_before,
        comments_after,
        external_edit,
        doc_id: rde_doc.doc_id.clone(),
        package: package_info,
    })
}
