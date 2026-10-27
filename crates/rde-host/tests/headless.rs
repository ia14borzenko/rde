//! Интеграционные тесты headless-хоста (`rde-host`): реальные библиотеки RDE, без GUI.
//!
//! Тесты работают на временной копии `demo-project/` (без `.rde`/`exports`/`out`/`.venv`),
//! как и e2e MCP-сервера; helper копирования — свой (в `rde-core` он `pub(crate)`).

use std::path::{Path, PathBuf};

use rde_host::HeadlessHost;
use rde_mcp::model::DocReadRequest;
use rde_mcp::{HostError, RdeErrorCode, RdeHost};

/// Уникальный временный каталог теста.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-host-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Рекурсивная копия проекта без локального состояния/выходов/окружений.
fn copy_project(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(&*name, ".rde" | "exports" | "out" | "__pycache__" | ".venv" | ".git" | "target") {
            continue;
        }
        if name.ends_with(".bak") {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&*name);
        if from.is_dir() {
            copy_project(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

/// Копия демо-проекта + открытый хост.
fn demo_host(name: &str) -> (HeadlessHost, PathBuf) {
    let root = temp_dir(name);
    copy_project(&rde_core::demo_project_dir(), &root);
    let host = HeadlessHost::open(&root).expect("демо-проект открывается headless-хостом");
    (host, root)
}

fn expect_code(error: HostError, code: RdeErrorCode) {
    assert_eq!(error.code, code, "ожидался код {code}, получено {error}");
}

#[test]
fn workspace_snapshot_has_counts_and_no_root_path() {
    let (host, root) = demo_host("workspace");
    let info = host.workspace_snapshot().unwrap();
    assert_eq!(info.id, "01M5VJ1JG0V3RS3DTR98H6BCFN");
    assert_eq!(info.name, "Демо: отчёт по НИР");
    assert_eq!(info.version, "rde/project@1");
    assert!(info.trusted, "демо-проект доверенный (R-RUN-06)");
    assert_eq!(
        info.object_count, 12,
        "1 документ + 4 артефакта + 1 константы + 3 источника + заметка + репозиторий + формула"
    );
    assert_eq!(info.object_counts_by_kind.get("document"), Some(&1));
    assert_eq!(info.object_counts_by_kind.get("artifact"), Some(&4));
    assert_eq!(info.object_counts_by_kind.get("constants"), Some(&1));
    assert_eq!(info.object_counts_by_kind.get("source"), Some(&3));
    assert_eq!(info.object_counts_by_kind.get("formula"), Some(&1));
    // Корень проекта наружу не отдаётся (A5).
    assert!(!serde_json::to_string(&info).unwrap().contains(root.to_str().unwrap()));
}

#[test]
fn project_tree_lists_groups_kinds_and_freshness() {
    let (host, _root) = demo_host("tree");
    let tree = host.project_tree().unwrap();
    assert_eq!(tree.len(), 12);
    assert!(tree.iter().any(|node| node.kind == "document" && node.group == "Документы"));
    assert!(tree.iter().any(|node| node.kind == "formula" && node.service));
    for node in &tree {
        assert!(!node.id.is_empty());
        assert!(!node.freshness.is_empty());
    }
}

#[test]
fn project_search_finds_objects_and_respects_limit() {
    let (host, _root) = demo_host("search");
    let all = host.search_project("статистик", 10).unwrap();
    assert!(!all.is_empty(), "в демо есть артефакт «Статистики…»");
    assert!(all.iter().all(|hit| !hit.snippet.is_empty()));
    let limited = host.search_project("01", 2).unwrap();
    assert!(limited.len() <= 2);
    // Регистронезависимость.
    let lower = host.search_project("ОТЧЁТ", 5).unwrap();
    assert!(!lower.is_empty());
}

#[test]
fn file_read_supports_bytes_lines_and_escapes() {
    let (host, root) = demo_host("files");
    let whole = host.read_project_file("rde.toml", None).unwrap();
    assert_eq!(whole.path, "rde.toml");
    assert!(whole.text.contains("rde/project@1"));
    assert!(!whole.truncated);

    let lines = host.read_project_file("rde.toml", Some(rde_mcp::model::FileRange::Lines { start: 1, count: 1 })).unwrap();
    assert_eq!(lines.mode, "lines");
    assert!(lines.text.starts_with("schema"), "первая строка: {:?}", lines.text);

    let bytes = host
        .read_project_file("rde.toml", Some(rde_mcp::model::FileRange::Bytes { offset: 0, limit: 6 }))
        .unwrap();
    assert_eq!(bytes.text, "schema");

    // Песочница: абсолютный путь, `..`, `.rde/**`, симлинк наружу.
    expect_code(host.read_project_file("/etc/passwd", None).unwrap_err(), RdeErrorCode::PathEscape);
    expect_code(host.read_project_file("../secret", None).unwrap_err(), RdeErrorCode::PathEscape);
    expect_code(host.read_project_file(".rde/agent/audit.log", None).unwrap_err(), RdeErrorCode::ApprovalDenied);

    #[cfg(unix)]
    {
        let outside = temp_dir("outside-file");
        let secret = outside.join("secret.txt");
        std::fs::write(&secret, "secret").unwrap();
        let link = root.join("escape-link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&secret, &link).unwrap();
        expect_code(host.read_project_file("escape-link", None).unwrap_err(), RdeErrorCode::PathEscape);
    }
}

#[test]
fn file_read_rejects_binary_files() {
    let (host, root) = demo_host("binary");
    let binary = root.join("blob.bin");
    std::fs::write(&binary, [0xff, 0xfe, 0x00, 0x01]).unwrap();
    expect_code(host.read_project_file("blob.bin", None).unwrap_err(), RdeErrorCode::Unsupported);
}

#[test]
fn doc_list_reports_migrated_document() {
    let (host, _root) = demo_host("docs");
    let documents = host.doc_list().unwrap();
    assert_eq!(documents.len(), 1);
    let document = &documents[0];
    assert_eq!(document.title, "Отчёт по НИР (демо)");
    assert!(document.migrated, "демо-документ — .doc.rde");
    assert!(document.file.ends_with(".doc.rde"));
    assert!(document.blocks.unwrap_or(0) > 0);
    assert!(document.inserts.unwrap_or(0) >= 10, "в демо 11 живых вставок");
}

#[test]
fn doc_outline_and_doc_read_use_block_registry() {
    let (host, _root) = demo_host("doc-read");
    let documents = host.doc_list().unwrap();
    let doc_id = documents[0].id.clone();
    let outline = host.doc_outline(&doc_id).unwrap();
    assert!(!outline.blocks.is_empty());
    assert_eq!(outline.total_blocks, outline.blocks.len());
    assert_eq!(outline.template, "gost-7.32-2017");
    assert!(!outline.truncated);

    let first_two: Vec<String> = outline.blocks.iter().take(2).map(|block| block.id.clone()).collect();
    let body = host
        .doc_read(&doc_id, DocReadRequest { block_ids: Some(first_two), start: None, limit: None })
        .unwrap();
    assert_eq!(body.returned_blocks, 2);
    assert!(!body.text.is_empty());
    assert_eq!(body.total_blocks, outline.total_blocks);

    let range = host
        .doc_read(&doc_id, DocReadRequest { block_ids: None, start: Some(0), limit: Some(1) })
        .unwrap();
    assert_eq!(range.returned_blocks, 1);
    assert!(range.truncated, "в документе больше одного блока");

    // Ошибки поиска/диапазона — машиночитаемые.
    expect_code(
        host.doc_read(&doc_id, DocReadRequest { block_ids: Some(vec!["nope".to_owned()]), start: None, limit: None })
            .unwrap_err(),
        RdeErrorCode::NotFound,
    );
    expect_code(host.doc_outline("01NOTADOCUMENT000000000000").unwrap_err(), RdeErrorCode::NotFound);
}

#[test]
fn doc_outline_requires_migrated_document() {
    let (host, root) = demo_host("doc-migration");
    // Имитируем «старый» документ: расширение .docx вместо .doc.rde.
    let documents = host.doc_list().unwrap();
    let manifest = root.join("objects/documents").join(&documents[0].id).join("document.toml");
    let text = std::fs::read_to_string(&manifest).unwrap().replace("report.doc.rde", "report.docx");
    std::fs::write(&manifest, text).unwrap();
    let object_dir = root.join("objects/documents").join(&documents[0].id);
    std::fs::rename(object_dir.join("report.doc.rde"), object_dir.join("report.docx")).unwrap();

    let error = host.doc_outline(&documents[0].id).unwrap_err();
    expect_code(error, RdeErrorCode::DocNotMigrated);
}

#[test]
fn artifact_list_and_artifact_read_expose_ledger_status_without_running() {
    let (host, _root) = demo_host("artifacts");
    let artifacts = host.artifact_list().unwrap();
    // objects/artifacts: 3 compute + 1 data + 1 constants (в дереве «константы» — служебный вид).
    assert_eq!(artifacts.len(), 5);
    for artifact in &artifacts {
        assert!(matches!(artifact.artifact_type.as_str(), "compute" | "data" | "constants"));
        assert!(
            matches!(artifact.status.as_str(), "fresh" | "stale" | "never_run" | "unknown"),
            "неизвестный статус {} у {}",
            artifact.status,
            artifact.id
        );
        // У части демо-артефактов выходы объявлены, у constants — нет (значения в манифесте).
        let _ = artifact.outputs;
    }
    assert!(artifacts.iter().any(|artifact| artifact.artifact_type == "data"));
    // «Честный» статус constants-артефакта без ledger — не «устарел», а never_run/unknown.
    assert!(artifacts.iter().any(|artifact| artifact.artifact_type == "constants"));
    let compute = artifacts
        .iter()
        .find(|artifact| artifact.artifact_type == "compute")
        .expect("в демо есть compute-артефакт");
    let detail = host.artifact_read(&compute.id).unwrap();
    assert_eq!(detail.info.id, compute.id);
    assert!(!detail.outputs_declared.is_empty());
    assert_eq!(detail.rebuild, "manual");
    assert!(detail.runtime.is_some());
    expect_code(host.artifact_read("01NOTANARTIFACT00000000000").unwrap_err(), RdeErrorCode::NotFound);
}

#[test]
fn sources_notes_and_templates_are_listed() {
    let (host, _root) = demo_host("lists");
    let sources = host.source_list().unwrap();
    assert_eq!(sources.len(), 3);
    assert!(sources.iter().any(|source| source.kind == "file"));
    assert!(sources.iter().any(|source| source.bibliography.is_some()));

    let notes = host.note_list().unwrap();
    assert_eq!(notes.len(), 1);
    assert!(notes[0].file.ends_with(".md"));
    assert!(notes[0].bytes.unwrap_or(0) > 0);

    let templates = host.template_list().unwrap();
    assert_eq!(templates.len(), 3, "ГОСТ 7.32, лабораторный отчёт, свободный");
    assert!(templates.iter().any(|template| template.id == "gost-7.32-2017"));
}

#[test]
fn git_status_is_read_only_snapshot() {
    let (host, _root) = demo_host("git");
    let status = host.git_status().unwrap();
    assert!(status.available);
    // Временная копия вне git-репозитория (если /tmp не внутри репозитория): ветки нет.
    if !status.is_repo {
        assert!(status.branch.is_none(), "без репозитория ветка не отдаётся: {status:?}");
    } else {
        assert!(status.branch.as_deref().is_some_and(|branch| !branch.is_empty()));
    }
}

#[test]
fn file_read_does_not_load_huge_files_whole() {
    let (host, root) = demo_host("huge-file");
    // «Большой» файл (9 MiB текста): без `range` файл целиком в память не читается —
    // RDE_OUT_OF_RANGE; с `range` (байты/строки) читается только запрошенное окно.
    let path = root.join("big.txt");
    let line = "0123456789abcdef0123456789abcdef\n";
    let mut content = String::with_capacity(9 * 1024 * 1024 + line.len());
    while content.len() < 9 * 1024 * 1024 {
        content.push_str(line);
    }
    std::fs::write(&path, &content).unwrap();
    let total = content.len() as u64;

    let error = host.read_project_file("big.txt", None).expect_err("большой файл без range");
    expect_code(error, RdeErrorCode::OutOfRange);

    let chunk = host
        .read_project_file("big.txt", Some(rde_mcp::model::FileRange::Bytes { offset: 1_000, limit: 16 }))
        .unwrap();
    assert_eq!(chunk.total_bytes, total);
    assert_eq!(chunk.text.len(), 16);
    assert!(chunk.truncated);

    let lines = host
        .read_project_file("big.txt", Some(rde_mcp::model::FileRange::Lines { start: 2, count: 3 }))
        .unwrap();
    assert_eq!(lines.mode, "lines");
    assert_eq!(lines.text, format!("{line}{line}{line}").trim_end_matches('\n'));
}

/// `.rde` не открывается (например, файл вместо каталога) — ошибка `RDE_NOT_READY`,
/// а не паника в протокольном слое (M1-review: без `expect` в production-пути).
#[test]
fn unavailable_store_is_error_not_panic() {
    let root = temp_dir("store-broken");
    copy_project(&rde_core::demo_project_dir(), &root);
    std::fs::write(root.join(".rde"), "не каталог").unwrap();
    let host = HeadlessHost::open(&root).expect("проект открывается без store");
    let error = host.store_roots().expect_err("store недоступен — ошибка");
    expect_code(error, RdeErrorCode::NotReady);
}

#[test]
fn instructions_are_stable_and_bounded() {
    let (host, _root) = demo_host("instructions");
    let first = host.instructions().unwrap();
    let second = host.instructions().unwrap();
    assert_eq!(first, second, "инструкции детерминированы при том же проекте");
    assert!(first.len() <= rde_mcp::MAX_INSTRUCTIONS_BYTES);
    assert!(first.contains("Демо: отчёт по НИР"));
    assert!(first.contains("project_tree"));
    assert!(!first.contains("unix"), "без времени/нестабильных полей");
}

#[test]
fn open_rejects_missing_project() {
    let dir = temp_dir("missing-project");
    let error = HeadlessHost::open(dir.join("nope")).err().expect("нет проекта — ошибка");
    expect_code(error, RdeErrorCode::NotFound);
}
