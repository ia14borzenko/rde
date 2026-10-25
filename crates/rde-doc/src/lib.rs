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
pub const ROOT_RELS: &str = "_rels/.rels";
pub const DOC_PROPS_CUSTOM: &str = "docProps/custom.xml";
/// Пространство имён Word 2010 для `w14:paraId` (стабильные анкоры блоков).
pub const W14_NS: &str = "http://schemas.microsoft.com/office/word/2010/wordml";
/// Префикс tag content-control (`w:sdt`) для вставок RDE.
pub const INSERT_TAG_PREFIX: &str = "rde:";

/// Блок документа, которым оперирует RDE (стабильный id + путь в WordCraft-модели).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdeBlock {
    pub id: String,
    /// Путь блока в модели WordCraft (`Path::to_string`, например `5` или `3/0/0/1`).
    pub path: String,
    /// `heading1|heading2|heading|paragraph|figure|caption|table`.
    pub kind: String,
    pub text: String,
    /// Стабильный анкор Word `w14:paraId` (R-DOC-04): карта block_id ↔ paraId.
    #[serde(default)]
    pub para_id: Option<String>,
}

/// Вставка RDE (рисунок/подпись/таблица), привязанная к блоку.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdeInsert {
    pub id: String,
    /// Блок-анкор (id блока) или paraId.
    pub block: String,
    /// `figure|caption|table|note|value`.
    pub kind: String,
    /// URI цели: `rde://artifact/<id>@<out_hash>/<selector>` или `rde://source/<id>`.
    pub target: String,
    pub caption: String,
    #[serde(default)]
    pub number: Option<u32>,
    #[serde(default)]
    pub hash: Option<String>,
    /// Кэш значения вставки (R-DOC-11): то, что показано в документе.
    #[serde(default)]
    pub cached: String,
    /// Селектор значения внутри выхода артефакта (JSON pointer `#/a/b` или путь файла).
    #[serde(default)]
    pub selector: String,
    /// `text|number|figure`.
    #[serde(default)]
    pub value_kind: String,
    /// Анкор `w14:paraId` content-control'а вставки (если есть).
    #[serde(default)]
    pub para_id: Option<String>,
    /// Стиль абзаца вставки (`w:pStyle`, например `Normal`) из текущего шаблона документа.
    #[serde(default)]
    pub style: String,
    /// Формат отображения значения: `auto|raw|int|dec:N|sci:N|percent:N` (R-DOC-11).
    #[serde(default)]
    pub format: String,
    /// Подсказка rich-форматирования значения (`italic`, `italic-subscript` — для констант вида
    /// `R_н`, R-ART-09); пусто — обычный текст.
    #[serde(default)]
    pub run_style: String,
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
            "    <rde:block id=\"{}\" path=\"{}\" kind=\"{}\" text=\"{}\"",
            escape_attr(&block.id),
            escape_attr(&block.path),
            escape_attr(&block.kind),
            escape_attr(&block.text)
        ));
        if let Some(para_id) = &block.para_id {
            out.push_str(&format!(" para_id=\"{}\"", escape_attr(para_id)));
        }
        out.push_str("/>\n");
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
        if !insert.cached.is_empty() {
            out.push_str(&format!(" cached=\"{}\"", escape_attr(&insert.cached)));
        }
        if !insert.selector.is_empty() {
            out.push_str(&format!(" selector=\"{}\"", escape_attr(&insert.selector)));
        }
        if !insert.value_kind.is_empty() {
            out.push_str(&format!(" value_kind=\"{}\"", escape_attr(&insert.value_kind)));
        }
        if let Some(para_id) = &insert.para_id {
            out.push_str(&format!(" para_id=\"{}\"", escape_attr(para_id)));
        }
        if !insert.style.is_empty() {
            out.push_str(&format!(" style=\"{}\"", escape_attr(&insert.style)));
        }
        if !insert.format.is_empty() {
            out.push_str(&format!(" format=\"{}\"", escape_attr(&insert.format)));
        }
        if !insert.run_style.is_empty() {
            out.push_str(&format!(" run_style=\"{}\"", escape_attr(&insert.run_style)));
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
                        let mut block = RdeBlock { id: String::new(), path: String::new(), kind: "paragraph".into(), text: String::new(), para_id: None };
                        for attr in event.attributes().flatten() {
                            let key = local_name(attr.key.as_ref());
                            let value = unescape(attr.value.as_ref());
                            match key.as_str() {
                                "id" => block.id = value,
                                "path" => block.path = value,
                                "kind" => block.kind = value,
                                "text" => block.text = value,
                                "para_id" => block.para_id = Some(value),
                                _ => {}
                            }
                        }
                        doc.blocks.push(block);
                    }
                    "insert" => {
                        let mut insert = RdeInsert {
                            id: String::new(),
                            block: String::new(),
                            kind: "figure".into(),
                            target: String::new(),
                            caption: String::new(),
                            number: None,
                            hash: None,
                            cached: String::new(),
                            selector: String::new(),
                            value_kind: String::new(),
                            para_id: None,
                            style: String::new(),
                            format: String::new(),
            run_style: String::new(),
                        };
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
                                "cached" => insert.cached = value,
                                "selector" => insert.selector = value,
                                "value_kind" => insert.value_kind = value,
                                "para_id" => insert.para_id = Some(value),
                                "style" => insert.style = value,
                                "format" => insert.format = value,
                                "run_style" => insert.run_style = value,
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

// ── §8.5: wiring пакета (root rels, tombstone, paraId, sdt-вставки) ─────────

/// Добавить relationship из корня пакета на `customXml/item1.xml` (R-DOC-02).
fn ensure_root_customxml_relationship(entries: &mut BTreeMap<String, Vec<u8>>) {
    let target = "customXml/item1.xml";
    let existing = entries.get(ROOT_RELS).map(|bytes| String::from_utf8_lossy(bytes).to_string()).unwrap_or_else(|| {
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>\n"
            .to_owned()
    });
    if existing.contains(&format!("Target=\"{target}\"")) {
        entries.insert(ROOT_RELS.to_owned(), existing.into_bytes());
        return;
    }
    let relationship = format!(
        "<Relationship Id=\"rIdRdeCustomXml\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/customXml\" Target=\"{target}\"/>"
    );
    let updated = match existing.rfind("</Relationships>") {
        Some(index) => {
            let mut out = String::with_capacity(existing.len() + relationship.len());
            out.push_str(&existing[..index]);
            out.push_str(&relationship);
            out.push_str(&existing[index..]);
            out
        }
        None => format!("{existing}{relationship}"),
    };
    entries.insert(ROOT_RELS.to_owned(), updated.into_bytes());
}

fn strip_root_customxml_relationship(entries: &mut BTreeMap<String, Vec<u8>>) {
    let Some(bytes) = entries.get(ROOT_RELS) else { return };
    let text = String::from_utf8_lossy(bytes).to_string();
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(start) = rest.find("<Relationship") {
        let Some(end) = rest[start..].find("/>") else {
            out.push_str(rest);
            rest = "";
            break;
        };
        let element = &rest[start..start + end + 2];
        if !element.contains("Target=\"customXml/item1.xml\"") {
            out.push_str(element);
        }
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    entries.insert(ROOT_RELS.to_owned(), out.into_bytes());
}

fn tombstone_xml(rde_doc: &RdeDoc) -> String {
    let parts = [RDE_DOC_PART, RDE_ITEM_PROPS, RDE_ITEM_RELS].join(",");
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
         <Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/custom-properties\" \
         xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\">\
         <property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"2\" name=\"rdeDocId\"><vt:lpwstr>{}</vt:lpwstr></property>\
         <property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"3\" name=\"rdeSchema\"><vt:lpwstr>rde/doc@{}</vt:lpwstr></property>\
         <property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"4\" name=\"rdeBodyHash\"><vt:lpwstr>{}</vt:lpwstr></property>\
         <property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"5\" name=\"rdeParts\"><vt:lpwstr>{parts}</vt:lpwstr></property>\
         </Properties>\n",
        escape_attr(&rde_doc.doc_id),
        escape_attr(&rde_doc.schema_version),
        escape_attr(&rde_doc.body_hash)
    )
}

/// Записать/обновить `docProps/custom.xml`-надгробие (R-DOC-03): RDE-части видны Word как
/// свойства документа, а при потере customXml остаётся след для диагностики.
fn upsert_custom_props(entries: &mut BTreeMap<String, Vec<u8>>, rde_doc: &RdeDoc) {
    let existing = entries
        .get(DOC_PROPS_CUSTOM)
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .unwrap_or_else(|| {
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
             <Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/custom-properties\" \
             xmlns:vt=\"http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes\">\n</Properties>\n"
                .to_owned()
        });
    let mut cleaned = existing;
    for name in ["rdeDocId", "rdeSchema", "rdeBodyHash", "rdeParts"] {
        cleaned = strip_custom_property(&cleaned, name);
    }
    let parts = [RDE_DOC_PART, RDE_ITEM_PROPS, RDE_ITEM_RELS].join(",");
    let properties = format!(
        "<property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"2\" name=\"rdeDocId\"><vt:lpwstr>{}</vt:lpwstr></property>\
         <property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"3\" name=\"rdeSchema\"><vt:lpwstr>rde/doc@{}</vt:lpwstr></property>\
         <property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"4\" name=\"rdeBodyHash\"><vt:lpwstr>{}</vt:lpwstr></property>\
         <property fmtid=\"{{D5CDD505-2E9C-101B-9397-08002B2CF9AE}}\" pid=\"5\" name=\"rdeParts\"><vt:lpwstr>{parts}</vt:lpwstr></property>",
        escape_attr(&rde_doc.doc_id),
        escape_attr(&rde_doc.schema_version),
        escape_attr(&rde_doc.body_hash)
    );
    let updated = match cleaned.rfind("</Properties>") {
        Some(index) => {
            let mut out = String::with_capacity(cleaned.len() + properties.len());
            out.push_str(&cleaned[..index]);
            out.push_str(&properties);
            out.push_str(&cleaned[index..]);
            out
        }
        None => format!("{cleaned}{properties}"),
    };
    entries.insert(DOC_PROPS_CUSTOM.to_owned(), updated.into_bytes());
    if let Some(content_types) = entries.get_mut(CONTENT_TYPES) {
        let text = String::from_utf8_lossy(content_types).to_string();
        let text = ensure_override(
            &text,
            DOC_PROPS_CUSTOM,
            "application/vnd.openxmlformats-officedocument.custom-properties+xml",
        );
        *content_types = text.into_bytes();
    }
    let _ = tombstone_xml(rde_doc);
}

fn strip_custom_property(xml: &str, name: &str) -> String {
    let needle = format!("name=\"{name}\"");
    let mut out = String::with_capacity(xml.len());
    let mut rest = xml;
    while let Some(start) = rest.find("<property") {
        let Some(rel_end) = rest[start..].find("</property>") else {
            out.push_str(rest);
            return out;
        };
        let element = &rest[start..start + rel_end + "</property>".len()];
        if !element.contains(&needle) {
            out.push_str(element);
        }
        rest = &rest[start + rel_end + "</property>".len()..];
    }
    out.push_str(rest);
    out
}

/// Убрать надгробие RDE из `docProps/custom.xml` (экспорт чистого `.docx`).
fn strip_custom_props(entries: &mut BTreeMap<String, Vec<u8>>) {
    let Some(bytes) = entries.get(DOC_PROPS_CUSTOM) else { return };
    let text = String::from_utf8_lossy(bytes).to_string();
    let mut cleaned = text;
    for name in ["rdeDocId", "rdeSchema", "rdeBodyHash", "rdeParts"] {
        cleaned = strip_custom_property(&cleaned, name);
    }
    let has_properties = cleaned.contains("<property");
    if has_properties {
        entries.insert(DOC_PROPS_CUSTOM.to_owned(), cleaned.into_bytes());
    } else {
        entries.remove(DOC_PROPS_CUSTOM);
        if let Some(content_types) = entries.get_mut(CONTENT_TYPES) {
            let text = String::from_utf8_lossy(content_types).to_string();
            *content_types = strip_content_type_overrides(&text, DOC_PROPS_CUSTOM).into_bytes();
        }
        if let Some(rels) = entries.get_mut(ROOT_RELS) {
            let text = String::from_utf8_lossy(rels).to_string();
            let mut out = String::with_capacity(text.len());
            let mut rest = text.as_str();
            while let Some(start) = rest.find("<Relationship") {
                let Some(end) = rest[start..].find("/>") else {
                    out.push_str(rest);
                    rest = "";
                    break;
                };
                let element = &rest[start..start + end + 2];
                if !element.contains("Target=\"docProps/custom.xml\"") {
                    out.push_str(element);
                }
                rest = &rest[start + end + 2..];
            }
            out.push_str(rest);
            *rels = out.into_bytes();
        }
    }
}

fn para_id_for(doc_id: &str, index: usize) -> String {
    let hash = rde_hash::sha256(format!("{doc_id}:para:{index}").as_bytes()).hex();
    hash.chars().take(8).collect::<String>().to_uppercase()
}

/// Проставить `w14:paraId` всем абзацам `word/document.xml` (R-DOC-04) и вернуть id
/// «видимых» абзацев (с текстом или рисунком) в порядке документа.
fn inject_para_ids(document_xml: &[u8], doc_id: &str) -> Result<(Vec<u8>, Vec<String>), String> {
    use quick_xml::events::{BytesEnd, BytesStart, BytesText, Event as XEvent};
    use quick_xml::Writer;
    let text = String::from_utf8_lossy(document_xml).to_string();
    let mut reader = Reader::from_str(&text);
    reader.config_mut().trim_text(false);
    let mut writer = Writer::new(Vec::new());
    let mut visible_ids = Vec::new();
    let mut para_index = 0usize;
    let mut in_para = false;
    let mut current_id = String::new();
    let mut has_content = false;
    loop {
        match reader.read_event() {
            Ok(XEvent::Start(event)) => {
                let name = local_name(event.name().as_ref());
                let name_owned = event.name().as_ref().to_string();
                let mut out = BytesStart::new(name_owned.as_str());
                for attr in event.attributes().flatten() {
                    // w14:paraId перезаписываем ниже (не дублируем атрибут).
                    if attr.key.as_ref() == "w14:paraId" {
                        continue;
                    }
                    out.push_attribute((attr.key.as_ref(), attr.value.as_ref()));
                }
                if name == "document" && !text.contains("xmlns:w14=") {
                    out.push_attribute(("xmlns:w14", W14_NS));
                }
                if name == "drawing" {
                    has_content = true;
                }
                if name == "p" {
                    let existing = event
                        .try_get_attribute("w14:paraId")
                        .ok()
                        .flatten()
                        .map(|attr| attr.value.as_ref().to_string());
                    let id = existing.unwrap_or_else(|| para_id_for(doc_id, para_index));
                    out.push_attribute(("w14:paraId", id.as_str()));
                    current_id = id;
                    in_para = true;
                    has_content = false;
                    para_index += 1;
                }
                writer.write_event(XEvent::Start(out)).map_err(|e| format!("document.xml: {e}"))?;
            }
            Ok(XEvent::Empty(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "drawing" {
                    has_content = true;
                }
                let name_owned = event.name().as_ref().to_string();
                let mut out = BytesStart::new(name_owned.as_str());
                for attr in event.attributes().flatten() {
                    out.push_attribute((attr.key.as_ref(), attr.value.as_ref()));
                }
                writer.write_event(XEvent::Empty(out)).map_err(|e| format!("document.xml: {e}"))?;
            }
            Ok(XEvent::Text(event)) => {
                if in_para {
                    let value = event.xml10_content().to_string();
                    if !value.trim().is_empty() {
                        has_content = true;
                    }
                }
                writer
                    .write_event(XEvent::Text(BytesText::new(&event.xml10_content())))
                    .map_err(|e| format!("document.xml: {e}"))?;
            }
            Ok(XEvent::End(event)) => {
                let name = local_name(event.name().as_ref());
                if name == "p" && in_para {
                    if has_content {
                        visible_ids.push(current_id.clone());
                    }
                    in_para = false;
                }
                let end_name = event.name().as_ref().to_string();
                writer
                    .write_event(XEvent::End(BytesEnd::new(end_name.as_str())))
                    .map_err(|e| format!("document.xml: {e}"))?;
            }
            Ok(XEvent::Eof) => break,
            Ok(other) => {
                writer.write_event(other).map_err(|e| format!("document.xml: {e}"))?;
            }
            Err(error) => return Err(format!("document.xml: {error}")),
        }
    }
    Ok((writer.into_inner(), visible_ids))
}

/// Назначить блокам `para_id` по порядку «видимых» абзацев документа.
fn assign_para_ids(rde_doc: &mut RdeDoc, visible_ids: &[String]) {
    if rde_doc.blocks.len() != visible_ids.len() {
        // Разное представление «видимых» абзацев — анкоры не назначаем (не ломаем документ).
        return;
    }
    for (block, para_id) in rde_doc.blocks.iter_mut().zip(visible_ids.iter()) {
        block.para_id = Some(para_id.clone());
    }
    for insert in &mut rde_doc.inserts {
        if insert.para_id.is_none() {
            insert.para_id = rde_doc
                .blocks
                .iter()
                .find(|block| block.id == insert.block)
                .and_then(|block| block.para_id.clone());
        }
    }
}

/// Внутренние `w:pPr`/`w:rPr` обычного текста документа (для вставок): берём первый
/// содержательный абзац тела (не наш sdt), чтобы вставка выглядела как основной текст шаблона
/// (в демо — Times New Roman 14 pt, а не `docDefaults` Aptos).
fn body_formatting(entries: &BTreeMap<String, Vec<u8>>) -> (String, String) {
    let Some(bytes) = entries.get(DOCUMENT_XML) else { return (String::new(), String::new()) };
    let xml = String::from_utf8_lossy(bytes).to_string();
    let mut rest = xml.as_str();
    while let Some(start) = rest.find("<w:p") {
        let Some(end_rel) = rest[start..].find("</w:p>") else { break };
        let paragraph = &rest[start..start + end_rel + "</w:p>".len()];
        rest = &rest[start + end_rel + "</w:p>".len()..];
        if paragraph.contains("<w:sdt") || paragraph.contains("<w:drawing") {
            continue;
        }
        let ppr = extract_inner(paragraph, "<w:pPr", "</w:pPr>").unwrap_or_default();
        let Some(rpr) = extract_inner(paragraph, "<w:rPr", "</w:rPr>") else { continue };
        if !rpr.contains("w:rFonts") {
            continue;
        }
        // Пропускаем заголовки/титул/оглавление/подписи и жирный текст: нужен именно
        // формат обычного (body) абзаца, как в ГОСТ-шаблоне.
        let styled = ["Heading", "Title", "TOC", "Caption", "Subtitle"]
            .iter()
            .any(|prefix| ppr.contains(&format!("w:pStyle w:val=\"{prefix}")));
        let bold = rpr.contains("<w:b/>") || rpr.contains("<w:bCs/>");
        if styled || bold {
            continue;
        }
        return (ppr, rpr);
    }
    (String::new(), String::new())
}

/// Внутреннее содержимое первого элемента `open_tag ... close_tag` в строке.
fn extract_inner(xml: &str, open_tag: &str, close_tag: &str) -> Option<String> {
    let start = xml.find(open_tag)?;
    let open_end = xml[start..].find('>')? + start + 1;
    let close = xml[open_end..].find(close_tag)? + open_end;
    Some(xml[open_end..close].to_owned())
}

/// `w:rPr` стиля из `word/styles.xml` (с учётом `w:basedOn`), для выбранного пользователем стиля.
fn style_run_props(entries: &BTreeMap<String, Vec<u8>>, style_id: &str) -> Option<String> {
    let bytes = entries.get("word/styles.xml")?;
    let xml = String::from_utf8_lossy(bytes).to_string();
    let mut current = style_id.to_owned();
    for _ in 0..8 {
        let marker = format!("w:styleId=\"{current}\"");
        let index = xml.find(&marker)?;
        let start = xml[..index].rfind("<w:style ")?;
        let end = xml[start..].find("</w:style>")? + start + "</w:style>".len();
        let element = &xml[start..end];
        if let Some(props) = extract_inner(element, "<w:rPr", "</w:rPr>") {
            return Some(props);
        }
        // Нет прямых rPr — идём по basedOn.
        let based = extract_attr(element, "w:basedOn", "w:val")?;
        current = based;
    }
    None
}

fn extract_attr(xml: &str, element: &str, attr: &str) -> Option<String> {
    let start = xml.find(element)?;
    let end = xml[start..].find('>')? + start;
    let tag = &xml[start..end];
    let marker = format!("{attr}=\"");
    let index = tag.find(&marker)? + marker.len();
    let rest = &tag[index..];
    let close = rest.find('"')?;
    Some(rest[..close].to_owned())
}

/// Форматирование вставки: явный стиль пользователя (pStyle + его rPr) или обычный текст документа.
/// Следующий свободный id вставки (`v3`, `v4`, …) — не переиспользуем номера после удалений.
fn next_insert_id(inserts: &[RdeInsert], document_xml: &str) -> String {
    let mut max = 0u32;
    for insert in inserts {
        if let Some(number) = insert.id.strip_prefix('v').and_then(|rest| rest.parse::<u32>().ok()) {
            max = max.max(number);
        }
    }
    // В теле могут остаться w:sdt с rde:<id> после удаления записей — их id тоже заняты.
    let mut rest = document_xml;
    while let Some(index) = rest.find("w:tag w:val=\"rde:v") {
        let after = &rest[index + "w:tag w:val=\"rde:v".len()..];
        let digits: String = after.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(number) = digits.parse::<u32>() {
            max = max.max(number);
        }
        rest = after;
    }
    format!("v{}", max + 1)
}

fn insert_formatting(
    entries: &BTreeMap<String, Vec<u8>>,
    style: &str,
) -> (String, String) {
    let (body_ppr, body_rpr) = body_formatting(entries);
    let wrap_ppr = |inner: String| if inner.is_empty() { String::new() } else { format!("<w:pPr>{inner}</w:pPr>") };
    let wrap_rpr = |inner: String| if inner.is_empty() { String::new() } else { format!("<w:rPr>{inner}</w:rPr>") };
    if style.trim().is_empty() {
        return (wrap_ppr(body_ppr), wrap_rpr(body_rpr));
    }
    let ppr = format!("<w:pPr><w:pStyle w:val=\"{}\"/></w:pPr>", escape_attr(style.trim()));
    let rpr = wrap_rpr(style_run_props(entries, style.trim()).unwrap_or(body_rpr));
    (ppr, rpr)
}

fn sdt_paragraph_xml(insert: &RdeInsert, paragraph_props: &str, run_props: &str) -> String {
    // Подпись/метка вставки — обычный run перед content-control, значение — внутри sdt.
    let label = if insert.caption.is_empty() {
        String::new()
    } else {
        format!("<w:r>{run_props}<w:t xml:space=\"preserve\">{}: </w:t></w:r>", escape_attr(&insert.caption))
    };
    let value_runs = rich_runs(&insert.cached, &insert.run_style, run_props);
    format!(
        "<w:p>{paragraph_props}{label}<w:sdt><w:sdtPr><w:tag w:val=\"{prefix}{id}\"/><w:alias w:val=\"{alias}\"/></w:sdtPr>\
         <w:sdtContent>{value_runs}</w:sdtContent></w:sdt></w:p>",
        prefix = INSERT_TAG_PREFIX,
        id = escape_attr(&insert.id),
        alias = escape_attr(if insert.caption.is_empty() { &insert.kind } else { &insert.caption }),
    )
}

/// Стиль абзаца из `word/styles.xml` (для выбора стиля вставки пользователем).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StyleInfo {
    pub id: String,
    pub name: String,
    /// `paragraph` | `character` | `table` | `numbering`.
    pub kind: String,
    /// Стиль абзаца по умолчанию (`w:default="1"`).
    pub is_default: bool,
}

/// Список стилей пакета (из `word/styles.xml`).
pub fn list_document_styles(package: &Path) -> Result<Vec<StyleInfo>, String> {
    let entries = read_package(package)?.entries;
    let Some(bytes) = entries.get("word/styles.xml") else { return Ok(Vec::new()) };
    let xml = String::from_utf8_lossy(bytes).to_string();
    let mut styles = Vec::new();
    let mut rest = xml.as_str();
    while let Some(start) = rest.find("<w:style ") {
        let Some(end) = rest[start..].find("</w:style>") else { break };
        let element = &rest[start..start + end + "</w:style>".len()];
        let attr = |name: &str| -> Option<String> {
            let marker = format!("w:{name}=\"");
            let index = element.find(&marker)?;
            let value = &element[index + marker.len()..];
            let close = value.find('"')?;
            Some(value[..close].to_owned())
        };
        let id = attr("styleId").unwrap_or_default();
        let kind = attr("type").unwrap_or_else(|| "paragraph".to_owned());
        let is_default = element.contains("w:default=\"1\"");
        let name = element
            .find("<w:name ")
            .and_then(|index| {
                let value = &element[index..];
                let marker = "w:val=\"";
                let value_start = value.find(marker)? + marker.len();
                let close = value[value_start..].find('"')?;
                Some(value[value_start..value_start + close].to_owned())
            })
            .unwrap_or_else(|| id.clone());
        if !id.is_empty() {
            styles.push(StyleInfo { id, name, kind, is_default });
        }
        rest = &rest[start + end + "</w:style>".len()..];
    }
    Ok(styles)
}

/// Стиль абзаца по умолчанию (для вставок): `w:default="1"`, иначе `Normal`, иначе пусто.
pub fn default_paragraph_style(package: &Path) -> String {
    let Ok(styles) = list_document_styles(package) else { return String::new() };
    if let Some(style) = styles.iter().find(|style| style.kind == "paragraph" && style.is_default) {
        return style.id.clone();
    }
    if styles.iter().any(|style| style.kind == "paragraph" && style.id == "Normal") {
        return "Normal".to_owned();
    }
    styles.iter().find(|style| style.kind == "paragraph").map(|style| style.id.clone()).unwrap_or_default()
}

/// Форматирование значения вставки (R-DOC-11): «компьютерный» вид — лишь один из вариантов.
/// `auto` — умный вид (целые без дробной части, иначе до 4 значащих), `raw` — как в артефакте,
/// `int` — целое, `dec:N` — N знаков после запятой, `sci:N` — стандартный вид `m·10^e`,
/// `percent:N` — проценты с N знаками.
pub fn format_value(raw: &str, format: &str) -> String {
    let format = format.trim();
    if format.is_empty() || format == "raw" {
        return raw.to_owned();
    }
    let Ok(value) = raw.trim().parse::<f64>() else {
        return raw.to_owned();
    };
    if !value.is_finite() {
        return raw.to_owned();
    }
    let precision = |prefix: &str, default: usize| -> usize {
        format
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix(':'))
            .and_then(|rest| rest.parse::<usize>().ok())
            .unwrap_or(default)
            .min(10)
    };
    match format {
        "int" => format!("{value:.0}"),
        _ if format.starts_with("dec") => {
            let digits = precision("dec", 2);
            format!("{value:.digits$}")
        }
        _ if format.starts_with("sci") => {
            let digits = precision("sci", 3);
            let text = format!("{value:.digits$e}"); // 1.235e3
            match text.split_once('e') {
                Some((mantissa, exponent)) => {
                    let exponent: i32 = exponent.parse().unwrap_or(0);
                    format!("{mantissa}·10^{exponent}")
                }
                None => text,
            }
        }
        _ if format.starts_with("percent") => {
            let digits = precision("percent", 1);
            format!("{:.digits$} %", value * 100.0)
        }
        _ => format_display_number_auto(value),
    }
}

fn format_display_number_auto(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let abs = value.abs();
    if (1e-3..1e6).contains(&abs) {
        let mut text = format!("{value:.4}");
        while text.ends_with('0') && text.contains('.') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
        text
    } else {
        let text = format!("{value:.4e}");
        match text.split_once('e') {
            Some((mantissa, exponent)) => {
                let exponent: i32 = exponent.parse().unwrap_or(0);
                format!("{mantissa}·10^{exponent}")
            }
            None => text,
        }
    }
}

/// Задать формат отображения вставки (R-DOC-11): `auto|raw|int|dec:N|sci:N|percent:N`.
pub fn set_insert_format(package: &Path, insert_id: &str, format: &str) -> Result<RdeDoc, String> {
    let mut entries = read_package(package)?.entries;
    let mut rde_doc = load_rde_doc_from_entries(&entries)?;
    let mut found = false;
    for insert in &mut rde_doc.inserts {
        if insert.id == insert_id {
            insert.format = format.to_owned();
            found = true;
        }
    }
    if !found {
        return Err(format!("вставка {insert_id} не найдена"));
    }
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
    write_entries_atomic(&entries, package)?;
    Ok(rde_doc)
}

/// Удалить записи вставок из `rde:doc`, не трогая тело документа (очистка авто-вставок миграции).
pub fn drop_insert_records(package: &Path, ids: &[String]) -> Result<RdeDoc, String> {
    let mut entries = read_package(package)?.entries;
    let mut rde_doc = load_rde_doc_from_entries(&entries)?;
    let before = rde_doc.inserts.len();
    rde_doc.inserts.retain(|insert| !ids.iter().any(|id| id == &insert.id));
    if rde_doc.inserts.len() != before {
        entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
        write_entries_atomic(&entries, package)?;
    }
    Ok(rde_doc)
}

/// Задать подсказку rich-форматирования значения вставки (`italic`, `italic-subscript`).
pub fn set_insert_run_style(package: &Path, insert_id: &str, run_style: &str) -> Result<RdeDoc, String> {
    let mut entries = read_package(package)?.entries;
    let mut rde_doc = load_rde_doc_from_entries(&entries)?;
    let Some(insert) = rde_doc.inserts.iter_mut().find(|insert| insert.id == insert_id) else {
        return Err(format!("вставка {insert_id} не найдена"));
    };
    insert.run_style = run_style.to_owned();
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
    write_entries_atomic(&entries, package)?;
    Ok(rde_doc)
}

/// Сменить стиль абзаца вставки (пользователь выбирает стиль из шаблона): правит `w:pStyle`
/// в документе и `rde:doc` (R-DOC-11/25).
pub fn apply_insert_style(package: &Path, insert_id: &str, style: &str) -> Result<PackageInfo, String> {
    let mut entries = read_package(package)?.entries;
    let mut rde_doc = load_rde_doc_from_entries(&entries)?;
    let (paragraph_props, run_props) = insert_formatting(&entries, style);
    let document = entries.get(DOCUMENT_XML).ok_or_else(|| "в пакете нет word/document.xml".to_owned())?;
    let document_xml = String::from_utf8_lossy(document).to_string();
    let tag = format!("<w:tag w:val=\"{INSERT_TAG_PREFIX}{insert_id}\"");
    let tag_index = document_xml.find(&tag).ok_or_else(|| format!("вставка {insert_id} не найдена в документе"))?;
    let paragraph_start = document_xml[..tag_index].rfind("<w:p").ok_or_else(|| "не найден абзац вставки".to_owned())?;
    let paragraph_end = document_xml[tag_index..]
        .find("</w:p>")
        .map(|index| tag_index + index + "</w:p>".len())
        .ok_or_else(|| "не найден конец абзаца вставки".to_owned())?;
    let paragraph = &document_xml[paragraph_start..paragraph_end];
    if paragraph.contains("<w:drawing") {
        // Рисунок: стиль абзаца не меняем (иначе можно потерять drawing).
        return Ok(info_from_entries(&entries));
    }
    let mut rebuilt = String::new();
    // Открывающий тег <w:p ...>.
    let open_end = paragraph.find('>').ok_or_else(|| "некорректный тег абзаца".to_owned())? + 1;
    rebuilt.push_str(&paragraph[..open_end]);
    rebuilt.push_str(&paragraph_props);
    let mut body_owned = paragraph[open_end..paragraph.len() - "</w:p>".len()].to_owned();
    // Убрать старый pPr.
    if let Some(ppr_start) = body_owned.find("<w:pPr") {
        if let Some(ppr_end_rel) = body_owned[ppr_start..].find("</w:pPr>") {
            let tail = body_owned[ppr_start + ppr_end_rel + "</w:pPr>".len()..].to_owned();
            body_owned.truncate(ppr_start);
            body_owned.push_str(&tail);
        }
    }
    // Заменить rPr у всех run'ов на форматирование обычного текста.
    let mut out_body = String::new();
    let body: &str = &body_owned;
    let mut rest = body;
    while let Some(rpr_start) = rest.find("<w:rPr") {
        let Some(open_end) = rest[rpr_start..].find('>') else { break };
        let inner_start = rpr_start + open_end + 1;
        let Some(close_rel) = rest[inner_start..].find("</w:rPr>") else { break };
        out_body.push_str(&rest[..rpr_start]);
        out_body.push_str(&run_props);
        rest = &rest[inner_start + close_rel + "</w:rPr>".len()..];
    }
    out_body.push_str(rest);
    rebuilt.push_str(&out_body);
    rebuilt.push_str("</w:p>");
    let new_xml = format!("{}{}{}", &document_xml[..paragraph_start], rebuilt, &document_xml[paragraph_end..]);
    for insert in &mut rde_doc.inserts {
        if insert.id == insert_id {
            insert.style = style.to_owned();
        }
    }
    rde_doc.body_hash = rde_hash::sha256(new_xml.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), new_xml.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
    upsert_custom_props(&mut entries, &rde_doc);
    let info = info_from_entries(&entries);
    write_entries_atomic(&entries, package)?;
    Ok(info)
}

/// `w:rPr` абзаца вставки (чтобы при обновлении значения сохранить шрифт/размер обычного текста).
fn paragraph_run_props(xml: &str, insert_id: &str) -> String {
    let tag = format!("<w:tag w:val=\"{INSERT_TAG_PREFIX}{insert_id}\"");
    let Some(tag_index) = xml.find(&tag) else { return String::new() };
    let Some(start) = xml[..tag_index].rfind("<w:p") else { return String::new() };
    let Some(end_rel) = xml[start..].find("</w:p>") else { return String::new() };
    let paragraph = &xml[start..start + end_rel];
    let mut rest = paragraph;
    while let Some(rpr_start) = rest.find("<w:rPr") {
        let Some(open_end) = rest[rpr_start..].find('>') else { break };
        let inner_start = rpr_start + open_end + 1;
        let Some(close_rel) = rest[inner_start..].find("</w:rPr>") else { break };
        let props = &rest[inner_start..inner_start + close_rel];
        if props.contains("w:rFonts") {
            return format!("<w:rPr>{props}</w:rPr>");
        }
        rest = &rest[inner_start + close_rel + "</w:rPr>".len()..];
    }
    String::new()
}

/// Разбить значение на run'ы по подсказке rich-форматирования (`R_н` → «R» + подстрочная «н»).
fn rich_runs(value: &str, run_style: &str, run_props: &str) -> String {
    if run_style.is_empty() {
        return format!("<w:r>{run_props}<w:t xml:space=\"preserve\">{}</w:t></w:r>", escape_attr(value));
    }
    let italic = run_style.contains("italic");
    let subscript = run_style.contains("subscript");
    let make_props = |extra: &str| {
        if run_props.is_empty() && extra.is_empty() {
            String::new()
        } else {
            let inner = extract_inner(run_props, "<w:rPr", "</w:rPr>").unwrap_or_default();
            format!("<w:rPr>{inner}{extra}</w:rPr>")
        }
    };
    let base_extra = if italic { "<w:i/><w:iCs/>" } else { "" };
    if subscript {
        // display вида `R_н`: до «_» — обычная часть, после — подстрочная.
        if let Some((base, sub)) = value.split_once('_') {
            let mut out = String::new();
            if !base.is_empty() {
                out.push_str(&format!(
                    "<w:r>{}{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                    make_props(base_extra),
                    "",
                    escape_attr(base)
                ));
            }
            let sub_extra = if italic { "<w:i/><w:iCs/><w:vertAlign w:val=\"subscript\"/>" } else { "<w:vertAlign w:val=\"subscript\"/>" };
            out.push_str(&format!(
                "<w:r>{}{}<w:t xml:space=\"preserve\">{}</w:t></w:r>",
                make_props(sub_extra),
                "",
                escape_attr(sub)
            ));
            return out;
        }
        let extra = if italic { "<w:i/><w:iCs/><w:vertAlign w:val=\"subscript\"/>" } else { "<w:vertAlign w:val=\"subscript\"/>" };
        return format!("<w:r>{}{}<w:t xml:space=\"preserve\">{}</w:t></w:r>", make_props(extra), "", escape_attr(value));
    }
    format!("<w:r>{}{}<w:t xml:space=\"preserve\">{}</w:t></w:r>", make_props(base_extra), "", escape_attr(value))
}

/// Заменить текст внутри `w:sdt` вставки (по тегу `rde:<id>`) на новое значение.
fn replace_sdt_text(xml: &str, insert: &RdeInsert, value: &str) -> Option<String> {
    let tag = format!("<w:tag w:val=\"{INSERT_TAG_PREFIX}{}\"", insert.id);
    let tag_index = xml.find(&tag)?;
    let sdt_start = xml[..tag_index].rfind("<w:sdt>")?;
    let content_start = xml[sdt_start..].find("<w:sdtContent>")? + sdt_start + "<w:sdtContent>".len();
    let content_end = xml[content_start..].find("</w:sdtContent>")? + content_start;
    let run_props = paragraph_run_props(xml, &insert.id);
    let replacement = rich_runs(value, &insert.run_style, &run_props);
    let mut out = String::with_capacity(xml.len() + replacement.len());
    out.push_str(&xml[..content_start]);
    out.push_str(&replacement);
    out.push_str(&xml[content_end..]);
    Some(out)
}

/// Вставить фрагмент после блока `anchor_block` (0-based индекс блока тела) или в конец.
fn insert_fragment(xml: &str, fragment: &str, anchor_block: Option<usize>) -> String {
    match anchor_block {
        Some(index) => insert_after_block(xml, fragment, index),
        None => append_paragraph(xml, fragment),
    }
}

/// Вставить фрагмент после N-го блока тела (абзацы/таблицы/content-control на верхнем уровне).
fn insert_after_block(xml: &str, fragment: &str, index: usize) -> String {
    let Some(body_open) = xml.find("<w:body>") else { return append_paragraph(xml, fragment) };
    let mut depth = 1usize;
    let mut block = 0usize;
    let mut pos = body_open + "<w:body>".len();
    while pos < xml.len() {
        let Some(rel) = xml[pos..].find('<') else { break };
        let start = pos + rel;
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
        let name = tag.trim_start_matches('/').split([' ', '\t', '\n', '/']).next().unwrap_or("");
        if depth == 1 && !self_closing && !declaration && matches!(name, "w:p" | "w:tbl" | "w:sdt" | "w:customXml") {
            if block == index + 1 {
                let mut out = String::with_capacity(xml.len() + fragment.len());
                out.push_str(&xml[..start]);
                out.push_str(fragment);
                out.push_str(&xml[start..]);
                return out;
            }
            block += 1;
        }
        if !self_closing && !declaration {
            depth += 1;
        }
        pos = end + 1;
    }
    append_paragraph(xml, fragment)
}

/// Вставить новый абзац в конец тела документа, но **перед** `w:sectPr` (он обязан быть
/// последним элементом `w:body`, иначе парсер WordCraft/Word теряет всё после него).
fn append_paragraph(xml: &str, paragraph: &str) -> String {
    let Some(body_end) = xml.rfind("</w:body>") else { return format!("{xml}{paragraph}") };
    let body = &xml[..body_end];
    let insert_at = body.rfind("<w:sectPr").unwrap_or(body_end);
    let mut out = String::with_capacity(xml.len() + paragraph.len());
    out.push_str(&xml[..insert_at]);
    out.push_str(paragraph);
    out.push_str(&xml[insert_at..]);
    out
}

/// Записать части пакета атомарно (temp + rename).
fn write_entries_atomic(entries: &BTreeMap<String, Vec<u8>>, out: &Path) -> Result<(), String> {
    let tmp = out.with_extension("doc.rde.tmp");
    write_package_entries(entries, &tmp)?;
    fs::rename(&tmp, out).map_err(|e| format!("заменить {}: {e}", out.display()))
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
    strip_root_customxml_relationship(&mut entries);
    strip_custom_props(&mut entries);
    if !entries.contains_key(DOCUMENT_XML) {
        return Err("в пакете нет word/document.xml".to_owned());
    }
    let info = info_from_entries(&entries);
    write_package_entries(&entries, out_docx)?;
    Ok(info)
}

/// Собрать `.doc.rde` из чистого `.docx` + `rde:doc` (§8.5: root rels на customXml, надгробие
/// `docProps/custom.xml`, `w14:paraId`-анкоры блоков). Мутирует `rde_doc` (body_hash/para_id).
pub fn write_package(clean_docx: &Path, rde_doc: &mut RdeDoc, out_package: &Path) -> Result<PackageInfo, String> {
    let mut entries = read_package(clean_docx)?.entries;
    if !entries.contains_key(DOCUMENT_XML) {
        return Err(format!("{} не похож на .docx: нет word/document.xml", clean_docx.display()));
    }
    for name in entries.keys().filter(|name| is_rde_part(name.as_str())).cloned().collect::<Vec<_>>() {
        entries.remove(&name);
    }
    // R-DOC-04: стабильные анкоры w14:paraId; body_hash — уже с анкорами.
    let document_xml = entries.get(DOCUMENT_XML).cloned().unwrap_or_default();
    let (document_xml, visible_ids) = inject_para_ids(&document_xml, &rde_doc.doc_id)?;
    rde_doc.body_hash = rde_hash::sha256(&document_xml).hex();
    assign_para_ids(rde_doc, &visible_ids);
    entries.insert(DOCUMENT_XML.to_owned(), document_xml);
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
    // R-DOC-02/03: relationship из корня пакета + надгробие в docProps/custom.xml.
    ensure_root_customxml_relationship(&mut entries);
    upsert_custom_props(&mut entries, rde_doc);
    let info = info_from_entries(&entries);
    write_entries_atomic(&entries, out_package)?;
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
    // Уникальность обязательна: тесты идут параллельно и могут открывать пакеты одновременно.
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let unique = format!("rde-doc-{tag}-{}-{}-{counter}.docx", std::process::id(), now_unix());
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
            para_id: None,
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
                cached: String::new(),
                selector: String::new(),
                value_kind: "figure".to_owned(),
                para_id: block.para_id.clone(),
                style: String::new(),
                format: String::new(),
                run_style: String::new(),
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
                cached: block.text.clone(),
                selector: String::new(),
                value_kind: "text".to_owned(),
                para_id: block.para_id.clone(),
                style: String::new(),
                format: String::new(),
                run_style: String::new(),
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
    let info = write_package(&temp_docx, &mut rde_doc, package)?;
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
    write_package(editor_docx, &mut rde_doc, package)?;
    Ok(rde_doc)
}

// ── живые вставки (R-DOC-11): sdt content-controls со значением из артефакта ──

fn load_rde_doc_from_entries(entries: &BTreeMap<String, Vec<u8>>) -> Result<RdeDoc, String> {
    let xml = entries
        .get(RDE_DOC_PART)
        .ok_or_else(|| "в пакете нет RDE-части (customXml/item1.xml)".to_owned())?;
    parse_rde_doc(&String::from_utf8_lossy(xml))
}

/// Запрос на вставку значения/рисунка в документ.
#[derive(Clone, Debug)]
pub struct InsertRequest {
    /// `value` (текст/число) или `figure`.
    pub kind: String,
    /// URI цели: `rde://artifact/<id>@<out_hash>/<selector>`.
    pub target: String,
    /// Селектор значения внутри выхода (`#/stats/x1/mean` или путь файла).
    pub selector: String,
    /// `text|number|figure`.
    pub value_kind: String,
    /// Кэш значения, который сразу попадает в документ.
    pub cached: String,
    pub caption: String,
    /// Стиль абзаца (`w:pStyle`), пусто — обычный стиль документа.
    pub style: String,
    /// Формат отображения значения (`auto|raw|int|dec:N|sci:N|percent:N`).
    pub format: String,
    /// Подсказка rich-форматирования значения (`italic`, `italic-subscript`), R-ART-09.
    pub run_style: String,
    /// Блок-якорь вставки (индекс блока тела, 0-based); `None` — в конец документа (P8-B1).
    pub anchor_block: Option<usize>,
}

/// Результат добавления вставки.
#[derive(Clone, Debug)]
pub struct InsertReport {
    pub insert: RdeInsert,
    pub package: PackageInfo,
}

/// Добавить вставку в `.doc.rde`: content-control `w:sdt` с тегом `rde:<id>` в конце тела
/// + запись в `rde:doc` (R-DOC-11). Значение обновляется [`update_insert_values`].
pub fn add_insert(package: &Path, request: InsertRequest) -> Result<InsertReport, String> {
    let mut entries = read_package(package)?.entries;
    let mut rde_doc = load_rde_doc_from_entries(&entries)?;
    let document_source = entries.get(DOCUMENT_XML).map(|bytes| String::from_utf8_lossy(bytes).to_string()).unwrap_or_default();
    let id = next_insert_id(&rde_doc.inserts, &document_source);
    let para_id = para_id_for(&format!("{}:insert:{id}", rde_doc.doc_id), 0);
    // Пустой style = «обычный текст документа» (формат абзаца-образца из тела).
    let style = request.style.clone();
    let (paragraph_props, run_props) = insert_formatting(&entries, &style);
    let insert = RdeInsert {
        id,
        block: String::new(),
        kind: request.kind,
        target: request.target,
        caption: request.caption,
        number: None,
        hash: None,
        cached: request.cached,
        selector: request.selector,
        value_kind: request.value_kind,
        para_id: Some(para_id.clone()),
        style,
        format: request.format,
        run_style: request.run_style,
    };
    let document = entries.get(DOCUMENT_XML).ok_or_else(|| "в пакете нет word/document.xml".to_owned())?;
    let document_xml = String::from_utf8_lossy(document).to_string();
    let paragraph = sdt_paragraph_xml(&insert, &paragraph_props, &run_props)
        .replacen("<w:p>", &format!("<w:p w14:paraId=\"{para_id}\">"), 1);
    let document_xml = insert_fragment(&document_xml, &paragraph, anchor_block);
    rde_doc.inserts.push(insert.clone());
    rde_doc.updated_unix = now_unix();
    rde_doc.body_hash = rde_hash::sha256(document_xml.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
    upsert_custom_props(&mut entries, &rde_doc);
    let info = info_from_entries(&entries);
    write_entries_atomic(&entries, package)?;
    Ok(InsertReport { insert, package: info })
}

/// Отчёт обновления вставок.
#[derive(Clone, Debug)]
pub struct InsertUpdateReport {
    pub updated: Vec<String>,
    pub package: PackageInfo,
    pub rde_doc: RdeDoc,
}

/// Обновить значения вставок из артефактов: текст в `w:sdt` и кэш в `rde:doc` (R-DOC-11/12).
/// `values`: insert_id → новое значение (кэш); hash/URI обновляет вызывающий слой.
pub fn update_insert_values(
    package: &Path,
    values: &BTreeMap<String, String>,
    hashes: &BTreeMap<String, String>,
) -> Result<InsertUpdateReport, String> {
    let mut entries = read_package(package)?.entries;
    let mut rde_doc = load_rde_doc_from_entries(&entries)?;
    let document = entries.get(DOCUMENT_XML).ok_or_else(|| "в пакете нет word/document.xml".to_owned())?;
    let mut document_xml = String::from_utf8_lossy(document).to_string();
    let mut updated = Vec::new();
    for insert in &mut rde_doc.inserts {
        let Some(value) = values.get(&insert.id) else { continue };
        if let Some(new_xml) = replace_sdt_text(&document_xml, insert, value) {
            document_xml = new_xml;
        }
        insert.cached = value.clone();
        if let Some(hash) = hashes.get(&insert.id) {
            insert.hash = Some(hash.clone());
        }
        updated.push(insert.id.clone());
    }
    rde_doc.updated_unix = now_unix();
    rde_doc.body_hash = rde_hash::sha256(document_xml.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
    upsert_custom_props(&mut entries, &rde_doc);
    let info = info_from_entries(&entries);
    write_entries_atomic(&entries, package)?;
    Ok(InsertUpdateReport { updated, package: info, rde_doc })
}

/// Проверить, что вставка актуальна: hash в `rde:doc` совпадает с ожидаемым (out_hash цели).
pub fn insert_is_stale(insert: &RdeInsert, expected_hash: &str) -> bool {
    insert.hash.as_deref() != Some(expected_hash)
}

// ── рисунки из артефактов (R-DOC-11) и транзакционное обновление документа ────

/// Имя корня документа в store: `document/<doc_id>` → tree_hash версии пакета.
pub fn document_root_name(doc_id: &str) -> String {
    format!("document/{doc_id}")
}

fn rels_path_for_document() -> &'static str {
    "word/_rels/document.xml.rels"
}

/// Растеризовать SVG в PNG: WordCraft DocView не умеет SVG (рисует серый прямоугольник),
/// а Word/LO умеют и PNG. Источник истины (target в rde:doc) остаётся SVG-выход артефакта.
pub fn rasterize_svg(svg: &[u8]) -> Result<Vec<u8>, String> {
    let mut options = resvg::usvg::Options::default();
    // Текст в SVG (подписи осей/значения): подгружаем системные шрифты, иначе resvg рисует
    // только геометрию, а текст молча пропадает.
    options.fontdb_mut().load_system_fonts();
    options.fontdb_mut().set_serif_family("Times New Roman");
    options.fontdb_mut().set_sans_serif_family("DejaVu Sans");
    options.fontdb_mut().set_monospace_family("DejaVu Sans Mono");
    let tree = resvg::usvg::Tree::from_data(svg, &options).map_err(|e| format!("SVG не разобран: {e}"))?;
    let size = tree.size().to_int_size();
    let mut pixmap = resvg::tiny_skia::Pixmap::new(size.width(), size.height())
        .ok_or_else(|| format!("SVG-холст {}×{} не создан", size.width(), size.height()))?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    pixmap.encode_png().map_err(|e| format!("SVG → PNG: {e}"))
}

/// Подготовить медиа рисунка для пакета: SVG растеризуется в PNG (иначе DocView показывает
/// серый прямоугольник); PNG/JPEG/GIF сохраняются как есть.
fn prepare_figure_media(ext: &str, bytes: Vec<u8>) -> (String, Vec<u8>) {
    let ext = ext.to_ascii_lowercase();
    if ext == "svg" {
        match rasterize_svg(&bytes) {
            Ok(png) => ("png".to_owned(), png),
            Err(error) => {
                // Не блокируем вставку: Word/LO покажут SVG, DocView — заглушку; причина в stderr.
                eprintln!("RDE: SVG → PNG не удалось ({error}); сохраняем SVG");
                (ext, bytes)
            }
        }
    } else {
        (ext, bytes)
    }
}

fn image_content_type(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

/// Размер изображения в EMU (1 px = 9525 EMU): SVG по width/height, PNG по IHDR.
fn image_extent_emu(bytes: &[u8], ext: &str) -> (u64, u64) {
    let fallback = (400u64 * 9525, 300u64 * 9525);
    if ext.eq_ignore_ascii_case("svg") {
        let text = String::from_utf8_lossy(bytes);
        let parse_attr = |name: &str| -> Option<f64> {
            let index = text.find(&format!("{name}=\""))?;
            let rest = &text[index + name.len() + 2..];
            let end = rest.find('"')?;
            rest[..end].trim_end_matches("px").parse().ok()
        };
        if let (Some(width), Some(height)) = (parse_attr("width"), parse_attr("height")) {
            return ((width * 9525.0) as u64, (height * 9525.0) as u64);
        }
    }
    if ext.eq_ignore_ascii_case("png") && bytes.len() >= 24 && bytes.starts_with(b"\x89PNG") {
        let width = u32::from_be_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]) as u64;
        let height = u32::from_be_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]) as u64;
        if width > 0 && height > 0 {
            return (width * 9525, height * 9525);
        }
    }
    fallback
}

fn drawing_paragraph_xml(insert: &RdeInsert, rel_id: &str, cx: u64, cy: u64, para_id: &str) -> String {
    format!(
        "<w:p w14:paraId=\"{para_id}\"><w:sdt><w:sdtPr><w:tag w:val=\"{prefix}{id}\"/><w:alias w:val=\"{alias}\"/></w:sdtPr>\
         <w:sdtContent><w:r><w:drawing><wp:inline distT=\"0\" distB=\"0\" distL=\"0\" distR=\"0\">\
         <wp:extent cx=\"{cx}\" cy=\"{cy}\"/><wp:docPr id=\"{doc_pr}\" name=\"{alias}\"/>\
         <a:graphic xmlns:a=\"http://schemas.openxmlformats.org/drawingml/2006/main\">\
         <a:graphicData uri=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
         <pic:pic xmlns:pic=\"http://schemas.openxmlformats.org/drawingml/2006/picture\">\
         <pic:nvPicPr><pic:cNvPr id=\"0\" name=\"{alias}\"/><pic:cNvPicPr/></pic:nvPicPr>\
         <pic:blipFill><a:blip r:embed=\"{rel_id}\"/><a:stretch><a:fillRect/></a:stretch></pic:blipFill>\
         <pic:spPr><a:xfrm><a:off x=\"0\" y=\"0\"/><a:ext cx=\"{cx}\" cy=\"{cy}\"/></a:xfrm><a:prstGeom prst=\"rect\"/></pic:spPr>\
         </pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:sdtContent></w:sdt></w:p>",
        prefix = INSERT_TAG_PREFIX,
        id = escape_attr(&insert.id),
        alias = escape_attr(if insert.caption.is_empty() { "рисунок" } else { &insert.caption }),
        doc_pr = para_id.len(),
    )
}

fn next_relationship_id(rels: &str) -> String {
    let mut index = 1u32;
    loop {
        let id = format!("rIdRde{index}");
        if !rels.contains(&format!("Id=\"{id}\"")) {
            return id;
        }
        index += 1;
    }
}

fn add_image_relationship(entries: &mut BTreeMap<String, Vec<u8>>, rel_id: &str, target: &str) -> Result<(), String> {
    let existing = entries
        .get(rels_path_for_document())
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .unwrap_or_else(|| {
            "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n\
             <Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\"/>\n"
                .to_owned()
        });
    let relationship = format!(
        "<Relationship Id=\"{rel_id}\" Type=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships/image\" Target=\"{target}\"/>"
    );
    let updated = match existing.rfind("</Relationships>") {
        Some(index) => {
            let mut out = String::with_capacity(existing.len() + relationship.len());
            out.push_str(&existing[..index]);
            out.push_str(&relationship);
            out.push_str(&existing[index..]);
            out
        }
        None => format!("{existing}{relationship}"),
    };
    entries.insert(rels_path_for_document().to_owned(), updated.into_bytes());
    Ok(())
}

fn ensure_default_content_type(entries: &mut BTreeMap<String, Vec<u8>>, ext: &str) {
    let Some(content_types) = entries.get_mut(CONTENT_TYPES) else { return };
    let text = String::from_utf8_lossy(content_types).to_string();
    if text.contains(&format!("Extension=\"{ext}\"")) {
        return;
    }
    let default = format!("<Default Extension=\"{ext}\" ContentType=\"{}\"/>", image_content_type(ext));
    let updated = match text.rfind("</Types>") {
        Some(index) => {
            let mut out = String::with_capacity(text.len() + default.len());
            out.push_str(&text[..index]);
            out.push_str(&default);
            out.push_str(&text[index..]);
            out
        }
        None => format!("{text}{default}"),
    };
    *content_types = updated.into_bytes();
}

/// XML таблицы Word из строк CSV/данных артефакта (R-DOC-18: границы, головка).
fn table_xml(rows: &[Vec<String>], run_props: &str) -> String {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
    let width = 9000 / columns as i64;
    let mut out = String::from("<w:tbl><w:tblGrid>");
    for _ in 0..columns {
        out.push_str(&format!("<w:gridCol w:w=\"{width}\"/>"));
    }
    out.push_str("</w:tblGrid>");
    for row in rows {
        out.push_str("<w:tr>");
        for column in 0..columns {
            let cell = row.get(column).map(String::as_str).unwrap_or("");
            let props = if run_props.is_empty() { String::new() } else { run_props.to_owned() };
            out.push_str(&format!(
                "<w:tc><w:p><w:r>{props}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:p></w:tc>",
                escape_attr(cell)
            ));
        }
        out.push_str("</w:tr>");
    }
    out.push_str("</w:tbl>");
    out
}
/// Вставить таблицу из данных артефакта (R-DOC-11/18): подпись над таблицей + `w:tbl`.
pub fn add_table_insert(package: &Path, request: InsertRequest, rows: &[Vec<String>]) -> Result<InsertReport, String> {
    let mut entries = read_package(package)?.entries;
    let mut rde_doc = load_rde_doc_from_entries(&entries)?;
    let document_source = entries.get(DOCUMENT_XML).map(|bytes| String::from_utf8_lossy(bytes).to_string()).unwrap_or_default();
    let id = next_insert_id(&rde_doc.inserts, &document_source);
    let para_id = para_id_for(&format!("{}:table:{id}", rde_doc.doc_id), 0);
    let (paragraph_props, run_props) = insert_formatting(&entries, &request.style);
    let caption = if request.caption.is_empty() { "Таблица".to_owned() } else { request.caption.clone() };
    let insert = RdeInsert {
        id: id.clone(),
        block: String::new(),
        kind: "table".to_owned(),
        target: request.target.clone(),
        caption: caption.clone(),
        number: None,
        hash: None,
        cached: format!("{}×{}", rows.len(), rows.iter().map(Vec::len).max().unwrap_or(0)),
        selector: request.selector.clone(),
        value_kind: "table".to_owned(),
        para_id: Some(para_id.clone()),
        style: request.style.clone(),
        format: String::new(),
        run_style: String::new(),
    };
    rde_doc.inserts.push(insert.clone());
    let mut document_xml = {
        let document = entries.get(DOCUMENT_XML).ok_or_else(|| "в пакете нет word/document.xml".to_owned())?;
        String::from_utf8_lossy(document).to_string()
    };
    let caption_paragraph = format!(
        "<w:p w14:paraId=\"{para_id}\">{paragraph_props}<w:r>{run_props}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
        escape_attr(&caption)
    );
    let mut fragment = caption_paragraph;
    fragment.push_str(&table_xml(rows, &run_props));
    document_xml = insert_fragment(&document_xml, &fragment, request.anchor_block);
    rde_doc.body_hash = rde_hash::sha256(document_xml.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
    upsert_custom_props(&mut entries, &rde_doc);
    write_entries_atomic(&entries, package)?;
    Ok(InsertReport { insert, package: info_from_entries(&entries) })
}

/// Обновить таблицы вставок из свежих данных артефакта (R-DOC-18).
pub fn update_table_inserts(package: &Path, tables: &BTreeMap<String, Vec<Vec<String>>>) -> Result<PackageInfo, String> {
    let mut entries = read_package(package)?.entries;
    let mut rde_doc = load_rde_doc_from_entries(&entries)?;
    let mut document_xml = {
        let document = entries.get(DOCUMENT_XML).ok_or_else(|| "в пакете нет word/document.xml".to_owned())?;
        String::from_utf8_lossy(document).to_string()
    };
    let mut changed = false;
    for insert in &mut rde_doc.inserts {
        let Some(rows) = tables.get(&insert.id) else { continue };
        let Some(para_id) = insert.para_id.clone() else { continue };
        let anchor = format!("<w:p w14:paraId=\"{para_id}\"");
        let Some(paragraph_start) = document_xml.find(&anchor) else { continue };
        let Some(paragraph_end_rel) = document_xml[paragraph_start..].find("</w:p>") else { continue };
        let search_from = paragraph_start + paragraph_end_rel + "</w:p>".len();
        let Some(table_start_rel) = document_xml[search_from..].find("<w:tbl>") else { continue };
        let table_start = search_from + table_start_rel;
        let Some(table_end_rel) = document_xml[table_start..].find("</w:tbl>") else { continue };
        let table_end = table_start + table_end_rel + "</w:tbl>".len();
        let (_, run_props) = insert_formatting(&entries, &insert.style);
        let new_table = table_xml(rows, &run_props);
        document_xml.replace_range(table_start..table_end, &new_table);
        insert.cached = format!("{}×{}", rows.len(), rows.iter().map(Vec::len).max().unwrap_or(0));
        changed = true;
    }
    if !changed {
        return Ok(info_from_entries(&entries));
    }
    rde_doc.body_hash = rde_hash::sha256(document_xml.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
    upsert_custom_props(&mut entries, &rde_doc);
    let info = info_from_entries(&entries);
    write_entries_atomic(&entries, package)?;
    Ok(info)
}

/// Перенумеровать подписи вставок рисунков/таблиц и перекрёстные ссылки на них
/// (R-DOC-15/17/18): «Рисунок N — …», «Таблица N — …», `rde://insert/<id>` → «Рисунок N».
pub fn renumber_insert_captions(package: &Path) -> Result<PackageInfo, String> {
    let mut entries = read_package(package)?.entries;
    let mut rde_doc = load_rde_doc_from_entries(&entries)?;
    let mut document_xml = {
        let document = entries.get(DOCUMENT_XML).ok_or_else(|| "в пакете нет word/document.xml".to_owned())?;
        String::from_utf8_lossy(document).to_string()
    };
    let mut changed = false;
    // Продолжаем нумерацию после подписей, уже имеющихся в теле документа (миграция из WordCraft),
    // но не считаем абзацы самих живых вставок — иначе перенумерация не идемпотентна.
    let live_para_ids: std::collections::HashSet<String> = rde_doc
        .inserts
        .iter()
        .filter(|insert| !insert.target.is_empty())
        .filter_map(|insert| insert.para_id.clone())
        .collect();
    let mut figures = max_caption_number(&document_xml, "Рисунок", &live_para_ids);
    let mut tables = max_caption_number(&document_xml, "Таблица", &live_para_ids);
    let mut numbering: BTreeMap<String, String> = BTreeMap::new();
    for insert in &mut rde_doc.inserts {
        // Авто-вставки миграции (без URI цели) не перенумеровываем: это исторические блоки.
        if insert.target.is_empty() {
            continue;
        }
        let (label, number) = match insert.kind.as_str() {
            "figure" => {
                figures += 1;
                ("Рисунок", figures)
            }
            "table" => {
                tables += 1;
                ("Таблица", tables)
            }
            _ => continue,
        };
        numbering.insert(insert.id.clone(), format!("{label} {number}"));
        let base = strip_caption_prefix(&insert.caption);
        let new_caption = if base.is_empty() { format!("{label} {number}") } else { format!("{label} {number} — {base}") };
        if new_caption == insert.caption {
            continue;
        }
        if let Some(para_id) = insert.para_id.clone() {
            if let Some(new_xml) = replace_paragraph_text(&document_xml, &para_id, &new_caption) {
                document_xml = new_xml;
                changed = true;
            }
        }
        insert.caption = new_caption.clone();
        insert.cached = new_caption;
    }
    // Перекрёстные ссылки — после того, как известны номера всех рисунков/таблиц.
    let crossrefs: Vec<(String, String)> = rde_doc
        .inserts
        .iter()
        .filter(|insert| insert.kind == "crossref")
        .filter_map(|insert| {
            let target_id = insert.target.strip_prefix("rde://insert/")?;
            let text = numbering.get(target_id)?;
            (insert.cached != *text).then(|| (insert.id.clone(), text.clone()))
        })
        .collect();
    for (id, text) in crossrefs {
        if let Some(insert) = rde_doc.inserts.iter().find(|insert| insert.id == id).cloned() {
            if let Some(new_xml) = replace_sdt_text(&document_xml, &insert, &text) {
                document_xml = new_xml;
                changed = true;
            }
        }
        if let Some(insert) = rde_doc.inserts.iter_mut().find(|insert| insert.id == id) {
            insert.cached = text;
        }
    }
    if !changed {
        return Ok(info_from_entries(&entries));
    }
    rde_doc.body_hash = rde_hash::sha256(document_xml.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
    upsert_custom_props(&mut entries, &rde_doc);
    let info = info_from_entries(&entries);
    write_entries_atomic(&entries, package)?;
    Ok(info)
}

/// Заменить текст первого `w:t` в абзаце с данным `w14:paraId`.
fn replace_paragraph_text(xml: &str, para_id: &str, text: &str) -> Option<String> {
    let anchor = format!("<w:p w14:paraId=\"{para_id}\"");
    let start = xml.find(&anchor)?;
    let end = xml[start..].find("</w:p>")? + start + "</w:p>".len();
    let paragraph = &xml[start..end];
    let mut rebuilt = String::new();
    let mut rest = paragraph;
    let mut replaced = false;
    while let Some(text_start) = rest.find("<w:t") {
        let open_end = rest[text_start..].find('>')?;
        let inner_start = text_start + open_end + 1;
        let close_rel = rest[inner_start..].find("</w:t>")?;
        rebuilt.push_str(&rest[..inner_start]);
        if replaced {
            rebuilt.push_str(&rest[inner_start..inner_start + close_rel]);
        } else {
            rebuilt.push_str(&escape_attr(text));
            replaced = true;
        }
        rebuilt.push_str("</w:t>");
        rest = &rest[inner_start + close_rel + "</w:t>".len()..];
    }
    rebuilt.push_str(rest);
    Some(format!("{}{}{}", &xml[..start], rebuilt, &xml[end..]))
}

/// Максимальный номер подписи «Рисунок N»/«Таблица N», уже присутствующей в документе.
fn max_caption_number(document_xml: &str, label: &str, skip_para_ids: &std::collections::HashSet<String>) -> u32 {
    let mut max = 0u32;
    let mut rest = document_xml;
    while let Some(paragraph_start) = rest.find("<w:p") {
        let Some(open_end) = rest[paragraph_start..].find('>') else { break };
        let open_tag = &rest[paragraph_start..paragraph_start + open_end];
        let Some(close_rel) = rest[paragraph_start..].find("</w:p>") else { break };
        let paragraph = &rest[paragraph_start..paragraph_start + close_rel];
        rest = &rest[paragraph_start + close_rel + "</w:p>".len()..];
        // Пропустить абзацы живых вставок (их номера мы и назначаем).
        if let Some(para_id) = open_tag.split("w14:paraId=\"").nth(1).and_then(|rest| rest.split('"').next()) {
            if skip_para_ids.contains(para_id) {
                continue;
            }
        }
        let mut scan = paragraph;
        while let Some(index) = scan.find(label) {
            let after = &scan[index + label.len()..];
            let digits: String = after.trim_start().chars().take_while(char::is_ascii_digit).collect();
            if let Ok(number) = digits.parse::<u32>() {
                max = max.max(number);
            }
            scan = &scan[index + label.len()..];
        }
    }
    max
}

/// Убрать префикс «Рисунок N —»/«Таблица N —» из подписи (для перенумерации).
fn strip_caption_prefix(caption: &str) -> String {
    let caption = caption.trim();
    for label in ["Рисунок", "Таблица"] {
        if let Some(rest) = caption.strip_prefix(label) {
            let rest = rest.trim_start();
            let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty() {
                let rest = rest[digits.len()..].trim_start();
                let rest = rest.strip_prefix('—').or_else(|| rest.strip_prefix('-')).unwrap_or(rest).trim_start();
                return rest.to_owned();
            }
        }
    }
    caption.to_owned()
}

/// Вставить рисунок из файла артефакта (R-DOC-11): media-часть + relationship + drawing в `w:sdt`.
/// `caption` — текст подписи (номер обновляется перенумерацией документа).
pub fn add_figure_insert(
    package: &Path,
    image: &Path,
    target: String,
    caption: String,
    out_hash: String,
    anchor_block: Option<usize>,
) -> Result<InsertReport, String> {
    let ext = image
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("png")
        .to_ascii_lowercase();
    let bytes = fs::read(image).map_err(|e| format!("прочитать рисунок {}: {e}", image.display()))?;
    let (ext, bytes) = prepare_figure_media(&ext, bytes);
    let mut entries = read_package(package)?.entries;
    let mut rde_doc = load_rde_doc_from_entries(&entries)?;
    let index = rde_doc.inserts.iter().filter(|insert| insert.kind == "figure").count() + 1;
    let id = next_insert_id(&rde_doc.inserts, &String::from_utf8_lossy(entries.get(DOCUMENT_XML).map(Vec::as_slice).unwrap_or(&[])));
    let para_id = para_id_for(&format!("{}:figure:{id}", rde_doc.doc_id), index);
    // Уникальная media-часть по id вставки (не по счётчику): счётчик мог совпасть с занятым
    // именем и молча перезаписать рисунок другой вставки (P8, DOC-02).
    let mut media_name = format!("word/media/rde-figure-{id}.{ext}");
    let mut suffix = 1u32;
    while entries.contains_key(&media_name) {
        media_name = format!("word/media/rde-figure-{id}-{suffix}.{ext}");
        suffix += 1;
    }
    let media_target = media_name.trim_start_matches("word/").to_owned();
    let rels = entries
        .get(rels_path_for_document())
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .unwrap_or_default();
    let rel_id = next_relationship_id(&rels);
    entries.insert(media_name, bytes.clone());
    ensure_default_content_type(&mut entries, &ext);
    add_image_relationship(&mut entries, &rel_id, &media_target)?;
    let (cx, cy) = image_extent_emu(&bytes, &ext);
    let mut insert = RdeInsert {
        id: id.clone(),
        block: String::new(),
        kind: "figure".to_owned(),
        target,
        caption: caption.clone(),
        number: None,
        hash: Some(out_hash),
        cached: caption,
        selector: String::new(),
        value_kind: "figure".to_owned(),
        para_id: Some(para_id.clone()),
        style: String::new(),
        format: String::new(),
            run_style: String::new(),
    };
    let document = entries.get(DOCUMENT_XML).ok_or_else(|| "в пакете нет word/document.xml".to_owned())?;
    let document_xml = String::from_utf8_lossy(document).to_string();
    let paragraph = drawing_paragraph_xml(&insert, &rel_id, cx, cy, &para_id);
    let mut fragment = paragraph;
    // Подпись — отдельный абзац после рисунка (номер присвоит перенумерация).
    let caption_para_id = para_id_for(&format!("{}:caption:{id}", rde_doc.doc_id), index);
    if !insert.cached.is_empty() {
        let caption_text = insert.cached.clone();
        insert.caption = caption_text.clone();
        let (caption_ppr, caption_rpr) = insert_formatting(&entries, &insert.style);
        let caption_paragraph = format!(
            "<w:p w14:paraId=\"{caption_para_id}\">{caption_ppr}<w:r>{caption_rpr}<w:t xml:space=\"preserve\">{}</w:t></w:r></w:p>",
            escape_attr(&caption_text)
        );
        fragment.push_str(&caption_paragraph);
    }
    let document_xml = insert_fragment(&document_xml, &fragment, anchor_block);
    rde_doc.inserts.push(insert.clone());
    rde_doc.updated_unix = now_unix();
    rde_doc.body_hash = rde_hash::sha256(document_xml.as_bytes()).hex();
    entries.insert(DOCUMENT_XML.to_owned(), document_xml.into_bytes());
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
    upsert_custom_props(&mut entries, &rde_doc);
    let info = info_from_entries(&entries);
    write_entries_atomic(&entries, package)?;
    Ok(InsertReport { insert, package: info })
}

/// Обновить рисунки вставок: `images`: insert_id → (расширение, байты). Возвращает
/// отчёт и признак изменения.
pub fn update_figure_images(
    package: &Path,
    images: &BTreeMap<String, (String, Vec<u8>)>,
    hashes: &BTreeMap<String, String>,
) -> Result<InsertUpdateReport, String> {
    let mut entries = read_package(package)?.entries;
    let mut rde_doc = load_rde_doc_from_entries(&entries)?;
    let mut updated = Vec::new();
    for insert in &mut rde_doc.inserts {
        if insert.kind != "figure" {
            continue;
        }
        let Some((ext, bytes)) = images.get(&insert.id) else { continue };
        let (ext, bytes) = prepare_figure_media(ext, bytes.clone());
        // media-часть для этой вставки: находим по drawing/relationship через tag → rel id.
        let document = entries.get(DOCUMENT_XML).ok_or_else(|| "в пакете нет word/document.xml".to_owned())?;
        let document_xml = String::from_utf8_lossy(document).to_string();
        let Some(media_part) = figure_media_part(&document_xml, &entries, &insert.id) else { continue };
        let ext = ext.to_ascii_lowercase();
        let mut new_part = media_part
            .rsplit_once('.')
            .map(|(stem, _)| format!("{stem}.{ext}"))
            .unwrap_or_else(|| format!("{media_part}.{ext}"));
        // Имя media-части должно быть привязано к id вставки: старые имена по счётчику
        // могли совпадать у разных вставок (P8, DOC-02).
        let desired_part = format!("word/media/rde-figure-{}.{}", insert.id, ext);
        if new_part != desired_part {
            if new_part != media_part {
                entries.remove(&media_part);
                ensure_default_content_type(&mut entries, &ext);
                replace_relationship_target(&mut entries, &media_part, &new_part);
            }
            let mut target = desired_part.clone();
            let mut suffix = 1u32;
            while entries.contains_key(&target) && target != new_part {
                target = format!("word/media/rde-figure-{}-{suffix}.{}", insert.id, ext);
                suffix += 1;
            }
            if target != new_part {
                replace_relationship_target(&mut entries, &new_part, &target);
                entries.remove(&new_part);
                new_part = target;
            }
        } else if new_part != media_part {
            entries.remove(&media_part);
            ensure_default_content_type(&mut entries, &ext);
            replace_relationship_target(&mut entries, &media_part, &new_part);
        }
        entries.insert(new_part, bytes.clone());
        insert.hash = hashes.get(&insert.id).cloned().or(insert.hash.clone());
        updated.push(insert.id.clone());
    }
    rde_doc.updated_unix = now_unix();
    entries.insert(RDE_DOC_PART.to_owned(), render_rde_doc(&rde_doc).into_bytes());
    upsert_custom_props(&mut entries, &rde_doc);
    let info = info_from_entries(&entries);
    write_entries_atomic(&entries, package)?;
    Ok(InsertUpdateReport { updated, package: info, rde_doc })
}

/// Найти media-часть рисунка вставки по tag `rde:<id>` → `r:embed` → relationship Target.
fn figure_media_part(
    document_xml: &str,
    entries: &BTreeMap<String, Vec<u8>>,
    insert_id: &str,
) -> Option<String> {
    let tag = format!("<w:tag w:val=\"{INSERT_TAG_PREFIX}{insert_id}\"");
    let tag_index = document_xml.find(&tag)?;
    // Границы content-control: не выходим за </w:sdt>, иначе можно захватить чужой рисунок.
    let sdt_start = document_xml[..tag_index].rfind("<w:sdt>")?;
    let content_start = document_xml[sdt_start..].find("<w:sdtContent>")? + sdt_start;
    let content_end = document_xml[content_start..].find("</w:sdtContent>")? + content_start;
    let window = &document_xml[content_start..content_end];
    let embed_index = window.find("r:embed=\"")?;
    let rest = &window[embed_index + "r:embed=\"".len()..];
    let end = rest.find('"')?;
    let rel_id = &rest[..end];
    let rels = entries
        .get(rels_path_for_document())
        .map(|bytes| String::from_utf8_lossy(bytes).to_string())
        .unwrap_or_default();
    let id_marker = format!("Id=\"{rel_id}\"");
    let rel_index = rels.find(&id_marker)?;
    let target_index = rels[rel_index..].find("Target=\"")? + rel_index + "Target=\"".len();
    let target_rest = &rels[target_index..];
    let target_end = target_rest.find('"')?;
    let target = &target_rest[..target_end];
    Some(format!("word/{target}"))
}

fn replace_relationship_target(entries: &mut BTreeMap<String, Vec<u8>>, old_part: &str, new_part: &str) {
    let Some(rels) = entries.get_mut(rels_path_for_document()) else { return };
    let text = String::from_utf8_lossy(rels).to_string();
    let old_target = old_part.trim_start_matches("word/");
    let new_target = new_part.trim_start_matches("word/");
    let updated = text.replace(&format!("Target=\"{old_target}\""), &format!("Target=\"{new_target}\""));
    *rels = updated.into_bytes();
}

/// Разница версий `rde:doc` (diff-предпросмотр обновления документа).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DocDiff {
    pub added_blocks: usize,
    pub removed_blocks: usize,
    pub changed_blocks: Vec<(String, String, String)>,
    pub added_inserts: Vec<String>,
    pub updated_inserts: Vec<String>,
    pub removed_inserts: Vec<String>,
}

impl DocDiff {
    pub fn is_empty(&self) -> bool {
        self.added_blocks == 0
            && self.removed_blocks == 0
            && self.changed_blocks.is_empty()
            && self.added_inserts.is_empty()
            && self.updated_inserts.is_empty()
            && self.removed_inserts.is_empty()
    }

    pub fn summary_ru(&self) -> String {
        format!(
            "блоки +{}/−{}, изменено {}, вставки +{}/~{}",
            self.added_blocks,
            self.removed_blocks,
            self.changed_blocks.len(),
            self.added_inserts.len(),
            self.updated_inserts.len()
        )
    }
}

/// Сравнить две версии `rde:doc` (по стабильным id/paraId).
pub fn diff_rde_docs(before: &RdeDoc, after: &RdeDoc) -> DocDiff {
    let mut diff = DocDiff::default();
    let key = |block: &RdeBlock| block.para_id.clone().unwrap_or_else(|| block.id.clone());
    let before_map: BTreeMap<String, &RdeBlock> = before.blocks.iter().map(|block| (key(block), block)).collect();
    let after_map: BTreeMap<String, &RdeBlock> = after.blocks.iter().map(|block| (key(block), block)).collect();
    diff.added_blocks = after_map.keys().filter(|key| !before_map.contains_key(*key)).count();
    diff.removed_blocks = before_map.keys().filter(|key| !after_map.contains_key(*key)).count();
    for (key, block) in &after_map {
        if let Some(old) = before_map.get(key) {
            if old.text != block.text || old.kind != block.kind {
                diff.changed_blocks.push((key.clone(), old.text.clone(), block.text.clone()));
            }
        }
    }
    let before_inserts: BTreeMap<&str, &RdeInsert> =
        before.inserts.iter().map(|insert| (insert.id.as_str(), insert)).collect();
    let after_inserts: BTreeMap<&str, &RdeInsert> =
        after.inserts.iter().map(|insert| (insert.id.as_str(), insert)).collect();
    diff.added_inserts = after_inserts.keys().filter(|id| !before_inserts.contains_key(*id)).map(|id| (*id).to_owned()).collect();
    diff.removed_inserts = before_inserts.keys().filter(|id| !after_inserts.contains_key(*id)).map(|id| (*id).to_owned()).collect();
    for (id, insert) in &after_inserts {
        if let Some(old) = before_inserts.get(id) {
            if old.cached != insert.cached || old.hash != insert.hash {
                diff.updated_inserts.push((*id).to_owned());
            }
        }
    }
    diff
}

// ── транзакционное обновление документа (R-DOC-12): бэкапы, версия в store ────

/// Каталог бэкапов документов и число хранимых версий.
pub fn doc_backups_dir(project_root: &Path) -> PathBuf {
    project_root.join(".rde/doc-backups")
}
const DOC_BACKUP_KEEP: usize = 10;

/// Сделать бэкап пакета перед изменением: `.bak` рядом + копия в `.rde/doc-backups/`
/// (ротация N последних). Возвращает путь бэкапа.
pub fn backup_package(project_root: &Path, package: &Path) -> Result<PathBuf, String> {
    let bytes = fs::read(package).map_err(|e| format!("прочитать {}: {e}", package.display()))?;
    let bak = package.with_extension("doc.rde.bak");
    fs::write(&bak, &bytes).map_err(|e| format!("записать {}: {e}", bak.display()))?;
    let dir = doc_backups_dir(project_root);
    fs::create_dir_all(&dir).map_err(|e| format!("создать {}: {e}", dir.display()))?;
    let name = package.file_name().and_then(|name| name.to_str()).unwrap_or("document.doc.rde");
    let stamp = now_unix();
    let target = dir.join(format!("{stamp}-{name}"));
    fs::write(&target, &bytes).map_err(|e| format!("записать {}: {e}", target.display()))?;
    // Ротация: оставляем DOC_BACKUP_KEEP последних файлов.
    let mut backups: Vec<PathBuf> = fs::read_dir(&dir)
        .map(|entries| entries.flatten().map(|entry| entry.path()).collect())
        .unwrap_or_default();
    backups.sort();
    while backups.len() > DOC_BACKUP_KEEP {
        let oldest = backups.remove(0);
        let _ = fs::remove_file(oldest);
    }
    Ok(target)
}

/// Зафиксировать версию пакета в store: blob + tree + транзакция с корнем `document/<doc_id>`.
pub fn commit_document(
    project_root: &Path,
    package: &Path,
    doc_id: &str,
    store: &mut rde_store::Store,
    note: &str,
) -> Result<String, String> {
    let bytes = fs::read(package).map_err(|e| format!("прочитать {}: {e}", package.display()))?;
    let blob = store.put_blob(&bytes).map_err(|e| e.to_string())?;
    let entries = [rde_hash::TreeEntry::new("document.doc.rde", 0o644, blob)];
    let tree = store.put_tree(&entries).map_err(|e| e.to_string())?;
    let mut roots = store.current_roots().map_err(|e| e.to_string())?;
    roots.insert(document_root_name(doc_id), tree);
    let payload = serde_json::json!({ "kind": "document-update", "doc_id": doc_id, "package": package.display().to_string() });
    let tx = store
        .begin_tx_with_roots(note, Some(payload), roots)
        .map_err(|e| e.to_string())?;
    store.commit_tx(&tx).map_err(|e| e.to_string())?;
    let _ = project_root;
    Ok(tx)
}

/// Восстановить пакет из текущего корня `document/<doc_id>` (после отката корней).
pub fn restore_document_from_store(
    project_root: &Path,
    package: &Path,
    doc_id: &str,
    store: &rde_store::Store,
) -> Result<Option<PathBuf>, String> {
    let roots = store.current_roots().map_err(|e| e.to_string())?;
    let Some(tree) = roots.get(&document_root_name(doc_id)) else { return Ok(None) };
    let entries = store
        .get_tree(tree)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("дерево документа {tree} отсутствует в store"))?;
    let entry = entries
        .iter()
        .find(|entry| entry.path == "document.doc.rde")
        .ok_or_else(|| "в дереве документа нет document.doc.rde".to_owned())?;
    let bytes = store
        .get_blob(entry.blob)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("блоб документа {} отсутствует в CAS", entry.blob))?;
    // Ротация бэкапов и атомарная замена.
    let _ = backup_package(project_root, package);
    let tmp = package.with_extension("doc.rde.tmp");
    fs::write(&tmp, bytes).map_err(|e| format!("записать {}: {e}", tmp.display()))?;
    fs::rename(&tmp, package).map_err(|e| format!("заменить {}: {e}", package.display()))?;
    Ok(Some(package.to_path_buf()))
}

/// Полный отчёт обновления документа (R-DOC-12): изменения, diff, версия в store.
#[derive(Clone, Debug, Default)]
pub struct DocumentUpdateFullReport {
    /// Сколько таблиц-вставок обновлено (R-DOC-18).
    pub table_updates: usize,
    pub caption_changes: Vec<CaptionChange>,
    pub insert_updates: Vec<String>,
    pub figure_updates: Vec<String>,
    pub diff: DocDiff,
    pub backup: Option<PathBuf>,
    pub txid: Option<String>,
    pub package: Option<PackageInfo>,
    pub rde_doc: Option<RdeDoc>,
}

/// Обновить документ транзакционно: бэкап → значения/рисунки вставок → перенумерация подписей
/// → пересохранение пакета → diff → версия в store (откат O(1) через корни).
pub fn update_document_transactional(
    project_root: &Path,
    package: &Path,
    doc_id: &str,
    values: &BTreeMap<String, String>,
    hashes: &BTreeMap<String, String>,
    images: &BTreeMap<String, (String, Vec<u8>)>,
    tables: &BTreeMap<String, Vec<Vec<String>>>,
    store: &mut rde_store::Store,
) -> Result<DocumentUpdateFullReport, String> {
    let before = load_rde_doc(package)?;
    let backup = backup_package(project_root, package)?;
    let mut report = DocumentUpdateFullReport { backup: Some(backup), ..DocumentUpdateFullReport::default() };
    // 1) Значения текстовых вставок.
    if !values.is_empty() {
        let updated = update_insert_values(package, values, hashes)?;
        report.insert_updates = updated.updated;
    }
    // 2) Рисунки вставок.
    if !images.is_empty() {
        let updated = update_figure_images(package, images, hashes)?;
        report.figure_updates = updated.updated;
    }
    // 3) Таблицы вставок из данных артефактов (R-DOC-18).
    if !tables.is_empty() {
        update_table_inserts(package, tables)?;
        report.table_updates = tables.len();
    }
    // 4) Перенумерация подписей/перекрёстных ссылок + refresh/сохранение пакета.
    renumber_insert_captions(package)?;
    let update = update_document_package(package)?;
    report.caption_changes = update.changes;
    if let Some(rde_doc) = &update.rde_doc {
        report.diff = diff_rde_docs(&before, rde_doc);
        report.rde_doc = Some(rde_doc.clone());
    }
    report.package = update.package;
    // 5) Транзакция store с корнем документа.
    let txid = commit_document(project_root, package, doc_id, store, "document update")?;
    report.txid = Some(txid);
    Ok(report)
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
    let package_info = write_package(external_docx, &mut rde_doc, package)?;
    Ok(ReviewImportReport {
        comments_before,
        comments_after,
        external_edit,
        doc_id: rde_doc.doc_id.clone(),
        package: package_info,
    })
}
