//! Fidelity-гейт `.docx`: `cargo run -p rde-docx --example fidelity -- <input.docx> [out.json]`
//!
//! Печатает сводку (части, медиа, roundtrip WordCraft, LO → PDF) и сохраняет JSON-отчёт.

use std::path::PathBuf;

fn main() {
    let input = std::env::args().nth(1).unwrap_or_else(|| {
        rde_core::demo_project_dir()
            .join("objects/documents")
            .to_string_lossy()
            .to_string()
    });
    let input = PathBuf::from(input);
    let work = rde_docx::temp_work_dir("example");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).expect("work dir");
    // .doc.rde: гейт проверяет чистый OOXML, извлечённый из пакета.
    let input = if input.extension().and_then(|ext| ext.to_str()) == Some("rde") {
        let clean = work.join("clean.docx");
        rde_doc::extract_docx(&input, &clean).expect("извлечь .docx из .doc.rde");
        clean
    } else {
        input
    };
    let report = match rde_docx::run_gate(&input, &work) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("гейт не выполнен: {error}");
            std::process::exit(1);
        }
    };
    println!("вход: {} ({} частей)", report.source, report.source_parts);
    println!("roundtrip: {} частей, пропало {}, добавлено {}, изменено {}", report.roundtrip_parts, report.missing.len(), report.extra.len(), report.changed.len());
    if !report.missing.is_empty() { println!("  пропало: {:?}", report.missing); }
    if !report.extra.is_empty() { println!("  добавлено: {:?}", report.extra); }
    if !report.critical_missing.is_empty() { println!("  КРИТИЧНО пропало: {:?}", report.critical_missing); }
    println!("медиа сохранены: {}", report.media_preserved);
    println!("абзацев: {} → {}", report.paragraphs_source, report.paragraphs_roundtrip);
    println!("LibreOffice: available={} version={:?} pdf={} байт valid={}", report.lo.available, report.lo.version, report.lo.pdf_bytes, report.lo.pdf_is_valid);
    if let Some(error) = &report.lo.error { println!("  LO error: {error}"); }
    println!("ГЕЙТ: {}", if report.passed() { "PASS" } else { "FAIL" });
    if let Some(out) = std::env::args().nth(2) {
        let json = rde_docx::report_json(&report).expect("json");
        std::fs::write(&out, json).expect("write report");
        println!("отчёт: {out}");
    }
    let _ = std::fs::remove_dir_all(&work);
    if !report.passed() {
        std::process::exit(2);
    }
}
