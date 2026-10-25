//! Интеграционные тесты DAG: обновление демо-цепочки, статусы из хешей, отказ и ручная правка выходов.

use std::path::{Path, PathBuf};

use rde_core::{load_project, Project};
use rde_dag::{evaluate, stale_artifacts, topological_compute, update, LiveStatus, StaleReason, UpdateOptions};
use rde_store::Store;

const STATS: &str = "01M5VJ1JG039MMV97SZGY6V9EQ";
const REALIZATIONS: &str = "01M5VJ1JG0HD99FDTGJ8YEPFZX";
const PLOT: &str = "01M5VJ1JG0QQJM2S6R76FQCZ25";

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-dag-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_tree(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".rde" || name == "exports" || name == "__pycache__" || name == "out" {
            continue;
        }
        let path = entry.path();
        let target = dst.join(&*name);
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(&path, &target).unwrap();
        }
    }
}

fn demo_copy(name: &str) -> (PathBuf, Project) {
    let root = tmp(name);
    copy_tree(&rde_core::demo_project_dir(), &root);
    let project = load_project(&root).unwrap();
    (root, project)
}

fn python_available() -> bool {
    std::process::Command::new("python3").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn artifact_dir(project: &Project, id: &str) -> PathBuf {
    project.artifact_object(id).unwrap().dir.clone()
}

#[test]
fn update_runs_compute_chain_and_marks_fresh() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("chain");
    let mut store = Store::open(&root).unwrap();

    let before = evaluate(&project, &store).unwrap();
    assert_eq!(before[STATS].status, LiveStatus::NeverRun);
    assert_eq!(before[PLOT].status, LiveStatus::NeverRun);
    assert_eq!(before[REALIZATIONS].status, LiveStatus::Fresh, "data-артефакт считается актуальным");

    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.failed(), 0, "ошибки: {:?}", report.errors);
    assert_eq!(report.ran(), 2, "отчёт: {:?}", report.artifacts);
    assert_eq!(report.artifacts[0].artifact_id, STATS, "топопорядок: статистики раньше графика");
    assert_eq!(report.artifacts[1].artifact_id, PLOT);
    assert!(artifact_dir(&project, STATS).join("out/stats.json").is_file());
    assert!(artifact_dir(&project, PLOT).join("out/plot.svg").is_file());
    assert!(store.stats().unwrap().blobs >= 2);

    let project = load_project(&root).unwrap();
    let after = evaluate(&project, &store).unwrap();
    for (id, status) in &after {
        assert_eq!(status.status, LiveStatus::Fresh, "артефакт {id} после обновления: {:?}", status.status);
    }

    // Повторный запуск ничего не делает: act_key и выходы совпадают.
    let txs_before = store.stats().unwrap().txs;
    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.ran(), 0, "{:?}", report.artifacts);
    assert_eq!(report.skipped(), 2);
    assert_eq!(store.stats().unwrap().txs, txs_before, "не должно быть новых транзакций");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn input_change_marks_downstream_stale_and_recomputes() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("input-change");
    let mut store = Store::open(&root).unwrap();
    update(&project, &mut store, &UpdateOptions::default()).unwrap();
    let project = load_project(&root).unwrap();
    let act_before = evaluate(&project, &store).unwrap()[STATS].act_key.clone().unwrap();
    let stats_file = artifact_dir(&project, STATS).join("out/stats.json");
    let stats_before = std::fs::read(&stats_file).unwrap();

    // Меняем входные данные data-артефакта.
    let input = artifact_dir(&project, REALIZATIONS).join("inputs/experiment.csv");
    let mut text = std::fs::read_to_string(&input).unwrap();
    text.push_str("50,50.0,50.0\n");
    std::fs::write(&input, text).unwrap();

    let project = load_project(&root).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert!(matches!(statuses[STATS].status, LiveStatus::Stale(_)), "{:?}", statuses[STATS].status);
    assert!(stale_artifacts(&project, &store).unwrap().contains(&STATS.to_owned()));

    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.failed(), 0, "{:?}", report.errors);
    assert_eq!(report.ran(), 2, "stats и зависимый plot должны пересчитаться: {:?}", report.artifacts);

    let project = load_project(&root).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    for (id, status) in &statuses {
        assert_eq!(status.status, LiveStatus::Fresh, "{id}: {:?}", status.status);
    }
    assert_ne!(statuses[STATS].act_key.clone().unwrap(), act_before);
    assert_ne!(std::fs::read(&stats_file).unwrap(), stats_before, "выход статистик должен измениться");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn failing_script_is_reported_and_keeps_previous_state() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("fail");
    let mut store = Store::open(&root).unwrap();
    update(&project, &mut store, &UpdateOptions::default()).unwrap();
    let project = load_project(&root).unwrap();
    let script = artifact_dir(&project, STATS).join("script.py");
    let original = std::fs::read_to_string(&script).unwrap();
    std::fs::write(&script, "import sys\nprint('boom', file=sys.stderr)\nsys.exit(2)\n").unwrap();

    let project = load_project(&root).unwrap();
    let report = update(&project, &mut store, &UpdateOptions { continue_on_error: true, ..Default::default() }).unwrap();
    assert_eq!(report.failed(), 1, "{:?}", report.artifacts);
    assert!(report.errors.iter().any(|e| e.contains("Статистики")), "{:?}", report.errors);
    // ledger не испорчен: запись осталась от прошлого успешного запуска.
    let statuses = evaluate(&project, &store).unwrap();
    assert!(matches!(statuses[STATS].status, LiveStatus::Stale(_)));
    assert!(statuses[STATS].updated_unix.is_some(), "старая запись ledger должна сохраниться");

    // Возвращаем скрипт: входы не менялись — артефакт снова актуален без запуска.
    std::fs::write(&script, original).unwrap();
    let project = load_project(&root).unwrap();
    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.failed(), 0);
    assert_eq!(report.ran(), 0, "восстановленный скрипт совпадает с ledger: {:?}", report.artifacts);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn manual_output_change_is_detected_and_regenerated() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("manual-out");
    let mut store = Store::open(&root).unwrap();
    update(&project, &mut store, &UpdateOptions::default()).unwrap();
    let stats = artifact_dir(&project, STATS).join("out/stats.json");
    std::fs::write(&stats, b"{\"broken\": true}").unwrap();

    let project = load_project(&root).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert_eq!(
        statuses[STATS].status,
        LiveStatus::Stale(vec![StaleReason::OutputsModified]),
        "ручная правка выхода — причина OutputsModified"
    );

    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.failed(), 0);
    assert_eq!(report.ran(), 1, "перегенерируется только stats; plot зависит от ledger-out_hash, а не от файла: {:?}", report.artifacts);
    let project = load_project(&root).unwrap();
    assert_eq!(evaluate(&project, &store).unwrap()[STATS].status, LiveStatus::Fresh);

    // Удаление выхода тоже детектируется — уже как OutputsMissing.
    std::fs::remove_file(&stats).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert_eq!(
        statuses[STATS].status,
        LiveStatus::Stale(vec![StaleReason::OutputsMissing]),
        "удалённый выход — причина OutputsMissing"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn topological_order_is_stats_before_plot() {
    let (_root, project) = (PathBuf::new(), load_project(&rde_core::demo_project_dir()).unwrap());
    let order = topological_compute(&project).unwrap();
    let stats = order.iter().position(|id| id == STATS).unwrap();
    let plot = order.iter().position(|id| id == PLOT).unwrap();
    assert!(stats < plot, "{order:?}");
    // data/constants не входят в порядок запуска.
    assert!(!order.contains(&REALIZATIONS.to_owned()));
}

// ── P1: причины по компонентам, ref-рёбра, точечное обновление ───────────────

#[test]
fn script_change_reports_script_reason() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("script-change");
    let mut store = Store::open(&root).unwrap();
    update(&project, &mut store, &UpdateOptions::default()).unwrap();

    let script = artifact_dir(&project, STATS).join("script.py");
    let mut text = std::fs::read_to_string(&script).unwrap();
    text.push_str("\n# правка скрипта\n");
    std::fs::write(&script, text).unwrap();

    let project = load_project(&root).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert_eq!(
        statuses[STATS].status,
        LiveStatus::Stale(vec![StaleReason::ScriptChanged]),
        "правка скрипта — причина ScriptChanged, а не общая «устарел»"
    );
    // data-артефакт не запускается и не активируется — у него нет act_key.
    assert_eq!(statuses[REALIZATIONS].status, LiveStatus::Fresh);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn params_change_reports_params_reason() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("params-change");
    let mut store = Store::open(&root).unwrap();
    update(&project, &mut store, &UpdateOptions::default()).unwrap();

    let manifest_path = artifact_dir(&project, STATS).join("artifact.toml");
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    std::fs::write(&manifest_path, text.replace("window = 128", "window = 256")).unwrap();

    let project = load_project(&root).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert_eq!(statuses[STATS].status, LiveStatus::Stale(vec![StaleReason::ParamsChanged]));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn ref_edge_does_not_cascade_and_ref_cycle_is_not_a_dag_cycle() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("ref-edge");
    // Добавляем к stats ref-вход на plot: визуальный цикл plot→stats (hashed) + stats→plot (ref).
    let stats_manifest = artifact_dir(&project, STATS).join("artifact.toml");
    let text = std::fs::read_to_string(&stats_manifest).unwrap();
    std::fs::write(
        &stats_manifest,
        format!(
            "{text}\n[[groups]]\nname = \"context\"\nmode = \"ref\"\ninputs = [ {{ artifact = \"{PLOT}\", selector = \"out/plot.svg\" }} ]\n"
        ),
    )
    .unwrap();
    let project = load_project(&root).unwrap();
    assert!(
        !project.issues.iter().any(|issue| issue.message.contains("цикл")),
        "ref-цикл — не ошибка DAG: {:?}",
        project.issues
    );
    let mut store = Store::open(&root).unwrap();
    update(&project, &mut store, &UpdateOptions::default()).unwrap();

    // Правка скрипта plot не должна делать stats устаревшим (ref не каскадит).
    let plot_script = artifact_dir(&project, PLOT).join("script.py");
    let mut text = std::fs::read_to_string(&plot_script).unwrap();
    text.push_str("\n# правка\n");
    std::fs::write(&plot_script, text).unwrap();
    let project = load_project(&root).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert_eq!(statuses[PLOT].status, LiveStatus::Stale(vec![StaleReason::ScriptChanged]));
    assert_eq!(statuses[STATS].status, LiveStatus::Fresh, "ref-вход не влияет на свежесть");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn single_artifact_update_and_force() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("single-update");
    let mut store = Store::open(&root).unwrap();
    update(&project, &mut store, &UpdateOptions::default()).unwrap();

    // «Обновить артефакт»: только цель, без зависимых; свежий артефакт не пропускается при force.
    let options = UpdateOptions {
        targets: Some(vec![STATS.to_owned()]),
        include_dependents: false,
        force: true,
        continue_on_error: false,
    };
    let report = update(&project, &mut store, &options).unwrap();
    assert_eq!(report.failed(), 0, "{:?}", report.errors);
    assert_eq!(report.ran(), 1, "принудительно пересчитан только stats: {:?}", report.artifacts);
    assert_eq!(report.artifacts.len(), 1, "plot в цель не входит: {:?}", report.artifacts);
    assert_eq!(report.artifacts[0].artifact_id, STATS);

    // «Обновить связанные»: цель + зависимые.
    let options = UpdateOptions {
        targets: Some(vec![STATS.to_owned()]),
        include_dependents: true,
        force: true,
        continue_on_error: false,
    };
    let report = update(&project, &mut store, &options).unwrap();
    assert_eq!(report.failed(), 0, "{:?}", report.errors);
    let ids: Vec<&str> = report.artifacts.iter().map(|a| a.artifact_id.as_str()).collect();
    assert!(ids.contains(&STATS) && ids.contains(&PLOT), "цель и зависимый: {ids:?}");
    let _ = std::fs::remove_dir_all(&root);
}

// ── P1: константы и glob-селекторы как реальные входы ───────────────────────

fn write_min_project(root: &Path) {
    std::fs::create_dir_all(root.join("objects/artifacts")).unwrap();
    std::fs::write(
        root.join("rde.toml"),
        "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"P1\"\n",
    )
    .unwrap();
}

fn write_artifact(root: &Path, id: &str, manifest: &str) {
    let dir = root.join("objects/artifacts").join(id);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("artifact.toml"), manifest).unwrap();
}

#[test]
fn constants_input_materializes_json_and_invalidates_on_change() {
    if !python_available() {
        return;
    }
    let root = tmp("constants-input");
    write_min_project(&root);
    let constants = "01M5VJ1JG0T9NS5SEZX35HGNFY";
    let compute = "01M5VJ1JG039MMV97SZGY6V9EQ";
    write_artifact(
        &root,
        constants,
        &format!(
            "schema=\"rde/artifact@1\"\nid=\"{constants}\"\ntitle=\"Константы\"\ntype=\"constants\"\n\
             [values.R_n]\nvalue=1.0\nunit=\"Ом\"\ndisplay=\"R_н\"\n"
        ),
    );
    write_artifact(
        &root,
        compute,
        &format!(
            "schema=\"rde/artifact@1\"\nid=\"{compute}\"\ntitle=\"Сумма\"\ntype=\"compute\"\n\
             outputs=[\"out/sum.txt\"]\n[runtime]\nkind=\"python\"\nenv=\"project\"\nentry=\"script.py\"\n\
             [[groups]]\nname=\"consts\"\nmode=\"hashed\"\ninputs=[{{ constants=\"{constants}\", selector=\"R_n\" }}]\n"
        ),
    );
    let script = format!(
        "import json\nfrom pathlib import Path\nv = json.loads(Path('inputs/constant-R_n.json').read_text(encoding='utf-8'))\nPath('out').mkdir(exist_ok=True)\nPath('out/sum.txt').write_text(str(v['value'] * 2))\n"
    );
    std::fs::write(root.join("objects/artifacts").join(compute).join("script.py"), script).unwrap();

    let project = load_project(&root).unwrap();
    assert!(project.issues.is_empty(), "{:?}", project.issues);
    let mut store = Store::open(&root).unwrap();
    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.failed(), 0, "{:?}", report.errors);
    let out = std::fs::read_to_string(artifact_dir(&project, compute).join("out/sum.txt")).unwrap();
    assert_eq!(out, "2.0", "константа материализована в inputs/constant-R_n.json");

    // Меняем значение константы — вход compute меняется (in_hash), артефакт устаревает.
    let const_manifest = artifact_dir(&project, constants).join("artifact.toml");
    let text = std::fs::read_to_string(&const_manifest).unwrap();
    std::fs::write(&const_manifest, text.replace("value=1.0", "value=2.5")).unwrap();
    let project = load_project(&root).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert_eq!(
        statuses[compute].status,
        LiveStatus::Stale(vec![StaleReason::InputsChanged]),
        "изменение константы — изменение входов"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn glob_input_materializes_all_matching_files() {
    if !python_available() {
        return;
    }
    let root = tmp("glob-input");
    write_min_project(&root);
    let data = "01M5VJ1JG0HD99FDTGJ8YEPFZX";
    let compute = "01M5VJ1JG039MMV97SZGY6V9EQ";
    write_artifact(
        &root,
        data,
        &format!("schema=\"rde/artifact@1\"\nid=\"{data}\"\ntitle=\"Данные\"\ntype=\"data\"\n"),
    );
    let data_dir = root.join("objects/artifacts").join(data);
    std::fs::create_dir_all(data_dir.join("inputs")).unwrap();
    std::fs::write(data_dir.join("inputs/a.csv"), "x\n1\n").unwrap();
    std::fs::write(data_dir.join("inputs/b.csv"), "x\n2\n").unwrap();
    write_artifact(
        &root,
        compute,
        &format!(
            "schema=\"rde/artifact@1\"\nid=\"{compute}\"\ntitle=\"Счётчик\"\ntype=\"compute\"\n\
             outputs=[\"out/count.txt\"]\n[runtime]\nkind=\"python\"\nenv=\"project\"\nentry=\"script.py\"\n\
             [[groups]]\nname=\"data\"\nmode=\"hashed\"\ninputs=[{{ artifact=\"{data}\", selector=\"inputs/*.csv\" }}]\n"
        ),
    );
    std::fs::write(
        root.join("objects/artifacts").join(compute).join("script.py"),
        "from pathlib import Path\nn = len(list(Path('inputs').glob('*.csv')))\nPath('out').mkdir(exist_ok=True)\nPath('out/count.txt').write_text(str(n))\n",
    )
    .unwrap();

    let project = load_project(&root).unwrap();
    assert!(project.issues.is_empty(), "{:?}", project.issues);
    let mut store = Store::open(&root).unwrap();
    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.failed(), 0, "{:?}", report.errors);
    let out = std::fs::read_to_string(artifact_dir(&project, compute).join("out/count.txt")).unwrap();
    assert_eq!(out, "2", "glob-селектор материализовал оба файла");

    // Изменение одного из файлов меняет in_hash (Merkle-дерево совпавших файлов).
    std::fs::write(data_dir.join("inputs/a.csv"), "x\n42\n").unwrap();
    let project = load_project(&root).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert_eq!(statuses[compute].status, LiveStatus::Stale(vec![StaleReason::InputsChanged]));
    let _ = std::fs::remove_dir_all(&root);
}
