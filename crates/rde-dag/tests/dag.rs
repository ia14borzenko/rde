//! Интеграционные тесты DAG: обновление демо-цепочки, статусы из хешей, отказ и ручная правка выходов.

use std::path::{Path, PathBuf};

use rde_core::{load_project, Project};
use rde_dag::{evaluate, stale_artifacts, topological_compute, update, LiveStatus, UpdateAction, UpdateOptions};
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
    assert_eq!(statuses[STATS].status, LiveStatus::Stale("выход изменён вручную".into()));

    let report = update(&project, &mut store, &UpdateOptions::default()).unwrap();
    assert_eq!(report.failed(), 0);
    assert_eq!(report.ran(), 1, "перегенерируется только stats; plot зависит от ledger-out_hash, а не от файла: {:?}", report.artifacts);
    let project = load_project(&root).unwrap();
    assert_eq!(evaluate(&project, &store).unwrap()[STATS].status, LiveStatus::Fresh);

    // Удаление выхода тоже детектируется.
    std::fs::remove_file(&stats).unwrap();
    let statuses = evaluate(&project, &store).unwrap();
    assert!(matches!(statuses[STATS].status, LiveStatus::Stale(_)));
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
