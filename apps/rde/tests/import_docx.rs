//! E2E R-DOC-39 (минимальный импорт): `rde import-docx` без GUI.
//!
//! Проверяется: CLI создаёт новый объект-документ «[IMPORTED] …», пакет `.doc.rde` валиден,
//! `import.json` фиксирует происхождение, исходный `.docx` не изменяется, повторный импорт
//! создаёт второй документ, ошибки (нет файла/нет проекта) — код 2 и понятный stderr.

use std::path::PathBuf;
use std::process::Command;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-cli-import-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn fixture_docx(name: &str) -> PathBuf {
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../crates/rde-doc/tests/fixtures/base-report.docx")
        .canonicalize()
        .expect("фикстура .docx");
    let dir = temp_dir(&format!("src-{name}"));
    let target = dir.join("vkr.docx");
    std::fs::copy(&source, &target).expect("копия фикстуры");
    target
}

fn create_project(name: &str) -> PathBuf {
    let root = temp_dir(name);
    rde_core::create_project(&root, "Импорт-тест").expect("создать проект");
    root
}

fn run_import(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rde"))
        .args(["import-docx"])
        .args(args)
        .output()
        .expect("запустить rde import-docx")
}

#[test]
fn cli_imports_docx_into_project_and_reports_provenance() {
    let root = create_project("happy");
    let source = fixture_docx("happy");
    let source_before = std::fs::read(&source).unwrap();

    let output = run_import(&[source.to_str().unwrap(), "--project", root.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "код {:?}, stderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("stdout — JSON-отчёт импорта");
    assert_eq!(report["source_sha256"].as_str().unwrap().len(), 64);
    assert!(report["blocks"].as_u64().unwrap() >= 8, "блоков: {}", report["blocks"]);
    let package = PathBuf::from(report["package"].as_str().expect("package в отчёте"));
    assert!(package.is_file(), "пакет создан: {}", package.display());
    assert!(rde_doc::is_rde_package(&package), "пакет содержит rde:doc");
    assert!(package.parent().unwrap().join("import.json").is_file(), "import.json");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("[IMPORTED]"), "маркировка в stderr: {stderr}");
    assert_eq!(std::fs::read(&source).unwrap(), source_before, "исходник не изменён");

    // Проект видит документ; заголовок помечен [IMPORTED].
    let project = rde_core::load(&root).expect("проект загружается");
    let object = project.document_object().expect("импортированный документ в проекте");
    assert!(object.title.starts_with("[IMPORTED]"), "title: {}", object.title);
    assert!(project.document_path().expect("путь документа").is_file());

    // Повторный импорт не перезаписывает первый документ — создаётся новый объект.
    let output = run_import(&[source.to_str().unwrap(), "--project", root.to_str().unwrap()]);
    assert!(output.status.success(), "повторный импорт");
    let project = rde_core::load(&root).expect("проект загружается");
    let documents = project
        .objects
        .iter()
        .filter(|object| matches!(object.data, rde_core::ObjectData::Document(_)))
        .count();
    assert_eq!(documents, 2, "два импортированных документа");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(source.parent().unwrap());
}

#[test]
fn cli_import_rejects_missing_file_and_missing_project() {
    let root = create_project("errors");
    let missing = root.join("нет-такого.docx");
    let output = run_import(&[missing.to_str().unwrap(), "--project", root.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2), "код ошибки аргументов");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("не найден"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Проект не указан → код 2 и подсказка про --project.
    let source = fixture_docx("errors");
    let output = run_import(&[source.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("--project"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(source.parent().unwrap());
}

#[test]
fn cli_import_keeps_marking_with_custom_title() {
    let root = create_project("title");
    let source = fixture_docx("title");
    let output = run_import(&[
        source.to_str().unwrap(),
        "--project",
        root.to_str().unwrap(),
        "--title",
        "ВКР Иванова",
    ]);
    assert!(output.status.success(), "stderr: {}", String::from_utf8_lossy(&output.stderr));
    let project = rde_core::load(&root).expect("проект");
    let object = project.document_object().expect("документ");
    assert_eq!(object.title, "[IMPORTED] ВКР Иванова");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(source.parent().unwrap());
}
