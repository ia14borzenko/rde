//! P2 (ADR-0005): сквозной пайплайн project-level формулы — `rde-dag::resolve_formula` →
//! `rde-doc::compile_formula` (OMML/SVG/PNG движком+poppler) → CAS/ledger (`formula-update`) →
//! `[state]` в `formula.toml` → статус «актуальна».

use std::path::PathBuf;

use rde_core::{load_project, ObjectData};
use rde_dag::{compile_and_update_formula, formula_ledger, formula_statuses, FormulaCompiled, FormulaLiveStatus, FormulaRenderer};
use rde_store::Store;

const F1: &str = "01M5VJ1JG039MMV97SZGY6V9EQ";
const RENDERER: FormulaRenderer<'static> =
    FormulaRenderer { id: rde_doc::FORMULA_RENDERER_ID, version: rde_doc::FORMULA_RENDERER_VERSION };

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-doc-formula-pipeline-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn poppler_available() -> bool {
    std::process::Command::new("pdftocairo")
        .arg("-v")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

#[test]
fn project_formula_compiles_caches_and_becomes_fresh() {
    if !poppler_available() {
        return; // без poppler SVG-пайплайн недоступен (честный skip, как другие CLI-зависимые тесты)
    }
    let dir = tmp("pipeline");
    std::fs::write(
        dir.join("rde.toml"),
        "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"Формулы\"\n",
    )
    .unwrap();
    let formula_dir = dir.join("objects/formulas").join(F1);
    std::fs::create_dir_all(&formula_dir).unwrap();
    std::fs::write(
        formula_dir.join("formula.toml"),
        format!(
            "schema = \"rde/formula@1\"\nid = \"{F1}\"\ntitle = \"Закон Ома\"\n\
             latex = \"\\\\sigma=\\\\frac{{F}}{{A}}\"\ndisplay = true\n\n[numbering]\nmode = \"auto\"\nlabel = \"\"\n"
        ),
    )
    .unwrap();
    let project = load_project(&dir).unwrap();
    assert!(project.issues.is_empty(), "issues: {:?}", project.issues);
    let mut store = Store::open(&dir).unwrap();

    let report = compile_and_update_formula(&project, &mut store, F1, &RENDERER, |latex, display| {
        let caches = rde_doc::compile_formula(latex, display)?;
        Ok(FormulaCompiled { omml: caches.omml, svg: caches.svg, png: caches.png })
    })
    .expect("компиляция и фиксация формулы");
    assert_eq!(report.act_key.len(), 64);
    assert_eq!(report.omml_hash.len(), 64);
    assert!(report.svg_hash.as_deref().is_some_and(|hash| hash.len() == 64));
    assert!(report.png_hash.as_deref().is_some_and(|hash| hash.len() == 64));

    // Ledger и корень кэшей в store.
    let ledger = formula_ledger(&store).unwrap();
    assert_eq!(ledger[F1].act_key, report.act_key);
    let roots = store.current_roots().unwrap();
    assert!(roots.get(&rde_dag::formula_root_name(F1)).is_some());
    // Кэш-дерево читается из CAS и содержит OMML/SVG/PNG.
    let tree_hash = roots.get(&rde_dag::formula_root_name(F1)).unwrap();
    let tree = store.get_tree(tree_hash).unwrap().expect("дерево кэшей");
    let names: Vec<&str> = tree.iter().map(|entry| entry.path.as_str()).collect();
    for expected in ["formula.omml", "formula.svg", "formula.png"] {
        assert!(names.contains(&expected), "в кэш-дереве нет {expected}: {names:?}");
    }
    // OMML из кэша — display-уравнение с дробью.
    let omml_entry = tree.iter().find(|entry| entry.path == "formula.omml").unwrap();
    let omml = String::from_utf8_lossy(&store.get_blob(omml_entry.blob).unwrap().unwrap()).to_string();
    assert!(omml.contains("<m:oMathPara"), "{omml}");
    assert!(omml.contains("<m:f>"), "{omml}");

    // `[state]` в formula.toml (зеркало ledger) и статус «актуальна».
    let project = load_project(&dir).unwrap();
    let object = project.formula_object(F1).unwrap();
    let ObjectData::Formula(formula) = &object.data else { panic!("не формула") };
    assert_eq!(formula.state.act_key, report.act_key);
    assert_eq!(formula.state.omml_hash, report.omml_hash);
    assert!(formula.state.computed_at > 0);
    let statuses = formula_statuses(&project, &store, &RENDERER).unwrap();
    assert_eq!(statuses[F1].status, FormulaLiveStatus::Fresh, "{:?}", statuses[F1].status);

    // Повторная компиляция идемпотентна по act_key (входы/источник не менялись), кэши те же.
    let report2 = compile_and_update_formula(&project, &mut store, F1, &RENDERER, |latex, display| {
        let caches = rde_doc::compile_formula(latex, display)?;
        Ok(FormulaCompiled { omml: caches.omml, svg: caches.svg, png: caches.png })
    })
    .unwrap();
    assert_eq!(report2.act_key, report.act_key);
    assert_eq!(report2.omml_hash, report.omml_hash);
    assert_eq!(report2.svg_hash, report.svg_hash);
    let _ = std::fs::remove_dir_all(&dir);
}
