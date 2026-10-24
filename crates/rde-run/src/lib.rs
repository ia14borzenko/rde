//! RDE artifact runtime execution (M2.3).
//!
//! Запуск скрипта артефакта в изолированном рабочем каталоге `.rde/runs/<run_id>/work`:
//! - входы копируются в `work/inputs/...` (детерминированно, без симлинков);
//! - скрипт копируется из каталога артефакта;
//! - stdout/stderr пишутся в `runs/<run_id>/stdout.log|stderr.log`;
//! - объявленные выходы должны появиться в work; их собирает вызывающая сторона (rde-dag);
//! - таймаут и отмена (SIGKILL процессной группы) — чтобы не оставлять потомков.
//!
//! Платформенная зависимость (см. ADR-0001 §3): process group/SIGKILL — Unix; на других ОС
//! код компилируется с fallback `Child::kill` (полноценный Windows Job Object — v1.x).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Тип runtime артефакта (пока python/shell; node/r — каркас).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeKind {
    Python,
    Shell,
}

impl RuntimeKind {
    pub fn from_kind(kind: &str) -> Result<Self, String> {
        match kind {
            "python" | "python3" => Ok(RuntimeKind::Python),
            "shell" | "sh" | "bash" => Ok(RuntimeKind::Shell),
            other => Err(format!("runtime `{other}` пока не поддерживается (python/shell)")),
        }
    }

    /// Программа-исполнитель и аргументы перед путём к скрипту.
    pub fn command(self) -> (&'static str, &'static [&'static str]) {
        match self {
            RuntimeKind::Python => ("python3", &[]),
            RuntimeKind::Shell => ("sh", &[]),
        }
    }
}

/// Вход, материализуемый в workdir: источник и относительный путь назначения.
#[derive(Clone, Debug)]
pub struct InputFile {
    pub source: PathBuf,
    pub dest: String,
}

impl InputFile {
    pub fn new(source: impl Into<PathBuf>, dest: impl Into<String>) -> Self {
        Self { source: source.into(), dest: dest.into() }
    }
}

/// Задание на запуск артефакта.
#[derive(Clone, Debug)]
pub struct RunSpec {
    pub artifact_id: String,
    /// `python` | `shell`.
    pub runtime: String,
    /// Относительный путь скрипта внутри каталога артефакта.
    pub entry: String,
    /// Каталог артефакта (откуда берётся скрипт).
    pub artifact_dir: PathBuf,
    pub inputs: Vec<InputFile>,
    /// Объявленные относительные выходы (например `out/stats.json`).
    pub outputs: Vec<String>,
    pub timeout: Option<Duration>,
    pub env: BTreeMap<String, String>,
}

/// Итог запуска.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunOutcome {
    pub run_id: String,
    pub status: RunStatus,
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout_log: PathBuf,
    pub stderr_log: PathBuf,
    pub work_dir: PathBuf,
    pub outputs: Vec<RunOutput>,
    /// Описание ошибки (например, нет объявленных выходов).
    pub error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Ok,
    Failed,
    Timeout,
    Cancelled,
    SpawnError,
}

impl RunOutcome {
    pub fn success(&self) -> bool {
        self.status == RunStatus::Ok
    }
}

/// Найденный объявленный выход.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunOutput {
    pub rel: String,
    pub path: PathBuf,
    pub size: u64,
}

/// Корень журналов запусков проекта.
pub fn runs_root(project_root: &Path) -> PathBuf {
    project_root.join(".rde/runs")
}

/// Запустить артефакт. `cancel` можно выставлять из другого потока — процесс будет убит.
pub fn run(project_root: &Path, spec: &RunSpec, cancel: Option<Arc<AtomicBool>>) -> Result<RunOutcome, String> {
    let run_id = ulid::Ulid::generate().to_string();
    let run_dir = runs_root(project_root).join(&run_id);
    let work_dir = run_dir.join("work");
    fs::create_dir_all(&work_dir).map_err(|e| format!("создать workdir: {e}"))?;
    let stdout_log = run_dir.join("stdout.log");
    let stderr_log = run_dir.join("stderr.log");

    // Скрипт и входы.
    let entry_src = spec.artifact_dir.join(&spec.entry);
    if !entry_src.is_file() {
        return Ok(spawn_error(&run_id, &run_dir, &work_dir, &stdout_log, &stderr_log, 0, format!("скрипт не найден: {}", entry_src.display())));
    }
    let entry_dst = work_dir.join(&spec.entry);
    copy_file(&entry_src, &entry_dst).map_err(|e| format!("скопировать скрипт: {e}"))?;
    for input in &spec.inputs {
        if !input.source.is_file() {
            return Ok(spawn_error(&run_id, &run_dir, &work_dir, &stdout_log, &stderr_log, 0, format!("вход не найден: {}", input.source.display())));
        }
        copy_file(&input.source, &work_dir.join(&input.dest)).map_err(|e| format!("скопировать вход {}: {e}", input.dest))?;
    }

    let runtime = RuntimeKind::from_kind(&spec.runtime)?;
    let (program, extra) = runtime.command();
    let started = Instant::now();
    let mut command = Command::new(program);
    command
        .args(extra)
        .arg(&spec.entry)
        .current_dir(&work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    // Отдельная процессная группа: при отмене/таймауте убиваем и всех потомков.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => {
            let error = format!("не удалось запустить `{program}`: {e}");
            return Ok(spawn_error(&run_id, &run_dir, &work_dir, &stdout_log, &stderr_log, 0, error));
        }
    };

    // Потоки чтения логов (без deadlock на заполненных pipe).
    let out_reader = child.stdout.take();
    let err_reader = child.stderr.take();
    let out_path = stdout_log.clone();
    let err_path = stderr_log.clone();
    let out_thread = out_reader.map(|mut reader| {
        std::thread::spawn(move || {
            let mut file = File::create(&out_path).ok();
            if let Some(file) = file.as_mut() {
                let _ = io::copy(&mut reader, file);
            }
        })
    });
    let err_thread = err_reader.map(|mut reader| {
        std::thread::spawn(move || {
            let mut file = File::create(&err_path).ok();
            if let Some(file) = file.as_mut() {
                let _ = io::copy(&mut reader, file);
            }
        })
    });

    let mut cancelled = false;
    let mut timeout_hit = false;
    let status = loop {
        if let Ok(Some(status)) = child.try_wait() {
            break Some(status);
        }
        if cancel.as_ref().is_some_and(|flag| flag.load(Ordering::Relaxed)) {
            cancelled = true;
            kill_tree(&mut child);
            let _ = child.wait();
            break None;
        }
        if let Some(timeout) = spec.timeout {
            if started.elapsed() >= timeout {
                timeout_hit = true;
                kill_tree(&mut child);
                let _ = child.wait();
                break None;
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    // Дождаться закрытия pipe и дозаписи логов.
    if let Some(thread) = out_thread {
        let _ = thread.join();
    }
    if let Some(thread) = err_thread {
        let _ = thread.join();
    }
    let duration = started.elapsed();

    let mut outcomes = Vec::new();
    let mut missing = Vec::new();
    for rel in &spec.outputs {
        let path = work_dir.join(rel);
        if path.is_file() {
            outcomes.push(RunOutput { rel: rel.clone(), path: path.clone(), size: path.metadata().map(|m| m.len()).unwrap_or(0) });
        } else {
            missing.push(rel.clone());
        }
    }
    let (final_status, exit_code, error) = if cancelled {
        (RunStatus::Cancelled, None, Some("отменено пользователем".to_owned()))
    } else if timeout_hit {
        (RunStatus::Timeout, None, Some("превышен таймаут".to_owned()))
    } else {
        match status {
            Some(status) if status.success() && missing.is_empty() => (RunStatus::Ok, status.code(), None),
            Some(status) if status.success() => (
                RunStatus::Failed,
                status.code(),
                Some(format!("нет объявленных выходов: {}", missing.join(", "))),
            ),
            Some(status) => (RunStatus::Failed, status.code(), None),
            None => (RunStatus::SpawnError, None, Some("процесс не вернул статус".to_owned())),
        }
    };
    let outcome = RunOutcome {
        run_id,
        status: final_status,
        exit_code,
        duration_ms: duration.as_millis() as u64,
        stdout_log,
        stderr_log,
        work_dir,
        outputs: outcomes,
        error,
    };
    // provenance: пишем итог рядом с логами.
    let run_json = run_dir.join("run.json");
    if let Ok(json) = serde_json::to_vec_pretty(&outcome) {
        let _ = fs::write(run_json, json);
    }
    Ok(outcome)
}

fn spawn_error(
    run_id: &str,
    run_dir: &Path,
    work_dir: &Path,
    stdout_log: &Path,
    stderr_log: &Path,
    duration_ms: u64,
    error: String,
) -> RunOutcome {
    let _ = fs::create_dir_all(run_dir);
    let _ = File::create(stdout_log);
    let _ = File::create(stderr_log);
    RunOutcome {
        run_id: run_id.to_owned(),
        status: RunStatus::SpawnError,
        exit_code: None,
        duration_ms,
        stdout_log: stdout_log.to_path_buf(),
        stderr_log: stderr_log.to_path_buf(),
        work_dir: work_dir.to_path_buf(),
        outputs: Vec::new(),
        error: Some(error),
    }
}

fn copy_file(source: &Path, dest: &Path) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(source, dest).map(|_| ())
}

/// Убить процесс и всю его группу (Unix); fallback — только сам процесс.
fn kill_tree(child: &mut std::process::Child) {
    #[cfg(unix)]
    {
        // Process group id == pid, потому что запускали с process_group(0).
        let pid = child.id() as i32;
        if pid > 0 {
            // SAFETY: kill() с отрицательным pid — стандартный способ послать сигнал группе.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
    let _ = child.kill();
}
