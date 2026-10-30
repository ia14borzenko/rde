//! P3 (ADR-0005 §3): вставки формул в документ, обновление/статусы, ручная правка, миграция,
//! откат транзакции. Кэши формулы — реальная компиляция (`rde_doc::compile_formula`, poppler).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rde_doc::{
    add_formula_insert, add_formula_insert_with_renumber_error, body_paragraph_ids, compile_formula, extract_docx,
    formula_insert_statuses, load_rde_doc, migrate_formulas, insert_equation, open_document, resolve_para_id_for_path,
    save_engine_document, unregistered_formulas, update_document_transactional, update_formula_inserts, FormulaAvailability,
    FormulaBody, FormulaInsertRequest, FormulaInsertState, FormulaMigrationPlan, FormulaNumbering, InsertAnchor,
};

const FORMULA_ID: &str = "01M5VJ1JG039MMV97SZGY6V9EQ";
const ARTIFACT_ID: &str = "01M5VJ1JG0HD99FDTGJ8YEPFZX";
const DOC_ID: &str = "01M5VJ1JG04TFF59TDWH9EDD1R";

fn poppler_available() -> bool {
    std::process::Command::new("pdftocairo").arg("-v").output().map(|output| output.status.success()).unwrap_or(false)
}

fn soffice_available() -> bool {
    std::process::Command::new("soffice").arg("--version").output().map(|output| output.status.success()).unwrap_or(false)
}

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-doc-formula-insert-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_project(root: &Path) {
    std::fs::write(
        root.join("rde.toml"),
        "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"Формулы\"\n",
    )
    .unwrap();
    let formula_dir = root.join("objects/formulas").join(FORMULA_ID);
    std::fs::create_dir_all(&formula_dir).unwrap();
    std::fs::write(
        formula_dir.join("formula.toml"),
        format!(
            "schema = \"rde/formula@1\"\nid = \"{FORMULA_ID}\"\ntitle = \"Закон Ома\"\n\
             latex = \"\\\\sigma=\\\\frac{{F}}{{A}}\"\ndisplay = true\n\n[numbering]\nmode = \"auto\"\nlabel = \"\"\n"
        ),
    )
    .unwrap();
}

/// Пустой `.doc.rde` проекта (чистый docx из WordCraft + rde:doc).
fn new_package(root: &Path) -> PathBuf {
    let docx = root.join("clean.docx");
    let document = wordcraft_engine::doc::Document::new();
    wordcraft_engine::io::save_path(&docx, &document).unwrap();
    let mut rde_doc = rde_doc::build_rde_doc(&docx, DOC_ID, "").unwrap();
    let package = root.join("report.doc.rde");
    rde_doc::write_package(&docx, &mut rde_doc, &package).unwrap();
    package
}

/// Содержимое части пакета.
fn part(package: &Path, name: &str) -> Option<String> {
    let file = std::fs::File::open(package).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let mut entry = archive.by_name(name).ok()?;
    let mut out = Vec::new();
    std::io::Read::read_to_end(&mut entry, &mut out).unwrap();
    Some(String::from_utf8_lossy(&out).to_string())
}

fn body(package: &Path) -> String {
    part(package, "word/document.xml").unwrap()
}

/// Полная пара кэшей формулы (OMML + SVG + PNG) из эффективного LaTeX.
fn compiled_body(latex: &str, display: bool, act_key: &str, numbered: bool) -> FormulaBody {
    let caches = compile_formula(latex, display).expect("компиляция формулы");
    FormulaBody {
        formula_id: FORMULA_ID.to_owned(),
        act_key: act_key.to_owned(),
        latex: latex.to_owned(),
        display,
        numbered,
        omml: caches.omml,
        svg: caches.svg,
        png: caches.png,
    }
}

/// Открыть → сохранить пакет движком WordCraft (как делает RDE при редактировании).
fn engine_roundtrip(package: &Path, root: &Path) -> rde_doc::RdeDoc {
    let temp = root.join("roundtrip.docx");
    extract_docx(package, &temp).unwrap();
    let (document, _encrypted) = open_document(&temp).unwrap();
    wordcraft_engine::io::save_path(&temp, &document).unwrap();
    save_engine_document(&temp, package, DOC_ID, "").unwrap()
}

#[test]
fn omml_insert_updates_and_roundtrips_through_engine() {
    if !poppler_available() {
        return; // SVG-пайплайн недоступен — честный skip
    }
    let root = tmp("omml");
    write_project(&root);
    let package = new_package(&root);

    let body_v1 = compiled_body("\\sigma=\\frac{F}{A}", true, "act-v1", true);
    let report = add_formula_insert(
        &package,
        FormulaInsertRequest { body: body_v1.clone(), render_mode: "omml".to_owned(), anchor: rde_doc::InsertAnchor::end(), numbering: rde_doc::FormulaNumbering::Continuous, },
    )
    .expect("вставка формулы");
    assert_eq!(report.insert.kind, "formula");
    assert_eq!(report.insert.render_mode, "omml");
    assert_eq!(report.insert.number, Some(1));
    assert!(report.insert.target.starts_with(&format!("rde://formula/{FORMULA_ID}@act-v1")));
    let document = body(&package);
    assert!(document.contains("rde:formula:"), "тег sdt формулы: {document}");
    assert!(document.contains("(1)"), "документно-локальный номер: {document}");

    let rde_doc = load_rde_doc(&package).unwrap();
    assert_eq!(rde_doc.schema_version, rde_doc::SCHEMA_VERSION);
    assert_eq!(rde_doc.inserts.iter().filter(|insert| insert.kind == "formula").count(), 1);

    // Статус «актуальна», затем «устарела» после изменения формулы.
    let availability = BTreeMap::from([(FORMULA_ID.to_owned(), FormulaAvailability::Current("act-v1".to_owned()))]);
    let statuses = formula_insert_statuses(&package, &availability).unwrap();
    assert_eq!(statuses.len(), 1);
    assert_eq!(statuses[0].state, FormulaInsertState::Fresh, "{}", statuses[0].detail);

    // Engine roundtrip (open → save) сохраняет sdt формулы и OMML.
    let roundtripped = engine_roundtrip(&package, &root);
    assert!(roundtripped.inserts.iter().any(|insert| insert.kind == "formula"));
    let document = body(&package);
    assert!(document.contains("rde:formula:"), "после roundtrip: {document}");
    assert!(document.contains("<m:oMathPara"), "OMML после roundtrip: {document}");
    let statuses = formula_insert_statuses(&package, &availability).unwrap();
    assert_eq!(statuses[0].state, FormulaInsertState::Fresh, "{}", statuses[0].detail);

    // Формула изменилась: вставка устарела, «Обновить документ» пересобирает её.
    let availability_v2 = BTreeMap::from([(FORMULA_ID.to_owned(), FormulaAvailability::Current("act-v2".to_owned()))]);
    let statuses = formula_insert_statuses(&package, &availability_v2).unwrap();
    assert_eq!(statuses[0].state, FormulaInsertState::Stale);
    let body_v2 = compiled_body("\\sigma=\\frac{F}{2A}", true, "act-v2", true);
    let bodies = BTreeMap::from([(FORMULA_ID.to_owned(), body_v2)]);
    let update = update_formula_inserts(&package, &bodies).unwrap();
    assert_eq!(update.updated.len(), 1, "обновлено: {:?}", update.updated);
    assert!(update.conflicts.is_empty());
    let document = body(&package);
    assert!(document.contains("<m:oMathPara"), "OMML обновлённой формулы: {document}");
    let statuses = formula_insert_statuses(&package, &availability_v2).unwrap();
    assert_eq!(statuses[0].state, FormulaInsertState::Fresh, "{}", statuses[0].detail);
    let updated_insert = &load_rde_doc(&package).unwrap().inserts[0];
    assert_eq!(updated_insert.hash.as_deref(), Some("act-v2"));
    assert!(
        updated_insert.target.ends_with("@act-v2"),
        "целевой URI обновлён вместе с act_key (иначе инспектор показывает «устарела»): {}",
        updated_insert.target
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn svg_insert_keeps_media_rels_and_content_types() {
    if !poppler_available() {
        return;
    }
    let root = tmp("svg");
    write_project(&root);
    let package = new_package(&root);
    let body_v1 = compiled_body("\\sigma=\\frac{F}{A}", true, "act-svg", true);
    let report = add_formula_insert(
        &package,
        FormulaInsertRequest { body: body_v1.clone(), render_mode: "svg".to_owned(), anchor: rde_doc::InsertAnchor::end(), numbering: rde_doc::FormulaNumbering::Continuous, },
    )
    .expect("вставка svg-формулы");
    let document = body(&package);
    assert_eq!(report.insert.render_mode, "svg");
    assert!(document.contains("svgBlip"), "векторная часть: {document}");
    assert!(document.contains("{96DAC541-7B7A-43D3-8B79-37D633B846F1}"), "URI расширения svgBlip");
    let svg_part = format!("word/media/rde-formula-{}.svg", report.insert.id);
    let png_part = format!("word/media/rde-formula-{}.png", report.insert.id);
    assert!(part(&package, &svg_part).is_some(), "нет svg-части {svg_part}");
    assert!(part(&package, &png_part).is_some(), "нет png-fallback {png_part}");
    let rels = part(&package, "word/_rels/document.xml.rels").unwrap();
    assert!(rels.contains(&format!("Target=\"media/rde-formula-{}.svg\"", report.insert.id)), "{rels}");
    let content_types = part(&package, "[Content_Types].xml").unwrap();
    assert!(content_types.contains("Extension=\"svg\""), "{content_types}");
    assert!(content_types.contains("image/svg+xml"), "{content_types}");

    // Engine roundtrip сохраняет svgBlip и SVG-часть (P2.5) — SVG-часть байт-в-байт.
    let svg_before = part(&package, &svg_part).unwrap();
    engine_roundtrip(&package, &root);
    assert_eq!(part(&package, &svg_part).as_deref(), Some(svg_before.as_str()), "SVG-часть после roundtrip");
    assert!(body(&package).contains("svgBlip"), "svgBlip после roundtrip: {}", body(&package));
    let availability = BTreeMap::from([(FORMULA_ID.to_owned(), FormulaAvailability::Current("act-svg".to_owned()))]);
    assert_eq!(formula_insert_statuses(&package, &availability).unwrap()[0].state, FormulaInsertState::Fresh);

    // Независимый оракул: LibreOffice открывает пакет с SVG-формулой (свой профиль в tmp —
    // ~/.cache на стенде read-only).
    if soffice_available() {
        let out_dir = root.join("lo-out");
        let profile = root.join("lo-profile");
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::create_dir_all(&profile).unwrap();
        let output = std::process::Command::new("soffice")
            .args(["--headless", "--nologo", "--nofirststartwizard", "--norestore"])
            .arg(format!("-env:UserInstallation=file://{}", profile.display()))
            .args(["--convert-to", "pdf", "--outdir"])
            .arg(&out_dir)
            .arg(&package)
            .output()
            .unwrap();
        assert!(output.status.success(), "LibreOffice не открыл пакет с svg-формулой: {}", String::from_utf8_lossy(&output.stderr));
        // LO называет файл по имени пакета (`report.doc.pdf`) — берём первый PDF в каталоге.
        let pdf = std::fs::read_dir(&out_dir)
            .unwrap()
            .flatten()
            .map(|entry| entry.path())
            .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("pdf"))
            .and_then(|path| std::fs::read(path).ok())
            .unwrap_or_default();
        assert!(pdf.starts_with(b"%PDF") && pdf.len() > 1024, "LO не создал валидный PDF ({} байт)", pdf.len());
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// P6 (ADR-0022 §3.3): в SVG-вставке display-формулы номер `(N)` поднят на глубину формулы
/// (`w:position`), чтобы базовая линия номера совпала с базовой линией формулы: низ картинки —
/// inline-объект — Word/WordCraft ставят на базовую линию абзаца. У OMML-вставки подъёма нет
/// (формула живёт на базовой линии сама).
#[test]
fn svg_numbered_insert_raises_number_to_math_baseline() {
    if !poppler_available() {
        return;
    }
    let root = tmp("svg-baseline");
    write_project(&root);
    let metrics = rde_doc::formula_metrics("\\sigma=\\frac{F}{A}", true).expect("метрики формулы");
    let body_v1 = compiled_body("\\sigma=\\frac{F}{A}", true, "act-baseline", true);

    // SVG + номер: подъём = descent + отступ кропа (half-points, как в w:position).
    let package = new_package(&root);
    add_formula_insert(
        &package,
        FormulaInsertRequest { body: body_v1.clone(), render_mode: "svg".to_owned(), anchor: rde_doc::InsertAnchor::end(), numbering: rde_doc::FormulaNumbering::Continuous },
    )
    .expect("svg-вставка");
    let document = body(&package);
    let expected = ((metrics.descent_pt + rde_doc::SVG_CROP_PAD_PT) * 2.0).round().max(1.0) as i64;
    assert!(
        document.contains(&format!("<w:position w:val=\"{expected}\"/>")),
        "номер поднят на {expected} half-points (descent {:.2}pt): {document}",
        metrics.descent_pt
    );
    // Низ картинки и номер — в одном абзаце: позиция стоит в run'е номера после второго таба.
    assert!(document.contains(&format!("({})", 1)), "номер формулы на месте: {document}");

    // SVG без номера: w:position не нужен.
    let package_plain = new_package(&root);
    let plain = FormulaBody { numbered: false, ..body_v1.clone() };
    add_formula_insert(
        &package_plain,
        FormulaInsertRequest { body: plain, render_mode: "svg".to_owned(), anchor: rde_doc::InsertAnchor::end(), numbering: rde_doc::FormulaNumbering::Continuous },
    )
    .expect("svg-вставка без номера");
    let document_plain = body(&package_plain);
    assert!(!document_plain.contains("<w:position"), "без номера подъём не нужен: {document_plain}");

    // OMML + номер: подъёма нет (номер и формула на одной базовой линии).
    let package_omml = new_package(&root);
    add_formula_insert(
        &package_omml,
        FormulaInsertRequest { body: body_v1, render_mode: "omml".to_owned(), anchor: rde_doc::InsertAnchor::end(), numbering: rde_doc::FormulaNumbering::Continuous },
    )
    .expect("omml-вставка");
    let document_omml = body(&package_omml);
    assert!(!document_omml.contains("<w:position"), "OMML — без подъёма номера: {document_omml}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn missing_target_and_manual_edit_are_noted_not_overwritten() {
    if !poppler_available() {
        return;
    }
    let root = tmp("conflict");
    write_project(&root);
    let package = new_package(&root);
    let body_v1 = compiled_body("\\sigma=\\frac{F}{A}", true, "act-v1", true);
    let report = add_formula_insert(
        &package,
        FormulaInsertRequest { body: body_v1, render_mode: "omml".to_owned(), anchor: rde_doc::InsertAnchor::end(), numbering: rde_doc::FormulaNumbering::Continuous, },
    )
    .unwrap();

    // Цель отсутствует.
    let missing = BTreeMap::from([(FORMULA_ID.to_owned(), FormulaAvailability::Missing)]);
    let statuses = formula_insert_statuses(&package, &missing).unwrap();
    assert_eq!(statuses[0].state, FormulaInsertState::MissingTarget);

    // «Не вычислена».
    let not_computed = BTreeMap::from([(FORMULA_ID.to_owned(), FormulaAvailability::NotComputed("нет входа".to_owned()))]);
    assert_eq!(formula_insert_statuses(&package, &not_computed).unwrap()[0].state, FormulaInsertState::NotComputed);

    // Ручная правка тела: меняем OMML внутри sdt — обновление не перезаписывает его (R-DOC-13).
    let document = body(&package);
    let marker = "rde:formula:";
    let tag_at = document.find(marker).unwrap();
    let math_at = document[tag_at..].find("<m:oMathPara").unwrap() + tag_at;
    let mut edited = document.clone();
    edited.replace_range(math_at..math_at + "<m:oMathPara".len(), "<m:oMathPara><m:r><m:t>X</m:t></m:r></m:oMathPara><m:oMathPara");
    write_part(&package, "word/document.xml", edited.as_bytes());
    let availability = BTreeMap::from([(FORMULA_ID.to_owned(), FormulaAvailability::Current("act-v1".to_owned()))]);
    let statuses = formula_insert_statuses(&package, &availability).unwrap();
    assert_eq!(statuses[0].state, FormulaInsertState::BodyModified, "{}", statuses[0].detail);
    let before = body(&package);
    let bodies = BTreeMap::from([(FORMULA_ID.to_owned(), compiled_body("\\sigma=\\frac{F}{2A}", true, "act-v2", true))]);
    let update = update_formula_inserts(&package, &bodies).unwrap();
    assert_eq!(update.conflicts, vec![report.insert.id.clone()]);
    assert_eq!(body(&package), before, "тело с ручной правкой не перезаписано");
    let _ = std::fs::remove_dir_all(&root);
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
    for (entry_name, data) in entries.iter_mut() {
        if entry_name == name {
            *data = bytes.to_vec();
        }
    }
    let out = std::fs::File::create(package).unwrap();
    let mut zip = zip::ZipWriter::new(out);
    let options = zip::write::SimpleFileOptions::default();
    for (entry_name, data) in &entries {
        zip.start_file(entry_name.as_str(), options).unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn migration_converts_phase1_equation_and_is_idempotent() {
    if !poppler_available() {
        return;
    }
    let root = tmp("migration");
    write_project(&root);
    let package = new_package(&root);
    // Формула «фазы 1» — обычный OMML в таблице 1×2 с номером (1), без объекта формулы.
    insert_equation(&package, "\\sigma=\\frac{F}{A}", Some(1)).expect("формула фазы 1");
    let unregistered = unregistered_formulas(&package).unwrap();
    assert_eq!(unregistered.len(), 1, "формул вне реестра: {unregistered:?}");
    assert!(unregistered[0].latex.contains("sigma"));
    assert!(unregistered[0].display);

    let caches = compile_formula(&unregistered[0].latex, true).unwrap();
    let plans = vec![FormulaMigrationPlan {
        formula_id: FORMULA_ID.to_owned(),
        act_key: "act-migrated".to_owned(),
        latex: unregistered[0].latex.clone(),
        omml: caches.omml,
    }];
    let report = migrate_formulas(&package, &plans).unwrap();
    assert_eq!(report.replaced_tables, 1, "мигрирована таблица: {report:?}");
    assert_eq!(report.insert_ids.len(), 1);
    let document = body(&package);
    assert!(document.contains("rde:formula:"), "вставка-ссылка: {document}");
    assert!(document.contains("(1)"), "номер сохранён: {document}");
    let rde_doc = load_rde_doc(&package).unwrap();
    let insert = rde_doc.inserts.iter().find(|insert| insert.kind == "formula").expect("вставка формулы в rde:doc");
    assert_eq!(insert.number, Some(1));
    assert_eq!(insert.target, format!("rde://formula/{FORMULA_ID}@act-migrated"));

    // Идемпотентность: формула уже в реестре — миграция ничего не делает.
    assert!(unregistered_formulas(&package).unwrap().is_empty());
    let again = migrate_formulas(&package, &[]).unwrap();
    assert!(again.insert_ids.is_empty());

    // Engine roundtrip не ломает мигрированную вставку.
    engine_roundtrip(&package, &root);
    assert!(body(&package).contains("rde:formula:"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rollback_transaction_restores_previous_formula_insert() {
    if !poppler_available() {
        return;
    }
    let root = tmp("rollback");
    write_project(&root);
    let package = new_package(&root);
    let mut store = rde_store::Store::open(&root).unwrap();
    let body_v1 = compiled_body("\\sigma=\\frac{F}{A}", true, "act-v1", true);
    add_formula_insert(&package, FormulaInsertRequest { body: body_v1, render_mode: "omml".to_owned(), anchor: rde_doc::InsertAnchor::end(), numbering: rde_doc::FormulaNumbering::Continuous, }).unwrap();
    let before = body(&package);
    rde_doc::commit_document(&root, &package, DOC_ID, &mut store, "v1").unwrap();

    // Обновление формульной вставки транзакционно (без пересчёта формул — кэши уже есть).
    let bodies_v2 = BTreeMap::from([(FORMULA_ID.to_owned(), compiled_body("\\sigma=\\frac{F}{2A}", true, "act-v2", true))]);
    let update = update_document_transactional(
        &root,
        &package,
        DOC_ID,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &bodies_v2,
        rde_doc::FormulaNumbering::Continuous,
        &BTreeMap::new(),
        Some(serde_json::json!({ "kind": "document-formula-update", "formulas": [] })),
        &mut store,
    )
    .expect("транзакционное обновление документа");
    assert_eq!(update.formula_updates.len(), 1);
    let tx = update.txid.expect("txid");
    assert_ne!(body(&package), before);

    // Откат транзакции восстанавливает пакет из CAS (вставки — прежние).
    store.rollback(&tx).unwrap();
    rde_doc::restore_document_from_store(&root, &package, DOC_ID, &store).unwrap().expect("пакет из store");
    assert_eq!(body(&package), before, "откат восстановил прежнюю формулу");
    let restored = load_rde_doc(&package).unwrap();
    assert_eq!(restored.inserts[0].hash.as_deref(), Some("act-v1"));
    let _ = std::fs::remove_dir_all(&root);
}

/// Кэш-бандл формулы из ledger/CAS (ADR-0005 §2.4): OMML/SVG/PNG по хешам состояния.
fn body_from_state(store: &rde_store::Store, formula_id: &str, state: &rde_dag::FormulaState, latex: &str, display: bool) -> FormulaBody {
    let blob = |hash: &Option<String>| -> Option<Vec<u8>> {
        let hash = hash.as_deref()?;
        let hash = rde_hash::Hash::from_hex(hash).ok()?;
        store.get_blob(hash).ok()?
    };
    FormulaBody {
        formula_id: formula_id.to_owned(),
        act_key: state.act_key.clone(),
        latex: latex.to_owned(),
        display,
        numbered: true,
        omml: blob(&state.omml_hash).expect("OMML-блоб кэша"),
        svg: blob(&state.svg_hash),
        png: blob(&state.png_hash),
    }
}

/// «Обновление при изменении входа»: значение входа формулы из артефакта меняется → `act_key`
/// формулы новый → вставка устарела; пересчёт кэшей и документа одной транзакцией (ADR-0005 §3.3)
/// пересобирает вставку, статус снова Fresh, ledger видит формулу по новой транзакции.
#[test]
fn input_change_marks_instale_then_updates_in_one_transaction() {
    if !poppler_available() {
        return;
    }
    let root = tmp("input-change");
    std::fs::write(root.join("rde.toml"), "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"Формулы\"\n").unwrap();
    let artifact_dir = root.join("objects/artifacts").join(ARTIFACT_ID);
    std::fs::create_dir_all(artifact_dir.join("out")).unwrap();
    std::fs::write(
        artifact_dir.join("artifact.toml"),
        format!(
            "schema = \"rde/artifact@1\"\nid = \"{ARTIFACT_ID}\"\ntitle = \"Данные\"\ntype = \"data\"\noutputs = [\"out/value.json\"]\n"
        ),
    )
    .unwrap();
    std::fs::write(artifact_dir.join("out/value.json"), "{\"mean\": 42}").unwrap();
    let formula_dir = root.join("objects/formulas").join(FORMULA_ID);
    std::fs::create_dir_all(&formula_dir).unwrap();
    std::fs::write(
        formula_dir.join("formula.toml"),
        format!(
            "schema = \"rde/formula@1\"\nid = \"{FORMULA_ID}\"\ntitle = \"Среднее\"\n\
             latex = \"\\\\sigma=\\\\rdeinput{{mean}}\"\ndisplay = true\n\n[numbering]\nmode = \"auto\"\nlabel = \"\"\n\n\
             [[inputs]]\nalias = \"mean\"\nmode = \"hashed\"\nsource = \"rde://artifact/{ARTIFACT_ID}/out/value.json#/mean\"\nas = \"raw\"\n"
        ),
    )
    .unwrap();
    let compile = |latex: &str, display: bool| {
        let caches = compile_formula(latex, display)?;
        Ok(rde_dag::FormulaCompiled { omml: caches.omml, svg: caches.svg, png: caches.png })
    };
    let renderer = rde_dag::FormulaRenderer { id: rde_doc::FORMULA_RENDERER_ID, version: rde_doc::FORMULA_RENDERER_VERSION };
    let mut store = rde_store::Store::open(&root).unwrap();
    let project = rde_core::load_project(&root).unwrap();
    assert!(project.issues.is_empty(), "issues: {:?}", project.issues);
    let prepared = rde_dag::prepare_formula_update(&project, &mut store, FORMULA_ID, &renderer, compile).expect("компиляция формулы");
    let act_key_v1 = prepared.state.act_key.clone();
    let omml_v1 = prepared.state.omml_hash.clone().unwrap_or_default();
    rde_dag::commit_formula_update(&mut store, &prepared.state, prepared.cache_tree, "formula v1").unwrap();
    rde_dag::record_prepared_formula_state(&project, &prepared).unwrap();

    let package = new_package(&root);
    let package = {
        let body_v1 = body_from_state(&store, FORMULA_ID, &prepared.state, "\\sigma=42", true);
        add_formula_insert(&package, FormulaInsertRequest { body: body_v1, render_mode: "omml".to_owned(), anchor: rde_doc::InsertAnchor::end(), numbering: rde_doc::FormulaNumbering::Continuous, }).expect("вставка");
        package
    };
    let availability = BTreeMap::from([(FORMULA_ID.to_owned(), FormulaAvailability::Current(act_key_v1.clone()))]);
    assert_eq!(formula_insert_statuses(&package, &availability).unwrap()[0].state, FormulaInsertState::Fresh);

    // Вход изменился: пересчёт кэшей готовит новую транзакцию; вставка устарела.
    std::fs::write(artifact_dir.join("out/value.json"), "{\"mean\": 43}").unwrap();
    let project = rde_core::load_project(&root).unwrap();
    let prepared_v2 = rde_dag::prepare_formula_update(&project, &mut store, FORMULA_ID, &renderer, compile).expect("пересчёт формулы");
    assert_ne!(prepared_v2.state.act_key, act_key_v1, "act_key зависит от входа");
    assert_ne!(prepared_v2.state.omml_hash.clone().unwrap_or_default(), omml_v1);
    let availability = BTreeMap::from([(FORMULA_ID.to_owned(), FormulaAvailability::Current(prepared_v2.state.act_key.clone()))]);
    assert_eq!(formula_insert_statuses(&package, &availability).unwrap()[0].state, FormulaInsertState::Stale);

    // Одна транзакция: кэши формулы (корни formula/*) + документ (корень document/*).
    let body_v2 = body_from_state(&store, FORMULA_ID, &prepared_v2.state, "\\sigma=43", true);
    let bodies = BTreeMap::from([(FORMULA_ID.to_owned(), body_v2)]);
    let roots = rde_dag::prepared_formula_roots(std::slice::from_ref(&prepared_v2));
    let mut payload = rde_dag::prepared_formulas_payload(std::slice::from_ref(&prepared_v2));
    payload["kind"] = serde_json::json!("document-formula-update");
    let update = update_document_transactional(
        &root,
        &package,
        DOC_ID,
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        &bodies,
        rde_doc::FormulaNumbering::Continuous,
        &roots,
        Some(payload),
        &mut store,
    )
    .expect("транзакционное обновление формулы и документа");
    assert_eq!(update.formula_updates.len(), 1, "вставка пересобрана");
    rde_dag::record_prepared_formula_state(&project, &prepared_v2).unwrap();
    assert_eq!(formula_insert_statuses(&package, &availability).unwrap()[0].state, FormulaInsertState::Fresh);
    // Ledger формулы читается из смешанной транзакции (`document-formula-update`).
    let ledger = rde_dag::formula_ledger(&store).unwrap();
    assert_eq!(ledger[FORMULA_ID].act_key, prepared_v2.state.act_key, "ledger видит новый act_key");
    let _ = std::fs::remove_dir_all(&root);
}

// ── P9 (итерация inline-formula-insert): однозначный якорь и позиционная inline-вставка ────────

/// `word/document.xml` с контролируемым телом (namespace `w14` — анкоры абзацев).
fn document_xml_with_body(body_xml: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\
         <w:document xmlns:wpc=\"http://schemas.microsoft.com/office/word/2010/wordprocessingCanvas\" \
         xmlns:r=\"http://schemas.openxmlformats.org/officeDocument/2006/relationships\" \
         xmlns:m=\"http://schemas.openxmlformats.org/officeDocument/2006/math\" \
         xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\" \
         xmlns:w14=\"http://schemas.microsoft.com/office/word/2010/wordml\"><w:body>{body_xml}<w:sectPr/></w:body></w:document>"
    )
}

/// Пакет `.doc.rde` с заданным телом `word/document.xml` (WordCraft читает его как обычный docx;
/// `write_package` проставляет `w14:paraId` и реестр блоков).
fn package_with_body(root: &Path, body_xml: &str) -> PathBuf {
    let skeleton = root.join("controlled.docx");
    let document = wordcraft_engine::doc::Document::new();
    wordcraft_engine::io::save_path(&skeleton, &document).unwrap();
    write_part(&skeleton, "word/document.xml", document_xml_with_body(body_xml).as_bytes());
    let mut rde_doc = rde_doc::build_rde_doc(&skeleton, DOC_ID, "").unwrap();
    let package = root.join("report.doc.rde");
    rde_doc::write_package(&skeleton, &mut rde_doc, &package).unwrap();
    package
}

fn para_ids(package: &Path) -> Vec<Option<String>> {
    body_paragraph_ids(&body(package)).unwrap()
}

/// Абзацы и их `paraId` для документа с двумя абзацами (целевой — с разными прогонами).
fn two_paragraph_body() -> String {
    "<w:p><w:pPr><w:ind w:firstLine=\"0\"/></w:pPr>\
     <w:r><w:rPr><w:b/></w:rPr><w:t xml:space=\"preserve\">До формулы </w:t></w:r>\
     <w:r><w:t>после запятой, и хвост.</w:t></w:r></w:p>\
     <w:p><w:r><w:t>Другой абзац</w:t></w:r></w:p>"
        .to_owned()
}

const TARGET_TEXT: &str = "До формулы после запятой, и хвост.";

/// Вставка inline-формулы в середину абзаца: sdt ровно между прогонами, чужой текст и
/// форматирование не изменены; повторная вставка — отдельный id в том же абзаце.
#[test]
fn inline_insert_lands_between_runs_in_target_paragraph() {
    if !poppler_available() {
        return;
    }
    let root = tmp("inline-mid");
    write_project(&root);
    let package = package_with_body(&root, &two_paragraph_body());
    let ids = para_ids(&package);
    assert_eq!(ids.len(), 2);
    let target = ids[0].clone().expect("paraId первого абзаца");
    let offset = TARGET_TEXT.find("после").unwrap();
    let body_v = compiled_body("\\left\\vert R_{оп}-R_{ц}\\right\\vert", false, "act-inline", false);
    let request = |offset: usize| FormulaInsertRequest {
        body: body_v.clone(),
        render_mode: "omml".to_owned(),
        anchor: InsertAnchor::paragraph(target.clone(), Some(offset), Some(TARGET_TEXT.to_owned())),
        numbering: FormulaNumbering::Continuous,
    };
    let report = add_formula_insert(&package, request(offset)).expect("inline-вставка");
    assert_eq!(report.warnings, Vec::<String>::new());
    let placement = report.placement.clone().expect("позиция вставки");
    assert!(placement.exact, "позиция каретки соблюдена: {placement:?}");
    assert_eq!(placement.para_id.as_deref(), Some(target.as_str()));
    assert_eq!(placement.offset, Some(offset));
    assert_eq!(report.insert.kind, "formula");
    assert_eq!(report.insert.render_mode, "omml");
    assert!(report.insert.number.is_none(), "inline без номера");
    assert_eq!(report.insert.para_id.as_deref(), Some(target.as_str()), "анкор inline-вставки — хост-абзац");
    let xml = body(&package);
    let tag = format!("rde:formula:{}", report.insert.id);
    let at = xml.find(&tag).expect("sdt в документе");
    let para_start = xml[..at].rfind("<w:p ").expect("абзац sdt");
    let para_end = xml[para_start..].find("</w:p>").unwrap() + para_start;
    assert!(at > para_start && at < para_end, "sdt внутри абзаца");
    assert!(
        xml[para_start..para_end].contains(&format!("w14:paraId=\"{target}\"")),
        "sdt в целевом абзаце, а не в чужом: {}",
        &xml[para_start..para_end]
    );
    assert_eq!(xml[..para_start].matches("rde:formula:").count(), 0, "в предыдущих абзацах вставок нет");
    assert_eq!(
        xml[para_end..].matches("rde:formula:").count(),
        0,
        "в следующих абзацах вставок нет (никакого append в конец документа)"
    );
    let left = &xml[para_start..at];
    let right = &xml[at..para_end];
    assert!(left.contains("До формулы "), "текст до вставки сохранён: {left}");
    assert!(!left.contains("после запятой"), "формула до текста после запятой: {left}");
    assert!(right.contains("после запятой"), "текст после вставки сохранён: {right}");
    assert_eq!(xml.matches("<w:b/>").count(), 1, "форматирование прогона не задвоено");
    assert_eq!(xml.matches("До формулы ").count(), 1, "текст не продублирован");
    // Реестр и статус.
    let rde_doc = load_rde_doc(&package).unwrap();
    let insert = rde_doc.inserts.iter().find(|insert| insert.id == report.insert.id).unwrap();
    assert_eq!(insert.para_id.as_deref(), Some(target.as_str()));
    let availability = BTreeMap::from([(FORMULA_ID.to_owned(), FormulaAvailability::Current("act-inline".to_owned()))]);
    assert_eq!(formula_insert_statuses(&package, &availability).unwrap()[0].state, FormulaInsertState::Fresh);
    // Документ переживает roundtrip через WordCraft (sdt и OMML на месте).
    let roundtripped = engine_roundtrip(&package, &root);
    assert!(roundtripped.inserts.iter().any(|insert| insert.id == report.insert.id));
    let xml = body(&package);
    assert!(xml.contains(&tag), "sdt после roundtrip: {xml}");
    assert!(xml.contains("<m:oMath"), "OMML после roundtrip");
    // Повторная вставка в то же место: новый id, тот же абзац, без чужих абзацев.
    let second = add_formula_insert(&package, request(offset)).expect("повторная inline-вставка");
    assert_ne!(second.insert.id, report.insert.id, "повторная вставка — отдельный id");
    let xml = body(&package);
    assert_eq!(xml.matches(&format!("rde:formula:{}", report.insert.id)).count(), 1);
    assert_eq!(xml.matches(&format!("rde:formula:{}", second.insert.id)).count(), 1);
    let target_pos = xml.find(&format!("w14:paraId=\"{target}\"")).unwrap();
    let target_end = xml[target_pos..].find("</w:p>").unwrap() + target_pos;
    assert_eq!(xml[..target_pos].matches("rde:formula:").count(), 0, "вставки только в целевом абзаце");
    assert_eq!(xml[target_end..].matches("rde:formula:").count(), 0, "вставки только в целевом абзаце");
    let _ = std::fs::remove_dir_all(&root);
}

/// Якорь по `paraId` в абзаце таблицы; абзац без `w14:paraId` — по порядковому номеру;
/// соответствие путь WordCraft ↔ paraId однозначно проверяется по числу абзацев.
#[test]
fn anchor_resolves_table_cell_and_para_without_para_id() {
    if !poppler_available() {
        return;
    }
    let root = tmp("anchor-table");
    write_project(&root);
    let package = package_with_body(
        &root,
        "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>В ячейке</w:t></w:r></w:p></w:tc>\
         <w:tc><w:p><w:r><w:t>Вторая ячейка</w:t></w:r></w:p></w:tc></w:tr></w:tbl>\
         <w:p><w:r><w:t>После таблицы</w:t></w:r></w:p>",
    );
    let ids = para_ids(&package);
    assert_eq!(ids.len(), 3, "два абзаца таблицы + абзац после: {ids:?}");
    let cell = ids[0].clone().expect("paraId первого абзаца таблицы");
    // 1) Таблица: путь WordCraft → paraId (число абзацев разметки и модели совпадает).
    let (doc, _is_rde) = open_document(&package).unwrap();
    let paths = doc.para_paths(wordcraft_engine::doc::StoryRef::Body);
    assert_eq!(paths.len(), ids.len(), "модель и разметка совпадают по числу абзацев");
    for (path, expected) in paths.iter().zip(ids.iter()) {
        let (para_id, ordinal) = resolve_para_id_for_path(&package, &doc, path).expect("резолв пути");
        assert_eq!(&Some(para_id), expected, "paraId пути {path} (ordinal {ordinal})");
    }
    // 2) Inline-вставка в абзац таблицы попадает в его ячейку.
    let body_v = compiled_body("x_{i}", false, "act-cell", false);
    let text = "В ячейке";
    let report = add_formula_insert(
        &package,
        FormulaInsertRequest {
            body: body_v,
            render_mode: "omml".to_owned(),
            anchor: InsertAnchor::paragraph(cell.clone(), Some(text.len()), Some(text.to_owned())),
            numbering: FormulaNumbering::Continuous,
        },
    )
    .expect("вставка в абзац таблицы");
    assert!(report.placement.as_ref().is_some_and(|placement| placement.exact));
    let xml = body(&package);
    let tag = format!("rde:formula:{}", report.insert.id);
    let at = xml.find(&tag).unwrap();
    assert!(at < xml.find("</w:tbl>").unwrap(), "sdt внутри таблицы: {xml}");
    assert_eq!(xml[..at].matches("<w:tc>").count(), 1, "sdt в первой ячейке");
    // 3) Абзац без `w14:paraId` (импортированный/внешний документ): якорь по порядковому номеру.
    let mut stripped = body(&package);
    let third_para = stripped.rfind("<w:p ").unwrap();
    let open_end = stripped[third_para..].find('>').unwrap() + third_para;
    let open_tag = stripped[third_para..open_end].to_owned();
    assert!(open_tag.contains("w14:paraId="), "{open_tag}");
    stripped.replace_range(third_para..open_end, "<w:p>");
    write_part(&package, "word/document.xml", stripped.as_bytes());
    let ids = para_ids(&package);
    assert_eq!(ids.len(), 3);
    assert!(ids[2].is_none(), "у абзаца удалён paraId: {ids:?}");
    let body_v = compiled_body("y_{i}", false, "act-ordinal", false);
    let text = "После таблицы";
    let report = add_formula_insert(
        &package,
        FormulaInsertRequest {
            body: body_v,
            render_mode: "omml".to_owned(),
            anchor: InsertAnchor::ordinal(2, Some(text.len()), Some(text.to_owned())),
            numbering: FormulaNumbering::Continuous,
        },
    )
    .expect("вставка по порядковому якорю");
    assert!(report.placement.as_ref().is_some_and(|placement| placement.exact));
    let xml = body(&package);
    let at = xml.find(&format!("rde:formula:{}", report.insert.id)).unwrap();
    let para_start = xml[..at].rfind("<w:p").unwrap();
    assert!(xml[para_start..at].contains("После таблицы"), "sdt в абзаце без paraId: {}", &xml[para_start..at]);
    // 4) Рассинхронизация разметки и модели → ошибка (никакого угадывания по номеру).
    let (doc, _) = open_document(&package).unwrap();
    let paths = doc.para_paths(wordcraft_engine::doc::StoryRef::Body);
    let mut extra = body(&package);
    extra = extra.replace("</w:body>", "<w:p><w:r><w:t>Лишний абзац</w:t></w:r></w:p></w:body>");
    write_part(&package, "word/document.xml", extra.as_bytes());
    let error = resolve_para_id_for_path(&package, &doc, &paths[0]).expect_err("рассинхронизация отклонена");
    assert!(error.contains("абзацев"), "понятная ошибка: {error}");
    let _ = std::fs::remove_dir_all(&root);
}

/// Неуспешная вставка не меняет пакет побайтово и не создаёт бэкапов (`.bak`/`doc-backups`).
#[test]
fn failed_inline_insert_leaves_package_untouched_without_backups() {
    if !poppler_available() {
        return;
    }
    let root = tmp("inline-rollback");
    write_project(&root);
    let package = package_with_body(&root, &two_paragraph_body());
    let before = std::fs::read(&package).unwrap();
    let before_sha = rde_hash::sha256(&before).hex();
    let body_v = compiled_body("z", false, "act-fail", false);
    let try_insert = |anchor: InsertAnchor| {
        add_formula_insert(
            &package,
            FormulaInsertRequest {
                body: body_v.clone(),
                render_mode: "omml".to_owned(),
                anchor,
                numbering: FormulaNumbering::Continuous,
            },
        )
    };
    let error = try_insert(InsertAnchor::paragraph("DEADBEEF", Some(1), Some(TARGET_TEXT.to_owned())))
        .expect_err("несуществующий paraId");
    assert!(error.contains("не найден"), "{error}");
    let error = try_insert(InsertAnchor::end()).expect_err("inline без якоря — ошибка, а не append в конец");
    assert!(error.contains("якоря"), "{error}");
    let target = para_ids(&package)[0].clone().unwrap();
    let error = try_insert(InsertAnchor::paragraph(target.clone(), Some(10_000), Some(TARGET_TEXT.to_owned())))
        .expect_err("позиция за концом абзаца");
    assert!(!error.is_empty());
    let error = try_insert(InsertAnchor::paragraph(target, Some(3), Some("чужой текст".to_owned())))
        .expect_err("текст не совпал");
    assert!(error.contains("не совпала"), "{error}");
    assert_eq!(rde_hash::sha256(std::fs::read(&package).unwrap()).hex(), before_sha, "пакет побайтово не изменён");
    assert_eq!(std::fs::read(&package).unwrap(), before);
    assert!(!root.join(".rde/doc-backups").exists(), "бэкапы не создавались");
    assert!(!package.with_extension("doc.rde.bak").exists(), ".bak не создавался");
    assert_eq!(load_rde_doc(&package).unwrap().inserts.iter().filter(|insert| insert.kind == "formula").count(), 0);
    let _ = std::fs::remove_dir_all(&root);
}

/// Ошибка перенумерации после вставки откатывает транзакцию: пакет не записан, вставки нет.
#[test]
fn renumber_failure_rolls_back_insert_transaction() {
    if !poppler_available() {
        return;
    }
    let root = tmp("renumber-rollback");
    write_project(&root);
    let package = package_with_body(&root, "<w:p><w:r><w:t>Текст документа</w:t></w:r></w:p>");
    let before = std::fs::read(&package).unwrap();
    let body_v = compiled_body("\\sigma=\\frac{F}{A}", true, "act-rollback", true);
    let error = add_formula_insert_with_renumber_error(
        &package,
        FormulaInsertRequest {
            body: body_v,
            render_mode: "omml".to_owned(),
            anchor: InsertAnchor::end(),
            numbering: FormulaNumbering::Continuous,
        },
    )
    .expect_err("инъекция ошибки перенумерации");
    assert!(error.contains("инъекция"), "{error}");
    assert_eq!(std::fs::read(&package).unwrap(), before, "пакет откатан (не записан)");
    assert!(!root.join(".rde/doc-backups").exists(), "бэкапы не создавались");
    assert!(!package.with_extension("doc.rde.bak").exists(), ".bak не создавался");
    assert!(load_rde_doc(&package).unwrap().inserts.is_empty(), "вставка не зарегистрирована");
    let _ = std::fs::remove_dir_all(&root);
}

/// Display-вставка с якорем по `paraId`: block-sdt после абзаца-якоря (а не в конце документа),
/// номер назначен схемой; legacy-якорь сохраняет прежнюю семантику «после блока N».
#[test]
fn display_insert_uses_paragraph_anchor_and_keeps_numbering() {
    if !poppler_available() {
        return;
    }
    let root = tmp("display-anchor");
    write_project(&root);
    let package = package_with_body(
        &root,
        "<w:p><w:r><w:t>Первый</w:t></w:r></w:p>\
         <w:p><w:r><w:t>Второй — якорь</w:t></w:r></w:p>\
         <w:p><w:r><w:t>Третий</w:t></w:r></w:p>",
    );
    let ids = para_ids(&package);
    let anchor_id = ids[1].clone().unwrap();
    let body_v = compiled_body("\\alpha=\\frac{\\lambda}{d}", true, "act-display", true);
    let report = add_formula_insert(
        &package,
        FormulaInsertRequest {
            body: body_v.clone(),
            render_mode: "omml".to_owned(),
            anchor: InsertAnchor::paragraph(anchor_id.clone(), None, None),
            numbering: FormulaNumbering::Continuous,
        },
    )
    .expect("display-вставка");
    assert!(report.insert.number.is_some(), "номер назначен");
    assert_eq!(
        report.insert.number_text.clone().or_else(|| report.insert.number.map(|number| number.to_string())).as_deref(),
        Some("1"),
        "отображаемый номер — 1"
    );
    let xml = body(&package);
    let tag = format!("rde:formula:{}", report.insert.id);
    let at = xml.find(&tag).unwrap();
    let anchor_pos = xml.find(&format!("w14:paraId=\"{anchor_id}\"")).unwrap();
    let anchor_end = xml[anchor_pos..].find("</w:p>").unwrap() + anchor_pos;
    assert!(at > anchor_end, "формула после абзаца-якоря");
    assert!(at < xml.find("Третий").unwrap(), "формула не в конце документа");
    assert!(xml[at..].contains("<w:tabs>"), "right-numbered без таблицы");
    // Legacy-совместимость: «после блока 0» — между первым и вторым абзацами.
    let legacy = add_formula_insert(
        &package,
        FormulaInsertRequest {
            body: body_v,
            render_mode: "omml".to_owned(),
            anchor: InsertAnchor::legacy_block(0),
            numbering: FormulaNumbering::Continuous,
        },
    )
    .expect("legacy display-вставка");
    let xml = body(&package);
    let legacy_at = xml.find(&format!("rde:formula:{}", legacy.insert.id)).unwrap();
    assert!(legacy_at > xml.find("Первый").unwrap());
    assert!(legacy_at < xml.find("Второй — якорь").unwrap(), "legacy-якорь после первого блока");
    let _ = std::fs::remove_dir_all(&root);
}
