//! Привязать живые вставки демо-документа к артефактам (сценарий C1):
//! `cargo run -p rde-doc --example link-demo -- [каталог проекта]`
//!
//! Показывает все основные виды вставок R-DOC-11 на демо-проекте: значение (два формата),
//! рисунок (гистограмма), второй рисунок (линия), таблица из CSV-выхода, константа с
//! rich-форматированием (`R_н`), перекрёстная ссылка, цитата и дата. Затем — формула
//! (R-DOC-21/22, OMML-фундамент: display-уравнение с номером справа в скобках), перенумерация
//! подписей и фиксация версии документа в store.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rde_doc::{
    EquationRequest, InsertRequest, add_equation, add_figure_insert, add_insert, add_table_insert, commit_document, list_equations,
    load_rde_doc, renumber_equations,
};

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

    // Очистить авто-вставки миграции (i1/i2): тело документа не трогаем, но в rde:doc они не
    // должны участвовать в нумерации/обновлении — это исторические блоки из WordCraft.
    let legacy: Vec<String> = rde_doc::load_rde_doc(&package)?
        .inserts
        .iter()
        .filter(|insert| insert.id.starts_with('i'))
        .map(|insert| insert.id.clone())
        .collect();
    if !legacy.is_empty() {
        rde_doc::drop_insert_records(&package, &legacy)?;
        println!("очищены авто-вставки миграции: {}", legacy.join(", "));
    }

    let stats = find_artifact(&project, "Статистики")?;
    let plot = find_artifact(&project, "График")?;
    let line = find_artifact(&project, "линия")?;
    let constants = find_artifact(&project, "Константы")?;

    // 1) Значение из JSON-выхода: «стандартный вид» (sci:3).
    let mean_uri = format!("rde://artifact/{}/out/stats.json#/stats/x1/mean", stats.id);
    let mean_id = upsert_value(&project, &store, &package, &mean_uri, "#/stats/x1/mean", "sci:3", "Среднее значение x1", "")?;
    println!("значение x1 (sci:3): {mean_id}");

    // 2) Второе значение — десятичная дробь с ограничением знаков (dec:2).
    let stdev_uri = format!("rde://artifact/{}/out/stats.json#/stats/x1/stdev", stats.id);
    let stdev_id = upsert_value(&project, &store, &package, &stdev_uri, "#/stats/x1/stdev", "dec:2", "Стандартное отклонение x1", "")?;
    println!("значение σ (dec:2): {stdev_id}");

    // 3) Рисунок-гистограмма из SVG-выхода артефакта.
    let figure_uri = format!("rde://artifact/{}/out/plot.svg", plot.id);
    let figure_id = upsert_figure(&project, &store, &package, &figure_uri, "Статистики реализаций")?;
    println!("рисунок-гистограмма: {figure_id}");

    // 4) Второй рисунок — линейный график реализаций.
    let line_uri = format!("rde://artifact/{}/out/line.svg", line.id);
    let line_id = upsert_figure(&project, &store, &package, &line_uri, "Реализации случайного процесса")?;
    println!("рисунок-линия: {line_id}");

    // 5) Таблица из CSV-выхода артефакта (R-DOC-18).
    let table_uri = format!("rde://artifact/{}/out/summary.csv", stats.id);
    let table_id = upsert_table(&project, &store, &package, &table_uri, "Сводные статистики реализаций")?;
    println!("таблица: {table_id}");

    // 6) Константа с rich-форматированием (R_н: курсив + подстрочный индекс, R-ART-09).
    let constant_uri = format!("rde://constants/{}/R_n", constants.id);
    let (constant_text, constant_style, _) = rde_dag::resolve_insert_constant(&project, &constant_uri)?;
    let constant_id = upsert_text(&package, "constant", &constant_uri, &constant_text, "Удельное сопротивление", &constant_style)?;
    println!("константа R_н: {constant_id}");

    // 7) Перекрёстная ссылка на рисунок (номер подставит перенумерация).
    let crossref_uri = format!("rde://insert/{figure_id}");
    let crossref_id = upsert_text(&package, "crossref", &crossref_uri, "Рисунок 1", "см. рисунок", "")?;
    println!("перекрёстная ссылка: {crossref_id}");

    // 8) Цитата на источник (номер — по порядку источников проекта).
    let sources: Vec<&rde_core::Object> = project.sources().collect();
    let source = sources
        .iter()
        .copied()
        .find(|source| source.title.contains("Cumming"))
        .ok_or_else(|| "в проекте нет источника Cumming & Wong".to_owned())?;
    let citation_uri = format!("rde://source/{}", source.id);
    let citation_index = sources.iter().position(|candidate| candidate.id == source.id).unwrap_or(0) + 1;
    let citation_id = upsert_text(&package, "citation", &citation_uri, &format!("[{citation_index}]"), "цитата", "")?;
    println!("цитата: {citation_id}");

    // 9) Дата/поле (кэш даты; Word-поле — v0.2).
    let date_uri = "rde://date/iso".to_owned();
    let date_id = upsert_text(&package, "date", &date_uri, &today_iso(), "дата", "")?;
    println!("дата: {date_id}");

    // 10) Формула (R-DOC-21/22, фаза 1): OMML display-уравнение + номер (1) справа в скобках.
    //     Вставка идемпотентна: если в документе уже есть формулы, повторно не добавляем.
    if list_equations(&package)?.is_empty() {
        let anchor = top_level_anchor(&package, "График построен по выходным данным")?;
        let formula = add_equation(
            &package,
            EquationRequest {
                formula: "\\sigma = \\sqrt{\\frac{1}{N-1}\\sum_{i=1}^{N}(x_i-\\bar{x})^2}".to_owned(),
                number: Some(1),
                display: true,
                anchor_block: Some(anchor),
            },
        )?;
        println!("формула: {} (display, номер {:?}, блок {}, LaTeX {})", formula.id, formula.number, formula.block, formula.latex);
    } else {
        let existing = list_equations(&package)?;
        println!("формулы уже есть: {} ({:?})", existing.len(), existing.iter().map(|equation| equation.block.clone()).collect::<Vec<_>>());
    }
    let renumbered = renumber_equations(&package)?;
    if !renumbered.is_empty() {
        println!("формулы перенумерованы: {renumbered:?}");
    }

    // 11) Перенумерация подписей/перекрёстных ссылок + версия документа в store.
    //     Стиль значений — «обычный текст документа» (без случайных pStyle после round-trip).
    let live = rde_doc::load_rde_doc(&package)?.inserts;
    for insert in live.iter().filter(|insert| matches!(insert.kind.as_str(), "value" | "crossref" | "citation" | "date")) {
        rde_doc::apply_insert_style(&package, &insert.id, "")?;
    }
    rde_doc::renumber_insert_captions(&package)?;
    let changes = rde_doc::update_document_package(&package)?.changes.len();
    let txid = commit_document(&root, &package, &doc_id, &mut store, "demo: all insert types")?;
    let inserts = load_rde_doc(&package)?.inserts;
    println!(
        "документ обновлён: подписей изменено {}, вставок {}, транзакция {}",
        changes,
        inserts.len(),
        &txid[..10.min(txid.len())]
    );
    for insert in &inserts {
        println!("  {} {:10} {:>28} = {}", insert.id, insert.kind, insert.caption, insert.cached);
    }
    Ok(())
}

/// Индекс верхнеуровневого блока тела, текст которого содержит `needle` (для `anchor_block`).
fn top_level_anchor(package: &Path, needle: &str) -> Result<usize, String> {
    let (document, _) = rde_doc::open_document(package)?;
    let blocks = rde_doc::extract_blocks(&document);
    let block = blocks.iter().find(|block| block.text.contains(needle)).ok_or_else(|| format!("в документе нет блока «{needle}»"))?;
    block
        .path
        .split('/')
        .next()
        .and_then(|index| index.parse::<usize>().ok())
        .ok_or_else(|| format!("путь блока без индекса тела: {}", block.path))
}

fn find_artifact(project: &rde_core::Project, needle: &str) -> Result<rde_core::Object, String> {
    project
        .objects
        .iter()
        .find(|object| matches!(&object.data, rde_core::ObjectData::Artifact(artifact) if artifact.title.contains(needle)))
        .cloned()
        .ok_or_else(|| format!("в проекте нет артефакта «{needle}»"))
}

fn find_insert<'a>(package: &Path, target: &str) -> Result<Option<rde_doc::RdeInsert>, String> {
    Ok(load_rde_doc(package)?.inserts.into_iter().find(|insert| insert.target == target))
}

/// Вставка/обновление значения выхода артефакта.
fn upsert_value(
    project: &rde_core::Project,
    store: &rde_store::Store,
    package: &Path,
    uri: &str,
    pointer: &str,
    format: &str,
    caption: &str,
    run_style: &str,
) -> Result<String, String> {
    let (value, hash) = rde_dag::resolve_insert_text(project, store, uri)?;
    let formatted = rde_doc::format_value(&value, format);
    if let Some(insert) = find_insert(package, uri)? {
        rde_doc::set_insert_format(package, &insert.id, format)?;
        let mut values = BTreeMap::new();
        values.insert(insert.id.clone(), formatted);
        let mut hashes = BTreeMap::new();
        hashes.insert(insert.id.clone(), hash);
        rde_doc::update_insert_values(package, &values, &hashes)?;
        return Ok(insert.id);
    }
    let report = add_insert(
        package,
        InsertRequest {
            kind: "value".to_owned(),
            target: uri.to_owned(),
            selector: pointer.to_owned(),
            value_kind: "number".to_owned(),
            cached: formatted,
            caption: caption.to_owned(),
            style: String::new(),
            format: format.to_owned(),
            run_style: run_style.to_owned(),
            anchor_block: None,
        },
    )?;
    Ok(report.insert.id)
}

/// Вставка/обновление рисунка из выхода артефакта.
fn upsert_figure(project: &rde_core::Project, store: &rde_store::Store, package: &Path, uri: &str, caption: &str) -> Result<String, String> {
    let (path, ext, hash) = rde_dag::resolve_insert_image(project, store, uri)?;
    if let Some(insert) = find_insert(package, uri)? {
        let bytes = std::fs::read(&path).map_err(|e| format!("прочитать рисунок: {e}"))?;
        let mut images = BTreeMap::new();
        images.insert(insert.id.clone(), (ext, bytes));
        let mut hashes = BTreeMap::new();
        hashes.insert(insert.id.clone(), hash);
        rde_doc::update_figure_images(package, &images, &hashes)?;
        return Ok(insert.id);
    }
    let report = add_figure_insert(package, &path, uri.to_owned(), caption.to_owned(), hash, None)?;
    Ok(report.insert.id)
}

/// Вставка/обновление таблицы из CSV-выхода артефакта.
fn upsert_table(project: &rde_core::Project, store: &rde_store::Store, package: &Path, uri: &str, caption: &str) -> Result<String, String> {
    let (rows, hash) = rde_dag::resolve_insert_table(project, store, uri)?;
    if let Some(insert) = find_insert(package, uri)? {
        let mut tables = BTreeMap::new();
        tables.insert(insert.id.clone(), rows);
        rde_doc::update_table_inserts(package, &tables)?;
        return Ok(insert.id);
    }
    let report = add_table_insert(
        package,
        InsertRequest {
            kind: "table".to_owned(),
            target: uri.to_owned(),
            selector: String::new(),
            value_kind: "table".to_owned(),
            cached: String::new(),
            caption: caption.to_owned(),
            style: String::new(),
            format: String::new(),
            run_style: String::new(),
            anchor_block: None,
        },
        &rows,
    )?;
    let _ = hash;
    Ok(report.insert.id)
}

/// Вставка/обновление текстовой вставки (константа/ссылка/цитата/дата).
fn upsert_text(package: &Path, kind: &str, target: &str, cached: &str, caption: &str, run_style: &str) -> Result<String, String> {
    if let Some(insert) = find_insert(package, target)? {
        let mut values = BTreeMap::new();
        values.insert(insert.id.clone(), cached.to_owned());
        rde_doc::update_insert_values(package, &values, &BTreeMap::new())?;
        return Ok(insert.id);
    }
    let report = add_insert(
        package,
        InsertRequest {
            kind: kind.to_owned(),
            target: target.to_owned(),
            selector: String::new(),
            value_kind: "text".to_owned(),
            cached: cached.to_owned(),
            caption: caption.to_owned(),
            style: String::new(),
            format: "raw".to_owned(),
            run_style: run_style.to_owned(),
            anchor_block: None,
        },
    )?;
    Ok(report.insert.id)
}

/// Сегодняшняя дата ISO (`YYYY-MM-DD`) без внешних зависимостей.
fn today_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    // Гражданская дата из дней от 1970-01-01 (алгоритм Ховарда Хиннанта).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}
