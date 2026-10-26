//! Интеграционные тесты DAG: обновление демо-цепочки, статусы из хешей, отказ и ручная правка выходов.

use std::path::{Path, PathBuf};

use rde_core::{load_project, Project};
use rde_dag::{evaluate, stale_artifacts, topological_compute, update, LiveStatus, StaleReason, UpdateOptions};
use rde_store::Store;

const STATS: &str = "01M5VJ1JG039MMV97SZGY6V9EQ";
const REALIZATIONS: &str = "01M5VJ1JG0HD99FDTGJ8YEPFZX";
const PLOT: &str = "01M5VJ1JG0QQJM2S6R76FQCZ25";
const LINE: &str = "01M5VJ1JG07RAAH8W7QHC1VX6P";

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
        if matches!(&*name, ".rde" | "exports" | "__pycache__" | "out" | ".venv") {
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
    assert_eq!(report.ran(), 3, "отчёт: {:?}", report.artifacts);
    let order: Vec<&str> = report.artifacts.iter().map(|artifact| artifact.artifact_id.as_str()).collect();
    let stats_pos = order.iter().position(|id| *id == STATS).expect("stats в отчёте");
    let plot_pos = order.iter().position(|id| *id == PLOT).expect("plot в отчёте");
    assert!(stats_pos < plot_pos, "топопорядок: статистики раньше графика: {order:?}");
    assert!(order.iter().any(|id| *id == LINE), "линейный график тоже обновлён: {order:?}");
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
    assert_eq!(report.skipped(), 3);
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
    assert_eq!(report.ran(), 3, "stats и зависимые plot/line должны пересчитаться: {:?}", report.artifacts);

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
        ..Default::default()
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
        batch: true,
        ..Default::default()
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
        "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"P1\"\ntrusted = true\n",
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

// ── P2: транзакции с корнями, откат O(1), GC после обновлений ────────────────

#[test]
fn batch_update_commits_single_transaction_with_roots() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("batch");
    let mut store = Store::open(&root).unwrap();
    let options = UpdateOptions { batch: true, ..UpdateOptions::default() };
    let report = update(&project, &mut store, &options).unwrap();
    assert_eq!(report.failed(), 0, "{:?}", report.errors);
    assert_eq!(report.ran(), 3);
    assert_eq!(report.tx_ids.len(), 1, "пачка — одна транзакция: {:?}", report.tx_ids);
    let roots = store.current_roots().unwrap();
    assert!(roots.get(&format!("artifact/{STATS}")).is_some(), "корень stats: {:?}", roots.names().collect::<Vec<_>>());
    assert!(roots.get(&format!("artifact/{PLOT}")).is_some());
    assert_eq!(store.current_txid().unwrap().as_deref(), Some(report.tx_ids[0].as_str()));
    // ledger восстановим из журнала: обе записи в одной транзакции.
    let ledger = rde_dag::ledger(&store).unwrap();
    assert!(ledger.contains_key(STATS) && ledger.contains_key(PLOT));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rollback_restores_outputs_and_ledger_state() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("rollback-dag");
    let mut store = Store::open(&root).unwrap();
    let options = UpdateOptions { batch: true, ..UpdateOptions::default() };
    update(&project, &mut store, &options).unwrap();
    let stats_out = artifact_dir(&project, STATS).join("out/stats.json");
    let v1 = std::fs::read_to_string(&stats_out).unwrap();

    // Меняем вход и пересчитываем пачку: появляется новая версия выходов и транзакция.
    let input = artifact_dir(&project, REALIZATIONS).join("inputs/experiment.csv");
    let mut text = std::fs::read_to_string(&input).unwrap();
    text.push_str("50,50.0,50.0\n");
    std::fs::write(&input, text).unwrap();
    let project = load_project(&root).unwrap();
    update(&project, &mut store, &options).unwrap();
    let v2 = std::fs::read_to_string(&stats_out).unwrap();
    assert_ne!(v1, v2, "вход изменился — выход другой");

    // Откат последней пачки: корни переключены O(1), выходы материализованы из CAS.
    let project = load_project(&root).unwrap();
    let outcome = rde_dag::rollback(&project, &mut store, None).unwrap().expect("есть что откатывать");
    assert!(outcome.restored.contains(&STATS.to_owned()), "stats восстановлен: {outcome:?}");
    assert_eq!(std::fs::read_to_string(&stats_out).unwrap(), v1, "выход восстановлен из CAS");
    let restored_ledger = rde_dag::ledger(&store).unwrap();
    let stats_state = restored_ledger.get(STATS).expect("состояние stats вернулось");
    assert_ne!(stats_state.act_key, v2, "ledger-состояние откатано");

    // Входной файл откат не трогает — статус честно «изменились входы».
    let project = load_project(&root).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert!(matches!(&statuses[STATS].status, LiveStatus::Stale(reasons) if reasons.contains(&StaleReason::InputsChanged)));

    // Возвращаем вход — и статус снова «актуален» (выходы/ledger от v1).
    let mut text = std::fs::read_to_string(&input).unwrap();
    text.truncate(text.rfind("50,50.0,50.0\n").unwrap());
    std::fs::write(&input, text).unwrap();
    let project = load_project(&root).unwrap();
    assert_eq!(evaluate(&project, &store).unwrap()[STATS].status, LiveStatus::Fresh);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn rollback_before_first_run_resets_artifact() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("rollback-reset");
    let mut store = Store::open(&root).unwrap();
    let options = UpdateOptions { batch: true, ..UpdateOptions::default() };
    update(&project, &mut store, &options).unwrap();
    let stats_out = artifact_dir(&project, STATS).join("out/stats.json");
    assert!(stats_out.is_file());

    let project = load_project(&root).unwrap();
    let outcome = rde_dag::rollback(&project, &mut store, None).unwrap().expect("откат первой пачки");
    assert_eq!(outcome.reset.len(), 3, "{outcome:?}");
    assert!(!stats_out.exists(), "выходы удалены (артефакт ещё не запускался)");
    let manifest = std::fs::read_to_string(artifact_dir(&project, STATS).join("artifact.toml")).unwrap();
    assert!(!manifest.contains("out_hash"), "out_hash снят при откате к «не запускался»");
    let project = load_project(&root).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert_eq!(statuses[STATS].status, LiveStatus::NeverRun);
    assert_eq!(statuses[PLOT].status, LiveStatus::NeverRun);
    // Откат сам — транзакция: его тоже можно откатить (вернуть обновление).
    let undo = rde_dag::rollback(&project, &mut store, None).unwrap().expect("откат отката доступен");
    assert!(!undo.restored.is_empty(), "{undo:?}");
    assert!(stats_out.is_file(), "повторный откат вернул выходы обновления");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gc_after_update_keeps_current_roots_and_frees_orphans() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("gc-dag");
    let mut store = Store::open(&root).unwrap();
    let options = UpdateOptions { batch: true, ..UpdateOptions::default() };
    update(&project, &mut store, &options).unwrap();
    let orphan = store.put_blob(b"orphan after update").unwrap();
    let roots = store.current_roots().unwrap();
    let stats_tree = roots.get(&format!("artifact/{STATS}")).expect("корень stats");

    let report = store
        .gc(&rde_store::GcOptions { keep_last_txs: 0, keep_days: 0, dry_run: false, prune_failed_txs: true })
        .unwrap();
    assert!(report.removed_blobs >= 1, "{report:?}");
    assert!(store.get_blob(orphan).unwrap().is_none(), "сиротский блоб удалён");
    // Текущие корни защищены: деревья и их блобы на месте, выходы можно восстановить из CAS.
    let entries = store.get_tree(stats_tree).unwrap().expect("дерево stats на месте");
    for entry in entries {
        assert!(store.get_blob(entry.blob).unwrap().is_some(), "блоб {} текущего корня удалён", entry.blob);
    }
    assert_eq!(store.current_roots().unwrap(), roots);
    let _ = std::fs::remove_dir_all(&root);
}

// ── P3: окружения и rde.lock ─────────────────────────────────────────────────

#[test]
fn update_records_rde_lock_and_env_change_marks_stale() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("env-lock");
    let mut store = Store::open(&root).unwrap();
    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.failed(), 0, "{:?}", report.errors);
    let lock_path = root.join("rde.lock");
    assert!(lock_path.is_file(), "rde.lock создан после запуска");
    let lock_text = std::fs::read_to_string(&lock_path).unwrap();
    assert!(lock_text.contains("schema = \"rde/lock@1\""), "{lock_text}");
    assert!(lock_text.contains("[envs.python]"), "{lock_text}");
    let env_lock = rde_env::load(&root).expect("rde.lock читается");
    let python = env_lock.envs.get("python").expect("запись python");
    assert!(!python.version.is_empty());
    assert_eq!(python.digest.len(), 64);

    // Повторное обновление не переписывает rde.lock (окружение не изменилось).
    let project = load_project(&root).unwrap();
    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.ran(), 0, "{:?}", report.artifacts);
    assert!(!report.notes.iter().any(|note| note.contains("rde.lock обновлён")), "{:?}", report.notes);

    // Смена режима окружения (project → system) меняет env_hash → «изменилось окружение».
    let stats_manifest = artifact_dir(&project, STATS).join("artifact.toml");
    let text = std::fs::read_to_string(&stats_manifest).unwrap();
    std::fs::write(&stats_manifest, text.replace("env = \"project\"", "env = \"system\"")).unwrap();
    let project = load_project(&root).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert_eq!(
        statuses[STATS].status,
        LiveStatus::Stale(vec![StaleReason::EnvChanged]),
        "смена окружения — причина EnvChanged"
    );
    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.failed(), 0, "{:?}", report.errors);
    let project = load_project(&root).unwrap();
    assert_eq!(evaluate(&project, &store).unwrap()[STATS].status, LiveStatus::Fresh);

    // Объявленные переменные окружения входят в env_hash (R-RUN-07).
    let text = std::fs::read_to_string(&stats_manifest).unwrap();
    std::fs::write(
        &stats_manifest,
        text.replace("entry = \"script.py\"", "entry = \"script.py\"\nvars = { RDE_MODE = \"test\" }"),
    )
    .unwrap();
    let project = load_project(&root).unwrap();
    if !project.objects.iter().any(|object| object.id == STATS) {
        eprintln!("ISSUES: {:?}", project.issues.iter().map(|issue| (issue.path.display().to_string(), issue.message.clone())).collect::<Vec<_>>());
        eprintln!("MANIFEST: {}", std::fs::read_to_string(&stats_manifest).unwrap());
    }
    let statuses = evaluate(&project, &store).unwrap();
    assert_eq!(statuses[STATS].status, LiveStatus::Stale(vec![StaleReason::EnvChanged]));
    let _ = std::fs::remove_dir_all(&root);
}

// ── P3: provenance, согласие на запуск, политика сети ───────────────────────

fn run_dir_of(report: &rde_dag::UpdateReport, artifact_id: &str) -> PathBuf {
    let entry = report
        .artifacts
        .iter()
        .find(|entry| entry.artifact_id == artifact_id)
        .expect("артефакт в отчёте");
    entry
        .log_path
        .as_ref()
        .expect("путь лога")
        .parent()
        .expect("каталог запуска")
        .to_path_buf()
}

#[test]
fn provenance_written_with_subject_and_resolved_dependencies() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("provenance");
    let mut store = Store::open(&root).unwrap();
    let report = update(&project, &mut store, &UpdateOptions { batch: true, ..UpdateOptions::default() }).unwrap();
    assert_eq!(report.failed(), 0, "{:?}", report.errors);
    let stats_report = report.artifacts.iter().find(|entry| entry.artifact_id == STATS).unwrap();
    let run_dir = run_dir_of(&report, STATS);
    let provenance: serde_json::Value =
        serde_json::from_slice(&std::fs::read(run_dir.join("provenance.json")).unwrap()).unwrap();
    assert_eq!(provenance["_type"], "https://in-toto.io/Statement/v1");
    assert_eq!(provenance["subject"][0]["name"], STATS);
    assert_eq!(
        provenance["subject"][0]["digest"]["sha256"].as_str(),
        stats_report.out_hash.as_deref(),
        "subject = out_hash"
    );
    let external = &provenance["predicate"]["buildDefinition"]["externalParameters"];
    assert_eq!(external["artifact"], STATS);
    assert_eq!(external["act_key"].as_str(), stats_report.act_key.as_deref());
    assert!(external["params"].is_object());
    assert_eq!(external["initiator"], "user");
    let dependencies = provenance["predicate"]["buildDefinition"]["resolvedDependencies"].as_array().unwrap();
    assert!(!dependencies.is_empty(), "resolvedDependencies: {provenance}");
    assert!(
        dependencies.iter().any(|dependency| dependency["uri"].as_str().unwrap_or("").starts_with("rde://artifact/")),
        "URI входа-артефакта: {dependencies:?}"
    );
    let details = &provenance["predicate"]["runDetails"];
    assert_eq!(details["status"], "ok");
    assert_eq!(details["network"], "deny");
    assert!(details["env_lock_digest"].as_str().is_some_and(|digest| digest.len() == 64));
    assert!(details["script_hash"].as_str().is_some_and(|digest| digest.len() == 64));
    assert!(provenance["predicate"]["error"].is_null());
    // Провенанс пишется для каждого запуска пачки.
    let plot_provenance = run_dir_of(&report, PLOT).join("provenance.json");
    assert!(plot_provenance.is_file());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn failed_run_writes_provenance_with_error() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("provenance-fail");
    // Ломаем скрипт графика: падение после stats.
    let plot_script = artifact_dir(&project, PLOT).join("script.py");
    std::fs::write(&plot_script, "raise SystemExit(3)\n").unwrap();
    let mut store = Store::open(&root).unwrap();
    let report = update(&project, &mut store, &UpdateOptions { continue_on_error: true, ..UpdateOptions::default() }).unwrap();
    assert_eq!(report.failed(), 1, "{:?}", report.artifacts);
    let run_dir = run_dir_of(&report, PLOT);
    let provenance: serde_json::Value =
        serde_json::from_slice(&std::fs::read(run_dir.join("provenance.json")).unwrap()).unwrap();
    assert!(provenance["subject"].as_array().unwrap().is_empty(), "у неуспешного запуска нет subject");
    assert!(provenance["predicate"]["error"].as_str().is_some_and(|error| !error.is_empty()));
    assert_eq!(provenance["predicate"]["runDetails"]["status"], "failed");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn untrusted_project_refuses_update() {
    let root = tmp("untrusted");
    write_min_project(&root);
    // write_min_project пишет trusted = true; убираем согласие.
    let text = std::fs::read_to_string(root.join("rde.toml")).unwrap();
    std::fs::write(root.join("rde.toml"), text.replace("trusted = true", "trusted = false")).unwrap();
    let project = load_project(&root).unwrap();
    assert!(!project.manifest.trusted);
    let mut store = Store::open(&root).unwrap();
    let error = update(&project, &mut store, &UpdateOptions::default()).unwrap_err();
    assert!(error.contains("доверен"), "{error}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn network_policy_deny_by_default_and_allow_declaratively() {
    if !python_available() {
        return;
    }
    let root = tmp("network-policy");
    write_min_project(&root);
    let compute = "01M5VJ1JG039MMV97SZGY6V9EQ";
    write_artifact(
        &root,
        compute,
        &format!(
            "schema=\"rde/artifact@1\"\nid=\"{compute}\"\ntitle=\"Сеть\"\ntype=\"compute\"\n\
             outputs=[\"out/net.txt\"]\nnetwork_allow=false\n[runtime]\nkind=\"python\"\nenv=\"system\"\nentry=\"script.py\"\n"
        ),
    );
    std::fs::write(
        root.join("objects/artifacts").join(compute).join("script.py"),
        "import os\nfrom pathlib import Path\nPath('out').mkdir(exist_ok=True)\nPath('out/net.txt').write_text(os.environ.get('RDE_NETWORK', 'unset'))\n",
    )
    .unwrap();
    let project = load_project(&root).unwrap();
    let mut store = Store::open(&root).unwrap();
    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.failed(), 0, "{:?}", report.errors);
    let out = artifact_dir(&project, compute).join("out/net.txt");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "deny", "сеть по умолчанию запрещена");

    // Декларативное разрешение сети на артефакте.
    let manifest_path = artifact_dir(&project, compute).join("artifact.toml");
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    std::fs::write(&manifest_path, text.replace("network_allow=false", "network_allow=true")).unwrap();
    let project = load_project(&root).unwrap();
    let options = UpdateOptions {
        targets: Some(vec![compute.to_owned()]),
        include_dependents: false,
        force: true,
        ..Default::default()
    };
    let report = update(&project, &mut store, &options).unwrap();
    assert_eq!(report.failed(), 0, "{:?}", report.errors);
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "allow");
    let _ = std::fs::remove_dir_all(&root);
}

// ── P4: разрешение целей вставок документа (R-DOC-11) ───────────────────────

#[test]
fn insert_targets_resolve_values_and_images() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("insert-targets");
    let mut store = Store::open(&root).unwrap();
    update(&project, &mut store, &UpdateOptions::default()).unwrap();
    let project = load_project(&root).unwrap();

    // Значение из JSON-выхода по JSON pointer.
    let uri = format!("rde://artifact/{STATS}/out/stats.json#/stats/x1/mean");
    let (value, hash) = rde_dag::resolve_insert_text(&project, &store, &uri).unwrap();
    let parsed: f64 = value.parse().expect("числовое значение");
    assert!(parsed.is_finite());
    assert_eq!(hash.len(), 64);
    // Текст файла целиком.
    let (text, _) = rde_dag::resolve_insert_text(&project, &store, &format!("rde://artifact/{STATS}/out/stats.json")).unwrap();
    assert!(text.contains("stats"), "{text}");
    // Рисунок из SVG-выхода.
    let (path, ext, hash) = rde_dag::resolve_insert_image(&project, &store, &format!("rde://artifact/{PLOT}/out/plot.svg")).unwrap();
    assert!(path.is_file());
    assert_eq!(ext, "svg");
    assert_eq!(hash.len(), 64);
    // Ошибки: несуществующий артефакт/указатель.
    assert!(rde_dag::resolve_insert_text(&project, &store, "rde://artifact/NOPE/out/x.json").is_err());
    assert!(rde_dag::resolve_insert_text(&project, &store, &format!("rde://artifact/{STATS}/out/stats.json#/nope")).is_err());
    assert!(rde_dag::parse_insert_target("bad-uri").is_err());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn resolves_constants_and_csv_tables_for_inserts() {
    if !python_available() {
        return;
    }
    let (root, project) = demo_copy("resolve-inserts");
    let mut store = Store::open(&root).unwrap();
    update(&project, &mut store, &UpdateOptions { batch: true, ..UpdateOptions::default() }).unwrap();
    let project = load_project(&root).unwrap();
    let constants = project
        .objects
        .iter()
        .find(|object| matches!(&object.data, rde_core::ObjectData::Artifact(artifact) if artifact.kind == "constants"))
        .expect("константы в демо");
    let uri = format!("rde://constants/{}/R_n", constants.id);
    let ledger = rde_dag::ledger(&store).unwrap();
    let (text, run_style, hash) = rde_dag::resolve_insert_constant(&project, &ledger, &uri).unwrap();
    assert_eq!(text, "R_н");
    assert_eq!(run_style, "italic-subscript");
    assert!(!hash.is_empty());
    let stats = project
        .objects
        .iter()
        .find(|object| matches!(&object.data, rde_core::ObjectData::Artifact(artifact) if artifact.title.contains("Статистики")))
        .expect("статистики");
    let table_uri = format!("rde://artifact/{}/out/summary.csv", stats.id);
    let (rows, _hash) = rde_dag::resolve_insert_table(&project, &store, &table_uri).unwrap();
    assert!(rows.len() >= 2, "строки CSV: {rows:?}");
    assert!(rows[0].len() >= 2, "колонки CSV: {rows:?}");
    let _ = std::fs::remove_dir_all(&root);
}

/// P1 (ADR-0005 §1): константа со `source` из выхода артефакта разрешается, digest источника
/// входит в хеш (пересчёт артефакта → другой хеш → зависимые stale), литеральные константы
/// хешируются прежней схемой.
#[test]
fn constant_with_source_resolves_and_changes_hash() {
    let (root, _project) = demo_copy("const-source");
    let constants_id = {
        let project = load_project(&root).unwrap();
        project
            .objects
            .iter()
            .find(|object| matches!(&object.data, rde_core::ObjectData::Artifact(artifact) if artifact.kind == "constants"))
            .expect("константы в демо")
            .id
            .clone()
    };
    let constants_dir = root.join("objects/artifacts").join(&constants_id);
    let manifest_path = constants_dir.join("artifact.toml");
    let mut text = std::fs::read_to_string(&manifest_path).unwrap();
    text.push_str(&format!(
        "\n[values.S]\nvalue = 0.0\nunit = \"Ом\"\n\
         source = \"rde://artifact/{STATS}/out/stats.json#/v\"\n\
         \n[values.T]\nvalue = 12.4\nunit = \"Ом\"\nsymbol = \"R\"\nsubscript = \"н\"\n"
    ));
    std::fs::write(&manifest_path, text).unwrap();
    // Выход артефакта-источника (в копии out/ нет — создаём руками, как после запуска).
    let stats_dir = root.join("objects/artifacts").join(STATS);
    std::fs::create_dir_all(stats_dir.join("out")).unwrap();
    std::fs::write(stats_dir.join("out/stats.json"), "{\"v\": 12.5}").unwrap();

    let project = load_project(&root).unwrap();
    assert!(project.issues.is_empty(), "issues: {:?}", project.issues);
    let store = Store::open(&root).unwrap();
    let ledger = rde_dag::ledger(&store).unwrap();

    // source разрешается значением из файла; digest — sha256 файла (ledger пуст).
    let resolved = rde_dag::resolve_constant(&project, &ledger, &constants_id, "S").unwrap();
    assert_eq!(resolved.value, 12.5);
    assert_eq!(resolved.text, "12.5\u{00A0}Ом");
    assert_eq!(resolved.source_digest.as_deref().map(str::len), Some(64));
    let hash_before = resolved.hash;
    // Обозначение из symbol/subscript: текст обозначения + rich-стиль, авто-стиль подстрочного.
    let designation = rde_dag::resolve_constant(&project, &ledger, &constants_id, "T").unwrap();
    assert_eq!(designation.text, "Rн");
    assert_eq!(designation.run_style, "italic-subscript");
    assert!(designation.source_digest.is_none());

    // Пересчёт артефакта (новый файл, без ledger) → другой хеш константы.
    std::fs::write(stats_dir.join("out/stats.json"), "{\"v\": 13.5}").unwrap();
    let project = load_project(&root).unwrap();
    let resolved_after = rde_dag::resolve_constant(&project, &ledger, &constants_id, "S").unwrap();
    assert_eq!(resolved_after.value, 13.5);
    assert_ne!(resolved_after.hash, hash_before);

    // Ledger с out_hash: digest источника = out_hash артефакта (даже если файл тот же),
    // смена out_hash меняет хеш константы → каскад на зависимые артефакты.
    let state_with = |out_hash: &str| rde_dag::ArtifactState {
        artifact_id: STATS.to_owned(),
        act_key: "0".repeat(64),
        in_hash: None,
        script_hash: None,
        params_hash: None,
        env_hash: None,
        schema_version: None,
        out_hash: Some(out_hash.to_owned()),
        outputs: Vec::new(),
        run_id: None,
        updated_unix: 0,
    };
    let mut ledger_with = std::collections::HashMap::new();
    ledger_with.insert(STATS.to_owned(), state_with(&"aa".repeat(32)));
    let with_aa = rde_dag::resolve_constant(&project, &ledger_with, &constants_id, "S").unwrap();
    assert_eq!(with_aa.source_digest.as_deref(), Some("aa".repeat(32).as_str()));
    ledger_with.insert(STATS.to_owned(), state_with(&"bb".repeat(32)));
    let with_bb = rde_dag::resolve_constant(&project, &ledger_with, &constants_id, "S").unwrap();
    assert_ne!(with_aa.hash, with_bb.hash, "digest источника входит в хеш константы");

    // Литеральная константа R_n: хеш — прежняя схема `sha256(canonical_json(ConstantValue))`.
    let object = project.artifact_object(&constants_id).unwrap();
    let rde_core::ObjectData::Artifact(manifest) = &object.data else { panic!() };
    let literal = manifest.values.get("R_n").unwrap();
    let expected = rde_hash::sha256(
        rde_hash::canonical_json(&serde_json::to_value(literal).unwrap()).unwrap().as_bytes(),
    );
    assert_eq!(
        rde_dag::constants_input_hash(&project, &ledger, &constants_id, "R_n").unwrap(),
        expected,
        "совместимость act_key: литеральная константа хешируется старой схемой"
    );
    let uri = format!("rde://constants/{constants_id}/R_n");
    let (text, run_style, hash_hex) = rde_dag::resolve_insert_constant(&project, &ledger, &uri).unwrap();
    assert_eq!(text, "R_н");
    assert_eq!(run_style, "italic-subscript");
    assert_eq!(hash_hex, expected.hex());

    // Недоступный источник: для документа — ошибка (UI покажет причину), для in_hash зависимого
    // артефакта — кэш value + sentinel-digest (артефакт не «невычислим», §7.3 п.2).
    std::fs::remove_file(stats_dir.join("out/stats.json")).unwrap();
    let project = load_project(&root).unwrap();
    assert!(rde_dag::resolve_constant(&project, &ledger, &constants_id, "S").is_err());
    let fallback_hash = rde_dag::constants_input_hash(&project, &ledger, &constants_id, "S").unwrap();
    std::fs::write(stats_dir.join("out/stats.json"), "{\"v\": 13.5}").unwrap();
    let project = load_project(&root).unwrap();
    let restored_hash = rde_dag::constants_input_hash(&project, &ledger, &constants_id, "S").unwrap();
    assert_ne!(fallback_hash, restored_hash, "sentinel-digest отличается от разрешённого источника");
    let _ = std::fs::remove_dir_all(&root);
}
