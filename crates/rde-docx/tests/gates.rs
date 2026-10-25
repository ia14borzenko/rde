//! Fidelity-гейты на демо-документе: части, roundtrip WordCraft, LO-оракул.

use std::path::PathBuf;

use rde_docx::{inventory, lo_oracle, report_json, roundtrip, run_gate, temp_work_dir, CRITICAL_PARTS};

fn demo_docx() -> PathBuf {
    let project = rde_core::load_project(&rde_core::demo_project_dir()).unwrap();
    let path = project.document_path().expect("документ демо-проекта");
    if path.extension().and_then(|ext| ext.to_str()) == Some("rde") {
        // M3: гейт проверяет чистый OOXML, извлечённый из .doc.rde (как его увидит Word).
        let out = temp_work_dir("clean").join("demo.docx");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        rde_doc::extract_docx(&path, &out).expect("извлечь чистый .docx");
        out
    } else {
        path
    }
}

fn work(tag: &str) -> PathBuf {
    let dir = temp_work_dir(tag);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn has_soffice() -> bool {
    std::process::Command::new("soffice").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn inventory_contains_critical_parts_and_media() {
    let parts = inventory(&demo_docx()).unwrap();
    let names: Vec<&str> = parts.iter().map(|part| part.name.as_str()).collect();
    for critical in CRITICAL_PARTS {
        assert!(names.contains(&critical), "нет критичной части {critical}: {names:?}");
    }
    assert!(names.iter().any(|name| name.starts_with("word/media/")), "нет медиа: {names:?}");
    assert!(parts.iter().all(|part| part.sha256.len() == 64));
}

#[test]
fn wordcraft_roundtrip_preserves_document() {
    let dir = work("roundtrip");
    let input = demo_docx();
    let output = dir.join("roundtrip.docx");
    roundtrip(&input, &output).unwrap();
    assert!(output.is_file());
    let source = inventory(&input).unwrap();
    let result = inventory(&output).unwrap();
    // Все медиа сохраняются байт-в-байт.
    for part in &source {
        if part.name.starts_with("word/media/") {
            let other = result.iter().find(|candidate| candidate.name == part.name).expect("медиа пропало");
            assert_eq!(part.sha256, other.sha256, "медиа изменилось: {}", part.name);
        }
    }
    let names: Vec<&str> = result.iter().map(|part| part.name.as_str()).collect();
    assert!(names.contains(&"word/document.xml"));
    assert!(names.contains(&"word/styles.xml"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lo_oracle_converts_to_pdf() {
    if !has_soffice() {
        eprintln!("LibreOffice не установлен — тест LO-оракула пропущен");
        return;
    }
    let dir = work("lo");
    let input = demo_docx();
    let oracle = lo_oracle(&input, &dir);
    assert!(oracle.available, "{oracle:?}");
    assert!(!oracle.version.is_empty());
    assert!(oracle.pdf_is_valid, "{oracle:?}");
    assert!(oracle.pdf_bytes > 1024, "{oracle:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn full_gate_passes_and_reports_json() {
    let dir = work("gate");
    let report = run_gate(&demo_docx(), &dir).unwrap();
    assert!(report.critical_missing.is_empty(), "{:?}", report.critical_missing);
    assert!(report.media_preserved, "медиа не сохранились");
    assert_eq!(report.paragraphs_source, report.paragraphs_roundtrip);
    if has_soffice() {
        assert!(report.lo.pdf_is_valid, "LO: {:?}", report.lo);
        assert!(report.passed(), "гейт не пройден: {report:?}");
    }
    // Отчёт валиден как JSON и содержит нужные поля.
    let json = report_json(&report).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert!(value.get("critical_missing").is_some());
    assert!(value.get("lo").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn gate_fails_on_broken_docx() {
    let dir = work("broken");
    let broken = dir.join("broken.docx");
    std::fs::write(&broken, b"not a zip").unwrap();
    let result = std::panic::catch_unwind(|| run_gate(&broken, &dir));
    assert!(result.is_ok(), "run_gate не должен паниковать на битом файле");
    assert!(result.unwrap().is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
