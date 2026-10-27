//! P3 (ADR-0005 §3): явная миграция формул демо/проекта — OMML вне реестра → project-level
//! формула + вставка-ссылка, плюс одна векторная (svg) вставка для демонстрации режима.
//!
//! ```bash
//! cargo run -p rde-doc --example migrate-formula -- [каталог-проекта]
//! ```
//!
//! Идемпотентно: уже зарегистрированные формулы повторно не мигрируются, svg-вставка не
//! дублируется; при открытии документа автоизменений нет — только эта явная команда.

use std::path::PathBuf;

use rde_doc::{
    FormulaBody, FormulaInsertRequest, FormulaMigrationPlan, add_formula_insert, commit_document, extract_docx, is_rde_package, load_rde_doc,
    migrate_formulas, open_document, save_engine_document, unregistered_formulas,
};
use rde_dag::{FormulaCompiled, FormulaLiveStatus, FormulaRenderer, PreparedFormulaUpdate};
use rde_store::Store;

fn main() -> Result<(), String> {
    let root = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("RDE_PROJECT").map(PathBuf::from))
        .unwrap_or_else(rde_core::demo_project_dir);
    println!("проект: {}", root.display());
    let mut store = Store::open(&root).map_err(|e| e.to_string())?;
    let renderer = FormulaRenderer { id: rde_doc::FORMULA_RENDERER_ID, version: rde_doc::FORMULA_RENDERER_VERSION };
    let package = {
        let project = rde_core::load_project(&root)?;
        project.document_path().ok_or("в проекте нет документа")?
    };
    if !is_rde_package(&package) {
        return Err(format!("документ {} ещё не в .doc.rde", package.display()));
    }
    let doc_id = {
        let project = rde_core::load_project(&root)?;
        document_id(&project)?
    };

    // 1) Формулы вне реестра → project-level объекты + вставки-ссылки (omml).
    let equations = unregistered_formulas(&package)?;
    if equations.is_empty() {
        println!("формул вне реестра нет — миграция не нужна");
    } else {
        let mut plans = Vec::new();
        for equation in &equations {
            let mut project = rde_core::load_project(&root)?;
            let existing = project
                .formulas()
                .find(|formula| formula.latex.trim() == equation.latex.trim())
                .map(|formula| formula.id.clone());
            let formula_id = match existing {
                Some(id) => id,
                None => {
                    let title: String = format!("Формула: {}", equation.latex.chars().take(32).collect::<String>());
                    let dir = rde_core::create_formula(&root, &title, &equation.latex)?;
                    project = rde_core::load_project(&root)?;
                    dir.file_name().and_then(|name| name.to_str()).map(str::to_owned).unwrap_or_default()
                }
            };
            let state = ensure_fresh_formula(&project, &mut store, &renderer, &formula_id)?;
            let omml = load_cache(&store, state.omml_hash.as_deref())?;
            plans.push(FormulaMigrationPlan {
                formula_id,
                act_key: state.act_key.clone(),
                latex: equation.latex.clone(),
                omml,
            });
        }
        let report = migrate_formulas(&package, &plans)?;
        commit_document(&root, &package, &doc_id, &mut store, "formula migrate")?;
        println!(
            "миграция: вставок {}, таблиц {}, абзацев {}, inline {}",
            report.insert_ids.len(),
            report.replaced_tables,
            report.replaced_blocks,
            report.replaced_inline
        );
    }

    // 2) Векторный режим (R-DOC-22 п.2): тот же документ получает svg-вставку формулы рядом
    //    с поясняющим абзацем (PNG-fallback + asvg:svgBlip; WordCraft сохраняет blip-extLst).
    let rde_doc = load_rde_doc(&package)?;
    if rde_doc.inserts.iter().any(|insert| insert.kind == "formula" && insert.render_mode == "svg") {
        println!("svg-вставка формулы уже есть");
    } else {
        let project = rde_core::load_project(&root)?;
        let formula_id = project.formulas().next().map(|formula| formula.id.clone()).ok_or("в проекте нет формул")?;
        let state = ensure_fresh_formula(&project, &mut store, &renderer, &formula_id)?;
        let body = FormulaBody {
            formula_id: formula_id.clone(),
            act_key: state.act_key.clone(),
            latex: state.resolved_latex.clone(),
            display: true,
            numbered: false,
            omml: load_cache(&store, state.omml_hash.as_deref())?,
            svg: load_optional_cache(&store, state.svg_hash.as_deref())?,
            png: load_optional_cache(&store, state.png_hash.as_deref())?,
        };
        append_paragraph(&package, "Тот же результат в векторном режиме (SVG + PNG-fallback, совместимость с Word 2016+):")?;
        let report = add_formula_insert(
            &package,
            FormulaInsertRequest { body, render_mode: "svg".to_owned(), anchor_block: None },
        )?;
        commit_document(&root, &package, &doc_id, &mut store, "formula svg insert")?;
        println!("svg-вставка формулы: {} (target {})", report.insert.id, report.insert.target);
    }

    // 3) Статусы вставок (только чтение): документ не перезаписывается, повторный запуск
    //    миграции не меняет `.doc.rde` (идемпотентность; явное обновление — кнопка в UI).
    let project = rde_core::load_project(&root)?;
    let statuses = rde_dag::formula_statuses(&project, &store, &renderer)?;
    let ledger = rde_dag::formula_ledger(&store)?;
    let mut availability = std::collections::BTreeMap::new();
    for formula in project.formulas() {
        match ledger.get(&formula.id) {
            Some(state) => {
                availability.insert(formula.id.clone(), rde_doc::FormulaAvailability::Current(state.act_key.clone()));
            }
            None => {
                availability.insert(formula.id.clone(), rde_doc::FormulaAvailability::NotComputed("нет кэшей".to_owned()));
            }
        }
    }
    let insert_statuses = rde_doc::formula_insert_statuses(&package, &availability)?;
    let inserted = insert_statuses.len();
    println!("документ: формульных вставок {inserted} (omml {}, svg {})", insert_statuses.iter().filter(|status| status.render_mode != "svg").count(), insert_statuses.iter().filter(|status| status.render_mode == "svg").count());
    for status in &insert_statuses {
        println!("  вставка {}: {} ({})", status.insert_id, status.state.label_ru(), status.detail);
    }
    for (id, status) in &statuses {
        println!("  формула {}: {}", &id[..8.min(id.len())], status.status.label_ru());
    }
    std::mem::drop(project);
    Ok(())
}

fn document_id(project: &rde_core::Project) -> Result<String, String> {
    let object = project.document_object().ok_or("в проекте нет документа")?;
    let rde_core::ObjectData::Document(manifest) = &object.data else { return Err("объект документа повреждён".to_owned()) };
    Ok(manifest.id.clone())
}

fn compile(latex: &str, display: bool) -> Result<FormulaCompiled, String> {
    let caches = rde_doc::compile_formula(latex, display)?;
    Ok(FormulaCompiled { omml: caches.omml, svg: caches.svg, png: caches.png })
}

/// Актуализировать кэши формулы (при устаревании/отсутствии) и вернуть состояние ledger.
fn ensure_fresh_formula(
    project: &rde_core::Project,
    store: &mut Store,
    renderer: &FormulaRenderer<'_>,
    formula_id: &str,
) -> Result<rde_dag::FormulaState, String> {
    let statuses = rde_dag::formula_statuses(project, store, renderer)?;
    let fresh = statuses.get(formula_id).map(|status| status.status == FormulaLiveStatus::Fresh).unwrap_or(false);
    if !fresh {
        let prepared: PreparedFormulaUpdate = rde_dag::prepare_formula_update(project, store, formula_id, renderer, compile)?;
        rde_dag::commit_formula_update(store, &prepared.state, prepared.cache_tree, &format!("formula migrate: {formula_id}"))?;
        rde_dag::record_prepared_formula_state(project, &prepared)?;
    }
    let ledger = rde_dag::formula_ledger(store)?;
    ledger.get(formula_id).cloned().ok_or_else(|| format!("формула {formula_id} не вычислена"))
}

fn load_cache(store: &Store, hash: Option<&str>) -> Result<Vec<u8>, String> {
    let hash = hash.ok_or("нет кэша формулы")?;
    let hash = rde_hash::Hash::from_hex(hash)?;
    store.get_blob(hash).map_err(|e| e.to_string())?.ok_or_else(|| "кэш формулы отсутствует в CAS".to_owned())
}

fn load_optional_cache(store: &Store, hash: Option<&str>) -> Result<Option<Vec<u8>>, String> {
    match hash {
        Some(hash) => load_cache(store, Some(hash)).map(Some),
        None => Ok(None),
    }
}

/// Дописать поясняющий абзац в конец документа (модель WordCraft → движок), сохранив rde:doc.
fn append_paragraph(package: &std::path::Path, text: &str) -> Result<(), String> {
    let work = std::env::temp_dir().join(format!("rde-formula-demo-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let clean = work.join("clean.docx");
    extract_docx(package, &clean)?;
    let (mut doc, _encrypted) = open_document(&clean)?;
    let paragraph = wordcraft_engine::doc::Paragraph::with_text(text, wordcraft_engine::doc::CharProps::default());
    doc.body.push(wordcraft_engine::doc::para_block(paragraph));
    wordcraft_engine::io::save_path(&clean, &doc).map_err(|e| format!("сохранить .docx: {e}"))?;
    save_engine_document(&clean, package, "", "")?;
    let _ = std::fs::remove_dir_all(&work);
    Ok(())
}
