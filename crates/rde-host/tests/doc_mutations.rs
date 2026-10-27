//! Интеграционные тесты M2.5 (`rde-host`): suggested edits документа, прямая правка bлока,
//! чекпоинты `.doc.rde`, ревизии в store (R-DOC-32/R-DOC-41/R-AGT-05).
//!
//! Работают на временной копии `demo-project/` (без `.rde`/`exports`/`out`/`.venv`/`.git`).
//! Правки — только по простым абзацам (без вставок/полей/рисунков), как в документации M2.5.

use std::path::{Path, PathBuf};

use rde_host::HeadlessHost;
use rde_mcp::model::{DocReadRequest, DocTextBlock};
use rde_mcp::{HostError, RdeErrorCode, RdeHost};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-host-doc-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_project(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(&*name, ".rde" | "exports" | "out" | "__pycache__" | ".venv" | ".git" | "target")
            || name.ends_with(".bak")
        {
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

fn demo_host(name: &str) -> (HeadlessHost, PathBuf) {
    let root = temp_dir(name);
    copy_project(&rde_core::demo_project_dir(), &root);
    let host = HeadlessHost::open(&root).expect("демо-проект открывается headless-хостом");
    (host, root)
}

fn expect_code(error: HostError, code: RdeErrorCode) {
    assert_eq!(error.code, code, "ожидался код {code}, получено {error}");
}

fn doc_setup(host: &HeadlessHost, root: &Path) -> (String, PathBuf) {
    let documents = host.doc_list().unwrap();
    let document = documents.iter().find(|document| document.migrated).expect("мигрированный документ");
    let package = root.join(&document.file);
    (document.id.clone(), package)
}

fn blocks(host: &HeadlessHost, doc_id: &str) -> Vec<DocTextBlock> {
    host.doc_read(
        doc_id,
        DocReadRequest { block_ids: None, start: Some(0), limit: Some(256) },
    )
    .expect("doc_read")
    .blocks
}

/// Найти редактируемый блок по `dry_run` прямой правки (текст не меняется).
fn editable_block(host: &HeadlessHost, doc_id: &str, skip: &[String]) -> (String, String) {
    for block in blocks(host, doc_id) {
        if skip.contains(&block.id) || block.text.chars().count() < 10 || block.text.chars().count() > 200 {
            continue;
        }
        let dry = host.doc_replace_range(doc_id, &block.id, &block.text, &block.text, None, true);
        if dry.is_ok() {
            return (block.id, block.text);
        }
    }
    panic!("в демо-документе нет простого редактируемого абзаца");
}

#[test]
fn doc_list_exposes_body_hash_for_concurrency() {
    let (host, _root) = demo_host("hash");
    let documents = host.doc_list().unwrap();
    let document = documents.iter().find(|document| document.migrated).unwrap();
    let hash = document.body_hash.clone().expect("body_hash мигрированного документа");
    assert_eq!(hash.len(), 64, "sha256 hex");
    assert!(hash.chars().all(|ch| ch.is_ascii_hexdigit()));
}

#[test]
fn suggest_edit_creates_pending_without_touching_document() {
    let (host, root) = demo_host("suggest");
    let (doc_id, package) = doc_setup(&host, &root);
    let before = std::fs::read(&package).unwrap();
    let (block_id, block_text) = editable_block(&host, &doc_id, &[]);

    let suggestion = host
        .doc_suggest_edit(&doc_id, &block_id, &block_text, "Предложенный текст M2.5", Some("из теста"))
        .expect("предложение создано");
    assert_eq!(suggestion.status, "pending");
    assert_eq!(suggestion.doc_id, doc_id);
    assert_eq!(suggestion.block_id, block_id);
    assert_eq!(suggestion.before, block_text);
    assert_eq!(suggestion.after, "Предложенный текст M2.5");
    assert_eq!(suggestion.author_kind, "agent");
    assert_eq!(suggestion.base_body_hash.len(), 64);
    // Документ не изменился (побайтово), предложение лежит в `.rde/agent/suggestions`.
    assert_eq!(std::fs::read(&package).unwrap(), before, "создание предложения не меняет документ");
    assert!(root.join(".rde/agent/suggestions").join(format!("{}.json", suggestion.id)).is_file());

    let pending = host.suggestion_list(Some(&doc_id), Some("pending"), 20).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, suggestion.id);
    assert!(host.suggestion_list(Some("01OTHERDOC"), None, 20).unwrap().is_empty());
    assert!(host.suggestion_list(None, Some("accepted"), 20).unwrap().is_empty());
    // Нельзя предложить пустой текст/чужой блок.
    expect_code(
        host.doc_suggest_edit(&doc_id, "b999", "x", "y", None).unwrap_err(),
        RdeErrorCode::NotFound,
    );
}

#[test]
fn suggestion_accept_applies_with_checkpoint_and_revision() {
    let (host, root) = demo_host("accept");
    let (doc_id, package) = doc_setup(&host, &root);
    let before_bytes = std::fs::read(&package).unwrap();
    let (block_id, block_text) = editable_block(&host, &doc_id, &[]);
    let suggestion = host
        .doc_suggest_edit(&doc_id, &block_id, &block_text, "Текст, принятый из предложения", None)
        .expect("предложение");

    let outcome = host.suggestion_accept(&suggestion.id).expect("применение предложения");
    assert!(!outcome.dry_run);
    let checkpoint_id = outcome.checkpoint_id.clone().expect("чекпоинт");
    let revision_id = outcome.revision_id.clone().expect("ревизия");
    assert_eq!(outcome.doc_id, doc_id);
    assert_eq!(outcome.block_id.as_deref(), Some(block_id.as_str()));
    assert_eq!(outcome.after.as_deref(), Some("Текст, принятый из предложения"));
    // Документ изменён: текст виден через doc_read, пакет отличается.
    let text_after = blocks(&host, &doc_id)
        .into_iter()
        .find(|block| block.id == block_id)
        .expect("блок на месте")
        .text;
    assert_eq!(text_after, "Текст, принятый из предложения");
    assert_ne!(std::fs::read(&package).unwrap(), before_bytes);
    // Чекпоинт и ревизия на месте.
    assert!(root.join(".rde/agent/checkpoints").join(&checkpoint_id).join("manifest.json").is_file());
    let revisions = host.revision_list(&doc_id, 20).unwrap();
    assert!(revisions.iter().any(|revision| revision.revision_id == revision_id && revision.current));
    // Предложение перешло в accepted; повторный accept — Conflict.
    let accepted = host.suggestion_list(Some(&doc_id), Some("accepted"), 20).unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].status, "accepted");
    expect_code(host.suggestion_accept(&suggestion.id).unwrap_err(), RdeErrorCode::Conflict);
    // body_hash в ответе соответствует документу после правки.
    let documents = host.doc_list().unwrap();
    let document = documents.iter().find(|document| document.id == doc_id).unwrap();
    assert_eq!(outcome.body_hash.as_deref(), document.body_hash.as_deref());
}

#[test]
fn suggestion_reject_keeps_document_intact() {
    let (host, root) = demo_host("reject");
    let (doc_id, package) = doc_setup(&host, &root);
    let before = std::fs::read(&package).unwrap();
    let (block_id, block_text) = editable_block(&host, &doc_id, &[]);
    let suggestion = host
        .doc_suggest_edit(&doc_id, &block_id, &block_text, "Не должно примениться", None)
        .expect("предложение");
    let rejected = host.suggestion_reject(&suggestion.id, Some("не нужно")).expect("отклонение");
    assert_eq!(rejected.status, "rejected");
    assert!(rejected.resolution.as_deref().unwrap_or_default().contains("не нужно"));
    assert_eq!(std::fs::read(&package).unwrap(), before, "reject не меняет документ");
    expect_code(host.suggestion_reject(&suggestion.id, None).unwrap_err(), RdeErrorCode::Conflict);
    expect_code(host.suggestion_accept(&suggestion.id).unwrap_err(), RdeErrorCode::Conflict);
    expect_code(host.suggestion_accept("01NOSUCHSUGGESTION0000000").unwrap_err(), RdeErrorCode::NotFound);
}

#[test]
fn stale_suggestion_is_rejected_after_other_edit() {
    let (host, root) = demo_host("stale");
    let (doc_id, _package) = doc_setup(&host, &root);
    let (block_a, text_a) = editable_block(&host, &doc_id, &[]);
    let (block_b, text_b) = editable_block(&host, &doc_id, &[block_a.clone()]);
    let suggestion = host
        .doc_suggest_edit(&doc_id, &block_a, &text_a, "Вариант агента", None)
        .expect("предложение");
    // Документ изменился другим путём — предложение устарело (base_body_hash).
    host.doc_replace_range(&doc_id, &block_b, &text_b, "Пользовательская правка", None, false)
        .expect("прямая правка");
    expect_code(host.suggestion_accept(&suggestion.id).unwrap_err(), RdeErrorCode::Conflict);
}

#[test]
fn doc_replace_range_guards_and_checkpoint_restore() {
    let (host, root) = demo_host("replace");
    let (doc_id, package) = doc_setup(&host, &root);
    let before_bytes = std::fs::read(&package).unwrap();
    let (block_id, block_text) = editable_block(&host, &doc_id, &[]);
    // dry_run: проверки проходят, файл не меняется.
    let dry = host
        .doc_replace_range(&doc_id, &block_id, &block_text, "Новый текст", None, true)
        .expect("dry-run");
    assert!(dry.dry_run && dry.checkpoint_id.is_none());
    assert_eq!(std::fs::read(&package).unwrap(), before_bytes);
    // Устаревший expected_body_hash — Conflict; чужой block_id — NotFound.
    expect_code(
        host.doc_replace_range(&doc_id, &block_id, &block_text, "x", Some(&"0".repeat(64)), false)
            .unwrap_err(),
        RdeErrorCode::Conflict,
    );
    expect_code(
        host.doc_replace_range(&doc_id, "b999", &block_text, "x", None, false).unwrap_err(),
        RdeErrorCode::NotFound,
    );
    // Реальная правка: чекпоинт + ревизия + обновлённый текст.
    let outcome = host
        .doc_replace_range(&doc_id, &block_id, &block_text, "Новый текст из M2.5", None, false)
        .expect("правка");
    let checkpoint_id = outcome.checkpoint_id.clone().expect("чекпоинт");
    assert_eq!(outcome.before.as_deref(), Some(block_text.as_str()));
    assert!(outcome.revision_id.is_some());
    assert_eq!(
        blocks(&host, &doc_id).into_iter().find(|block| block.id == block_id).unwrap().text,
        "Новый текст из M2.5"
    );
    // Откат чекпоинта возвращает документ байт-в-байт (и корни store).
    host.checkpoint_restore(&checkpoint_id).expect("откат чекпоинта");
    assert_eq!(std::fs::read(&package).unwrap(), before_bytes, "документ восстановлен");
    // Устаревший expected_text после отката — Conflict (R-DOC-13).
    expect_code(
        host.doc_replace_range(&doc_id, &block_id, "Новый текст из M2.5", "x", None, false).unwrap_err(),
        RdeErrorCode::Conflict,
    );
}

#[test]
fn revisions_lifecycle_list_and_restore() {
    let (host, root) = demo_host("revisions");
    let (doc_id, _package) = doc_setup(&host, &root);
    let (block_id, text) = editable_block(&host, &doc_id, &[]);
    // Две последовательные правки → две ревизии.
    let first = host
        .doc_replace_range(&doc_id, &block_id, &text, "Версия один", None, false)
        .expect("правка 1");
    let second = host
        .doc_replace_range(&doc_id, &block_id, "Версия один", "Версия два", None, false)
        .expect("правка 2");
    let revisions = host.revision_list(&doc_id, 20).unwrap();
    assert!(revisions.len() >= 2, "ревизии: {revisions:?}");
    assert_eq!(revisions[0].revision_id, second.revision_id.clone().unwrap(), "новые первыми");
    assert!(revisions[0].current, "последняя версия — текущая");
    assert!(revisions.iter().filter(|revision| revision.current).count() <= 1, "текущая ровно одна");
    let first_revision = first.revision_id.clone().unwrap();

    // Откат к первой версии (текст «Версия один»), восстановление фиксируется новой ревизией.
    let restored = host.revision_restore(&doc_id, &first_revision).expect("восстановление ревизии");
    assert_eq!(restored.detail.as_deref().unwrap_or_default().contains("восстановлена"), true);
    let text_now = blocks(&host, &doc_id).into_iter().find(|block| block.id == block_id).unwrap().text;
    assert_eq!(text_now, "Версия один");
    let revisions_after = host.revision_list(&doc_id, 20).unwrap();
    assert_eq!(revisions_after[0].revision_id, restored.revision_id.clone().unwrap());
    assert!(revisions_after[0].current);
    // Неизвестная ревизия — NotFound.
    expect_code(host.revision_restore(&doc_id, "01NOSUCHREVISION000000000").unwrap_err(), RdeErrorCode::NotFound);
    expect_code(host.revision_list("01NOSUCHDOC0000000000000", 10).unwrap_err(), RdeErrorCode::NotFound);
}

#[test]
fn document_mutations_require_migrated_document() {
    let (host, root) = demo_host("not-migrated");
    // Убираем rde:doc из пакета: пересобираем «чистый» docx поверх .doc.rde.
    let (doc_id, package) = doc_setup(&host, &root);
    let clean_docx = root.join("clean.docx");
    rde_doc::extract_docx(&package, &clean_docx).expect("извлечь docx");
    std::fs::copy(&clean_docx, &package).expect("подменить пакет чистым docx");
    let blocks = rde_doc::document_blocks(&package).expect("блоки");
    let block = blocks.iter().find(|block| block.text.len() > 10).expect("блок");
    let error = host
        .doc_suggest_edit(&doc_id, &block.id, &block.text, "x", None)
        .expect_err("немаркированный документ");
    assert_eq!(error.code, RdeErrorCode::DocNotMigrated);
}

// --- M2.6: вставка/удаление абзацев ----------------------------------------

#[test]
fn doc_insert_paragraph_is_checkpointed_and_versioned() {
    let (host, root) = demo_host("insert");
    let (doc_id, package) = doc_setup(&host, &root);
    let before_bytes = std::fs::read(&package).unwrap();
    let (anchor_id, anchor_text) = editable_block(&host, &doc_id, &[]);
    let before_blocks = blocks(&host, &doc_id);
    // dry_run: текст не пишется, чекпоинта нет.
    let dry = host
        .doc_insert_paragraph(&doc_id, Some(&anchor_id), "after", "Абзац-черновик", None, true)
        .expect("dry-run вставки");
    assert!(dry.dry_run && dry.checkpoint_id.is_none());
    assert_eq!(std::fs::read(&package).unwrap(), before_bytes);
    // Вставка после якоря: блоков +1, текст сразу после якоря; чекпоинт + ревизия.
    let outcome = host
        .doc_insert_paragraph(&doc_id, Some(&anchor_id), "after", "Вставленный абзац M2.6", Some("Normal"), false)
        .expect("вставка абзаца");
    let checkpoint_id = outcome.checkpoint_id.clone().expect("чекпоинт");
    let revision_id = outcome.revision_id.clone().expect("ревизия");
    assert_eq!(outcome.after.as_deref(), Some("Вставленный абзац M2.6"));
    assert!(outcome.block_id.is_some());
    assert!(root.join(".rde/agent/checkpoints").join(&checkpoint_id).join("manifest.json").is_file());
    let after_blocks = blocks(&host, &doc_id);
    assert_eq!(after_blocks.len(), before_blocks.len() + 1);
    let anchor_index = after_blocks.iter().position(|block| block.id == anchor_id).expect("якорь");
    assert_eq!(after_blocks[anchor_index].text, anchor_text);
    assert_eq!(after_blocks[anchor_index + 1].text, "Вставленный абзац M2.6");
    assert_eq!(Some(after_blocks[anchor_index + 1].id.clone()), outcome.block_id);
    let revisions = host.revision_list(&doc_id, 20).unwrap();
    assert!(revisions.iter().any(|revision| revision.revision_id == revision_id && revision.current));
    // Вставка в конец (без якоря) — последний блок.
    let at_end = host
        .doc_insert_paragraph(&doc_id, None, "after", "Абзац в конец", None, false)
        .expect("вставка в конец");
    let after_end = blocks(&host, &doc_id);
    assert_eq!(after_end.last().map(|block| block.text.as_str()), Some("Абзац в конец"));
    assert!(at_end.block_id.is_some());
}

#[test]
fn doc_delete_paragraph_is_checkpointed_and_restorable() {
    let (host, root) = demo_host("delete");
    let (doc_id, package) = doc_setup(&host, &root);
    let before_bytes = std::fs::read(&package).unwrap();
    let before_blocks = blocks(&host, &doc_id);
    let (block_id, text) = editable_block(&host, &doc_id, &[]);
    // dry_run + stale-текст не меняют документ.
    let dry = host.doc_delete_block(&doc_id, &block_id, &text, true).expect("dry-run удаления");
    assert!(dry.dry_run && dry.checkpoint_id.is_none());
    expect_code(
        host.doc_delete_block(&doc_id, &block_id, "другой текст", false).unwrap_err(),
        RdeErrorCode::Conflict,
    );
    expect_code(
        host.doc_delete_block(&doc_id, "b9999", &text, false).unwrap_err(),
        RdeErrorCode::NotFound,
    );
    assert_eq!(std::fs::read(&package).unwrap(), before_bytes);
    // Реальное удаление: блоков -1, текст исчез, чекпоинт/ревизия есть.
    let outcome = host.doc_delete_block(&doc_id, &block_id, &text, false).expect("удаление абзаца");
    assert_eq!(outcome.before.as_deref(), Some(text.as_str()));
    assert!(outcome.after.is_none());
    let checkpoint_id = outcome.checkpoint_id.clone().expect("чекпоинт");
    let revision_id = outcome.revision_id.clone().expect("ревизия");
    let after_blocks = blocks(&host, &doc_id);
    assert_eq!(after_blocks.len(), before_blocks.len() - 1);
    assert!(!after_blocks.iter().any(|block| block.text == text));
    assert!(host.revision_list(&doc_id, 20).unwrap().iter().any(|revision| revision.revision_id == revision_id));
    // Откат чекпоинта возвращает документ байт-в-байт (и корни store).
    host.checkpoint_restore(&checkpoint_id).expect("откат чекпоинта");
    assert_eq!(std::fs::read(&package).unwrap(), before_bytes, "документ восстановлен");
    assert_eq!(blocks(&host, &doc_id).len(), before_blocks.len());
}

#[test]
fn doc_insert_delete_validation() {
    let (host, root) = demo_host("insert-delete-validation");
    let (doc_id, package) = doc_setup(&host, &root);
    let before_bytes = std::fs::read(&package).unwrap();
    let (_block_id, _text) = editable_block(&host, &doc_id, &[]);
    // Неизвестная позиция/якорь — ошибки, документ не меняется.
    expect_code(
        host.doc_insert_paragraph(&doc_id, None, "sideways", "x", None, false).unwrap_err(),
        RdeErrorCode::InvalidArgument,
    );
    expect_code(
        host.doc_insert_paragraph(&doc_id, Some("b9999"), "after", "x", None, false).unwrap_err(),
        RdeErrorCode::NotFound,
    );
    // Пустой текст/переводы строк — Unsupported (домен).
    expect_code(
        host.doc_insert_paragraph(&doc_id, None, "after", "  ", None, false).unwrap_err(),
        RdeErrorCode::Unsupported,
    );
    expect_code(
        host.doc_insert_paragraph(&doc_id, None, "after", "a\nb", None, false).unwrap_err(),
        RdeErrorCode::Unsupported,
    );
    assert_eq!(std::fs::read(&package).unwrap(), before_bytes);
}

