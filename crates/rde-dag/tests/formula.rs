//! Интеграционные тесты P2 (ADR-0005): project-level формулы — активация `act_key` с входами и
//! фрагментами, ledger состояний, статусы актуальности, кэши в CAS.

use std::path::PathBuf;

use rde_core::{load_project, ObjectData, Project};
use rde_dag::{
    commit_formula_update, formula_ledger, formula_statuses, resolve_formula, FormulaLiveStatus, FormulaRenderer,
    FormulaStaleReason, FormulaState,
};
use rde_store::Store;

const ART: &str = "01M5VJ1JG0HD99FDTGJ8YEPFZX"; // data-артефакт-источник
const CONSTS: &str = "01M5VJ1JG0T9NS5SEZX35HGNFY"; // константы
const F1: &str = "01M5VJ1JG039MMV97SZGY6V9EQ";
const F2: &str = "01M5VJ1JG0QQJM2S6R76FQCZ25";
const RENDERER: FormulaRenderer<'static> = FormulaRenderer { id: "test-renderer", version: "1" };

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-dag-formula-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &std::path::Path, text: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, text).unwrap();
}

/// Проект: data-артефакт (out/values.json), константы (X=2.5 литерал, Y из артефакта),
/// формулы F1 (вход-константа) и F2 (фрагмент F1).
fn fixture(dir: &std::path::Path, fragment_source: &str) {
    write(
        &dir.join("rde.toml"),
        "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"Формулы\"\n",
    );
    write(
        &dir.join(format!("objects/artifacts/{ART}/artifact.toml")),
        &format!(
            "schema = \"rde/artifact@1\"\nid = \"{ART}\"\ntitle = \"Данные\"\ntype = \"data\"\n\
             [[groups]]\nname = \"data\"\nmode = \"hashed\"\ninputs = [{{ file = \"inputs/data.csv\" }}]\n"
        ),
    );
    write(&dir.join(format!("objects/artifacts/{ART}/inputs/data.csv")), "x\n1\n");
    write(&dir.join(format!("objects/artifacts/{ART}/out/values.json")), "{\"y\": 7.5}");
    write(
        &dir.join(format!("objects/artifacts/{CONSTS}/artifact.toml")),
        &format!(
            "schema = \"rde/artifact@1\"\nid = \"{CONSTS}\"\ntitle = \"Константы\"\ntype = \"constants\"\n\
             [values.X]\nvalue = 2.5\nunit = \"м\"\n\n\
             [values.Y]\nvalue = 0.0\nunit = \"м\"\nsource = \"rde://artifact/{ART}/out/values.json#/y\"\n"
        ),
    );
    write(
        &dir.join(format!("objects/formulas/{F1}/formula.toml")),
        &format!(
            "schema = \"rde/formula@1\"\nid = \"{F1}\"\ntitle = \"F1\"\nlatex = \"x=\\\\rdeinput{{x}}\"\ndisplay = true\n\
             \n[numbering]\nmode = \"auto\"\nlabel = \"\"\n\
             \n[[inputs]]\nalias = \"x\"\nmode = \"hashed\"\nsource = \"rde://constants/{CONSTS}/X\"\nas = \"value\"\n"
        ),
    );
    write(
        &dir.join(format!("objects/formulas/{F2}/formula.toml")),
        &format!(
            "schema = \"rde/formula@1\"\nid = \"{F2}\"\ntitle = \"F2\"\nlatex = \"y=\\\\rdefragment{{f}}\"\ndisplay = true\n\
             \n[numbering]\nmode = \"none\"\nlabel = \"\"\n\
             \n[[fragments]]\nalias = \"f\"\nsource = \"{fragment_source}\"\n"
        ),
    );
}

fn formula_manifest(project: &Project, id: &str) -> rde_core::FormulaManifest {
    let object = project.formula_object(id).unwrap();
    let ObjectData::Formula(formula) = &object.data else { panic!("не формула") };
    formula.clone()
}

#[test]
fn formula_activation_inputs_fragments_and_act_key() {
    let dir = tmp("activation");
    fixture(&dir, &format!("rde://formula/{F1}"));
    let project = load_project(&dir).unwrap();
    assert!(project.issues.is_empty(), "issues: {:?}", project.issues);
    let store = Store::open(&dir).unwrap();
    let artifact_ledger = rde_dag::ledger(&store).unwrap();
    let mut formula_ledger = formula_ledger(&store).unwrap();
    assert!(formula_ledger.is_empty());

    // F1: вход-константа подставлен числом, act_key стабилен.
    let f1 = resolve_formula(&project, &artifact_ledger, &formula_ledger, F1, &RENDERER).unwrap();
    assert_eq!(f1.effective_latex, "x=2.5");
    assert_eq!(f1.resolved_latex_hash.len(), 64);
    let f1_again = resolve_formula(&project, &artifact_ledger, &formula_ledger, F1, &RENDERER).unwrap();
    assert_eq!(f1.act_key, f1_again.act_key, "act_key стабилен при одинаковых входах");

    // F2: фрагмент без ledger-состояния не разрешается; с закреплённым act_key — разрешается.
    assert!(
        resolve_formula(&project, &artifact_ledger, &formula_ledger, F2, &RENDERER).is_err(),
        "фрагмент без act_key и без ledger-состояния — ошибка"
    );
    formula_ledger.insert(
        F1.to_owned(),
        FormulaState {
            formula_id: F1.to_owned(),
            act_key: f1.act_key.hex(),
            in_hash: None,
            src_hash: None,
            params_hash: None,
            env_hash: None,
            resolved_latex: f1.effective_latex.clone(),
            resolved_latex_hash: None,
            omml_hash: None,
            svg_hash: None,
            png_hash: None,
            updated_unix: 0,
        },
    );
    let f2 = resolve_formula(&project, &artifact_ledger, &formula_ledger, F2, &RENDERER).unwrap();
    assert!(f2.effective_latex.contains("\\left(x=2.5\\right)"), "{}", f2.effective_latex);

    // Пересчёт входа (константа X) меняет act_key F1 — каскад «вход → формула».
    let constants_manifest = dir.join(format!("objects/artifacts/{CONSTS}/artifact.toml"));
    let text = std::fs::read_to_string(&constants_manifest)
        .unwrap()
        .replace("value = 2.5", "value = 3.5");
    std::fs::write(&constants_manifest, text).unwrap();
    let project = load_project(&dir).unwrap();
    let f1_changed = resolve_formula(&project, &artifact_ledger, &formula_ledger, F1, &RENDERER).unwrap();
    assert_ne!(f1.act_key, f1_changed.act_key, "изменение входа меняет act_key");
    assert_eq!(f1_changed.effective_latex, "x=3.5");

    // Литеральная константа Y со source: значение из артефакта (резолвится и без ledger-состояния).
    let y = rde_dag::resolve_constant(&project, &artifact_ledger, CONSTS, "Y").unwrap();
    assert_eq!(y.value, 7.5, "значение Y из out/values.json");
    assert!(y.source_digest.is_some());
    let f1_manifest = formula_manifest(&project, F1);
    assert_eq!(f1_manifest.id, F1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn formula_ledger_statuses_and_cache_roots() {
    let dir = tmp("statuses");
    fixture(&dir, &format!("rde://formula/{F1}"));
    let project = load_project(&dir).unwrap();
    let mut store = Store::open(&dir).unwrap();
    let artifact_ledger = rde_dag::ledger(&store).unwrap();
    let formula_ledger_before = formula_ledger(&store).unwrap();
    let f1 = resolve_formula(&project, &artifact_ledger, &formula_ledger_before, F1, &RENDERER).unwrap();

    // До вычисления: F1 — NeverComputed; F2 не разрешается (фрагмент без act_key и без
    // ledger-состояния) — честный NotComputable с причиной.
    let statuses = formula_statuses(&project, &store, &RENDERER).unwrap();
    assert_eq!(statuses[F1].status, FormulaLiveStatus::NeverComputed);
    match &statuses[F2].status {
        FormulaLiveStatus::NotComputable(reason) => assert!(reason.contains("ещё не вычислена"), "{reason}"),
        other => panic!("F2 до F1: ожидалось NotComputable, получено {other:?}"),
    }

    // Записываем кэши в CAS + состояние в ledger → Fresh.
    let omml = store.put_blob(b"<m:oMathPara><m:oMath/></m:oMathPara>").unwrap();
    let svg = store.put_blob(b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
    let png = store.put_blob(b"PNG").unwrap();
    let tree = store
        .put_tree(&[
            rde_hash::TreeEntry::new("formula.omml", 0o644, omml),
            rde_hash::TreeEntry::new("formula.svg", 0o644, svg),
            rde_hash::TreeEntry::new("formula.png", 0o644, png),
        ])
        .unwrap();
    let state = FormulaState {
        formula_id: F1.to_owned(),
        act_key: f1.act_key.hex(),
        in_hash: Some(f1.in_hash.hex()),
        src_hash: Some(f1.src_hash.hex()),
        params_hash: Some(f1.params_hash.hex()),
        env_hash: Some(f1.env_hash.hex()),
        resolved_latex: f1.effective_latex.clone(),
        resolved_latex_hash: Some(f1.resolved_latex_hash.clone()),
        omml_hash: Some(omml.hex()),
        svg_hash: Some(svg.hex()),
        png_hash: Some(png.hex()),
        updated_unix: 42,
    };
    let tx = commit_formula_update(&mut store, &state, Some(tree), "formula update: F1").unwrap();
    assert!(!tx.is_empty());
    let ledger = formula_ledger(&store).unwrap();
    assert_eq!(ledger[F1].act_key, f1.act_key.hex());
    assert_eq!(ledger[F1].updated_unix, 42);
    let roots = store.current_roots().unwrap();
    assert_eq!(roots.get(&rde_dag::formula_root_name(F1)), Some(tree));
    let statuses = formula_statuses(&project, &store, &RENDERER).unwrap();
    assert_eq!(statuses[F1].status, FormulaLiveStatus::Fresh);
    assert_eq!(statuses[F2].status, FormulaLiveStatus::NeverComputed);

    // Изменение входа → Stale(InputsChanged) у F1; смена env рендерера → EnvChanged.
    let constants_manifest = dir.join(format!("objects/artifacts/{CONSTS}/artifact.toml"));
    let text = std::fs::read_to_string(&constants_manifest)
        .unwrap()
        .replace("value = 2.5", "value = 4.5");
    std::fs::write(&constants_manifest, text).unwrap();
    let project = load_project(&dir).unwrap();
    let statuses = formula_statuses(&project, &store, &RENDERER).unwrap();
    match &statuses[F1].status {
        FormulaLiveStatus::Stale(reasons) => assert!(reasons.contains(&FormulaStaleReason::InputsChanged), "{reasons:?}"),
        other => panic!("ожидалось Stale, получено {other:?}"),
    }
    let other_renderer = FormulaRenderer { id: "test-renderer", version: "2" };
    let statuses = formula_statuses(&project, &store, &other_renderer).unwrap();
    match &statuses[F1].status {
        FormulaLiveStatus::Stale(reasons) => assert!(reasons.contains(&FormulaStaleReason::EnvChanged), "{reasons:?}"),
        other => panic!("ожидалось Stale/EnvChanged, получено {other:?}"),
    }
    let _ = std::fs::remove_dir_all(&dir);
}
