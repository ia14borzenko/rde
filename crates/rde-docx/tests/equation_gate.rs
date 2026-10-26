//! OMML-гейт (R-DOC-22): формулы не теряются при roundtrip WordCraft; негатив — вырезанный
//! OMML роняет гейт; LO-оракул собирает PDF для документа с формулой.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rde_docx::{compare_gate, compare_omml, lo_oracle, omml_stats, run_gate, temp_work_dir};

fn work(tag: &str) -> PathBuf {
    let dir = temp_work_dir(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Чистый `.docx` демо-отчёта — фикстура rde-doc (та же, что у тестов пакета).
fn demo_docx() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rde-doc/tests/fixtures/base-report.docx")
}

/// Документ с одной нумерованной формулой `(1)`: собираем `.doc.rde`, извлекаем чистый `.docx`.
fn formula_docx(dir: &Path) -> PathBuf {
    let clean = dir.join("clean.docx");
    std::fs::copy(demo_docx(), &clean).unwrap();
    let mut rde_doc = rde_doc::build_rde_doc(&clean, "01TESTDOC", "gost-7.32-2017").unwrap();
    let package = dir.join("report.doc.rde");
    rde_doc::write_package(&clean, &mut rde_doc, &package).unwrap();
    rde_doc::insert_equation(&package, "\\frac{a+b}{c}", Some(1)).unwrap();
    let out = dir.join("formula.docx");
    rde_doc::extract_docx(&package, &out).unwrap();
    out
}

fn has_soffice() -> bool {
    std::process::Command::new("soffice").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

/// Убрать все OMML-элементы из document.xml (имитация офиса, потерявшего формулы).
fn strip_math(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    loop {
        let Some(index) = rest.find("<m:oMath") else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..index]);
        let after = &rest[index..];
        let open_end = after.find('>').expect("конец открывающего тега OMML");
        if after[..=open_end].trim_end().ends_with("/>") {
            rest = &after[open_end + 1..];
            continue;
        }
        let (open, close) = if after.starts_with("<m:oMathPara") { ("<m:oMathPara", "</m:oMathPara>") } else { ("<m:oMath", "</m:oMath>") };
        assert!(after.starts_with(open), "неожиданный тег: {}", &after[..open.len().min(after.len())]);
        let close_index = after.find(close).unwrap_or_else(|| panic!("нет {close}"));
        rest = &after[close_index + close.len()..];
    }
    out
}

/// Переписать одну часть zip-пакета.
fn rewrite_part(input: &Path, output: &Path, part: &str, data: &[u8]) {
    let file = std::fs::File::open(input).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut writer = zip::ZipWriter::new(std::fs::File::create(output).unwrap());
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let name = entry.name().to_owned();
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).unwrap();
        writer.start_file(name.clone(), options).unwrap();
        if name == part { writer.write_all(data).unwrap() } else { writer.write_all(&bytes).unwrap() }
    }
    writer.finish().unwrap();
}

fn document_xml(path: &Path) -> String {
    let file = std::fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut part = zip.by_name("word/document.xml").unwrap();
    let mut text = String::new();
    part.read_to_string(&mut text).unwrap();
    text
}

#[test]
fn omml_stats_counts_equations() {
    let dir = work("omml-stats");
    let formula = formula_docx(&dir);
    let xml = document_xml(&formula);
    let stats = omml_stats(&xml);
    assert_eq!(stats.o_math, 1, "одна m:oMath: {stats:?}");
    assert_eq!(stats.o_math_para, 1, "display → m:oMathPara: {stats:?}");
    assert_eq!(stats.eq_arr, 0);
    assert!(stats.nodes > 0);
    assert_eq!(stats.empty, 0);
    assert_eq!(stats.signature.len(), 64, "sha256-подпись структуры");
    // Без формул — нулевые счётчики, пустой OMML не выдумывается.
    let empty = omml_stats(&document_xml(&demo_docx()));
    assert_eq!((empty.o_math, empty.o_math_para, empty.empty), (0, 0, 0));
    // Негатив: пустой OMML — дефект.
    let broken = omml_stats("<w:p><m:oMath/></w:p>");
    assert_eq!(broken.empty, 1);
    // Негатив: структурное сравнение ловит удалённую формулу.
    let check = compare_omml(&xml, &strip_math(&xml));
    assert!(!check.ok, "вырезанный OMML должен ломать сравнение");
    assert!(check.errors.iter().any(|error| error.contains("OMML вырезан")), "{:?}", check.errors);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gate_passes_on_formula_document() {
    let dir = work("formula-gate");
    let formula = formula_docx(&dir);
    let report = run_gate(&formula, &dir.join("gate")).unwrap();
    assert!(report.omml.ok, "OMML-гейт: {:?}", report.omml);
    assert_eq!(report.omml.source.o_math, 1);
    assert_eq!(report.omml.roundtrip.o_math, 1, "формула переживает roundtrip WordCraft");
    assert_eq!(report.omml.source.o_math_para, report.omml.roundtrip.o_math_para);
    assert_eq!(report.omml.source.signature, report.omml.roundtrip.signature, "структура OMML та же");
    assert_eq!(report.paragraphs_source, report.paragraphs_roundtrip);
    if has_soffice() {
        assert!(report.lo.pdf_is_valid, "LO: {:?}", report.lo);
        assert!(report.passed(), "гейт не пройден: {report:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gate_fails_when_omml_is_cut_out() {
    // Негативный тест гейта: source с формулой vs «roundtrip», из которого OMML вырезан.
    let dir = work("omml-negative");
    let formula = formula_docx(&dir);
    let stripped = dir.join("stripped.docx");
    let xml = document_xml(&formula);
    rewrite_part(&formula, &stripped, "word/document.xml", strip_math(&xml).as_bytes());

    let report = compare_gate(&formula, &stripped, &dir.join("gate")).unwrap();
    assert!(!report.omml.ok, "гейт должен упасть: {:?}", report.omml);
    assert!(report.omml.errors.iter().any(|error| error.contains("OMML вырезан")), "{:?}", report.omml.errors);
    assert_eq!(report.omml.source.o_math, 1);
    assert_eq!(report.omml.roundtrip.o_math, 0);
    assert!(!report.passed(), "FidelityReport::passed() обязан учитывать OMML");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lo_oracle_builds_pdf_for_formula_document() {
    // (г) независимый оракул: LibreOffice открывает документ с OMML и делает PDF.
    if !has_soffice() {
        eprintln!("LibreOffice не установлен — тест LO-оракула пропущен");
        return;
    }
    let dir = work("formula-lo");
    let formula = formula_docx(&dir);
    let oracle = lo_oracle(&formula, &dir);
    assert!(oracle.available, "{oracle:?}");
    assert!(!oracle.version.is_empty());
    assert!(oracle.pdf_is_valid, "{oracle:?}");
    assert!(oracle.pdf_bytes > 1024, "{oracle:?}");
    let _ = std::fs::remove_dir_all(&dir);
}
