//! RDE docx fidelity gates (M3): инвентаризация частей пакета, roundtrip через WordCraft и
//! независимый оракул LibreOffice (headless-конвертация в PDF).
//!
//! Гейт отвечает на вопросы:
//! 1) все ли критичные части OOXML на месте после roundtrip;
//! 2) сохранились ли медиа-файлы байт-в-байт;
//! 3) какие части пропали/добавились/изменились (отчёт для ревью);
//! 4) открывает ли документ независимый офисный пакет (LO) и получается ли PDF.
//!
//! Отчёт — JSON, пригодный для CI/артефакта.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use zip::ZipArchive;

/// Обязательные части пакета (ECMA-376/OPC): без них документ не откроется.
/// Остальные (`settings.xml`, `fontTable.xml`, `numbering.xml`, `theme/*`) — опциональные для
/// документов WordCraft; гейт проверяет только те, что реально есть в исходнике.
pub const CRITICAL_PARTS: [&str; 5] = [
    "[Content_Types].xml",
    "_rels/.rels",
    "word/document.xml",
    "word/_rels/document.xml.rels",
    "word/styles.xml",
];

/// Метаданные части пакета.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartInfo {
    pub name: String,
    pub size: u64,
    pub sha256: String,
}

/// Результат независимой проверки LibreOffice.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LoOracle {
    pub available: bool,
    pub version: String,
    /// Размер полученного PDF в байтах (0 — конвертация не удалась).
    pub pdf_bytes: u64,
    pub pdf_is_valid: bool,
    pub error: Option<String>,
}

/// Полный отчёт fidelity-гейта.
/// Результат OPC/XML-валидации пакета (XSD-lite: без внешних XSD-схем, но с проверкой
/// well-formed XML, покрытия [Content_Types].xml и целостности relationships).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpcValidation {
    pub ok: bool,
    pub parts: usize,
    pub xml_parts: usize,
    pub errors: Vec<String>,
}

/// Проверить OPC-инварианты `.docx`/`.doc.rde`: well-formed XML, Content_Types покрывает части,
/// все внутренние цели relationships существуют, `word/document.xml` на месте.
pub fn validate_opc(input: &Path) -> Result<OpcValidation, String> {
    let file = File::open(input).map_err(|e| format!("открыть {}: {e}", input.display()))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("zip {}: {e}", input.display()))?;
    let mut entries: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| format!("часть #{index}: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut bytes).map_err(|e| format!("прочитать {}: {e}", entry.name()))?;
        entries.insert(entry.name().to_owned(), bytes);
    }
    let mut validation = OpcValidation { parts: entries.len(), ..OpcValidation::default() };
    if !entries.contains_key("word/document.xml") {
        validation.errors.push("нет word/document.xml".to_owned());
    }
    let content_types = entries.get("[Content_Types].xml").map(|bytes| String::from_utf8_lossy(bytes).to_string());
    let Some(content_types) = content_types else {
        validation.errors.push("нет [Content_Types].xml".to_owned());
        validation.ok = false;
        return Ok(validation);
    };
    let defaults: BTreeSet<String> = extract_attr_values(&content_types, "Default", "Extension")
        .into_iter()
        .map(|ext| ext.to_ascii_lowercase())
        .collect();
    let overrides: BTreeSet<String> = extract_attr_values(&content_types, "Override", "PartName")
        .into_iter()
        .map(|name| name.trim_start_matches('/').to_owned())
        .collect();
    let mut xml_parts = 0usize;
    for (name, bytes) in &entries {
        if name == "[Content_Types].xml" {
            continue;
        }
        let is_xml = name.ends_with(".xml") || name.ends_with(".rels");
        if is_xml {
            xml_parts += 1;
            // Well-formed XML: читаем все события до EOF.
            let text = String::from_utf8_lossy(bytes);
            let mut reader = quick_xml::Reader::from_str(&text);
            reader.config_mut().trim_text(false);
            let mut malformed = None;
            loop {
                match reader.read_event() {
                    Ok(quick_xml::events::Event::Eof) => break,
                    Ok(_) => {}
                    Err(error) => {
                        malformed = Some(format!("{name}: XML не well-formed: {error}"));
                        break;
                    }
                }
            }
            if let Some(error) = malformed {
                validation.errors.push(error);
            }
        }
        // Content_Types покрывает часть (по Override или Default-расширению).
        if !overrides.contains(name) {
            let ext = name.rsplit_once('.').map(|(_, ext)| ext.to_ascii_lowercase()).unwrap_or_default();
            if !defaults.contains(&ext) {
                validation.errors.push(format!("{name}: нет Content_Types (ни Override, ни Default .{ext})"));
            }
        }
        // Relationships: внутренние цели должны существовать.
        if name.ends_with(".rels") {
            let text = String::from_utf8_lossy(bytes);
            let base = relationship_base_dir(name);
            for target in extract_attr_values(&text, "Relationship", "Target") {
                if target.starts_with("http://") || target.starts_with("https://") || target.starts_with("mailto:") {
                    continue;
                }
                let resolved = normalize_zip_path(&format!("{base}{target}"));
                if !entries.contains_key(&resolved) {
                    validation.errors.push(format!("{name}: цель связи не найдена: {target} ({resolved})"));
                }
            }
        }
    }
    validation.xml_parts = xml_parts;
    validation.ok = validation.errors.is_empty();
    Ok(validation)
}

fn extract_attr_values(xml: &str, element: &str, attr: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&format!("<{element}")) {
        let Some(end) = rest[start..].find('>') else { break };
        let tag = &rest[start..start + end + 1];
        if let Some(attr_start) = tag.find(&format!("{attr}=\"")) {
            let value = &tag[attr_start + attr.len() + 2..];
            if let Some(close) = value.find('"') {
                values.push(value[..close].to_owned());
            }
        }
        rest = &rest[start + end + 1..];
    }
    values
}

fn relationship_base_dir(rels_name: &str) -> String {
    // word/_rels/document.xml.rels → word/
    match rels_name.rsplit_once("_rels/") {
        Some((prefix, _)) => prefix.to_owned(),
        None => String::new(),
    }
}

fn normalize_zip_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for segment in path.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    parts.join("/")
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct FidelityReport {
    pub source: String,
    pub source_parts: usize,
    pub roundtrip_parts: usize,
    /// Части исходника, пропавшие после roundtrip.
    pub missing: Vec<String>,
    /// Части, добавленные WordCraft.
    pub extra: Vec<String>,
    /// Части с изменившимся содержимым (одинаковое имя, другой sha256).
    pub changed: Vec<String>,
    pub critical_missing: Vec<String>,
    pub media_preserved: bool,
    pub paragraphs_source: usize,
    pub paragraphs_roundtrip: usize,
    pub lo: LoOracle,
    /// OPC/XML-валидация (XSD-lite, P5).
    #[serde(default)]
    pub opc: OpcValidation,
    pub generated_unix: u64,
}

impl FidelityReport {
    pub fn passed(&self) -> bool {
        self.critical_missing.is_empty()
            && self.media_preserved
            && self.paragraphs_source == self.paragraphs_roundtrip
            && self.lo.pdf_is_valid
            && self.opc.ok
    }
}

/// Прогнать гейт по корпусу `.docx`-файлов (S6): возвращает (файл, отчёт).
pub fn run_corpus(dir: &Path) -> Vec<(String, Result<FidelityReport, String>)> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("docx"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    let work = temp_work_dir("corpus");
    let mut reports = Vec::new();
    for file in files {
        let name = file.file_name().and_then(|name| name.to_str()).unwrap_or("file.docx").to_owned();
        let work_dir = work.join(name.replace(".docx", ""));
        let report = run_gate(&file, &work_dir);
        reports.push((name, report));
    }
    reports
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Инвентаризация частей пакета (.docx) с sha256.
pub fn inventory(path: &Path) -> Result<Vec<PartInfo>, String> {
    let file = fs::File::open(path).map_err(|e| format!("открыть {}: {e}", path.display()))?;
    let mut archive = ZipArchive::new(file).map_err(|e| format!("zip {}: {e}", path.display()))?;
    let mut parts = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|e| format!("часть #{index}: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        let name = entry.name().to_owned();
        let mut data = Vec::with_capacity(entry.size() as usize);
        use std::io::Read;
        entry.read_to_end(&mut data).map_err(|e| format!("прочитать {name}: {e}"))?;
        parts.push(PartInfo { name, size: data.len() as u64, sha256: rde_hash::sha256(&data).hex() });
    }
    parts.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(parts)
}

/// Roundtrip: открыть WordCraft-движком и сохранить в новый `.docx`.
pub fn roundtrip(input: &Path, output: &Path) -> Result<(), String> {
    let region = wordcraft_engine::Region::from_tag("ru-RU");
    let (doc, _encrypted) = wordcraft_engine::io::open_path_in(input, None, &region)?;
    wordcraft_engine::io::save_path(output, &doc)
}

fn paragraph_count(path: &Path) -> Result<usize, String> {
    let region = wordcraft_engine::Region::from_tag("ru-RU");
    let (doc, _encrypted) = wordcraft_engine::io::open_path_in(path, None, &region)?;
    let story = wordcraft_engine::doc::StoryRef::Body;
    Ok(doc.para_paths(story).len())
}

/// Независимый оракул: LibreOffice headless-конвертация в PDF.
pub fn lo_oracle(docx: &Path, work_dir: &Path) -> LoOracle {
    let version = Command::new("soffice")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .unwrap_or_default();
    if version.is_empty() {
        return LoOracle { available: false, version, error: Some("soffice не найден в PATH".to_owned()), ..LoOracle::default() };
    }
    let out_dir = work_dir.join("lo-out");
    let profile = work_dir.join("lo-profile");
    let _ = fs::create_dir_all(&out_dir);
    let _ = fs::create_dir_all(&profile);
    let profile_uri = format!("file://{}", profile.display());
    let output = Command::new("soffice")
        .args([
            "--headless",
            "--nologo",
            "--nofirststartwizard",
            "--norestore",
            &format!("-env:UserInstallation={profile_uri}"),
            "--convert-to",
            "pdf",
            "--outdir",
        ])
        .arg(&out_dir)
        .arg(docx)
        .output();
    let mut oracle = LoOracle { available: true, version, ..LoOracle::default() };
    match output {
        Ok(output) if output.status.success() => {
            let stem = docx.file_stem().map(|stem| stem.to_string_lossy().to_string()).unwrap_or_default();
            let pdf = out_dir.join(format!("{stem}.pdf"));
            match fs::read(&pdf) {
                Ok(bytes) => {
                    oracle.pdf_bytes = bytes.len() as u64;
                    oracle.pdf_is_valid = bytes.starts_with(b"%PDF") && bytes.len() > 1024;
                }
                Err(error) => oracle.error = Some(format!("PDF не создан ({}): {error}", pdf.display())),
            }
        }
        Ok(output) => {
            oracle.error = Some(format!(
                "soffice exit {:?}: {}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Err(error) => oracle.error = Some(format!("не удалось запустить soffice: {error}")),
    }
    oracle
}

/// Полный fidelity-гейт: инвентаризация → roundtrip WordCraft → сравнение → LO-оракул.
pub fn run_gate(input: &Path, work_dir: &Path) -> Result<FidelityReport, String> {
    fs::create_dir_all(work_dir).map_err(|e| format!("создать {}: {e}", work_dir.display()))?;
    let source = inventory(input)?;
    let roundtrip_path = work_dir.join("roundtrip.docx");
    roundtrip(input, &roundtrip_path)?;
    let roundtrip_parts = inventory(&roundtrip_path)?;

    let source_map: BTreeMap<&str, &PartInfo> = source.iter().map(|part| (part.name.as_str(), part)).collect();
    let roundtrip_map: BTreeMap<&str, &PartInfo> = roundtrip_parts.iter().map(|part| (part.name.as_str(), part)).collect();

    let mut missing: Vec<String> = source_map.keys().filter(|name| !roundtrip_map.contains_key(**name)).map(|name| (*name).to_owned()).collect();
    let mut extra: Vec<String> = roundtrip_map.keys().filter(|name| !source_map.contains_key(**name)).map(|name| (*name).to_owned()).collect();
    let mut changed: Vec<String> = source_map
        .iter()
        .filter_map(|(name, part)| roundtrip_map.get(name).filter(|other| other.sha256 != part.sha256).map(|_| (*name).to_owned()))
        .collect();
    missing.sort();
    extra.sort();
    changed.sort();

    let critical_missing: Vec<String> = CRITICAL_PARTS
        .iter()
        .filter(|name| !roundtrip_map.contains_key(**name))
        .map(|name| (*name).to_owned())
        .collect();

    let media_preserved = source_map
        .iter()
        .filter(|(name, _)| name.starts_with("word/media/"))
        .all(|(name, part)| roundtrip_map.get(name).is_some_and(|other| other.sha256 == part.sha256));

    let paragraphs_source = paragraph_count(input)?;
    let paragraphs_roundtrip = paragraph_count(&roundtrip_path)?;
    let lo = lo_oracle(&roundtrip_path, work_dir);
    let opc = validate_opc(&roundtrip_path)?;
    Ok(FidelityReport {
        source: input.display().to_string(),
        source_parts: source.len(),
        roundtrip_parts: roundtrip_parts.len(),
        missing,
        extra,
        changed,
        critical_missing,
        media_preserved,
        paragraphs_source,
        paragraphs_roundtrip,
        lo,
        opc,
        generated_unix: now_unix(),
    })
}

/// Сериализовать отчёт в JSON (pretty).
pub fn report_json(report: &FidelityReport) -> Result<String, String> {
    serde_json::to_string_pretty(report).map_err(|e| e.to_string())
}

/// Универсальный «временный каталог» для гейта.
pub fn temp_work_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("rde-docx-{tag}-{}", std::process::id()))
}
