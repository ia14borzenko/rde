//! ADR-0018: экспорт `.docx` для Word — math-шрифт OMML (STIX Two Math, Times-стиль):
//! `m:mathPr/m:mathFont` в `word/settings.xml` и `w:rFonts` math-ранов в `word/document.xml`,
//! OMML при этом остаётся OMML (R-DOC-22). Независимый оракул — LibreOffice: экспорт
//! конвертируется в PDF, номер формулы `(1)` стоит у правого края текстового поля (Word на
//! стенде нет — соответствие «по спецификации + LO», честное ограничение в docs/fidelity).

use std::path::{Path, PathBuf};

use rde_doc::{FormulaBody, FormulaInsertRequest, FormulaNumbering, add_formula_insert, compile_formula};

const DOC_ID: &str = "01M5VJ1JG04TFF59TDWH9EDD1R";
const FORMULA_ID: &str = "01M5VJ1JG039MMV97SZGY6V9EQ";

fn poppler_available() -> bool {
    std::process::Command::new("pdftocairo").arg("-v").output().map(|output| output.status.success()).unwrap_or(false)
}

fn soffice_available() -> bool {
    std::process::Command::new("soffice").arg("--version").output().map(|output| output.status.success()).unwrap_or(false)
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-doc-export-font-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_project(root: &Path) {
    std::fs::write(
        root.join("rde.toml"),
        "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"Шрифт формул\"\n",
    )
    .unwrap();
    let dir = root.join("objects/formulas").join(FORMULA_ID);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("formula.toml"),
        format!(
            "schema = \"rde/formula@1\"\nid = \"{FORMULA_ID}\"\ntitle = \"σ\"\nlatex = \"\\\\sigma=\\\\sqrt{{\\\\frac{{F}}{{A}}}}\"\ndisplay = true\n\n[numbering]\nmode = \"auto\"\nlabel = \"\"\n"
        ),
    )
    .unwrap();
}

/// Пакет `.doc.rde` с нумерованной display-формулой (tab-вёрстка, ADR-0018).
fn package_with_formula(root: &Path) -> PathBuf {
    let document = wordcraft_engine::doc::Document::from_text("Формула:");
    let docx = root.join("clean.docx");
    wordcraft_engine::io::save_path(&docx, &document).unwrap();
    let mut rde_doc = rde_doc::build_rde_doc(&docx, DOC_ID, "").unwrap();
    let package = root.join("report.doc.rde");
    rde_doc::write_package(&docx, &mut rde_doc, &package).unwrap();
    let caches = compile_formula("\\sigma=\\sqrt{\\frac{F}{A}}", true).expect("компиляция формулы");
    add_formula_insert(
        &package,
        FormulaInsertRequest {
            body: FormulaBody {
                formula_id: FORMULA_ID.to_owned(),
                act_key: "act-font".to_owned(),
                latex: "\\sigma=\\sqrt{\\frac{F}{A}}".to_owned(),
                display: true,
                numbered: true,
                omml: caches.omml,
                svg: caches.svg,
                png: caches.png,
            },
            render_mode: "omml".to_owned(),
            anchor_block: None,
            numbering: FormulaNumbering::Continuous,
        },
    )
    .expect("вставка формулы");
    package
}

fn part(docx: &Path, name: &str) -> String {
    let file = std::fs::File::open(docx).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut entry = archive.by_name(name).unwrap();
    let mut out = String::new();
    std::io::Read::read_to_string(&mut entry, &mut out).unwrap();
    out
}

/// Атрибут twips (`w:w`/`w:right`) из первого `w:sectPr` документа.
fn sect_attr(xml: &str, tag: &str, name: &str) -> Option<f32> {
    let start = xml.find(tag)?;
    let end = xml[start..].find('>')? + start;
    let block = &xml[start..end];
    let marker = format!("{name}=\"");
    let index = block.find(&marker)? + marker.len();
    let rest = &block[index..];
    let end = rest.find('"')?;
    rest[..end].parse::<f32>().ok()
}

/// Правый край текстового поля в pt (A4/letter из sectPr).
fn text_right_edge_pt(document_xml: &str) -> f32 {
    let page_w = sect_attr(document_xml, "<w:pgSz", "w:w").unwrap_or(11906.0);
    let right = sect_attr(document_xml, "<w:pgMar", "w:right").unwrap_or(850.0);
    (page_w - 2.0 * right) / 20.0 + right / 20.0
}

#[test]
fn export_sets_math_font_and_keeps_omml() {
    if !poppler_available() {
        return;
    }
    let root = tmp("settings");
    write_project(&root);
    let package = package_with_formula(&root);
    let docx = root.join("export.docx");
    rde_doc::export_docx(&package, &docx).expect("экспорт docx");
    let settings = part(&docx, "word/settings.xml");
    assert!(settings.contains("<m:mathPr"), "settings: m:mathPr добавлен: {settings}");
    assert!(
        settings.contains("<m:mathFont m:val=\"STIX Two Math\"/>"),
        "settings: m:mathFont = STIX Two Math (Times-стиль): {settings}"
    );
    assert!(settings.contains("xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\""), "namespace m объявлен");
    let document = part(&docx, "word/document.xml");
    assert!(document.contains("w:ascii=\"STIX Two Math\""), "math-раны OMML используют STIX Two Math");
    assert!(!document.contains("Cambria Math"), "жёстко зашитый Cambria Math заменён: {}", &document[..document.len().min(400)]);
    // OMML остаётся OMML: гейт R-DOC-22 не ломается.
    let package_xml = {
        let file = std::fs::File::open(&package).unwrap();
        let mut archive = zip::ZipArchive::new(file).unwrap();
        let mut entry = archive.by_name("word/document.xml").unwrap();
        let mut out = String::new();
        std::io::Read::read_to_string(&mut entry, &mut out).unwrap();
        out
    };
    assert_eq!(document.matches("<m:oMathPara>").count(), package_xml.matches("<m:oMathPara>").count(), "display OMML сохранён");
    assert_eq!(document.matches("<m:oMath>").count(), package_xml.matches("<m:oMath>").count(), "OMML сохранён");
    // Экспорт не содержит RDE-частей.
    let file = std::fs::File::open(&docx).unwrap();
    let archive = zip::ZipArchive::new(file).unwrap();
    assert!(archive.file_names().all(|name| !name.starts_with("customXml/item1.xml")), "RDE-часть вырезана");
}

#[test]
fn export_math_font_family_override() {
    if !poppler_available() {
        return;
    }
    let root = tmp("override");
    write_project(&root);
    let package = package_with_formula(&root);
    let docx = root.join("export.docx");
    rde_doc::export_docx(&package, &docx).expect("экспорт docx");
    assert!(rde_doc::patch_export_math_font_with(&docx, "TeX Gyre Termes Math").expect("патч шрифта"));
    let settings = part(&docx, "word/settings.xml");
    assert!(settings.contains("<m:mathFont m:val=\"TeX Gyre Termes Math\"/>"), "переопределение шрифта: {settings}");
    let document = part(&docx, "word/document.xml");
    assert!(document.contains("w:ascii=\"TeX Gyre Termes Math\""));
}

#[test]
fn libreoffice_renders_number_at_right_margin() {
    if !poppler_available() || !soffice_available() {
        return; // LO-оракул недоступен — честный skip
    }
    let root = tmp("lo");
    write_project(&root);
    let package = package_with_formula(&root);
    let docx = root.join("export.docx");
    rde_doc::export_docx(&package, &docx).expect("экспорт docx");
    let pdf = root.join("export.pdf");
    let status = std::process::Command::new("soffice")
        .args(["--headless", "--convert-to", "pdf", "--outdir"])
        .arg(&root)
        .arg(&docx)
        .output()
        .expect("soffice запуск");
    assert!(status.status.success(), "LibreOffice: {}", String::from_utf8_lossy(&status.stderr));
    assert!(pdf.is_file(), "LO создал PDF");
    assert!(std::fs::metadata(&pdf).map(|meta| meta.len() > 1024).unwrap_or(false), "PDF непустой");
    let bbox = std::process::Command::new("pdftotext").args(["-bbox"]).arg(&pdf).arg("-").output().expect("pdftotext");
    let text = String::from_utf8_lossy(&bbox.stdout);
    let mut number_box: Option<(f32, f32)> = None;
    for line in text.lines() {
        if !line.contains("<word") || !line.contains("(1)") {
            continue;
        }
        let attr = |name: &str| -> Option<f32> {
            let marker = format!("{name}=\"");
            let index = line.find(&marker)? + marker.len();
            let rest = &line[index..];
            let end = rest.find('"')?;
            rest[..end].parse().ok()
        };
        if let (Some(x_min), Some(x_max)) = (attr("xMin"), attr("xMax")) {
            number_box = Some((x_min, x_max));
        }
    }
    let (x_min, x_max) = number_box.expect("номер (1) в PDF-оракуле");
    let document = part(&docx, "word/document.xml");
    let right_edge = text_right_edge_pt(&document);
    assert!(x_max <= right_edge + 1.5, "номер не выходит за правый край текста: {x_max:.1} pt > {right_edge:.1} pt");
    assert!(x_max >= right_edge - 3.0, "номер прижат к правому краю текста: {x_max:.1} pt vs {right_edge:.1} pt");
    assert!(x_min < right_edge - 5.0, "номер — отдельный текст у правого поля: {x_min:.1}..{x_max:.1}");
    let header = std::fs::read(&pdf).unwrap();
    assert!(header.starts_with(b"%PDF"), "LO-оракул: валидный PDF");
}
