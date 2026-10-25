//! Привязать живые вставки демо-документа к артефактам (сценарий C1):
//! `cargo run -p rde-doc --example link-demo -- [каталог проекта]`
//!
//! Добавляет/обновляет вставку значения из `out/stats.json` (JSON pointer) и рисунок из
//! `out/plot.svg`, перенумеровывает подписи и фиксирует версию документа в store.

use std::path::PathBuf;

use rde_doc::{InsertRequest, add_figure_insert, add_insert, commit_document, load_rde_doc, update_document_package};

fn main() -> Result<(), String> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("RDE_PROJECT").map(PathBuf::from))
        .unwrap_or_else(rde_core::demo_project_dir);
    let project = rde_core::load_project(&root)?;
    let package = project.document_path().ok_or_else(|| "в проекте нет документа".to_owned())?;
    let document = project.document_object().ok_or_else(|| "нет объекта документа".to_owned())?;
    let doc_id = match &document.data {
        rde_core::ObjectData::Document(manifest) => manifest.id.clone(),
        _ => return Err("объект не документ".to_owned()),
    };
    let mut store = rde_store::Store::open(&root).map_err(|e| e.to_string())?;

    let stats = project
        .artifacts()
        .find(|artifact| artifact.title.contains("Статистики"))
        .ok_or_else(|| "в проекте нет артефакта статистик".to_owned())?;
    let plot = project
        .artifacts()
        .find(|artifact| artifact.title.contains("График"))
        .ok_or_else(|| "в проекте нет артефакта графика".to_owned())?;

    // 1) Вставка значения из JSON-выхода (среднее x1).
    let value_uri = format!("rde://artifact/{}/out/stats.json#/stats/x1/mean", stats.id);
    let (value, hash) = rde_dag::resolve_insert_text(&project, &store, &value_uri)?;
    let existing = load_rde_doc(&package)?;
    if let Some(insert) = existing.inserts.iter().find(|insert| insert.target == value_uri) {
        let mut values = std::collections::BTreeMap::new();
        values.insert(insert.id.clone(), value.clone());
        let mut hashes = std::collections::BTreeMap::new();
        hashes.insert(insert.id.clone(), hash.clone());
        rde_doc::update_insert_values(&package, &values, &hashes)?;
        println!("вставка значения обновлена: {} = {value}", insert.id);
    } else {
        let report = add_insert(
            &package,
            InsertRequest {
                kind: "value".to_owned(),
                target: value_uri.clone(),
                selector: "#/stats/x1/mean".to_owned(),
                value_kind: "number".to_owned(),
                cached: value.clone(),
                caption: "Среднее значение x1".to_owned(),
            },
        )?;
        println!("вставка значения добавлена: {} = {value}", report.insert.id);
    }

    // 2) Рисунок из артефакта (SVG).
    let figure_uri = format!("rde://artifact/{}/out/plot.svg", plot.id);
    let (path, ext, figure_hash) = rde_dag::resolve_insert_image(&project, &store, &figure_uri)?;
    let existing = load_rde_doc(&package)?;
    // Обновляем только «живую» figure-вставку (с content-control), а не авто-вставку из миграции.
    if let Some(insert) = existing
        .inserts
        .iter()
        .find(|insert| insert.target == figure_uri && insert.para_id.is_some() && insert.value_kind == "figure")
    {
        let bytes = std::fs::read(&path).map_err(|e| format!("прочитать рисунок: {e}"))?;
        let mut images = std::collections::BTreeMap::new();
        images.insert(insert.id.clone(), (ext, bytes));
        let mut hashes = std::collections::BTreeMap::new();
        hashes.insert(insert.id.clone(), figure_hash);
        rde_doc::update_figure_images(&package, &images, &hashes)?;
        println!("рисунок обновлён: {}", insert.id);
    } else {
        let report = add_figure_insert(&package, &path, figure_uri, "График статистик".to_owned(), figure_hash)?;
        println!("рисунок добавлен: {}", report.insert.id);
    }

    // 3) Перенумерация подписей + версия документа в store.
    let report = update_document_package(&package)?;
    let txid = commit_document(&root, &package, &doc_id, &mut store, "demo: live inserts")?;
    println!(
        "документ обновлён: подписей изменено {}, вставок {}, транзакция {}",
        report.changes.len(),
        report.rde_doc.as_ref().map(|doc| doc.inserts.len()).unwrap_or(0),
        &txid[..10.min(txid.len())]
    );
    Ok(())
}
