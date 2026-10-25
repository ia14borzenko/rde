//! Прогнать fidelity/OPC-гейты по корпусу docx (S6, G7):
//! `cargo run -p rde-docx --example docx-gates -- [каталог-корпуса] [out.json]`
//!
//! Для каждого файла: инвентаризация частей, WordCraft-roundtrip, OPC/XML-валидация,
//! LibreOffice → PDF. Итог — PASS/FAIL и JSON-отчёт.

use std::path::PathBuf;

fn main() {
    let corpus = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/corpus"));
    let out = std::env::args().nth(2).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("docs/fidelity/corpus-report.json"));
    let reports = rde_docx::run_corpus(&corpus);
    if reports.is_empty() {
        eprintln!("корпус пуст: {}", corpus.display());
        std::process::exit(1);
    }
    let mut passed = 0;
    let mut json = Vec::new();
    for (name, report) in &reports {
        match report {
            Ok(report) => {
                if report.passed() {
                    passed += 1;
                }
                println!(
                    "{:<28} {}  частей {}→{}  абзацев {}→{}  opc={}  LO={}",
                    name,
                    if report.passed() { "PASS" } else { "FAIL" },
                    report.source_parts,
                    report.roundtrip_parts,
                    report.paragraphs_source,
                    report.paragraphs_roundtrip,
                    report.opc.ok,
                    report.lo.pdf_is_valid
                );
                if !report.opc.errors.is_empty() {
                    for error in report.opc.errors.iter().take(5) {
                        println!("    OPC: {error}");
                    }
                }
                if !report.missing.is_empty() || !report.critical_missing.is_empty() {
                    println!("    пропало: {:?} / критично {:?}", report.missing, report.critical_missing);
                }
                json.push(serde_json::json!({ "name": name, "passed": report.passed(), "report": report }));
            }
            Err(error) => {
                println!("{name:<28} ERROR {error}");
                json.push(serde_json::json!({ "name": name, "passed": false, "error": error }));
            }
        }
    }
    let summary = serde_json::json!({
        "corpus": corpus.display().to_string(),
        "total": reports.len(),
        "passed": passed,
        "failed": reports.len() - passed,
        "generated_unix": std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
        "reports": json,
    });
    if let Some(parent) = out.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec_pretty(&summary) {
        let _ = std::fs::write(&out, bytes);
    }
    println!("итого: {passed}/{} PASS; отчёт: {}", reports.len(), out.display());
    if passed != reports.len() {
        std::process::exit(2);
    }
}
