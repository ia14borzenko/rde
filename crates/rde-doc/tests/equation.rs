//! Формулы (R-DOC-21/22, фаза 1 — OMML-фундамент): вставка через движок WordCraft,
//! нумерация таблицей 1×2, roundtrip пакета и чистое извлечение .docx, перенумерация.

use std::io::Read;
use std::path::{Path, PathBuf};

use rde_doc::*;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-doc-equation-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Стабильная фикстура: чистый .docx демо-отчёта (без живых вставок).
fn demo_docx() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/base-report.docx")
}

/// Пакет `.doc.rde` из фикстуры.
fn package(dir: &Path) -> PathBuf {
    let clean = dir.join("clean.docx");
    std::fs::copy(demo_docx(), &clean).unwrap();
    let mut rde_doc = build_rde_doc(&clean, "01TESTDOC", "gost-7.32-2017").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&clean, &mut rde_doc, &package).unwrap();
    package
}

fn read_document_xml(path: &Path) -> String {
    let file = std::fs::File::open(path).unwrap();
    let mut zip = zip::ZipArchive::new(file).unwrap();
    let mut part = zip.by_name("word/document.xml").unwrap();
    let mut text = String::new();
    part.read_to_string(&mut text).unwrap();
    text
}

/// Упорядоченная последовательность элементов `m:*` — структурное сравнение OMML.
fn omml_signature(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(index) = rest.find("<m:") {
        let after = &rest[index + "<m:".len()..];
        let end = after.find(|c: char| c.is_whitespace() || c == '>' || c == '/').unwrap_or(after.len());
        out.push(after[..end].to_owned());
        rest = &after[end..];
    }
    out
}

/// Фрагмент таблицы, содержащий подстроку (для проверки ячейки с номером).
fn enclosing_table(xml: &str, needle: &str) -> String {
    let index = xml.find(needle).unwrap_or_else(|| panic!("нет {needle:?} в document.xml"));
    let start = xml[..index].rfind("<w:tbl>").expect("таблица вокруг номера");
    let end = xml[start..].find("</w:tbl>").expect("конец таблицы") + start + "</w:tbl>".len();
    xml[start..end].to_owned()
}

#[test]
fn equation_syntax_is_detected() {
    assert_eq!(EquationSyntax::detect("\\frac{a}{b}"), EquationSyntax::Latex);
    assert_eq!(EquationSyntax::detect("a/b"), EquationSyntax::Linear);
    assert_eq!(EquationSyntax::detect("E = mc^2"), EquationSyntax::Linear);
    // LaTeX и linear дают одно и то же дерево для дроби.
    let latex = EquationSyntax::Latex.parse("\\frac{a+b}{c}");
    let linear = EquationSyntax::Linear.parse("(a+b)/c");
    assert_eq!(latex, linear);
}

#[test]
fn equation_survives_package_roundtrip_as_omml() {
    // (а) вставка → save_engine_document → .doc.rde → extract_docx: OMML на месте, структура та же.
    let dir = tmp("roundtrip");
    let package = package(&dir);
    let report = insert_equation(&package, "\\frac{a+b}{c}", Some(1)).unwrap();
    assert_eq!(report.id, "e1");
    assert_eq!(report.number, Some(1));
    assert_eq!(report.display, true);
    assert!(report.block.ends_with("/0/0/0"), "формула в таблице 1×2: {}", report.block);
    assert!(report.para_id.is_some(), "у абзаца формулы есть w14:paraId");
    assert_eq!(report.latex, "\\frac{a+b}{c}");
    assert_eq!(report.linear, "(a+b)/c");

    let in_package = read_document_xml(&package);
    assert_eq!(in_package.matches("<m:oMath>").count(), 1, "одна формула в пакете");
    assert_eq!(in_package.matches("<m:oMathPara>").count(), 1, "display-формула — m:oMathPara");
    assert_eq!(in_package.matches("<w:tbl>").count(), 1, "нумерация — таблица 1×2");
    assert!(in_package.contains(">(1)<"), "номер справа в скобках: {in_package}");
    let table = enclosing_table(&in_package, ">(1)<");
    assert!(table.contains("<m:oMathPara>"), "формула в левой ячейке: {table}");
    assert!(table.contains("w:jc w:val=\"right\""), "номер выровнен по правому краю: {table}");
    let signature = omml_signature(&in_package);

    // Сохранение из UI (движок → пакет) не меняет структуру формулы.
    let editor_docx = dir.join("editor.docx");
    extract_docx(&package, &editor_docx).unwrap();
    let saved = save_engine_document(&editor_docx, &package, "", "").unwrap();
    assert_eq!(saved.doc_id, "01TESTDOC");
    let resaved = read_document_xml(&package);
    assert_eq!(omml_signature(&resaved), signature, "структура OMML переживает сохранение");
    assert_eq!(resaved.matches("<m:oMath>").count(), 1);
    assert!(resaved.contains(">(1)<"));

    // Чистый .docx для Word: OMML на месте и структурно тот же.
    let clean = dir.join("export.docx");
    extract_docx(&package, &clean).unwrap();
    let exported = read_document_xml(&clean);
    assert_eq!(omml_signature(&exported), signature, "структура OMML переживает извлечение .docx");
    assert_eq!(exported.matches("<m:oMathPara>").count(), 1);
    assert!(exported.contains("<m:f>"), "дробь — OMML-структура, а не текст: {exported}");
    assert!(!exported.contains("customXml/item1.xml"), "RDE-части вырезаны");

    // Движок WordCraft видит ровно одну display-формулу.
    let (document, from_package) = open_document(&package).unwrap();
    assert!(from_package);
    let story = wordcraft_engine::doc::StoryRef::Body;
    let equations: Vec<bool> = document
        .para_paths(story)
        .iter()
        .filter_map(|path| document.para(story, path))
        .flat_map(|paragraph| paragraph.objects.iter())
        .filter_map(|object| match object {
            wordcraft_engine::doc::InlineObject::Equation { display, .. } => Some(*display),
            _ => None,
        })
        .collect();
    assert_eq!(equations, vec![true], "одна display-формула в модели");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn equations_are_numbered_in_document_order() {
    // (б) нумерация: два номера → перенумерация (1), (2); номер справа в скобках.
    let dir = tmp("numbering");
    let package = package(&dir);
    let first = insert_equation(&package, "\\frac{a}{b}", Some(5)).unwrap();
    let second = add_equation(
        &package,
        EquationRequest { formula: "E = mc^2".into(), number: Some(9), display: true, anchor_block: None },
    )
    .unwrap();
    assert_eq!(first.id, "e1");
    assert_eq!(second.id, "e2");

    let listed = list_equations(&package).unwrap();
    assert_eq!(listed.len(), 2, "{listed:?}");
    assert_eq!(listed[0].block, first.block);
    assert_eq!(listed[0].number, Some(5));
    assert_eq!(listed[0].latex, "\\frac{a}{b}");
    assert!(listed[0].display);
    assert_eq!(listed[1].number, Some(9));
    assert!(listed[1].latex.starts_with("E=m"), "LaTeX второй формулы: {}", listed[1].latex);

    let xml = read_document_xml(&package);
    assert!(xml.contains(">(5)<") && xml.contains(">(9)<"), "номера в документе: {xml}");

    // Сквозная перенумерация: (5),(9) → (1),(2).
    let changes = renumber_equations(&package).unwrap();
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert_eq!((changes[0].old_number, changes[0].new_number), (5, 1));
    assert_eq!((changes[1].old_number, changes[1].new_number), (9, 2));
    let xml = read_document_xml(&package);
    assert!(xml.contains(">(1)<") && xml.contains(">(2)<"), "{xml}");
    assert!(!xml.contains(">(5)<") && !xml.contains(">(9)<"), "старые номера заменены: {xml}");
    let listed = list_equations(&package).unwrap();
    assert_eq!(listed.iter().map(|equation| equation.number).collect::<Vec<_>>(), vec![Some(1), Some(2)]);
    // Второй прогон идемпотентен.
    assert!(renumber_equations(&package).unwrap().is_empty());

    // «Обновить документ» (пересохранение пакета через движок) не теряет формулы и номера.
    let update = update_document_package(&package).unwrap();
    assert!(update.package.is_some());
    let xml = read_document_xml(&package);
    assert_eq!(xml.matches("<m:oMath>").count(), 2);
    assert!(xml.contains(">(1)<") && xml.contains(">(2)<"), "номера переживают обновление: {xml}");
    assert_eq!(list_equations(&package).unwrap().len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn inline_equation_is_inserted_into_paragraph() {
    let dir = tmp("inline");
    let package = package(&dir);
    let report = add_equation(
        &package,
        EquationRequest { formula: "x^2+y^2".into(), number: None, display: false, anchor_block: Some(2) },
    )
    .unwrap();
    assert!(!report.display);
    assert_eq!(report.number, None);
    assert_eq!(report.block, "2");
    assert_eq!(report.linear, "x^2+y^2");

    let xml = read_document_xml(&package);
    // Inline-формула — m:oMath внутри абзаца, без m:oMathPara и без таблицы.
    assert_eq!(xml.matches("<m:oMath>").count(), 1);
    assert_eq!(xml.matches("<m:oMathPara>").count(), 0, "inline не оборачивается в oMathPara");
    assert_eq!(xml.matches("<w:tbl>").count(), 0, "inline-формуле таблица не нужна");
    let listed = list_equations(&package).unwrap();
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].display);
    assert_eq!(listed[0].block, "2");
    assert_eq!(listed[0].number, None);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn equation_api_rejects_bad_requests() {
    let dir = tmp("bad");
    let package = package(&dir);
    assert!(insert_equation(&package, "   ", None).is_err(), "пустая формула");
    assert!(insert_equation(&package, "\\frac{a}{b}", Some(0)).is_err(), "номер с 1");
    assert!(
        add_equation(&package, EquationRequest { formula: "x^2".into(), number: None, display: false, anchor_block: None }).is_err(),
        "inline без anchor_block"
    );
    assert!(
        add_equation(&package, EquationRequest { formula: "x^2".into(), number: None, display: true, anchor_block: Some(9999) }).is_err(),
        "anchor_block вне документа"
    );
    // Не .doc.rde — отказ, а не порча файла.
    let clean = dir.join("clean-only.docx");
    std::fs::copy(demo_docx(), &clean).unwrap();
    assert!(insert_equation(&clean, "x^2", None).is_err(), "не .doc.rde");
    let _ = std::fs::remove_dir_all(&dir);
}
