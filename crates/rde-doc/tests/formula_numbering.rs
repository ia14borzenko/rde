//! ADR-0018: схемы нумерации формул проекта в документе (сквозная/по разделам/приложения),
//! автоперенумерация и перекрёстные ссылки на формулы `rde://formula/<id>` (B3).
//!
//! Кэши формул — реальная компиляция (`rde_doc::compile_formula`, poppler); без poppler — честный
//! skip, как в тестах формульных вставок.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rde_doc::{
    FormulaBody, FormulaInsertRequest, FormulaNumbering, add_formula_insert, add_insert, compile_formula, load_rde_doc,
    next_formula_number_text, renumber_formula_numbers, renumber_insert_captions, update_formula_inserts_with,
};

const FORMULA_A: &str = "01M5VJ1JG039MMV97SZGY6V9EQ";
const FORMULA_B: &str = "01M5VJ1JG0HD99FDTGJ8YEPFZX";
const DOC_ID: &str = "01M5VJ1JG04TFF59TDWH9EDD1R";

fn poppler_available() -> bool {
    std::process::Command::new("pdftocairo").arg("-v").output().map(|output| output.status.success()).unwrap_or(false)
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-doc-numbering-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Проект с двумя project-level формулами.
fn write_project(root: &Path) {
    std::fs::write(
        root.join("rde.toml"),
        "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"Нумерация\"\n",
    )
    .unwrap();
    for (id, latex) in [(FORMULA_A, "\\\\sigma=\\\\frac{F}{A}"), (FORMULA_B, "\\\\tau=\\\\frac{2R}{c}")] {
        let dir = root.join("objects/formulas").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("formula.toml"),
            format!(
                "schema = \"rde/formula@1\"\nid = \"{id}\"\ntitle = \"{id}\"\nlatex = \"{latex}\"\ndisplay = true\n\n[numbering]\nmode = \"auto\"\nlabel = \"\"\n"
            ),
        )
        .unwrap();
    }
}

/// Пакет `.doc.rde` с двумя разделами: «1 Введение», «2 Метод» (уровень заголовка — прямо в
/// `w:pPr/w:outlineLvl`, как у заголовков Word).
fn new_package(root: &Path) -> PathBuf {
    let mut document = wordcraft_engine::doc::Document::from_text("1 Введение\nТекст раздела\n2 Метод\nТекст метода");
    for index in [0usize, 2] {
        let paragraph = document.para_mut(wordcraft_engine::doc::StoryRef::Body, &wordcraft_engine::doc::Path::top(index)).unwrap();
        paragraph.props.outline_level = Some(0);
    }
    let docx = root.join("clean.docx");
    wordcraft_engine::io::save_path(&docx, &document).unwrap();
    let mut rde_doc = rde_doc::build_rde_doc(&docx, DOC_ID, "").unwrap();
    let package = root.join("report.doc.rde");
    rde_doc::write_package(&docx, &mut rde_doc, &package).unwrap();
    package
}

fn body(package: &Path) -> String {
    let file = std::fs::File::open(package).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut entry = archive.by_name("word/document.xml").unwrap();
    let mut out = String::new();
    std::io::Read::read_to_string(&mut entry, &mut out).unwrap();
    out
}

fn compiled_body(formula_id: &str, latex: &str, act_key: &str) -> FormulaBody {
    let caches = compile_formula(latex, true).expect("компиляция формулы");
    FormulaBody {
        formula_id: formula_id.to_owned(),
        act_key: act_key.to_owned(),
        latex: latex.to_owned(),
        display: true,
        numbered: true,
        omml: caches.omml,
        svg: caches.svg,
        png: caches.png,
    }
}

fn insert_formula(package: &Path, formula_id: &str, latex: &str, act_key: &str, anchor: Option<usize>, numbering: FormulaNumbering) {
    let body = compiled_body(formula_id, latex, act_key);
    add_formula_insert(
        package,
        FormulaInsertRequest { body, render_mode: "omml".to_owned(), anchor_block: anchor, numbering },
    )
    .expect("вставка формулы");
}

/// Вставки формул документа в порядке следования.
fn formula_numbers(package: &Path) -> Vec<(String, Option<String>)> {
    let doc = load_rde_doc(package).unwrap();
    let mut inserts: Vec<_> = doc.inserts.iter().filter(|insert| insert.kind == "formula").collect();
    let xml = body(package);
    let position = |id: &str| xml.find(&format!("rde:formula:{id}")).unwrap_or(usize::MAX);
    inserts.sort_by_key(|insert| position(&insert.id));
    inserts.iter().map(|insert| (insert.id.clone(), insert.number_text.clone())).collect()
}

#[test]
fn tab_layout_has_no_table_and_right_number() {
    if !poppler_available() {
        return;
    }
    let root = tmp("layout");
    write_project(&root);
    let package = new_package(&root);
    insert_formula(&package, FORMULA_A, "\\sigma=\\frac{F}{A}", "act-a", Some(0), FormulaNumbering::Continuous);
    let xml = body(&package);
    let sdt = {
        let start = xml.find("rde:formula:").unwrap();
        &xml[start..xml.len().min(start + 1200)]
    };
    assert!(sdt.contains("<w:tabs>"), "абзац формулы с таб-стопами: {sdt}");
    assert!(sdt.contains("w:val=\"center\""), "центр-таб: {sdt}");
    assert!(sdt.contains("w:val=\"right\""), "правый таб: {sdt}");
    assert!(sdt.contains("<w:tab/>"), "табы-раны: {sdt}");
    assert!(sdt.contains("<m:oMathPara>"), "display OMML на месте: {sdt}");
    assert!(sdt.contains("(1)"), "номер в тексте абзаца: {sdt}");
    assert!(!sdt.contains("<w:tbl>"), "видимой таблицы больше нет: {sdt}");
}

#[test]
fn section_scheme_renumbers_and_switches_live() {
    if !poppler_available() {
        return;
    }
    let root = tmp("section");
    write_project(&root);
    let package = new_package(&root);
    // Вставка 1 — после заголовка «1 Введение»; вставка 2 — после заголовка «2 Метод»
    // (индексы блоков сдвигаются после первой вставки).
    insert_formula(&package, FORMULA_A, "\\sigma=\\frac{F}{A}", "act-a", Some(0), FormulaNumbering::Section);
    insert_formula(&package, FORMULA_B, "\\tau=\\frac{2R}{c}", "act-b", Some(3), FormulaNumbering::Section);
    let numbers = formula_numbers(&package);
    assert_eq!(numbers.len(), 2, "{numbers:?}");
    assert_eq!(numbers[0].1.as_deref(), Some("1.1"), "формула первого раздела: {numbers:?}");
    assert_eq!(numbers[1].1.as_deref(), Some("2.1"), "формула второго раздела: {numbers:?}");
    let xml = body(&package);
    assert!(xml.contains("(1.1)") && xml.contains("(2.1)"), "номера в теле документа (ГОСТ): схема по разделам");
    // Переключение на сквозную схему — номера меняются без пересборки тел.
    let report = renumber_formula_numbers(&package, FormulaNumbering::Continuous).unwrap();
    assert_eq!(report.formulas.len(), 2, "{report:?}");
    assert!(report.formulas.iter().any(|(_, number)| number == "1") && report.formulas.iter().any(|(_, number)| number == "2"));
    let numbers = formula_numbers(&package);
    assert_eq!(numbers[0].1.as_deref(), Some("1"));
    assert_eq!(numbers[1].1.as_deref(), Some("2"));
    // Идемпотентность: повторный проход ничего не меняет.
    let again = renumber_formula_numbers(&package, FormulaNumbering::Continuous).unwrap();
    assert!(again.formulas.is_empty() && again.crossrefs.is_empty(), "{again:?}");
    // И обратно на схемы по разделам.
    renumber_formula_numbers(&package, FormulaNumbering::Section).unwrap();
    let numbers = formula_numbers(&package);
    assert_eq!(numbers[0].1.as_deref(), Some("1.1"));
    assert_eq!(numbers[1].1.as_deref(), Some("2.1"));
}

#[test]
fn formula_crossref_updates_on_renumber_and_missing_target_placeholder() {
    if !poppler_available() {
        return;
    }
    let root = tmp("crossref");
    write_project(&root);
    let package = new_package(&root);
    insert_formula(&package, FORMULA_A, "\\sigma=\\frac{F}{A}", "act-a", Some(0), FormulaNumbering::Continuous);
    // Ссылка на формулу проекта — как её вставляет диалог «Перекрёстная ссылка».
    let crossref = add_insert(
        &package,
        rde_doc::InsertRequest {
            kind: "crossref".to_owned(),
            target: format!("rde://formula/{FORMULA_A}"),
            selector: String::new(),
            value_kind: "text".to_owned(),
            cached: "(?)".to_owned(),
            caption: "см. формулу".to_owned(),
            style: String::new(),
            format: "raw".to_owned(),
            run_style: String::new(),
            anchor_block: None,
        },
    )
    .expect("вставка ссылки");
    renumber_insert_captions(&package).unwrap();
    let doc = load_rde_doc(&package).unwrap();
    let reference = doc.inserts.iter().find(|insert| insert.id == crossref.insert.id).unwrap();
    assert_eq!(reference.cached, "(1)", "ссылка на формулу получила номер: {reference:?}");
    assert!(body(&package).contains("(1)"));
    // Перенумерация: схема по разделам → ссылка обновляется автоматически.
    let report = renumber_formula_numbers(&package, FormulaNumbering::Section).unwrap();
    assert_eq!(report.crossrefs, vec![(crossref.insert.id.clone(), "(1.1)".to_owned())], "{report:?}");
    let doc = load_rde_doc(&package).unwrap();
    assert_eq!(doc.inserts.iter().find(|insert| insert.id == crossref.insert.id).unwrap().cached, "(1.1)");
    // Цель-формула удалена из документа (вставки нет): placeholder в тексте, не тихая ошибка.
    let missing = add_insert(
        &package,
        rde_doc::InsertRequest {
            kind: "crossref".to_owned(),
            target: format!("rde://formula/{FORMULA_B}"),
            selector: String::new(),
            value_kind: "text".to_owned(),
            cached: "(?)".to_owned(),
            caption: "см. формулу".to_owned(),
            style: String::new(),
            format: "raw".to_owned(),
            run_style: String::new(),
            anchor_block: None,
        },
    )
    .expect("вставка ссылки на отсутствующую формулу");
    let report = renumber_formula_numbers(&package, FormulaNumbering::Section).unwrap();
    assert!(
        report.warnings.iter().any(|warning| warning.contains(FORMULA_B) || warning.contains("placeholder")),
        "предупреждение о ссылке без цели: {report:?}"
    );
    let doc = load_rde_doc(&package).unwrap();
    assert_eq!(doc.inserts.iter().find(|insert| insert.id == missing.insert.id).unwrap().cached, "(?)");
}

#[test]
fn update_pass_assigns_section_numbers_and_next_number_preview() {
    if !poppler_available() {
        return;
    }
    let root = tmp("update");
    write_project(&root);
    let package = new_package(&root);
    insert_formula(&package, FORMULA_A, "\\sigma=\\frac{F}{A}", "act-a", Some(0), FormulaNumbering::Continuous);
    // Превью следующего номера (для диалога вставки): после «1 Введение» при схеме по разделам
    // следующая формула получила бы (1.1) — но сначала документ на сквозной: (2).
    let (next, warnings) = next_formula_number_text(&package, FormulaNumbering::Continuous).expect("превью номера");
    assert_eq!(next, "2", "{warnings:?}");
    // Превью считает позицию «в конец документа» — после «2 Метод», т. е. раздел 2.
    let (next, _) = next_formula_number_text(&package, FormulaNumbering::Section).expect("превью номера");
    assert_eq!(next, "2.1", "конец документа — второй раздел");
    // Обновление документа по схеме разделов: номер формулы пересобран в теле, number_text в rde:doc.
    let bodies = BTreeMap::from([(FORMULA_A.to_owned(), compiled_body(FORMULA_A, "\\sigma=\\frac{F}{A}", "act-a"))]);
    let report = update_formula_inserts_with(&package, &bodies, FormulaNumbering::Section).unwrap();
    assert!(report.updated.len() == 1, "{report:?}");
    let numbers = formula_numbers(&package);
    assert_eq!(numbers[0].1.as_deref(), Some("1.1"));
    assert!(body(&package).contains("(1.1)"));
}

#[test]
fn schema_version_4_roundtrips_number_text() {
    let root = tmp("schema");
    write_project(&root);
    let package = new_package(&root);
    let rde_doc = load_rde_doc(&package).unwrap();
    assert_eq!(rde_doc.schema_version, "4", "новые пакеты пишутся схемой v4 (ADR-0018)");
    // Пакет v3 (без number_text) читается: поле необязательное.
    let mut legacy = rde_doc.clone();
    legacy.schema_version = "3".to_owned();
    legacy.inserts.push(rde_doc::RdeInsert {
        id: "v99".to_owned(),
        block: String::new(),
        kind: "formula".to_owned(),
        target: format!("rde://formula/{FORMULA_A}@act-a"),
        caption: String::new(),
        number: Some(1),
        number_text: None,
        hash: Some("act-a".to_owned()),
        cached: String::new(),
        selector: String::new(),
        value_kind: "formula".to_owned(),
        para_id: None,
        style: String::new(),
        format: String::new(),
        run_style: String::new(),
        render_mode: "omml".to_owned(),
        body_hash: None,
    });
    let rendered = rde_doc::render_rde_doc(&legacy);
    assert!(rendered.contains("schema_version=\"3\""));
    let parsed = rde_doc::parse_rde_doc(&rendered).expect("v3 пакет читается");
    assert_eq!(parsed.schema_version, "3");
    assert!(rde_doc::FormulaNumbering::parse("section").is_some(), "схема из document.toml");
}
