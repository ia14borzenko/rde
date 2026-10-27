//! P2.5: docx с `asvg:svgBlip` переживает roundtrip через движок WordCraft (open → save):
//! blip-extLst, SVG-часть, relationship и Content_Types сохраняются (ADR-0005, P2.5;
//! фикс writer'а — форк `vendor/wordcraft`, коммит `rde/main`).
//!
//! Фикстура написана по MS-ODRAWXML/ECMA-376 (файлов, произведённых Word, в репозитории нет).

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG_REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const CT_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const IMAGE_REL: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
/// The `a:ext` URI Word puts an SVG's `asvg:svgBlip` under (MS-ODRAWXML §2.3.1).
const SVG_BLIP_EXT: &str = "{96DAC541-7B7A-43D3-8B79-37D633B846F1}";
const SVG_NS: &str = "http://schemas.microsoft.com/office/drawing/2016/SVG/main";

fn png() -> Vec<u8> {
    b"\x89PNG\r\n\x1a\n a raster preview of the vector picture".to_vec()
}

fn svg() -> Vec<u8> {
    br##"<?xml version="1.0" encoding="UTF-8"?><svg xmlns="http://www.w3.org/2000/svg" width="144" height="72"><rect width="144" height="72" fill="#156082"/></svg>"##.to_vec()
}

fn zip(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut zw = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        zw.start_file(*name, zip::write::SimpleFileOptions::default()).unwrap();
        zw.write_all(bytes).unwrap();
    }
    zw.finish().unwrap().into_inner()
}

fn unzip(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for i in 0..z.len() {
        let mut f = z.by_index(i).unwrap();
        let mut b = Vec::new();
        f.read_to_end(&mut b).unwrap();
        out.insert(f.name().to_string(), b);
    }
    out
}

fn text(files: &BTreeMap<String, Vec<u8>>, name: &str) -> String {
    String::from_utf8(files.get(name).unwrap_or_else(|| panic!("no {name} in {:?}", files.keys())).clone()).unwrap()
}

fn drawing() -> String {
    format!(
        r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="1828800" cy="914400"/><wp:docPr id="1" name="Picture 1"/><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:pic><pic:nvPicPr><pic:cNvPr id="0" name="image1.png"/><pic:cNvPicPr/></pic:nvPicPr><pic:blipFill><a:blip r:embed="rIdImg"><a:extLst><a:ext uri="{SVG_BLIP_EXT}"><asvg:svgBlip xmlns:asvg="{SVG_NS}" r:embed="rIdSvg"/></a:ext><a:ext uri="{{28A0092B-C50C-407E-A947-70E740481C1C}}"><a14:useLocalDpi xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main" val="0"/></a:ext></a:extLst></a:blip><a:stretch><a:fillRect/></a:stretch></pic:blipFill><pic:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1828800" cy="914400"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></pic:spPr></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r></w:p>"#
    )
}

/// Мини-пакет: картинка с растровым preview и SVG-источником в `a:blip/a:extLst`.
fn package() -> Vec<u8> {
    let document = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W_NS}" xmlns:r="{REL_NS}" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><w:body>{}<w:p><w:r><w:t>Before</w:t></w:r></w:p><w:sectPr/></w:body></w:document>"#,
        drawing()
    );
    let root = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{PKG_REL_NS}"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#
    );
    let doc_rels = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="{PKG_REL_NS}"><Relationship Id="rIdImg" Type="{IMAGE_REL}" Target="media/image1.png"/><Relationship Id="rIdSvg" Type="{IMAGE_REL}" Target="media/image1.svg"/></Relationships>"#
    );
    let types = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="{CT_NS}"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Default Extension="png" ContentType="image/png"/><Default Extension="svg" ContentType="image/svg+xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#
    );
    zip(&[
        ("[Content_Types].xml", types.into_bytes()),
        ("_rels/.rels", root.into_bytes()),
        ("word/document.xml", document.into_bytes()),
        ("word/_rels/document.xml.rels", doc_rels.into_bytes()),
        ("word/media/image1.png", png()),
        ("word/media/image1.svg", svg()),
    ])
}

/// `word/document.xml` раундтрипнутого пакета сохраняет SVG-расширение и его часть.
fn assert_svg_blip_kept(path: &std::path::Path) {
    let saved = unzip(&fs::read(path).unwrap());
    let document = text(&saved, "word/document.xml");
    assert!(document.contains("svgBlip"), "svgBlip потерян при engine roundtrip: {document}");
    assert!(document.contains(SVG_BLIP_EXT), "URI расширения потерян: {document}");
    assert!(document.contains("useLocalDpi"), "остальной blip-extLst потерян: {document}");
    assert_eq!(saved.get("word/media/image1.svg").map(|b| b.as_slice()), Some(svg().as_slice()), "SVG-часть потеряна");
    let rels = text(&saved, "word/_rels/document.xml.rels");
    assert!(rels.contains("media/image1.svg") && rels.contains("/relationships/image"), "relationship на SVG потерян: {rels}");
    let types = text(&saved, "[Content_Types].xml");
    assert!(types.contains("image/svg+xml"), "Content_Types не объявляет SVG: {types}");
    // И relationship'ы, на которые ссылается blip, разрешаются в пакете.
    for id in document.match_indices("r:embed=\"").map(|(i, m)| &document[i + m.len()..]).filter_map(|r| r.split('"').next()) {
        assert!(rels.contains(&format!(r#"Id="{id}""#)), "blip ссылается на потерянный relationship {id}: {document}");
    }
}

#[test]
fn svg_blip_survives_engine_roundtrip() {
    let dir = rde_docx::temp_work_dir("svg-blip");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let src = dir.join("svg-blip.docx");
    let out = dir.join("svg-blip-roundtrip.docx");
    fs::write(&src, package()).unwrap();

    rde_docx::roundtrip(&src, &out).expect("roundtrip через движок WordCraft");
    assert_svg_blip_kept(&out);
    let parts = rde_docx::inventory(&out).unwrap();
    assert!(parts.iter().any(|p| p.name == "word/media/image1.svg"), "SVG-части нет в пакете раундтрипа");
    // Независимый оракул: пакет с svgBlip открывается LibreOffice (если он есть на стенде).
    let oracle = rde_docx::lo_oracle(&out, &dir.join("lo"));
    if oracle.available {
        assert!(oracle.pdf_is_valid, "LibreOffice не открыл docx с svgBlip: {:?}", oracle.error);
    }

    // Повторный open → save не теряет расширение: оно читается, а не только пишется.
    let out2 = dir.join("svg-blip-roundtrip2.docx");
    rde_docx::roundtrip(&out, &out2).expect("второй roundtrip");
    assert_svg_blip_kept(&out2);
}
