//! ADR-0019: импорт/миграция MathType-документов с нумерацией формул — display-формулы становятся
//! block-вставками с номером справа, inline остаются inline, номера из Word сохраняются (или
//! перенумеровываются по схеме), пунктуация после формул не теряется.
//!
//! Проверки идут через публичный API `rde_doc`: OMML вне реестра → `migrate_formulas_with` →
//! `update_formula_inserts_with`/`renumber_formula_numbers`. Кэши формул — реальная компиляция
//! (`compile_formula`, нужен poppler — иначе честный skip).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rde_doc::{
    compile_formula, load_rde_doc, migrate_formulas, migrate_formulas_with, renumber_formula_numbers,
    update_formula_inserts_with, FormulaBody, FormulaMigrationOptions, FormulaNumbering,
    NUMBER_SOURCE_WORD,
};

const DOC_ID: &str = "01M63SAMNCNP5EVECKQ3G890MD";

fn poppler_available() -> bool {
    std::process::Command::new("pdftocairo").arg("-v").output().map(|output| output.status.success()).unwrap_or(false)
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-doc-import-numbered-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_project(root: &Path) {
    std::fs::write(root.join("rde.toml"), "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"Импорт нумерованных формул\"\n").unwrap();
}

/// Пустой `.doc.rde` проекта (чистый docx WordCraft + rde:doc).
fn new_package(root: &Path) -> PathBuf {
    let docx = root.join("clean.docx");
    let document = wordcraft_engine::doc::Document::new();
    wordcraft_engine::io::save_path(&docx, &document).unwrap();
    let rde_doc = rde_doc::build_rde_doc(&docx, DOC_ID, "").unwrap();
    let package = root.join("report.doc.rde");
    rde_doc::write_package(&docx, &mut rde_doc.clone(), &package).unwrap();
    package
}

fn read_part(package: &Path, name: &str) -> Option<Vec<u8>> {
    let file = std::fs::File::open(package).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let mut entry = archive.by_name(name).ok()?;
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut out).ok()?;
    Some(out)
}

fn body(package: &Path) -> String {
    read_part(package, "word/document.xml").map(|bytes| String::from_utf8_lossy(&bytes).to_string()).unwrap_or_default()
}

fn write_part(package: &Path, name: &str, bytes: &[u8]) {
    use std::io::{Read, Write};
    let file = std::fs::File::open(package).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        entries.push((entry.name().to_owned(), data));
    }
    drop(archive);
    let mut found = false;
    for (entry_name, data) in entries.iter_mut() {
        if entry_name == name {
            *data = bytes.to_vec();
            found = true;
        }
    }
    assert!(found, "в пакете нет части {name}");
    let out = std::fs::File::create(package).unwrap();
    let mut zip = zip::ZipWriter::new(out);
    let options = zip::write::SimpleFileOptions::default();
    for (entry_name, data) in &entries {
        zip.start_file(entry_name.as_str(), options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

/// Документ с телом `body` (namespace `m` обязателен для OMML).
fn document(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document xmlns:wpc=\"http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas\" \
         xmlns:o=\"urn:schemas-microsoft-com:office:office\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
         xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\" \
         xmlns:v=\"urn:schemas-microsoft-com:vml\" \
         xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>{body}</w:body></w:document>"
    )
}

fn display_math(text: &str) -> String {
    format!("<m:oMathPara><m:oMath><m:r><m:t>{text}</m:t></m:r></m:oMath></m:oMathPara>")
}

fn inline_math(text: &str) -> String {
    format!("<m:oMath><m:r><m:t>{text}</m:t></m:r></m:oMath>")
}

/// Номер формулы MathType/Word: поля `MACROBUTTON MTPlaceRef` + `SEQ MTEqn \h/\c` с кэшем номера
/// (как в реальных документах: кэш результата лежит в `w:instrText` после `separate`).
fn field_number(number: &str) -> String {
    format!(
        "<w:r><w:tab/></w:r>\
         <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>\
         <w:r><w:instrText xml:space=\"preserve\"> MACROBUTTON MTPlaceRef \\* MERGEFORMAT </w:instrText></w:r>\
         <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>\
         <w:r><w:instrText xml:space=\"preserve\"> SEQ MTEqn \\h \\* MERGEFORMAT </w:instrText></w:r>\
         <w:r><w:fldChar w:fldCharType=\"end\"/></w:r>\
         <w:r><w:instrText>(</w:instrText></w:r>\
         <w:r><w:fldChar w:fldCharType=\"begin\"/></w:r>\
         <w:r><w:instrText xml:space=\"preserve\"> SEQ MTEqn \\c \\* Arabic \\* MERGEFORMAT </w:instrText></w:r>\
         <w:r><w:fldChar w:fldCharType=\"separate\"/></w:r>\
         <w:r><w:rPr><w:noProof/></w:rPr><w:instrText>{number}</w:instrText></w:r>\
         <w:r><w:fldChar w:fldCharType=\"end\"/></w:r>\
         <w:r><w:instrText>)</w:instrText></w:r>\
         <w:r><w:fldChar w:fldCharType=\"end\"/></w:r>"
    )
}

/// Display-абзац как в Word/MathType: таб, формула, пунктуация, таб и (опционально) номер в поле.
fn display_paragraph(math: &str, punctuation: &str, number: Option<&str>) -> String {
    let punctuation = if punctuation.is_empty() { String::new() } else { format!("<w:r><w:t xml:space=\"preserve\">{punctuation}</w:t></w:r>") };
    let number = number.map(field_number).unwrap_or_default();
    format!("<w:p><w:pPr><w:pStyle w:val=\"MTDisplayEquation\"/></w:pPr><w:r><w:tab/></w:r>{math}{punctuation}{number}</w:p>")
}

/// Тело абзаца с inline-формулой в тексте.
fn inline_paragraph(math: &str) -> String {
    format!("<w:p><w:r><w:t xml:space=\"preserve\">где </w:t></w:r>{math}<w:r><w:t xml:space=\"preserve\"> — постоянная.</w:t></w:r></w:p>")
}

/// План миграции на основе реальной компиляции LaTeX (том же пайплайне, что в GUI).
fn plan(formula_id: &str, act_key: &str, latex: &str, display: bool) -> rde_doc::FormulaMigrationPlan {
    let caches = compile_formula(latex, display).expect("компиляция формулы миграции");
    rde_doc::FormulaMigrationPlan { formula_id: formula_id.to_owned(), act_key: act_key.to_owned(), latex: latex.to_owned(), omml: caches.omml }
}

/// Форма тела вставки для «Обновить документ» (кэши реальные).
fn body_from_plan(plan: &rde_doc::FormulaMigrationPlan, display: bool, numbered: bool) -> FormulaBody {
    let caches = compile_formula(&plan.latex, display).expect("компиляция формулы");
    FormulaBody {
        formula_id: plan.formula_id.clone(),
        act_key: plan.act_key.clone(),
        latex: plan.latex.clone(),
        display,
        numbered,
        omml: caches.omml,
        svg: caches.svg,
        png: caches.png,
    }
}

fn inserts_of(package: &Path) -> rde_doc::RdeDoc {
    load_rde_doc(package).expect("rde:doc пакета")
}

#[test]
fn migration_turns_display_into_numbered_blocks_and_keeps_punctuation() {
    if !poppler_available() {
        return;
    }
    let root = tmp("blocks");
    write_project(&root);
    let package = new_package(&root);
    let document_xml = document(&format!(
        "{}{}{}",
        display_paragraph(&display_math("α=λ/d"), ",", None),
        display_paragraph(&display_math("x=y"), ".", None),
        inline_paragraph(&inline_math("z=1"))
    ));
    write_part(&package, "word/document.xml", document_xml.as_bytes());

    let plans = vec![
        plan("F_DISPLAY_1", "act-1", "\\alpha=\\frac{\\lambda}{d}", true),
        plan("F_DISPLAY_2", "act-2", "x=y", true),
        plan("F_INLINE", "act-3", "z=1", false),
    ];
    let report = migrate_formulas(&package, &plans).expect("миграция формул");
    assert_eq!(report.display, 2, "{report:?}");
    assert_eq!(report.inline, 1, "{report:?}");
    assert_eq!(report.mixed, 0, "{report:?}");
    assert_eq!(report.replaced_blocks, 2, "display → block-вставки: {report:?}");
    assert_eq!(report.replaced_inline, 1, "inline остался inline: {report:?}");
    assert_eq!(report.numbers_assigned, 2, "номера назначены (в Word номеров не было): {report:?}");
    assert_eq!(report.numbers_preserved, 0);
    assert!(report.warnings.is_empty(), "предупреждений быть не должно: {:?}", report.warnings);

    let xml = body(&package);
    // Канонический right-numbered абзац: табы center+right, номер справа, без таблицы.
    assert_eq!(xml.matches("<w:tabs>").count(), 2, "два нумерованных абзаца: {xml}");
    assert!(xml.contains("<w:tab w:val=\"center\""), "центр-таб: {xml}");
    assert!(xml.contains("<w:tab w:val=\"right\""), "правый таб: {xml}");
    assert!(!xml.contains("<w:tbl>"), "нумерованные формулы — без таблицы: {xml}");
    assert!(xml.contains("(1)"), "номер первой формулы: {xml}");
    assert!(xml.contains("(2)"), "номер второй формулы: {xml}");
    // Пунктуация перенесена внутрь таб-абзаца сразу после формулы (до правого таба).
    let first_close = xml.find("</m:oMathPara>").unwrap();
    let after_first = &xml[first_close..];
    let punctuation_at = after_first.find("<w:t xml:space=\"preserve\">,</w:t>").expect("запятая после формулы");
    let number_tab_at = after_first.find("<w:r><w:tab/></w:r>").expect("таб перед номером");
    assert!(punctuation_at < number_tab_at, "пунктуация — до таба к номеру: {after_first}");
    assert!(xml.contains("где "), "содержательный текст inline-абзаца сохранён");
    assert!(xml.contains("постоянная."), "хвост inline-абзаца сохранён");

    let rde_doc = inserts_of(&package);
    let formulas: Vec<_> = rde_doc.inserts.iter().filter(|insert| insert.kind == "formula").collect();
    assert_eq!(formulas.len(), 3, "3 project-вставки");
    let display: Vec<_> = formulas.iter().filter(|insert| insert.number.is_some()).collect();
    assert_eq!(display.len(), 2);
    let mut numbers: Vec<_> = display.iter().map(|insert| insert.number_text.clone().unwrap_or_default()).collect();
    numbers.sort();
    assert_eq!(numbers, vec!["1".to_owned(), "2".to_owned()]);
    let suffixes: Vec<_> = display.iter().map(|insert| insert.suffix.as_str()).collect();
    assert!(suffixes.contains(&","), "запятая сохранена в rde:doc: {suffixes:?}");
    assert!(suffixes.contains(&"."), "точка сохранена в rde:doc: {suffixes:?}");
    assert!(formulas.iter().any(|insert| insert.number.is_none()), "inline-формула без номера");

    // Идемпотентность миграции: повторный вызов ничего не меняет.
    let before = body(&package);
    let again = migrate_formulas(&package, &[]).expect("повторная миграция");
    assert!(again.insert_ids.is_empty());
    assert_eq!(body(&package), before);

    // «Обновить документ»: тела пересобраны из тех же кэшей — документ не меняется (нумерация
    // сохранена, пунктуация не потеряна).
    let bodies: BTreeMap<String, FormulaBody> = plans
        .iter()
        .zip([true, true, false])
        .map(|(plan, display)| (plan.formula_id.clone(), body_from_plan(plan, display, display)))
        .collect();
    let update = update_formula_inserts_with(&package, &bodies, FormulaNumbering::Continuous).expect("обновление");
    assert!(update.updated.is_empty(), "повторный update идемпотентен: {update:?}");
    assert_eq!(body(&package), before, "update не трогает пунктуацию/номер");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn migration_preserves_word_numbers_and_update_does_not_renumber() {
    if !poppler_available() {
        return;
    }
    let root = tmp("preserve");
    write_project(&root);
    let package = new_package(&root);
    let document_xml = document(&format!(
        "{}{}",
        display_paragraph(&display_math("a=b"), ",", Some("2")),
        display_paragraph(&display_math("c=d"), ".", Some("5"))
    ));
    write_part(&package, "word/document.xml", document_xml.as_bytes());

    let plans = vec![
        plan("F_NUM_2", "act-2", "a=b", true),
        plan("F_NUM_5", "act-5", "c=d", true),
    ];
    let report = migrate_formulas(&package, &plans).expect("миграция формул");
    assert_eq!(report.numbers_preserved, 2, "номера из Word сохранены: {report:?}");
    assert_eq!(report.numbers_assigned, 0);
    assert_eq!(report.warnings.len(), 0, "{:?}", report.warnings);
    let xml = body(&package);
    assert!(xml.contains("<w:t xml:space=\"preserve\">(2)</w:t>"), "номер (2) в тексте: {xml}");
    assert!(xml.contains("<w:t xml:space=\"preserve\">(5)</w:t>"), "номер (5) в тексте: {xml}");
    let rde_doc = inserts_of(&package);
    for insert in rde_doc.inserts.iter().filter(|insert| insert.kind == "formula") {
        assert_eq!(insert.number_source, NUMBER_SOURCE_WORD, "источник номера — Word: {insert:?}");
        assert!(insert.suffix == "," || insert.suffix == ".");
    }

    // «Обновить документ» уважает номер из Word: не перенумеровывает.
    let bodies: BTreeMap<String, FormulaBody> =
        plans.iter().map(|plan| (plan.formula_id.clone(), body_from_plan(plan, true, true))).collect();
    let update = update_formula_inserts_with(&package, &bodies, FormulaNumbering::Continuous).expect("обновление");
    assert!(update.updated.is_empty(), "номера из Word не перезаписаны: {update:?}");
    let xml = body(&package);
    assert!(xml.contains("(2)") && xml.contains("(5)"), "номера на месте: {xml}");

    // Явная смена схемы — перенумерация снимает фиксацию «номер из Word».
    let renumbered = renumber_formula_numbers(&package, FormulaNumbering::Continuous).expect("перенумерация");
    assert_eq!(renumbered.formulas.len(), 2, "{renumbered:?}");
    let xml = body(&package);
    assert!(xml.contains("(1)") && xml.contains("(2)"), "после явной смены схемы — (1),(2): {xml}");
    let rde_doc = inserts_of(&package);
    assert!(rde_doc.inserts.iter().filter(|insert| insert.kind == "formula").all(|insert| insert.number_source.is_empty()));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn migration_assigns_scheme_when_word_numbers_partial() {
    if !poppler_available() {
        return;
    }
    let root = tmp("partial");
    write_project(&root);
    let package = new_package(&root);
    let document_xml = document(&format!(
        "{}{}",
        display_paragraph(&display_math("a=b"), ",", Some("7")),
        display_paragraph(&display_math("c=d"), ".", None)
    ));
    write_part(&package, "word/document.xml", document_xml.as_bytes());
    let plans = vec![plan("F_PARTIAL_1", "act-1", "a=b", true), plan("F_PARTIAL_2", "act-2", "c=d", true)];
    let report = migrate_formulas(&package, &plans).expect("миграция формул");
    assert_eq!(report.numbers_preserved, 0);
    assert_eq!(report.numbers_assigned, 2, "номера назначены по схеме: {report:?}");
    assert!(
        report.warnings.iter().any(|warning| warning.contains("номера") || warning.contains("display")),
        "предупреждение о частичных номерах: {:?}",
        report.warnings
    );
    let xml = body(&package);
    assert!(xml.contains("(1)") && xml.contains("(2)"), "схема сквозная: {xml}");
    assert!(!xml.contains("(7)"), "номер (7) не сохранён (номера не у всех): {xml}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn migration_section_scheme_uses_document_headings() {
    if !poppler_available() {
        return;
    }
    let root = tmp("section");
    write_project(&root);
    let package = new_package(&root);
    std::fs::write(
        root.join("styles.xml"),
        "<w:styles xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:style w:type=\"paragraph\" w:styleId=\"Heading1\"><w:name w:val=\"heading 1\"/><w:outlineLvl w:val=\"0\"/></w:style></w:styles>",
    )
    .unwrap();
    write_part(&package, "word/styles.xml", std::fs::read(root.join("styles.xml")).unwrap().as_slice());
    let heading = |text: &str| format!("<w:p><w:pPr><w:pStyle w:val=\"Heading1\"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>");
    let document_xml = document(&format!(
        "{}{}{}{}",
        heading("2 Метод"),
        display_paragraph(&display_math("a=b"), ",", None),
        heading("3 Результаты"),
        display_paragraph(&display_math("c=d"), ".", None)
    ));
    write_part(&package, "word/document.xml", document_xml.as_bytes());
    let plans = vec![plan("F_SEC_1", "act-1", "a=b", true), plan("F_SEC_2", "act-2", "c=d", true)];
    let options = FormulaMigrationOptions {
        numbering: FormulaNumbering::Section,
        preserve_word_numbers: true,
        number_display: true,
    };
    let report = migrate_formulas_with(&package, &plans, options).expect("миграция формул");
    assert_eq!(report.numbers_assigned, 2, "{report:?}");
    let xml = body(&package);
    assert!(xml.contains("(2.1)"), "номер по разделу 2: {xml}");
    assert!(xml.contains("(3.1)"), "номер по разделу 3: {xml}");

    // Смена схемы на сквозную перенумеровывает и обновляет текст.
    renumber_formula_numbers(&package, FormulaNumbering::Continuous).expect("перенумерация");
    let xml = body(&package);
    assert!(xml.contains("(1)") && xml.contains("(2)"), "сквозная схема: {xml}");
    assert!(!xml.contains("(2.1)"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn migration_keeps_mixed_display_inline_with_warning() {
    if !poppler_available() {
        return;
    }
    let root = tmp("mixed");
    write_project(&root);
    let package = new_package(&root);
    // display-формула внутри абзаца с содержательным текстом — редко, но не ломаем абзац.
    let mixed = format!(
        "<w:p><w:r><w:t xml:space=\"preserve\">где </w:t></w:r>{}<w:r><w:t xml:space=\"preserve\"> — постоянная.</w:t></w:r></w:p>",
        display_math("a=b")
    );
    let document_xml = document(&format!("{}{}", mixed, display_paragraph(&display_math("c=d"), ",", None)));
    write_part(&package, "word/document.xml", document_xml.as_bytes());
    let plans = vec![plan("F_MIXED", "act-1", "a=b", true), plan("F_CLEAN", "act-2", "c=d", true)];
    let report = migrate_formulas(&package, &plans).expect("миграция формул");
    assert_eq!(report.mixed, 1, "смешанный абзац: {report:?}");
    assert_eq!(report.replaced_blocks, 1);
    assert!(report.replaced_inline >= 1);
    assert!(
        report.warnings.iter().any(|warning| warning.contains("оставлена inline")),
        "предупреждение о смешанном абзаце: {:?}",
        report.warnings
    );
    let xml = body(&package);
    // Смешанный абзац цел: текст на месте, формула — inline-вставка без номера.
    assert!(xml.contains("где "), "{xml}");
    assert!(xml.contains("постоянная."), "{xml}");
    let mixed_insert = inserts_of(&package)
        .inserts
        .into_iter()
        .find(|insert| insert.kind == "formula" && insert.number.is_none())
        .expect("inline-вставка смешанной формулы");
    // В rde:doc inline-вставка не нумеруется; отображаемый номер в абзаце не появляется.
    assert!(mixed_insert.number.is_none());
    let _ = std::fs::remove_dir_all(&root);
}
