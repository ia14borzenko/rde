//! Тесты конверсии MathType OLE-объектов (R-DOC-44) на синтетическом `.doc.rde`.

use super::convert_math_type_objects;
use crate::{RDE_DOC_PART, RdeDoc, render_rde_doc};

/// MTEF v5-стрим с одной формулой (LINE + CHAR + END). Заголовок: версия/платформа/продукт/
/// версия продукта/подверсия, NUL-строка приложения, inline.
fn mtef_char(mtcode: u16) -> Vec<u8> {
    let mut out = vec![5, 1, 0, 6, 9];
    out.extend_from_slice(b"MTEF\0");
    out.push(1);
    out.push(1); // LINE
    out.push(0); // options = 0
    out.push(2); // CHAR
    out.push(0); // options
    out.push(0x82); // typeface: 0x80 | 2 (FUNCTION)
    out.extend_from_slice(&mtcode.to_le_bytes());
    out.push(0); // END
    out
}

/// OLE Compound File с потоком `Equation Native` (EQNOLEFILEHDR + MTEF).
fn ole_with_mtef(mtef: &[u8]) -> Vec<u8> {
    let mut compound = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new())).expect("cfb");
    {
        let mut stream = compound.create_stream("Equation Native").expect("stream");
        let mut header = vec![28u8, 0];
        header.extend_from_slice(&[0u8; 6]);
        header.extend_from_slice(&(mtef.len() as u32).to_le_bytes());
        header.extend_from_slice(&[0u8; 16]);
        std::io::Write::write_all(&mut stream, &header).expect("header");
        std::io::Write::write_all(&mut stream, mtef).expect("body");
    }
    compound.into_inner().into_inner()
}

/// Синтетический `.doc.rde`: один MathType-объект (display-абзац) + `rde:doc`.
fn synthetic_package(path: &std::path::Path) {
    let document = r##"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:wpc="http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas" xmlns:o="urn:schemas-microsoft-com:office:office" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math" xmlns:v="urn:schemas-microsoft-com:vml" xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="MTDisplayEquation"/></w:pPr><w:r><w:rPr><w:position w:val="-28"/></w:rPr><w:object w:dxaOrig="740" w:dyaOrig="720"><v:shape id="_x0000_i1025" type="#_x0000_t75" style="width:37.1pt;height:36.65pt" o:ole=""><v:imagedata r:id="rId2" o:title=""/></v:shape><o:OLEObject Type="Embed" ProgID="Equation.DSMT4" ShapeID="_x0000_i1025" DrawAspect="Content" ObjectID="_1" r:id="rId1"/></w:object></w:r><w:r><w:t>текст</w:t></w:r></w:p></w:body></w:document>"##;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/oleObject" Target="embeddings/oleObject1.bin"/><Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.wmf"/></Relationships>"#;
    let ole = ole_with_mtef(&mtef_char(0x0078)); // 'x'
    let preview = vec![0xD7, 0xCD, 0xC6, 0x9A, 0, 0, 0, 0];
    let rde_doc = RdeDoc { schema_version: "2".to_owned(), body_hash: "old".to_owned(), ..RdeDoc::default() };
    let rde_xml = render_rde_doc(&rde_doc);
    let file = std::fs::File::create(path).expect("create zip");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    let mut add = |name: &str, bytes: &[u8]| {
        use std::io::Write;
        writer.start_file(name, options).expect("start");
        writer.write_all(bytes).expect("write");
    };
    add("word/document.xml", document.as_bytes());
    add("word/_rels/document.xml.rels", rels.as_bytes());
    add("word/embeddings/oleObject1.bin", &ole);
    add("word/media/image1.wmf", &preview);
    add(RDE_DOC_PART, rde_xml.as_bytes());
    writer.finish().expect("finish");
}

fn hex_decode(text: &str) -> Vec<u8> {
    text.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let hi = char::from(pair[0]).to_digit(16).unwrap_or(0);
            let lo = char::from(pair[1]).to_digit(16).unwrap_or(0);
            u8::try_from(hi * 16 + lo).unwrap_or(0)
        })
        .collect()
}

#[test]
fn math_type_object_becomes_native_formula_and_hidden_object() {
    let dir = std::env::temp_dir().join(format!("rde-doc-math-type-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let package = dir.join("doc.doc.rde");
    synthetic_package(&package);
    let report = convert_math_type_objects(&package).expect("convert");
    assert_eq!(report.found, 1);
    assert_eq!(report.converted, 1, "warnings: {:?}", report.warnings);
    assert_eq!(report.failed, 0, "warnings: {:?}", report.warnings);
    assert_eq!(report.display, 1, "display-формула");
    assert_eq!(report.formulas.len(), 1);
    let formula = &report.formulas[0];
    assert_eq!(formula.latex, "x");
    assert_eq!(formula.source, "word/embeddings/oleObject1.bin");
    assert_eq!(formula.preview, "word/media/image1.wmf");
    assert_eq!(formula.mtef_sha256.len(), 64);
    assert_eq!(hex_decode(&formula.ole_hex).first(), Some(&0xD0));
    assert_eq!(hex_decode(&formula.preview_hex), vec![0xD7, 0xCD, 0xC6, 0x9A, 0, 0, 0, 0]);

    // В документе больше нет OLE-объекта, вместо него — OMML-формула в том же абзаце.
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&package).expect("open")).expect("zip");
    let mut xml = String::new();
    {
        use std::io::Read;
        archive.by_name("word/document.xml").expect("document").read_to_string(&mut xml).expect("read");
    }
    assert!(!xml.contains("<w:object"), "OLE-объект заменён: {xml}");
    assert!(xml.contains("<m:oMathPara"), "display-формула: {xml}");
    assert!(xml.contains("текст"), "соседний текст сохранён");

    // rde:doc: скрытый объект, версия схемы, body_hash от нового document.xml.
    let doc = crate::load_rde_doc(&package).expect("rde:doc");
    assert_eq!(doc.schema_version, crate::SCHEMA_VERSION);
    assert_eq!(doc.formulas.len(), 1);
    assert_eq!(doc.formulas[0].latex, "x");
    assert_eq!(doc.body_hash, rde_hash::sha256(xml.as_bytes()).hex());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn package_without_math_type_is_untouched() {
    let dir = std::env::temp_dir().join(format!("rde-doc-math-type-none-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let package = dir.join("plain.doc.rde");
    let file = std::fs::File::create(&package).expect("create");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    {
        use std::io::Write;
        writer.start_file("word/document.xml", options).expect("start");
        writer
            .write_all(r#"<?xml version="1.0"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>без формул</w:t></w:r></w:p></w:body></w:document>"#.as_bytes())
            .expect("write");
        writer.start_file(RDE_DOC_PART, options).expect("start");
        writer.write_all(render_rde_doc(&RdeDoc::default()).as_bytes()).expect("write");
    }
    writer.finish().expect("finish");
    let before = std::fs::read(&package).expect("read");
    let report = convert_math_type_objects(&package).expect("convert");
    assert_eq!(report.found, 0);
    assert!(report.formulas.is_empty());
    // Пакет не перезаписан.
    assert_eq!(std::fs::read(&package).expect("read after"), before);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unreadable_mtef_keeps_the_object_and_reports_the_failure() {
    let dir = std::env::temp_dir().join(format!("rde-doc-math-type-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let package = dir.join("bad.doc.rde");
    synthetic_package(&package);
    // Испортить MTEF: положить мусор в стрим Equation Native.
    let mut archive = zip::ZipArchive::new(std::fs::File::open(&package).expect("open")).expect("zip");
    let mut entries = std::collections::BTreeMap::new();
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).expect("entry");
        let name = file.name().to_owned();
        let mut bytes = Vec::new();
        {
            use std::io::Read;
            file.read_to_end(&mut bytes).expect("read");
        }
        entries.insert(name, bytes);
    }
    drop(archive);
    let mut compound = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new())).expect("cfb");
    {
        let mut stream = compound.create_stream("Equation Native").expect("stream");
        let garbage = [0u8; 16];
        std::io::Write::write_all(&mut stream, &garbage).expect("write");
    }
    entries.insert("word/embeddings/oleObject1.bin".to_owned(), compound.into_inner().into_inner());
    let file = std::fs::File::create(&package).expect("create");
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    for (name, bytes) in &entries {
        use std::io::Write;
        writer.start_file(name.as_str(), options).expect("start");
        writer.write_all(bytes).expect("write");
    }
    writer.finish().expect("finish");
    let report = convert_math_type_objects(&package).expect("convert");
    assert_eq!(report.found, 1);
    assert_eq!(report.converted, 0);
    assert_eq!(report.failed, 1);
    assert!(!report.warnings.is_empty());
    let doc = crate::load_rde_doc(&package).expect("rde:doc");
    assert!(doc.formulas.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
