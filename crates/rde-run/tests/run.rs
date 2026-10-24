//! Проверка исполнения артефактов: успех, выходы, логи, ошибки, таймаут и отмена.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rde_run::{run, InputFile, RunSpec, RunStatus};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-run-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn python_available() -> bool {
    std::process::Command::new("python3").arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

fn spec(project: &Path, script: &str, outputs: &[&str]) -> RunSpec {
    let artifact_dir = project.join("objects/artifacts/art/");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("script.py"), script).unwrap();
    RunSpec {
        artifact_id: "art".into(),
        runtime: "python".into(),
        entry: "script.py".into(),
        artifact_dir,
        inputs: Vec::new(),
        outputs: outputs.iter().map(|s| (*s).to_owned()).collect(),
        timeout: None,
        env: BTreeMap::new(),
    }
}

#[test]
fn successful_run_writes_outputs_and_logs() {
    if !python_available() {
        eprintln!("python3 недоступен — тест пропущен");
        return;
    }
    let project = tmp("ok");
    let spec = spec(&project, "from pathlib import Path\nprint('hello rde')\nPath('out').mkdir(exist_ok=True)\nPath('out/result.txt').write_text('42')\n", &["out/result.txt"]);
    let outcome = run(&project, &spec, None).unwrap();
    assert!(outcome.success(), "{outcome:?}");
    assert_eq!(outcome.exit_code, Some(0));
    assert_eq!(outcome.outputs.len(), 1);
    assert_eq!(std::fs::read_to_string(&outcome.outputs[0].path).unwrap(), "42");
    assert!(std::fs::read_to_string(&outcome.stdout_log).unwrap().contains("hello rde"));
    assert!(outcome.work_dir.join("out/result.txt").is_file());
    // provenance run.json
    assert!(outcome.stdout_log.parent().unwrap().join("run.json").is_file());
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn inputs_are_materialized_in_workdir() {
    if !python_available() {
        return;
    }
    let project = tmp("inputs");
    let source = project.join("data/input.csv");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "a,b\n1,2\n").unwrap();
    let mut spec = spec(&project, "from pathlib import Path\ntext = Path('inputs/input.csv').read_text()\nPath('out').mkdir(exist_ok=True)\nPath('out/copy.txt').write_text(text.upper())\n", &["out/copy.txt"]);
    spec.inputs.push(InputFile::new(source, "inputs/input.csv"));
    let outcome = run(&project, &spec, None).unwrap();
    assert!(outcome.success(), "{outcome:?}");
    assert_eq!(std::fs::read_to_string(&outcome.outputs[0].path).unwrap(), "A,B\n1,2\n");
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn nonzero_exit_is_failed_and_stderr_captured() {
    if !python_available() {
        return;
    }
    let project = tmp("fail");
    let spec = spec(&project, "import sys\nprint('oops', file=sys.stderr)\nsys.exit(3)\n", &[]);
    let outcome = run(&project, &spec, None).unwrap();
    assert_eq!(outcome.status, RunStatus::Failed);
    assert_eq!(outcome.exit_code, Some(3));
    assert!(std::fs::read_to_string(&outcome.stderr_log).unwrap().contains("oops"));
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn missing_declared_output_is_failed() {
    if !python_available() {
        return;
    }
    let project = tmp("missing-out");
    let spec = spec(&project, "print('no outputs')\n", &["out/expected.json"]);
    let outcome = run(&project, &spec, None).unwrap();
    assert_eq!(outcome.status, RunStatus::Failed);
    assert!(outcome.error.unwrap_or_default().contains("out/expected.json"));
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn timeout_kills_process_tree() {
    if !python_available() {
        return;
    }
    let project = tmp("timeout");
    let mut spec = spec(&project, "import time\ntime.sleep(30)\n", &[]);
    spec.timeout = Some(Duration::from_millis(250));
    let started = std::time::Instant::now();
    let outcome = run(&project, &spec, None).unwrap();
    assert_eq!(outcome.status, RunStatus::Timeout, "{outcome:?}");
    assert!(started.elapsed() < Duration::from_secs(5), "таймаут не сработал быстро");
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn cancel_flag_kills_process() {
    if !python_available() {
        return;
    }
    let project = tmp("cancel");
    let spec = spec(&project, "import time\ntime.sleep(30)\n", &[]);
    let flag = Arc::new(AtomicBool::new(false));
    let flag2 = flag.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(200));
        flag2.store(true, Ordering::Relaxed);
    });
    let started = std::time::Instant::now();
    let outcome = run(&project, &spec, Some(flag)).unwrap();
    assert_eq!(outcome.status, RunStatus::Cancelled, "{outcome:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn missing_script_reports_spawn_error_without_panic() {
    let project = tmp("no-script");
    let spec = RunSpec {
        artifact_id: "art".into(),
        runtime: "python".into(),
        entry: "missing.py".into(),
        artifact_dir: project.join("art"),
        inputs: Vec::new(),
        outputs: Vec::new(),
        timeout: None,
        env: BTreeMap::new(),
    };
    std::fs::create_dir_all(project.join("art")).unwrap();
    let outcome = run(&project, &spec, None).unwrap();
    assert_eq!(outcome.status, RunStatus::SpawnError);
    assert!(outcome.error.unwrap_or_default().contains("скрипт не найден"));
    let _ = std::fs::remove_dir_all(&project);
}
