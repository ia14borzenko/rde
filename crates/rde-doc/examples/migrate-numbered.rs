//! Headless-миграция формул импортированного документа (ADR-0019) — тот же путь, что у кнопки
//! «Принять формулы как объекты» в GUI: OMML вне реестра → project-level формулы + вставки-ссылки
//! с сохранением/назначением номеров, затем тела и перекрёстные ссылки.
//!
//! ```bash
//! cargo run -p rde-doc --example migrate-numbered -- <проект> [--section] [--renumber] [--no-numbers] [--export out.docx]
//! cargo run -p rde-doc --example migrate-numbered -- <проект> --export-only --export imported.docx
//! cargo run -p rde-doc --example migrate-numbered -- <проект> --renumber-only [--section]
//! ```
//!
//! Коды/ключи: `--section` — схема «по разделам» вместо сквозной; `--renumber` — не сохранять
//! номера Word, а перенумеровать по схеме; `--no-numbers` — не нумеровать display-формулы;
//! `--export <файл>` — выгрузить `.docx` после (или вместо) миграции с math-шрифтом (ADR-0018).

use std::collections::BTreeMap;
use std::path::PathBuf;

use rde_dag::FormulaRenderer;
use rde_store::Store;

fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let root = match args.next() {
        Some(path) => PathBuf::from(path),
        None => std::env::var_os("RDE_PROJECT").map(PathBuf::from).unwrap_or_else(rde_core::demo_project_dir),
    };
    let flags: Vec<String> = args.collect();
    let section = flags.iter().any(|flag| flag == "--section");
    let renumber = flags.iter().any(|flag| flag == "--renumber");
    let no_numbers = flags.iter().any(|flag| flag == "--no-numbers");
    let export = flags.windows(2).find(|pair| pair[0] == "--export").map(|pair| PathBuf::from(&pair[1]));
    println!("проект: {}", root.display());
    let mut store = Store::open(&root).map_err(|error| error.to_string())?;
    let project = rde_core::load_project(&root)?;
    let package = project.document_path().ok_or("в проекте нет документа")?;
    if !rde_doc::is_rde_package(&package) {
        return Err(format!("документ {} не в .doc.rde", package.display()));
    }
    let doc_id = project
        .document_object()
        .and_then(|object| match &object.data {
            rde_core::ObjectData::Document(manifest) => Some(manifest.id.clone()),
            _ => None,
        })
        .ok_or("нет объекта документа")?;
    let numbering = if section { rde_doc::FormulaNumbering::Section } else { rde_doc::FormulaNumbering::Continuous };
    if flags.iter().any(|flag| flag == "--export-only") {
        let out = export.clone().ok_or("--export-only требует --export <файл>")?;
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        rde_doc::export_docx(&package, &out)?;
        println!("экспорт .docx: {}", out.display());
        return Ok(());
    }
    if flags.iter().any(|flag| flag == "--renumber-only") {
        let report = rde_doc::renumber_formula_numbers(&package, numbering)?;
        println!("перенумерация ({numbering}): формул {}, ссылок {}, предупреждений {}", report.formulas.len(), report.crossrefs.len(), report.warnings.len());
        for warning in &report.warnings {
            println!("  предупреждение: {warning}");
        }
        rde_doc::commit_document(&root, &package, &doc_id, &mut store, "formula renumber")?;
        let rde_doc = rde_doc::load_rde_doc(&package)?;
        let mut numbers: Vec<(u32, String)> = rde_doc
            .inserts
            .iter()
            .filter(|insert| insert.kind == "formula")
            .filter_map(|insert| insert.number.map(|number| (number, insert.number_text.clone().unwrap_or_default())))
            .collect();
        numbers.sort();
        let texts: Vec<String> = numbers.iter().map(|(_, text)| format!("({text})")).collect();
        println!("номера после перенумерации: {} … {} (всего {})", texts.first().cloned().unwrap_or_default(), texts.last().cloned().unwrap_or_default(), texts.len());
        println!("первые: {:?}", texts.iter().take(8).collect::<Vec<_>>());
        return Ok(());
    }
    let renderer = FormulaRenderer { id: rde_doc::FORMULA_RENDERER_ID, version: rde_doc::FORMULA_RENDERER_VERSION };

    // 1) Классификация до миграции.
    let equations = rde_doc::unregistered_formulas(&package)?;
    let display = equations.iter().filter(|equation| equation.display).count();
    let found = equations.iter().filter(|equation| equation.display && equation.number_text.is_some()).count();
    let mixed = equations.iter().filter(|equation| equation.display && equation.mixed).count();
    let auto = equations.iter().filter(|equation| equation.auto_number).count();
    println!(
        "формул вне реестра: {} (display {display}, inline {}, номеров найдено {found}, смешанных {mixed}, автономеров {auto})",
        equations.len(),
        equations.len() - display,
    );
    let numbers: Vec<String> = equations.iter().filter_map(|equation| equation.number_text.clone()).collect();
    println!("номера из Word: {numbers:?}");

    // 2) Formula-объекты + кэши (как UI-команда).
    let mut plans = Vec::new();
    let mut bodies: BTreeMap<String, rde_doc::FormulaBody> = BTreeMap::new();
    let mut created = 0usize;
    for equation in &equations {
        let project = rde_core::load_project(&root)?;
        let existing = project.formulas().find(|formula| formula.latex.trim() == equation.latex.trim()).map(|formula| formula.id.clone());
        let formula_id = match existing {
            Some(id) => id,
            None => {
                let title: String = equation.latex.chars().take(40).collect();
                let dir = rde_core::create_formula_with_display(&root, &title, &equation.latex, equation.display)?;
                created += 1;
                dir.file_name().and_then(|name| name.to_str()).map(str::to_owned).unwrap_or_default()
            }
        };
        let project = rde_core::load_project(&root)?;
        let prepared = rde_dag::prepare_formula_update(&project, &mut store, &formula_id, &renderer, |latex, display| {
            let caches = rde_doc::compile_formula(latex, display)?;
            Ok(rde_dag::FormulaCompiled { omml: caches.omml, svg: caches.svg, png: caches.png })
        })?;
        rde_dag::commit_formula_update(&mut store, &prepared.state, prepared.cache_tree, &format!("formula migrate: {formula_id}"))?;
        let _ = rde_dag::record_prepared_formula_state(&project, &prepared);
        let hash = prepared.state.omml_hash.as_deref().unwrap_or_default();
        let omml = rde_hash::Hash::from_hex(hash).ok().and_then(|hash| store.get_blob(hash).ok().flatten()).unwrap_or_default();
        // Тело для «Обновить документ» — из уже посчитанного состояния формулы.
        bodies.insert(
            formula_id.clone(),
            rde_doc::FormulaBody {
                formula_id: formula_id.clone(),
                act_key: prepared.state.act_key.clone(),
                latex: prepared.state.resolved_latex.clone(),
                display: equation.display,
                numbered: equation.display,
                omml: load_hash(&store, &prepared.state.omml_hash)?,
                svg: load_hash_opt(&store, &prepared.state.svg_hash)?,
                png: load_hash_opt(&store, &prepared.state.png_hash)?,
            },
        );
        plans.push(rde_doc::FormulaMigrationPlan {
            formula_id,
            act_key: prepared.state.act_key.clone(),
            latex: equation.latex.clone(),
            omml,
        });
    }
    println!("объектов формул создано: {created}");

    // 3) Миграция.
    let options = rde_doc::FormulaMigrationOptions {
        numbering,
        preserve_word_numbers: !renumber,
        number_display: !no_numbers,
    };
    let report = rde_doc::migrate_formulas_with(&package, &plans, options)?;
    println!(
        "миграция: вставок {} (display {}, inline {}), таблиц {}, абзацев {}, inline-замен {}, номеров сохранено {}, назначено {}, схема {}",
        report.insert_ids.len(),
        report.display,
        report.inline,
        report.replaced_tables,
        report.replaced_blocks,
        report.replaced_inline,
        report.numbers_preserved,
        report.numbers_assigned,
        report.numbering
    );
    for warning in &report.warnings {
        println!("  предупреждение: {warning}");
    }

    // 4) Сразу тела + ссылки (как UI: без отдельного «Обновить документ»).
    let update = rde_doc::update_formula_inserts_with(&package, &bodies, numbering)?;
    println!(
        "обновление тел: обновлено {}, конфликтов {}, отсутствует {}, предупреждений {}",
        update.updated.len(),
        update.conflicts.len(),
        update.missing.len(),
        update.warnings.len()
    );
    for warning in &update.warnings {
        println!("  предупреждение нумерации: {warning}");
    }
    let refresh = rde_doc::refresh_formula_crossrefs(&package)?;
    println!("ссылки: обновлено {}", refresh.crossrefs.len());
    rde_doc::commit_document(&root, &package, &doc_id, &mut store, "formula migrate")?;

    // 5) Итоги по пакету.
    let extract_dir = std::env::temp_dir().join(format!("rde-migrate-numbered-{}", std::process::id()));
    let _ = std::fs::remove_file(&extract_dir);
    rde_doc::extract_docx(&package, &extract_dir)?;
    let xml = {
        use std::io::Read;
        let file = std::fs::File::open(&extract_dir).map_err(|error| error.to_string())?;
        let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
        let mut entry = archive.by_name("word/document.xml").map_err(|error| error.to_string())?;
        let mut bytes = Vec::new();
        entry.read_to_end(&mut bytes).map_err(|error| error.to_string())?;
        String::from_utf8_lossy(&bytes).to_string()
    };
    let _ = std::fs::remove_file(&extract_dir);
    let rde_doc = rde_doc::load_rde_doc(&package)?;
    let formulas: Vec<_> = rde_doc.inserts.iter().filter(|insert| insert.kind == "formula").collect();
    let numbered: Vec<_> = formulas.iter().filter(|insert| insert.number.is_some()).collect();
    let mut number_texts: Vec<String> = numbered
        .iter()
        .map(|insert| insert.number_text.clone().or_else(|| insert.number.map(|number| number.to_string())).unwrap_or_default())
        .collect();
    number_texts.sort_by_key(|text| text.parse::<u32>().unwrap_or(0));
    println!(
        "итог: формульных вставок {}, нумерованных {}, номеров {:?}; пунктуация после формул: {}; суффиксы {:?}",
        formulas.len(),
        numbered.len(),
        number_texts.first().zip(number_texts.last()).map(|(first, last)| format!("{first}…{last}")),
        numbered.iter().filter(|insert| !insert.suffix.is_empty()).count(),
        numbered.iter().filter(|insert| !insert.suffix.is_empty()).map(|insert| insert.suffix.clone()).collect::<Vec<_>>()
    );
    println!(
        "document.xml: sdt формул {}, нумерованных таб-абзацев {}, w:tbl {}, oMathPara {}, (N) в w:t {}",
        xml.matches("w:tag w:val=\"rde:formula:").count(),
        xml.matches("<w:tabs>").count(),
        xml.matches("<w:tbl>").count(),
        xml.matches("<m:oMathPara").count(),
        xml.matches("<w:t xml:space=\"preserve\">(").count()
    );
    let numbering_sources = numbered.iter().filter(|insert| insert.number_source == "word").count();
    println!("источник номера: word {numbering_sources} / авто {}", numbered.len() - numbering_sources);

    if let Some(out) = export {
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        rde_doc::export_docx(&package, &out)?;
        println!("экспорт .docx: {}", out.display());
    }
    Ok(())
}

fn load_hash(store: &Store, hash: &Option<String>) -> Result<Vec<u8>, String> {
    let hash = hash.as_deref().ok_or("нет кэша формулы")?;
    let hash = rde_hash::Hash::from_hex(hash).map_err(|error| format!("хеш кэша: {error}"))?;
    store.get_blob(hash).map_err(|error| error.to_string())?.ok_or_else(|| "кэш отсутствует в CAS".to_owned())
}

fn load_hash_opt(store: &Store, hash: &Option<String>) -> Result<Option<Vec<u8>>, String> {
    match hash.as_deref() {
        Some(_) => load_hash(store, hash).map(Some),
        None => Ok(None),
    }
}
