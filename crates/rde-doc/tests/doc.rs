//! Пакет `.doc.rde`: rde:doc XML, roundtrip, чистое извлечение .docx, внешнее редактирование,
//! перенумерация подписей и сохранение под контролем RDE.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rde_doc::*;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-doc-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Стабильная фикстура: чистый .docx демо-отчёта (снимок до живых вставок).
/// Тесты не зависят от текущего состояния demo-project (в нём могут быть вставки).
fn demo_docx() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/base-report.docx")
}

fn copy_file(src: &Path, dst: &Path) {
    std::fs::copy(src, dst).unwrap();
}

#[test]
fn rde_doc_xml_roundtrip_handles_unicode_and_escapes() {
    let mut doc = RdeDoc {
        schema_version: "1".into(),
        doc_id: "01TESTDOC".into(),
        template: "gost-7.32-2017".into(),
        body_hash: "abc123".into(),
        blocks: vec![
            RdeBlock { id: "b1".into(), path: "0".into(), kind: "heading1".into(), text: "Введение & «кавычки» <x>".into(), para_id: Some("1A2B3C4D".into()) },
            RdeBlock { id: "b2".into(), path: "1".into(), kind: "paragraph".into(), text: "Текст с амперсандом & и \"кавычками\"".into(), para_id: None },
        ],
        inserts: vec![RdeInsert {
            id: "i1".into(),
            block: "b3".into(),
            kind: "figure".into(),
            target: "rde://artifact/01ABC/out/plot.svg".into(),
            caption: "Рисунок 1 — График".into(),
            number: Some(1),
            hash: Some("sha256:ff".into()),
            cached: "42.5".into(),
            selector: "#/stats/x1/mean".into(),
            value_kind: "number".into(),
            para_id: Some("5E6F7A8B".into()),
            style: "Normal".into(),
            format: "dec:1".into(),
        }],
        review: RdeReview { open: 2, resolved: 5 },
        updated_unix: 42,
    };
    let xml = render_rde_doc(&doc);
    let parsed = parse_rde_doc(&xml).unwrap();
    assert_eq!(parsed, doc);
    // Невалидный XML не паникует.
    let broken = parse_rde_doc("<other/>");
    assert!(broken.is_err());
    // Хеш/вставки переживают пустой template.
    doc.template = String::new();
    assert_eq!(parse_rde_doc(&render_rde_doc(&doc)).unwrap(), doc);
}

#[test]
fn package_roundtrip_and_clean_extraction() {
    let dir = tmp("roundtrip");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    let original_paragraphs = {
        let (doc, from_package) = open_document(&clean).unwrap();
        assert!(!from_package);
        doc.paragraph_count()
    };

    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "gost-7.32-2017").unwrap();
    assert!(rde_doc.blocks.len() >= 8, "блоков: {}", rde_doc.blocks.len());
    assert!(rde_doc.inserts.iter().any(|i| i.kind == "figure"), "вставки: {:?}", rde_doc.inserts);
    assert_eq!(rde_doc.body_hash.len(), 64);

    let package = dir.join("report.doc.rde");
    let info = write_package(&clean, &mut rde_doc, &package).unwrap();
    assert!(info.has_rde_doc);
    assert!(package.is_file());
    assert_eq!(load_rde_doc(&package).unwrap(), rde_doc);

    // Чистое извлечение: получается валидный .docx без RDE-частей.
    let extracted = dir.join("extracted.docx");
    let clean_info = extract_docx(&package, &extracted).unwrap();
    assert!(!clean_info.has_rde_doc);
    assert!(!clean_info.has_part(RDE_DOC_PART));
    let (doc, from_package) = open_document(&extracted).unwrap();
    assert!(!from_package);
    assert_eq!(doc.paragraph_count(), original_paragraphs);
    let content_types = {
        let file = std::fs::File::open(&extracted).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut part = zip.by_name("[Content_Types].xml").unwrap();
        let mut text = String::new();
        part.read_to_string(&mut text).unwrap();
        text
    };
    assert!(!content_types.contains("customXml/item1.xml"), "RDE override остался: {content_types}");
    // Пакет открывается как .doc.rde и даёт тот же документ.
    let (doc_pkg, from_package) = open_document(&package).unwrap();
    assert!(from_package);
    assert_eq!(doc_pkg.paragraph_count(), original_paragraphs);
    let _ = std::fs::remove_dir_all(&dir);
}

fn rewrite_part(package: &Path, target: &str, new_data: &[u8], remove: bool) -> Vec<u8> {
    let file = std::fs::File::open(package).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().to_owned();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            if name == target {
                if remove {
                    continue;
                }
                data = new_data.to_vec();
            }
            writer.start_file(name, options).unwrap();
            writer.write_all(&data).unwrap();
        }
        writer.finish().unwrap();
    }
    cursor.into_inner()
}

#[test]
fn external_edit_detection() {
    let dir = tmp("external");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();

    assert_eq!(check_external_edit(&package).unwrap().verdict, EditVerdict::NotModified);

    // Word пересохранил документ: RDE-часть пропала.
    let stripped = dir.join("stripped.doc.rde");
    std::fs::write(&stripped, rewrite_part(&package, RDE_DOC_PART, &[], true)).unwrap();
    let check = check_external_edit(&stripped).unwrap();
    assert_eq!(check.verdict, EditVerdict::MissingRdeDoc);

    // Документ изменён, rde:doc остался (например, правка через OOXML): body_hash расходится.
    let changed = dir.join("changed.doc.rde");
    std::fs::write(&changed, rewrite_part(&package, DOCUMENT_XML, b"<w:document/>", false)).unwrap();
    let check = check_external_edit(&changed).unwrap();
    assert_eq!(check.verdict, EditVerdict::ExternalEdit);
    assert_eq!(check.body_hash_matches, Some(false));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn update_package_renumbers_captions() {
    let dir = tmp("renumber");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);

    // Портим номер подписи в WordCraft-модели и сохраняем как .docx.
    let (mut doc, _) = open_document(&clean).unwrap();
    let story = wordcraft_engine::doc::StoryRef::Body;
    let mut changed = false;
    for path in doc.para_paths(story) {
        let Some(paragraph) = doc.para(story, &path) else { continue };
        if paragraph.text.trim_start().starts_with("Рисунок") {
            if let Ok(paragraph) = doc.para_mut(story, &path) {
                let len = paragraph.len();
                let props = paragraph.props_of_char(0).clone();
                if paragraph.delete_except(0, len, &[]).is_ok() {
                    paragraph.insert_text(0, "Рисунок 5 — Проверка перенумерации", &props).unwrap();
                    changed = true;
                }
            }
        }
    }
    assert!(changed, "в демо-документе нет подписи «Рисунок»");
    wordcraft_engine::io::save_path(&clean, &doc).unwrap();

    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();

    let report = update_document_package(&package).unwrap();
    assert_eq!(report.changes.len(), 1, "{:?}", report.changes);
    assert!(report.changes[0].new_text.starts_with("Рисунок 1 — Проверка перенумерации"), "{:?}", report.changes);
    let updated = load_rde_doc(&package).unwrap();
    assert!(updated.blocks.iter().any(|b| b.text.starts_with("Рисунок")));
    // После обновления body_hash совпадает — внешних правок нет.
    assert_eq!(check_external_edit(&package).unwrap().verdict, EditVerdict::NotModified);
    // И подпись действительно переписана в самом .docx.
    let (doc_after, _) = open_document(&package).unwrap();
    let text = doc_after.plain_text(story);
    assert!(text.contains("Рисунок 1 — Проверка перенумерации"), "текст: {text:.300}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn save_engine_document_keeps_identity_and_inserts() {
    let dir = tmp("save");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "gost-7.32-2017").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();
    let inserts_before = load_rde_doc(&package).unwrap().inserts.len();

    // Пустой шаблон — сохраняем прежний; явный — переопределяем (сценарий «Оформить по ГОСТ»).
    let saved = save_engine_document(&clean, &package, "IGNORED", "").unwrap();
    assert_eq!(saved.doc_id, "01TESTDOC", "doc_id должен сохраняться");
    assert_eq!(saved.template, "gost-7.32-2017");
    assert_eq!(saved.inserts.len(), inserts_before);
    assert_eq!(check_external_edit(&package).unwrap().verdict, EditVerdict::NotModified);

    let saved = save_engine_document(&clean, &package, "IGNORED", "lab-report").unwrap();
    assert_eq!(saved.template, "lab-report");
    assert_eq!(saved.doc_id, "01TESTDOC");
    assert_eq!(saved.inserts.len(), inserts_before);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── M5: комментарии Word и импорт внешнего ревью ─────────────────────────────

fn with_comments(input: &Path, out: &Path) -> PathBuf {
    let file = std::fs::File::open(input).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for index in 0..archive.len() {
            let mut entry = archive.by_index(index).unwrap();
            let name = entry.name().to_owned();
            let mut data = Vec::new();
            entry.read_to_end(&mut data).unwrap();
            writer.start_file(name, options).unwrap();
            writer.write_all(&data).unwrap();
        }
        let comments = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:comments xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:comment w:id="1" w:author="Рецензент" w:initials="Р" w:date="2026-10-25T10:00:00Z"><w:p><w:r><w:t>Проверить рисунок 1</w:t></w:r></w:p></w:comment><w:comment w:id="2" w:author="Научрук" w:initials="Н" w:date="2026-10-25T11:00:00Z"><w:p><w:r><w:t>Добавить ссылку</w:t></w:r></w:p></w:comment></w:comments>"#;
        writer.start_file("word/comments.xml", options).unwrap();
        writer.write_all(comments.as_bytes()).unwrap();
        writer.finish().unwrap();
    }
    std::fs::write(out, cursor.into_inner()).unwrap();
    out.to_path_buf()
}

#[test]
fn comments_are_read_from_docx_and_package() {
    let dir = tmp("comments");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    assert!(comments(&clean).unwrap().is_empty(), "у демо-документа нет комментариев");
    let reviewed = with_comments(&clean, &dir.join("reviewed.docx"));
    let parsed = comments(&reviewed).unwrap();
    assert_eq!(parsed.len(), 2, "{parsed:?}");
    assert_eq!(parsed[0].author, "Рецензент");
    assert_eq!(parsed[0].text, "Проверить рисунок 1");
    assert_eq!(parsed[1].author, "Научрук");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_external_review_preserves_rde_metadata() {
    let dir = tmp("import-review");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "gost-7.32-2017").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();
    let blocks_before = load_rde_doc(&package).unwrap().blocks.len();

    let reviewed = with_comments(&clean, &dir.join("reviewed.docx"));
    let report = import_external_review(&package, &reviewed).unwrap();
    assert_eq!(report.comments_before, 0);
    assert_eq!(report.comments_after, 2);
    assert_eq!(report.doc_id, "01TESTDOC");
    assert_eq!(report.external_edit, EditVerdict::MissingRdeDoc, "чистый .docx — внешний файл без RDE-части");
    let after = load_rde_doc(&package).unwrap();
    assert_eq!(after.doc_id, "01TESTDOC");
    assert_eq!(after.template, "gost-7.32-2017");
    assert_eq!(after.blocks.len(), blocks_before);
    assert_eq!(after.review.open, 2);
    assert_eq!(check_external_edit(&package).unwrap().verdict, EditVerdict::NotModified);
    assert_eq!(comments(&package).unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

// ── P4: §8.5 wiring (root rels, tombstone, paraId) и живые вставки ──────────

#[test]
fn package_has_normative_wiring() {
    let dir = tmp("wiring");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();

    let info = package_info(&package).unwrap();
    // R-DOC-02: relationship из корня пакета на customXml/item1.xml.
    let root_rels = {
        let file = std::fs::File::open(&package).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut part = zip.by_name("_rels/.rels").unwrap();
        let mut text = String::new();
        use std::io::Read;
        part.read_to_string(&mut text).unwrap();
        text
    };
    assert!(root_rels.contains("customXml/item1.xml"), "root rels: {root_rels}");
    assert!(info.has_part("docProps/custom.xml"), "надгробие docProps/custom.xml");
    // R-DOC-03: tombstone содержит rdeDocId.
    let custom = {
        let file = std::fs::File::open(&package).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut part = zip.by_name("docProps/custom.xml").unwrap();
        let mut text = String::new();
        use std::io::Read;
        part.read_to_string(&mut text).unwrap();
        text
    };
    assert!(custom.contains("name=\"rdeDocId\""), "{custom}");
    assert!(custom.contains("01TESTDOC"), "{custom}");
    // R-DOC-04: w14:paraId в документе, блоки получили анкоры.
    let document = {
        let file = std::fs::File::open(&package).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut part = zip.by_name("word/document.xml").unwrap();
        let mut text = String::new();
        use std::io::Read;
        part.read_to_string(&mut text).unwrap();
        text
    };
    assert!(document.contains("w14:paraId="), "paraId в document.xml");
    let loaded = load_rde_doc(&package).unwrap();
    assert!(loaded.blocks.iter().all(|block| block.para_id.is_some()), "у блоков есть para_id");
    assert_eq!(loaded.body_hash, rde_hash::sha256(document.as_bytes()).hex(), "body_hash учитывает paraId");

    // Экспорт чистого .docx: RDE-части/связи/надгробие удалены.
    let extracted = dir.join("clean-out.docx");
    extract_docx(&package, &extracted).unwrap();
    let extracted_info = package_info(&extracted).unwrap();
    assert!(!extracted_info.has_rde_doc);
    assert!(!extracted_info.has_part("docProps/custom.xml"));
    let clean_rels = {
        let file = std::fs::File::open(&extracted).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut part = zip.by_name("_rels/.rels").unwrap();
        let mut text = String::new();
        use std::io::Read;
        part.read_to_string(&mut text).unwrap();
        text
    };
    assert!(!clean_rels.contains("customXml/item1.xml"), "связь RDE удалена: {clean_rels}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn para_ids_are_stable_across_resaves() {
    let dir = tmp("paraids");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();
    let first: Vec<Option<String>> = load_rde_doc(&package).unwrap().blocks.iter().map(|b| b.para_id.clone()).collect();
    assert!(first.iter().all(Option::is_some));
    // Повторное сохранение того же документа: анкоры не перегенерируются.
    save_engine_document(&clean, &package, "01TESTDOC", "").unwrap();
    let second: Vec<Option<String>> = load_rde_doc(&package).unwrap().blocks.iter().map(|b| b.para_id.clone()).collect();
    assert_eq!(first, second, "paraId стабильны между сохранениями");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn live_insert_value_is_sdt_and_updates() {
    let dir = tmp("inserts");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "gost-7.32-2017").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();
    let paragraphs_before = open_document(&package).unwrap().0.paragraph_count();

    let report = add_insert(
        &package,
        InsertRequest {
            kind: "value".into(),
            target: "rde://artifact/01ABC@deadbeef/out/stats.json".into(),
            selector: "#/stats/x1/mean".into(),
            value_kind: "number".into(),
            cached: "10".into(),
            caption: "Среднее x1".into(),
            style: "Normal".into(),
            format: "dec:1".into(),
        },
    )
    .unwrap();
    let insert_id = report.insert.id.clone();
    assert!(report.insert.para_id.is_some());

    let read_document = || {
        let file = std::fs::File::open(&package).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut part = zip.by_name("word/document.xml").unwrap();
        let mut text = String::new();
        use std::io::Read;
        part.read_to_string(&mut text).unwrap();
        text
    };
    let document = read_document();
    assert!(document.contains(&format!("<w:tag w:val=\"rde:{insert_id}\"/>")), "sdt-тег вставки: {document}");
    assert!(document.contains(">10<"), "кэш значения в content-control");

    // Обновление значения: sdt-текст и rde:doc-кэш меняются, документ остаётся валидным.
    let mut values = std::collections::BTreeMap::new();
    values.insert(insert_id.clone(), "42.5".to_owned());
    let mut hashes = std::collections::BTreeMap::new();
    hashes.insert(insert_id.clone(), "cafe".to_owned());
    let updated = update_insert_values(&package, &values, &hashes).unwrap();
    assert_eq!(updated.updated, vec![insert_id.clone()]);
    let document = read_document();
    assert!(document.contains(">42.5<"), "новое значение в sdt: {document}");
    assert!(!document.contains(">10<"));
    let loaded = load_rde_doc(&package).unwrap();
    let insert = loaded.inserts.iter().find(|insert| insert.id == insert_id).unwrap();
    assert_eq!(insert.cached, "42.5");
    assert_eq!(insert.hash.as_deref(), Some("cafe"));
    assert_eq!(loaded.body_hash, rde_hash::sha256(document.as_bytes()).hex());
    // WordCraft по-прежнему открывает документ (добавился один абзац).
    let (doc, from_package) = open_document(&package).unwrap();
    assert!(from_package);
    assert_eq!(doc.paragraph_count(), paragraphs_before + 1);
    assert_eq!(check_external_edit(&package).unwrap().verdict, EditVerdict::NotModified);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tombstone_remains_when_rde_doc_lost() {
    let dir = tmp("tombstone");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();
    // Имитация потери customXml при пересохранении в Word: убираем только item1.xml.
    let stripped = dir.join("stripped.doc.rde");
    std::fs::write(&stripped, rewrite_part(&package, RDE_DOC_PART, &[], true)).unwrap();
    assert_eq!(check_external_edit(&stripped).unwrap().verdict, EditVerdict::MissingRdeDoc);
    let info = package_info(&stripped).unwrap();
    assert!(info.has_part("docProps/custom.xml"), "надгробие остаётся для диагностики");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sdt_and_figure_survive_document_update() {
    let dir = tmp("sdt-survive");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "gost-7.32-2017").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();
    // Рисунок-вставка из «выхода артефакта» (для теста — копия демо-рисунка из docx).
    let figure = add_figure_insert(&package, &clean, "rde://artifact/01A/out/plot.svg".into(), "Рисунок — тест".into(), "aa".into());
    assert!(figure.is_ok(), "{figure:?}");
    add_insert(
        &package,
        InsertRequest {
            kind: "value".into(),
            target: "rde://artifact/01A/out/stats.json#/mean".into(),
            selector: "#/mean".into(),
            value_kind: "number".into(),
            cached: "3.14".into(),
            caption: "среднее".into(),
            style: String::new(),
            format: "auto".into(),
        },
    )
    .unwrap();
    let read_document = || {
        let file = std::fs::File::open(&package).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut part = zip.by_name("word/document.xml").unwrap();
        let mut text = String::new();
        use std::io::Read;
        part.read_to_string(&mut text).unwrap();
        text
    };
    let before = read_document();
    assert!(before.contains("rde:v"), "sdt до обновления");
    let report = update_document_package(&package).unwrap();
    assert!(report.package.is_some());
    let after = read_document();
    assert!(after.contains("rde:v"), "sdt переживает обновление документа (WordCraft round-trip)");
    let media_parts: Vec<String> = {
        let file = std::fs::File::open(&package).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        (0..zip.len()).map(|index| zip.by_index(index).unwrap().name().to_owned()).collect()
    };
    assert!(media_parts.iter().any(|name| name.starts_with("word/media/rde-figure")), "media переживает обновление: {media_parts:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn svg_figure_is_rasterized_for_docview() {
    let dir = tmp("svg-raster");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();
    let svg = dir.join("plot.svg");
    std::fs::write(
        &svg,
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="60"><rect width="120" height="60" fill="#3366cc"/><text x="10" y="35" font-size="14">test</text></svg>"##,
    )
    .unwrap();
    add_figure_insert(&package, &svg, "rde://artifact/01A/out/plot.svg".into(), "Рисунок — SVG".into(), "aa".into()).unwrap();
    // Медиа-часть — PNG (DocView WordCraft не рендерит SVG), а не исходный SVG.
    let (media_name, media) = {
        let file = std::fs::File::open(&package).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let name = (0..zip.len())
            .map(|index| zip.by_index(index).unwrap().name().to_owned())
            .find(|name| name.starts_with("word/media/rde-figure"))
            .expect("медиа-часть рисунка");
        let mut part = zip.by_name(&name).unwrap();
        let mut bytes = Vec::new();
        use std::io::Read;
        part.read_to_end(&mut bytes).unwrap();
        (name, bytes)
    };
    assert!(media_name.ends_with(".png"), "SVG растеризован в PNG: {media_name}");
    assert!(media.starts_with(b"\x89PNG"), "PNG-сигнатура: {} байт", media.len());
    // Модель WordCraft видит рисунок (иначе DocView покажет серый прямоугольник).
    let (doc, from_package) = open_document(&package).unwrap();
    assert!(from_package);
    let story = wordcraft_engine::doc::StoryRef::Body;
    let images: usize = doc
        .para_paths(story)
        .iter()
        .filter_map(|path| doc.para(story, path))
        .map(|paragraph| paragraph.objects.iter().filter(|object| matches!(object, wordcraft_engine::doc::InlineObject::Image { .. })).count())
        .sum();
    assert!(images >= 2, "в модели два рисунка (исходный + вставленный): {images}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn insert_value_formats_are_configurable() {
    // R-DOC-11: формат результата артефакта в тексте — параметр вставки, а не «как вышло».
    let cases = [
        ("1234.5678", "int", "1235"),
        ("1234.5678", "dec:2", "1234.57"),
        ("1234.5678", "dec:0", "1235"),
        ("0.00075978125", "sci:3", "7.598·10^-4"),
        ("1234.5678", "sci:2", "1.23·10^3"),
        ("0.125", "percent:1", "12.5 %"),
        ("0.125", "percent:0", "12 %"), // Rust округляет половину к чётному
        ("42", "raw", "42"),
        ("1234.5678", "", "1234.5678"),
        ("не число", "dec:2", "не число"),
    ];
    for (raw, format, expected) in cases {
        assert_eq!(format_value(raw, format), expected, "format_value({raw}, {format})");
    }
    // auto: целые без дробной части, крупные/малые — стандартный вид.
    assert_eq!(format_value("42", "auto"), "42");
    assert_eq!(format_value("42.5", "auto"), "42.5");
    assert_eq!(format_value("0.00001", "auto"), "1.0000·10^-5");
}

#[test]
fn inserts_use_body_text_formatting_of_current_document() {
    let dir = tmp("insert-font");
    let clean = dir.join("clean.docx");
    copy_file(&demo_docx(), &clean);
    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "gost-7.32-2017").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();
    let report = add_insert(
        &package,
        InsertRequest {
            kind: "value".into(),
            target: "rde://artifact/01A/out/stats.json#/mean".into(),
            selector: "#/mean".into(),
            value_kind: "number".into(),
            cached: "1.23".into(),
            caption: "Среднее x1".into(),
            style: String::new(),
            format: "dec:2".into(),
        },
    )
    .unwrap();
    let read_document = || {
        let file = std::fs::File::open(&package).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let mut part = zip.by_name("word/document.xml").unwrap();
        let mut text = String::new();
        use std::io::Read;
        part.read_to_string(&mut text).unwrap();
        text
    };
    let paragraph_of = |xml: &str, tag: &str| -> String {
        let index = xml.find(tag).expect("тег вставки");
        let start = xml[..index].rfind("<w:p ").unwrap();
        let end = xml[index..].find("</w:p>").unwrap() + index + "</w:p>".len();
        xml[start..end].to_owned()
    };
    let tag = format!("<w:tag w:val=\"rde:{}\"", report.insert.id);
    let document = read_document();
    let paragraph = paragraph_of(&document, &tag);
    // Обычный текст текущего документа/шаблона — Times New Roman, а не docDefaults Aptos.
    assert!(paragraph.contains("w:rFonts"), "в абзаце вставки есть шрифт: {paragraph}");
    assert!(!paragraph.contains("Aptos"), "вставка не должна наследовать Aptos: {paragraph}");
    assert!(paragraph.contains("Times New Roman"), "шрифт обычного текста документа: {paragraph}");
    // Обновление значения сохраняет шрифт (rPr переносится в новый run).
    let mut values = std::collections::BTreeMap::new();
    values.insert(report.insert.id.clone(), "4.56".to_owned());
    update_insert_values(&package, &values, &std::collections::BTreeMap::new()).unwrap();
    let paragraph = paragraph_of(&read_document(), &tag);
    assert!(paragraph.contains("Times New Roman"), "rPr сохранён при обновлении: {paragraph}");
    assert!(paragraph.contains("4.56"));
    // Явный стиль пользователя: pStyle + форматирование стиля (или обычного текста).
    let styles = list_document_styles(&package).unwrap();
    if let Some(style) = styles.iter().find(|style| style.kind == "paragraph" && style.id != "Normal") {
        apply_insert_style(&package, &report.insert.id, &style.id).unwrap();
        let paragraph = paragraph_of(&read_document(), &tag);
        assert!(paragraph.contains(&format!("w:pStyle w:val=\"{}\"", style.id)), "pStyle {style:?}: {paragraph}");
        assert!(paragraph.contains("w:rFonts"), "у run'ов есть шрифт: {paragraph}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
