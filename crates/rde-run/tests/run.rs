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
        outputs: outputs.iter().map(|s| (*s).to_owned()).collect(),
        trusted: true,
        ..RunSpec::new("art", "python", "script.py", artifact_dir)
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
        trusted: true,
        ..RunSpec::new("art", "python", "missing.py", project.join("art"))
    };
    std::fs::create_dir_all(project.join("art")).unwrap();
    let outcome = run(&project, &spec, None).unwrap();
    assert_eq!(outcome.status, RunStatus::SpawnError);
    assert!(outcome.error.unwrap_or_default().contains("скрипт не найден"));
    let _ = std::fs::remove_dir_all(&project);
}

// ── P3: доверенность, лишние файлы, read-only входы, сеть, provenance, SIGTERM ──

#[test]
fn untrusted_project_is_refused_without_spawning() {
    let project = tmp("trust");
    let mut spec = spec(&project, "print('should not run')\n", &[]);
    spec.trusted = false;
    let outcome = run(&project, &spec, None).unwrap();
    assert_eq!(outcome.status, RunStatus::SpawnError);
    assert!(
        outcome.error.as_deref().unwrap_or("").contains("доверенн"),
        "сообщение о согласии: {:?}",
        outcome.error
    );
    assert!(!outcome.work_dir.join("script.py").is_file(), "процесс не запускался");
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn extra_files_outside_declared_outputs_are_an_error() {
    if !python_available() {
        return;
    }
    let project = tmp("extra");
    let spec = spec(
        &project,
        "from pathlib import Path\nPath('out').mkdir(exist_ok=True)\nPath('out/ok.txt').write_text('ok')\nPath('junk.tmp').write_text('junk')\n",
        &["out/ok.txt"],
    );
    let outcome = run(&project, &spec, None).unwrap();
    assert_eq!(outcome.status, RunStatus::Failed, "{outcome:?}");
    assert_eq!(outcome.extra_files, vec!["junk.tmp".to_owned()]);
    assert!(outcome.error.as_deref().unwrap_or("").contains("лишние файлы"));
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn inputs_are_read_only_for_the_script() {
    if !python_available() {
        return;
    }
    let project = tmp("readonly");
    let source = project.join("data/input.csv");
    std::fs::create_dir_all(source.parent().unwrap()).unwrap();
    std::fs::write(&source, "a,b\n1,2\n").unwrap();
    let mut spec = spec(
        &project,
        "from pathlib import Path\ntry:\n    Path('inputs/input.csv').write_text('hacked')\n    verdict = 'writable'\nexcept PermissionError:\n    verdict = 'readonly'\nPath('out').mkdir(exist_ok=True)\nPath('out/verdict.txt').write_text(verdict)\n",
        &["out/verdict.txt"],
    );
    spec.inputs.push(InputFile::new(source, "inputs/input.csv"));
    let outcome = run(&project, &spec, None).unwrap();
    assert!(outcome.success(), "{outcome:?}");
    assert_eq!(std::fs::read_to_string(&outcome.outputs[0].path).unwrap(), "readonly");
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn network_is_denied_best_effort_and_recorded_in_provenance() {
    if !python_available() {
        return;
    }
    let project = tmp("network");
    let spec = spec(
        &project,
        "import os\nfrom pathlib import Path\nPath('out').mkdir(exist_ok=True)\nPath('out/net.txt').write_text(f\"{os.environ.get('RDE_NETWORK')}|{os.environ.get('HTTPS_PROXY')}\")\n",
        &["out/net.txt"],
    );
    let outcome = run(&project, &spec, None).unwrap();
    assert!(outcome.success(), "{outcome:?}");
    assert_eq!(std::fs::read_to_string(&outcome.outputs[0].path).unwrap(), "deny|http://127.0.0.1:9");
    assert!(outcome.network_denied);
    let details: serde_json::Value = serde_json::from_slice(&std::fs::read(&outcome.run_details_path).unwrap()).unwrap();
    assert_eq!(details["network"], "deny");
    assert_eq!(details["network_denied_best_effort"], true);
    assert_eq!(details["artifact_id"], "art");
    assert!(details["platform"].as_str().is_some_and(|platform| !platform.is_empty()));
    assert_eq!(details["status"], "ok");
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn run_details_provenance_has_hashes_and_logs() {
    if !python_available() {
        return;
    }
    let project = tmp("details");
    let mut spec = spec(&project, "print('stdout line')\n", &[]);
    spec.script_hash = Some("ab".repeat(32));
    spec.env_lock_digest = Some("cd".repeat(32));
    spec.initiator = "agent".to_owned();
    let outcome = run(&project, &spec, None).unwrap();
    assert!(outcome.success(), "{outcome:?}");
    let details: serde_json::Value = serde_json::from_slice(&std::fs::read(&outcome.run_details_path).unwrap()).unwrap();
    assert_eq!(details["script_hash"], "ab".repeat(32));
    assert_eq!(details["env_lock_digest"], "cd".repeat(32));
    assert_eq!(details["initiator"], "agent");
    assert!(details["stdout_digest"].as_str().is_some_and(|digest| digest.len() == 64));
    assert!(details["started_unix"].as_u64().is_some());
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn cancel_sends_sigterm_with_grace_before_sigkill() {
    if !python_available() {
        return;
    }
    let project = tmp("sigterm");
    let spec = spec(
        &project,
        "import signal, sys, time\nfrom pathlib import Path\ndef handler(*_):\n    Path('terminated.txt').write_text('grace')\n    sys.exit(0)\nsignal.signal(signal.SIGTERM, handler)\nwhile True:\n    time.sleep(0.05)\n",
        &[],
    );
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        flag.store(true, Ordering::Relaxed);
    });
    let outcome = run(&project, &spec, Some(cancel)).unwrap();
    assert_eq!(outcome.status, RunStatus::Cancelled, "{outcome:?}");
    assert!(
        outcome.work_dir.join("terminated.txt").is_file(),
        "скрипт успел обработать SIGTERM (grace), а не получил сразу SIGKILL"
    );
    let _ = std::fs::remove_dir_all(&project);
}

#[test]
fn log_helpers_read_search_and_prune() {
    let project = tmp("logs");
    let run_dir = rde_run::runs_root(&project).join("run1");
    std::fs::create_dir_all(&run_dir).unwrap();
    let log = run_dir.join("stdout.log");
    std::fs::write(&log, "line one\nerror: boom\nline three\n").unwrap();
    let chunk = rde_run::read_log_from(&log, 0, 8).unwrap();
    assert_eq!(chunk.bytes, b"line one");
    assert!(!chunk.eof);
    let rest = rde_run::read_log_from(&log, chunk.next_offset, 1024).unwrap();
    assert!(rest.eof);
    assert!(String::from_utf8_lossy(&rest.bytes).contains("error: boom"));
    let hits = rde_run::search_log(&log, "ERROR", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].line_no, 2);
    assert!(hits[0].text.contains("boom"));
    // Ретенция: два запуска, оставляем один.
    std::fs::create_dir_all(rde_run::runs_root(&project).join("run2")).unwrap();
    let removed = rde_run::prune_runs(&project, 1).unwrap();
    assert_eq!(removed, 1);
    let _ = std::fs::remove_dir_all(&project);
}
