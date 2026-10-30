//! P3 (ADR-0005 §3): fidelity-гейт формульных вставок демо/пакета — OMML-формула (omml) и
//! векторная (svg) переживают roundtrip WordCraft: OMML не теряется, `svgBlip` и `word/media/*.svg`
//! сохраняются, LO открывает пакет, OPC-инварианты целы.

use std::path::PathBuf;

use rde_doc::{
    add_formula_insert, build_rde_doc, compile_formula, extract_docx, write_package, FormulaBody, FormulaInsertRequest,
};

const FORMULA_ID: &str = "01M5VJ1JG039MMV97SZGY6V9EQ";
const DOC_ID: &str = "01M5VJ1JG04TFF59TDWH9EDD1R";

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-docx-formula-gate-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn has_soffice() -> bool {
    std::process::Command::new("soffice").arg("--version").output().map(|output| output.status.success()).unwrap_or(false)
}

fn poppler_available() -> bool {
    std::process::Command::new("pdftocairo").arg("-v").output().map(|output| output.status.success()).unwrap_or(false)
}

fn body(latex: &str, act_key: &str, numbered: bool) -> FormulaBody {
    let caches = compile_formula(latex, true).expect("компиляция формулы");
    FormulaBody {
        formula_id: FORMULA_ID.to_owned(),
        act_key: act_key.to_owned(),
        latex: latex.to_owned(),
        display: true,
        numbered,
        omml: caches.omml,
        svg: caches.svg,
        png: caches.png,
    }
}

#[test]
fn formula_inserts_pass_fidelity_gate() {
    if !poppler_available() || !has_soffice() {
        return; // LO-оракул и SVG-пайплайн — внешние CLI (честный skip, как другие гейты)
    }
    let dir = tmp("both-modes");
    let docx = dir.join("clean.docx");
    wordcraft_engine::io::save_path(&docx, &wordcraft_engine::doc::Document::new()).unwrap();
    let mut rde_doc = build_rde_doc(&docx, DOC_ID, "").unwrap();
    let package = dir.join("report.doc.rde");
    write_package(&docx, &mut rde_doc, &package).unwrap();
    // Одна formula-вставка в режиме omml (с номером) и одна в svg (с номером) — оба режима в гейте.
    add_formula_insert(
        &package,
        FormulaInsertRequest { body: body("\\sigma=\\frac{F}{A}", "act-omml", true), render_mode: "omml".to_owned(), anchor: rde_doc::InsertAnchor::end(), numbering: rde_doc::FormulaNumbering::Continuous, },
    )
    .unwrap();
    add_formula_insert(
        &package,
        FormulaInsertRequest { body: body("\\sigma=\\sqrt{\\frac{F}{A}}", "act-svg", true), render_mode: "svg".to_owned(), anchor: rde_doc::InsertAnchor::end(), numbering: rde_doc::FormulaNumbering::Continuous, },
    )
    .unwrap();
    let clean = dir.join("with-formulas.docx");
    extract_docx(&package, &clean).unwrap();
    let report = rde_docx::run_gate(&clean, &dir.join("work")).expect("гейт");
    assert!(report.passed(), "гейт не пройден: {report:#?}");
    assert!(report.omml.source.nodes >= 1, "OMML-формула в исходнике: {:?}", report.omml.source);
    assert!(report.svg_blip.source >= 1, "svgBlip в исходнике: {:?}", report.svg_blip);
    assert_eq!(report.svg_blip.roundtrip, report.svg_blip.source, "svgBlip после roundtrip: {:?}", report.svg_blip);
    assert!(report.svg_blip.roundtrip_parts >= 1, "svg-часть после roundtrip: {:?}", report.svg_blip);
    let _ = std::fs::remove_dir_all(&dir);
}
