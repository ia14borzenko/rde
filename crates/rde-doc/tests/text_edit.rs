//! M2.5: правка текста блоков `.doc.rde` и домен suggested edits (R-AGT-05 / R-DOC-32).
//!
//! Пакет строится из фикстуры `tests/fixtures/base-report.docx` (стабильной, не зависящей от
//! текущего состояния demo-project) через `save_engine_document` — как это делает RDE.
//!
//! Проверяется: замена текста блока с сохранением форматирования/paraId/body_hash; защита от
//! устаревшего текста (`Stale`); ограничения M2.5 (пустой текст/переводы строк/вставки sdt);
//! жизненный цикл предложения (create → list → accept/reject), устаревание по `body_hash`.

use std::path::{Path, PathBuf};

use rde_doc::{
    add_insert, block_edit_support, delete_block, document_blocks, insert_paragraph, load_rde_doc,
    package_info, replace_block_text, save_engine_document, update_document_package, BlockDelete,
    BlockTextEdit, BlockTextEditError, InsertPosition, InsertRequest, NewSuggestion, ParagraphInsert,
    Suggestions, STATUS_ACCEPTED, STATUS_PENDING, STATUS_REJECTED,
};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-doc-m25-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn demo_docx() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/base-report.docx")
}

/// Собрать `.doc.rde`-пакет из фикстуры (как «Сохранить проект» RDE): rde:doc + paraId.
fn demo_package(dir: &Path) -> PathBuf {
    let package = dir.join("report.doc.rde");
    save_engine_document(&demo_docx(), &package, "01TESTDOC0000000000000000", "free")
        .expect("пакет .doc.rde из фикстуры");
    package
}

fn plain_paragraph(package: &Path) -> (String, String) {
    let blocks = document_blocks(package).expect("блоки документа");
    let block = blocks
        .iter()
        .find(|block| block.kind == "paragraph" && block.text.len() > 10)
        .expect("в фикстуре есть обычный абзац");
    (block.id.clone(), block.text.clone())
}

#[test]
fn replace_block_text_updates_text_preserving_registry() {
    let dir = tmp("replace");
    let package = demo_package(&dir);
    let before_blocks = document_blocks(&package).unwrap();
    let (block_id, before_text) = plain_paragraph(&package);
    let registry_before = load_rde_doc(&package).unwrap();
    let para_before = registry_before
        .blocks
        .iter()
        .find(|block| block.id == block_id)
        .and_then(|block| block.para_id.clone())
        .expect("paraId блока");

    let report = replace_block_text(
        &package,
        &BlockTextEdit {
            block_id: block_id.clone(),
            expected_text: before_text.clone(),
            new_text: "Новый текст абзаца из M2.5".to_owned(),
        },
    )
    .expect("замена текста блока");
    assert_eq!(report.before, before_text);
    assert_eq!(report.after, "Новый текст абзаца из M2.5");
    assert_eq!(report.para_id, para_before);

    // Текст виден через обычное чтение блоков; число блоков не изменилось (id стабильны).
    let after_blocks = document_blocks(&package).unwrap();
    assert_eq!(after_blocks.len(), before_blocks.len());
    let after = after_blocks.iter().find(|block| block.id == block_id).expect("блок на месте");
    assert_eq!(after.text, "Новый текст абзаца из M2.5");
    // Реестр rde:doc актуален: body_hash совпадает с фактическим пакетом, блоки перечитаны.
    let registry_after = load_rde_doc(&package).unwrap();
    assert_eq!(registry_after.body_hash, package_info(&package).unwrap().body_hash);
    assert_eq!(registry_after.blocks.len(), registry_before.blocks.len());
    assert!(registry_after
        .blocks
        .iter()
        .any(|block| block.id == block_id && block.text.starts_with("Новый текст")));

    // Устаревшее ожидание текста — Stale (R-DOC-13: чужую правку не перезаписываем).
    let error = replace_block_text(
        &package,
        &BlockTextEdit {
            block_id: block_id.clone(),
            expected_text: before_text,
            new_text: "ещё раз".to_owned(),
        },
    )
    .expect_err("устаревший expected_text");
    assert!(matches!(error, BlockTextEditError::Stale { .. }), "{error}");

    // Неизвестный блок — NotFound; неподдерживаемые операции — Unsupported.
    assert!(matches!(
        replace_block_text(
            &package,
            &BlockTextEdit { block_id: "b999".to_owned(), expected_text: "x".to_owned(), new_text: "y".to_owned() },
        )
        .expect_err("нет блока"),
        BlockTextEditError::NotFound(_)
    ));
    for bad_text in ["", "   ", "строка\nвторая"] {
        let error = replace_block_text(
            &package,
            &BlockTextEdit {
                block_id: block_id.clone(),
                expected_text: "Новый текст абзаца из M2.5".to_owned(),
                new_text: bad_text.to_owned(),
            },
        )
        .expect_err(bad_text);
        assert!(matches!(error, BlockTextEditError::Unsupported(_)), "{bad_text:?}: {error}");
    }
    assert!(block_edit_support(&package, &block_id).is_ok());
}

#[test]
fn insert_paragraph_with_sdt_is_not_editable() {
    let dir = tmp("sdt");
    let package = demo_package(&dir);
    // Добавляем RDE-вставку (content-control w:sdt) — её абзац править текстом нельзя.
    add_insert(
        &package,
        InsertRequest {
            kind: "value".to_owned(),
            target: "rde://artifact/01ART000000000000000000/out/value.txt".to_owned(),
            selector: "#/value".to_owned(),
            value_kind: "text".to_owned(),
            cached: "42".to_owned(),
            caption: String::new(),
            style: String::new(),
            format: "auto".to_owned(),
            run_style: String::new(),
            anchor_block: None,
        },
    )
    .expect("вставка значения");
    update_document_package(&package).expect("обновить пакет");
    let registry = load_rde_doc(&package).unwrap();
    let para_id = registry.inserts.first().and_then(|insert| insert.para_id.clone()).expect("paraId вставки");
    let insert_block = registry
        .blocks
        .iter()
        .find(|block| block.para_id.as_deref() == Some(para_id.as_str()));
    let Some(insert_block) = insert_block else {
        eprintln!("движок не показал абзац вставки как блок — проверка sdt пропущена");
        return;
    };
    let error = block_edit_support(&package, &insert_block.id).expect_err("sdt-абзац не редактируется");
    assert!(matches!(error, BlockTextEditError::Unsupported(_)), "{error}");
}

#[test]
fn suggestions_lifecycle_accept_and_reject() {
    let dir = tmp("suggestions");
    let package = demo_package(&dir);
    let root = dir.clone();
    let store = Suggestions::new(&root);
    let (block_id, before_text) = plain_paragraph(&package);
    let base_hash = package_info(&package).unwrap().body_hash;

    let create = |after: &str| {
        store
            .create(NewSuggestion {
                doc_id: "01TESTDOC0000000000000000".to_owned(),
                block_id: block_id.clone(),
                para_id: None,
                before: before_text.clone(),
                after: after.to_owned(),
                author: "MCP agent".to_owned(),
                author_kind: "agent".to_owned(),
                base_body_hash: base_hash.clone(),
                note: Some("M2.5 test".to_owned()),
            })
            .expect("предложение")
    };
    let edit = create("Предложенный текст M2.5");
    assert_eq!(edit.status, STATUS_PENDING);
    assert_eq!(edit.author_kind, "agent");

    let pending = store.list(Some("01TESTDOC0000000000000000"), Some(STATUS_PENDING)).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, edit.id);
    assert_eq!(store.load(&edit.id).unwrap(), edit);
    assert!(store.list(Some("01OTHERDOC"), None).unwrap().is_empty());

    let accepted = store.apply(&edit.id, &package).expect("применить предложение");
    assert_eq!(accepted.status, STATUS_ACCEPTED);
    assert!(accepted.resolved_unix.is_some());
    let blocks = document_blocks(&package).unwrap();
    assert!(blocks.iter().any(|block| block.id == block_id && block.text == "Предложенный текст M2.5"));
    // Повторное применение/отклонение обработанного — Conflict.
    assert!(matches!(store.apply(&edit.id, &package).expect_err("уже принято"), rde_doc::SuggestionError::Conflict(_)));
    assert!(matches!(store.reject(&edit.id, None).expect_err("уже принято"), rde_doc::SuggestionError::Conflict(_)));

    // Reject: новая версия текста блока, новое предложение → отклонено, документ не меняется.
    let (block_id_after, text_after) = plain_paragraph(&package);
    let base_hash_after = package_info(&package).unwrap().body_hash;
    let rejected = store
        .create(NewSuggestion {
            doc_id: "01TESTDOC0000000000000000".to_owned(),
            block_id: block_id_after.clone(),
            para_id: None,
            before: text_after.clone(),
            after: "Это не должно примениться".to_owned(),
            author: "MCP agent".to_owned(),
            author_kind: "agent".to_owned(),
            base_body_hash: base_hash_after,
            note: None,
        })
        .unwrap();
    let rejected = store.reject(&rejected.id, Some("не нужно в M2.5")).unwrap();
    assert_eq!(rejected.status, STATUS_REJECTED);
    assert!(rejected.resolution.as_deref().unwrap_or_default().contains("не нужно"));
    let blocks = document_blocks(&package).unwrap();
    assert!(blocks.iter().any(|block| block.id == block_id_after && block.text == text_after));

    // Валидация создания: пустые/равные before/after/base_body_hash.
    let invalid = |params: NewSuggestion| store.create(params).expect_err("невалидное предложение");
    let mut base = NewSuggestion {
        doc_id: "01DOC".to_owned(),
        block_id: "b1".to_owned(),
        para_id: None,
        before: "старый".to_owned(),
        after: "новый".to_owned(),
        author: "agent".to_owned(),
        author_kind: "agent".to_owned(),
        base_body_hash: "a".repeat(64),
        note: None,
    };
    assert!(matches!(invalid(NewSuggestion { before: String::new(), ..base.clone() }), rde_doc::SuggestionError::Invalid(_)));
    assert!(matches!(invalid(NewSuggestion { after: String::new(), ..base.clone() }), rde_doc::SuggestionError::Invalid(_)));
    assert!(matches!(invalid(NewSuggestion { base_body_hash: String::new(), ..base.clone() }), rde_doc::SuggestionError::Invalid(_)));
    base.after = "старый".to_owned();
    assert!(matches!(invalid(base), rde_doc::SuggestionError::Invalid(_)));
    // Неизвестное предложение — NotFound.
    assert!(matches!(store.load("01NOPE").expect_err("нет предложения"), rde_doc::SuggestionError::NotFound(_)));
}

#[test]
fn stale_suggestion_is_not_applied_after_document_change() {
    let dir = tmp("stale");
    let package = demo_package(&dir);
    let store = Suggestions::new(&dir);
    let (block_id, before_text) = plain_paragraph(&package);
    let edit = store
        .create(NewSuggestion {
            doc_id: "01TESTDOC0000000000000000".to_owned(),
            block_id: block_id.clone(),
            para_id: None,
            before: before_text.clone(),
            after: "Вариант агента".to_owned(),
            author: "MCP agent".to_owned(),
            author_kind: "agent".to_owned(),
            base_body_hash: package_info(&package).unwrap().body_hash,
            note: None,
        })
        .unwrap();
    // Документ изменился вне предложения (другая правка/пользователь) — база устарела.
    replace_block_text(
        &package,
        &BlockTextEdit {
            block_id,
            expected_text: before_text,
            new_text: "Пользовательская правка".to_owned(),
        },
    )
    .unwrap();
    let error = store.apply(&edit.id, &package).expect_err("устаревшее предложение");
    assert!(matches!(error, rde_doc::SuggestionError::Conflict(_)), "{error}");
    assert_eq!(store.load(&edit.id).unwrap().status, STATUS_PENDING, "статус не меняется при отказе");
}

// ── M2.6: вставка/удаление абзацев ─────────────────────────────────────────────

#[test]
fn insert_paragraph_after_anchor_adds_block_and_shifts_ids() {
    let dir = tmp("insert-after");
    let package = demo_package(&dir);
    let before = document_blocks(&package).unwrap();
    let (anchor_id, anchor_text) = plain_paragraph(&package);
    let anchor_index = before.iter().position(|block| block.id == anchor_id).expect("якорь");
    let report = insert_paragraph(
        &package,
        &ParagraphInsert {
            anchor_block: Some(anchor_id.clone()),
            position: InsertPosition::After,
            text: "Вставленный абзац M2.6".to_owned(),
            style: String::new(),
        },
    )
    .expect("вставка абзаца");
    assert_eq!(report.text, "Вставленный абзац M2.6");
    assert_eq!(report.anchor_block.as_deref(), Some(anchor_id.as_str()));
    assert_eq!(report.para_id.len(), 8, "w14:paraId — 8 hex");
    // Новый блок — сразу после якоря; якорь сохранил текст и paraId (сдвинулись только id).
    let after = document_blocks(&package).unwrap();
    assert_eq!(after.len(), before.len() + 1);
    assert_eq!(after[anchor_index].text, anchor_text, "якорь на месте");
    assert_eq!(after[anchor_index + 1].text, "Вставленный абзац M2.6");
    assert_eq!(after[anchor_index + 1].id, report.block_id);
    let registry = load_rde_doc(&package).unwrap();
    assert_eq!(registry.body_hash, package_info(&package).unwrap().body_hash);
    let new_block = registry.blocks.iter().find(|block| block.id == report.block_id).expect("новый блок");
    assert_eq!(new_block.para_id.as_deref(), Some(report.para_id.as_str()));
    // paraId уникальны (engine-сохранение переинжектирует их позиционно, но не дублирует).
    let mut ids: Vec<_> = registry.blocks.iter().filter_map(|block| block.para_id.clone()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), registry.blocks.len());
}

#[test]
fn insert_at_end_and_before_anchor() {
    let dir = tmp("insert-positions");
    let package = demo_package(&dir);
    let before = document_blocks(&package).unwrap();
    let last_text = before.last().unwrap().text.clone();
    // В конец документа (anchor = None).
    let at_end = insert_paragraph(
        &package,
        &ParagraphInsert {
            anchor_block: None,
            position: InsertPosition::After,
            text: "Абзац в конце документа".to_owned(),
            style: String::new(),
        },
    )
    .expect("вставка в конец");
    let after_end = document_blocks(&package).unwrap();
    assert_eq!(after_end.last().map(|block| block.text.as_str()), Some("Абзац в конце документа"));
    assert_eq!(after_end[after_end.len() - 2].text, last_text);
    // Перед якорем.
    let (anchor_id, anchor_text) = plain_paragraph(&package);
    let anchor_index = after_end.iter().position(|block| block.id == anchor_id).expect("якорь");
    let before_insert = insert_paragraph(
        &package,
        &ParagraphInsert {
            anchor_block: Some(anchor_id.clone()),
            position: InsertPosition::Before,
            text: "Абзац перед якорем".to_owned(),
            style: String::new(),
        },
    )
    .expect("вставка перед якорем");
    let after_before = document_blocks(&package).unwrap();
    assert_eq!(after_before.len(), after_end.len() + 1);
    assert_eq!(after_before[anchor_index].text, "Абзац перед якорем");
    assert_eq!(after_before[anchor_index + 1].text, anchor_text);
    assert_ne!(before_insert.block_id, at_end.block_id);
}

#[test]
fn delete_block_removes_paragraph_and_guards() {
    let dir = tmp("delete");
    let package = demo_package(&dir);
    let before = document_blocks(&package).unwrap();
    let (block_id, text) = plain_paragraph(&package);
    // Устаревший ожидаемый текст — Stale (R-DOC-13), документ не меняется.
    let error = delete_block(
        &package,
        &BlockDelete { block_id: block_id.clone(), expected_text: "другой текст".to_owned() },
    )
    .expect_err("устаревший текст");
    assert!(matches!(error, BlockTextEditError::Stale { .. }), "{error}");
    assert_eq!(document_blocks(&package).unwrap().len(), before.len());
    // Неизвестный блок — NotFound.
    let error = delete_block(
        &package,
        &BlockDelete { block_id: "b9999".to_owned(), expected_text: "x".to_owned() },
    )
    .expect_err("нет блока");
    assert!(matches!(error, BlockTextEditError::NotFound(_)), "{error}");
    // Удаление: блок исчез, остальные сохранились (count-1).
    let report = delete_block(
        &package,
        &BlockDelete { block_id: block_id.clone(), expected_text: text.clone() },
    )
    .expect("удаление абзаца");
    assert_eq!(report.before, text);
    assert_eq!(report.block_id, block_id);
    let after = document_blocks(&package).unwrap();
    assert_eq!(after.len(), before.len() - 1);
    assert!(!after.iter().any(|block| block.text == text), "удалённый текст исчез");
    let registry = load_rde_doc(&package).unwrap();
    assert_eq!(registry.body_hash, package_info(&package).unwrap().body_hash);
    assert!(!registry.blocks.iter().any(|block| block.text == text), "блок удалён из реестра");
}

#[test]
fn insert_and_delete_validation_and_sdt_guard() {
    let dir = tmp("insert-validation");
    let package = demo_package(&dir);
    for text in ["", "   ", "первая\nвторая"] {
        let error = insert_paragraph(
            &package,
            &ParagraphInsert {
                anchor_block: None,
                position: InsertPosition::After,
                text: text.to_owned(),
                style: String::new(),
            },
        )
        .expect_err("невалидный текст");
        assert!(matches!(error, BlockTextEditError::Unsupported(_)), "{text:?}: {error}");
    }
    let error = insert_paragraph(
        &package,
        &ParagraphInsert {
            anchor_block: Some("b9999".to_owned()),
            position: InsertPosition::After,
            text: "x".to_owned(),
            style: String::new(),
        },
    )
    .expect_err("нет якоря");
    assert!(matches!(error, BlockTextEditError::NotFound(_)), "{error}");
    // Блок с RDE-вставкой (w:sdt) нельзя ни править, ни удалять.
    add_insert(
        &package,
        InsertRequest {
            kind: "value".to_owned(),
            target: "rde://artifact/01ART000000000000000000/out/value.txt".to_owned(),
            selector: "#/value".to_owned(),
            value_kind: "text".to_owned(),
            cached: "42".to_owned(),
            caption: String::new(),
            style: String::new(),
            format: "auto".to_owned(),
            run_style: String::new(),
            anchor_block: None,
        },
    )
    .expect("вставка значения");
    update_document_package(&package).expect("обновить пакет");
    let registry = load_rde_doc(&package).unwrap();
    if let Some(insert_block) = registry.blocks.iter().find(|block| {
        registry.inserts.iter().any(|insert| insert.para_id.is_some() && insert.para_id == block.para_id)
    }) {
        let error = delete_block(
            &package,
            &BlockDelete { block_id: insert_block.id.clone(), expected_text: insert_block.text.clone() },
        )
        .expect_err("sdt-блок не удаляется");
        assert!(matches!(error, BlockTextEditError::Unsupported(_)), "{error}");
    } else {
        eprintln!("движок не показал абзац вставки как блок — проверка sdt-удаления пропущена");
    }
}
