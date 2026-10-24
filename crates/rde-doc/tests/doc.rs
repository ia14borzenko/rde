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

fn demo_docx() -> PathBuf {
    let project = rde_core::load_project(&rde_core::demo_project_dir()).unwrap();
    project.document_path().expect("документ демо-проекта")
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
            RdeBlock { id: "b1".into(), path: "0".into(), kind: "heading1".into(), text: "Введение & «кавычки» <x>".into() },
            RdeBlock { id: "b2".into(), path: "1".into(), kind: "paragraph".into(), text: "Текст с амперсандом & и \"кавычками\"".into() },
        ],
        inserts: vec![RdeInsert {
            id: "i1".into(),
            block: "b3".into(),
            kind: "figure".into(),
            target: "rde://artifact/01ABC/out/plot.svg".into(),
            caption: "Рисунок 1 — График".into(),
            number: Some(1),
            hash: Some("sha256:ff".into()),
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

    let rde_doc = build_rde_doc(&clean, "01TESTDOC", "gost-7.32-2017").unwrap();
    assert!(rde_doc.blocks.len() >= 8, "блоков: {}", rde_doc.blocks.len());
    assert!(rde_doc.inserts.iter().any(|i| i.kind == "figure"), "вставки: {:?}", rde_doc.inserts);
    assert_eq!(rde_doc.body_hash.len(), 64);

    let package = dir.join("report.doc.rde");
    let info = write_package(&clean, &rde_doc, &package).unwrap();
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
    let rde_doc = build_rde_doc(&clean, "01TESTDOC", "").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &rde_doc, &package).unwrap();

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

    let rde_doc = build_rde_doc(&clean, "01TESTDOC", "").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &rde_doc, &package).unwrap();

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
    let rde_doc = build_rde_doc(&clean, "01TESTDOC", "gost-7.32-2017").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &rde_doc, &package).unwrap();
    let inserts_before = load_rde_doc(&package).unwrap().inserts.len();

    let saved = save_engine_document(&clean, &package, "IGNORED", "ignored-template").unwrap();
    assert_eq!(saved.doc_id, "01TESTDOC", "doc_id должен сохраняться");
    assert_eq!(saved.template, "gost-7.32-2017");
    assert_eq!(saved.inserts.len(), inserts_before);
    assert_eq!(check_external_edit(&package).unwrap().verdict, EditVerdict::NotModified);
    let _ = std::fs::remove_dir_all(&dir);
}
