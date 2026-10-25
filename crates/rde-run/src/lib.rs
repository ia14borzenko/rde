//! RDE artifact runtime execution (M2.3).
//!
//! Запуск скрипта артефакта в изолированном рабочем каталоге `.rde/runs/<run_id>/work`:
//! - входы копируются в `work/inputs/...` (детерминированно, без симлинков);
//! - скрипт копируется из каталога артефакта;
//! - stdout/stderr пишутся в `runs/<run_id>/stdout.log|stderr.log`;
//! - входы материализуются только для чтения (R-RUN-04), лишние файлы вне объявленных
//!   выходов — ошибка (защита от загрязнения графа);
//! - таймаут и отмена: SIGTERM → grace → SIGKILL процессной группы (R-RUN-05);
//! - сеть по умолчанию запрещена best-effort (proxy-blackhole + флаг в provenance), включается
//!   декларативно (`network_allow`); запуск требует согласия «проект доверенный» (R-RUN-06);
//! - `runDetails` provenance пишется в `runs/<run_id>/run_details.json` (R-ART-08);
//! - хелперы логов: чтение с байтового смещения, поиск, ретенция запусков (R-RUN-08).
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
///
/// Вход бывает файлом проекта (`source`) или сгенерированным содержимым (`inline`) — так
/// материализуются константы проекта и другие вычисляемые входы (R-ART-09).
#[derive(Clone, Debug)]
pub struct InputFile {
    pub source: PathBuf,
    pub dest: String,
    pub inline: Option<Vec<u8>>,
}

impl RunSpec {
    /// Минимальный RunSpec: python/shell-скрипт в каталоге артефакта, сеть запрещена,
    /// запуск требует согласия (`trusted` выставляет вызывающий слой после проверки проекта).
    pub fn new(
        artifact_id: impl Into<String>,
        runtime: impl Into<String>,
        entry: impl Into<String>,
        artifact_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            artifact_id: artifact_id.into(),
            runtime: runtime.into(),
            entry: entry.into(),
            artifact_dir: artifact_dir.into(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            timeout: None,
            env: BTreeMap::new(),
            interpreter: None,
            network: NetworkPolicy::Deny,
            trusted: false,
            initiator: "user".to_owned(),
            script_hash: None,
            env_lock_digest: None,
        }
    }
}

impl InputFile {
    pub fn new(source: impl Into<PathBuf>, dest: impl Into<String>) -> Self {
        Self { source: source.into(), dest: dest.into(), inline: None }
    }

    /// Вход из сгенерированного содержимого (например, JSON констант проекта).
    pub fn from_bytes(dest: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        Self { source: PathBuf::new(), dest: dest.into(), inline: Some(bytes.into()) }
    }
}

/// Политика сети для запуска (R-RUN-06): по умолчанию запрещена, best-effort без ОС-песочницы.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NetworkPolicy {
    Allow,
    Deny,
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
    /// Интерпретатор из разрешённого окружения (R-RUN-02); None — по runtime.
    pub interpreter: Option<String>,
    pub network: NetworkPolicy,
    /// Согласие пользователя «проект доверенный» (R-RUN-06).
    pub trusted: bool,
    /// Инициатор запуска (`user`, `agent`, …) — в provenance.
    pub initiator: String,
    /// `script_hash` и digest окружения — в `runDetails` provenance.
    pub script_hash: Option<String>,
    pub env_lock_digest: Option<String>,
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
    /// Файлы, созданные скриптом вне объявленных выходов (ошибка R-RUN-04).
    pub extra_files: Vec<String>,
    /// Сеть блокировалась best-effort (proxy-blackhole) — в provenance.
    pub network_denied: bool,
    /// Путь к `runDetails`-provenance запуска.
    pub run_details_path: PathBuf,
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
    let started_unix = now_unix();

    // R-RUN-06: запуск произвольного кода требует явного согласия «проект доверенный».
    if !spec.trusted {
        return Ok(spawn_error(
            spec,
            &run_id,
            &run_dir,
            &work_dir,
            &stdout_log,
            &stderr_log,
            started_unix,
            "проект не помечен доверенным: запуск скриптов требует согласия (R-RUN-06)".to_owned(),
        ));
    }

    // Скрипт и входы.
    let entry_src = spec.artifact_dir.join(&spec.entry);
    if !entry_src.is_file() {
        return Ok(spawn_error(spec, &run_id, &run_dir, &work_dir, &stdout_log, &stderr_log, started_unix, format!("скрипт не найден: {}", entry_src.display())));
    }
    let entry_dst = work_dir.join(&spec.entry);
    copy_file(&entry_src, &entry_dst).map_err(|e| format!("скопировать скрипт: {e}"))?;
    set_read_only(&entry_dst);
    for input in &spec.inputs {
        let dest = work_dir.join(&input.dest);
        match &input.inline {
            Some(bytes) => {
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent).map_err(|e| format!("создать каталог входа {}: {e}", input.dest))?;
                }
                fs::write(&dest, bytes).map_err(|e| format!("записать вход {}: {e}", input.dest))?;
            }
            None => {
                if !input.source.is_file() {
                    return Ok(spawn_error(spec, &run_id, &run_dir, &work_dir, &stdout_log, &stderr_log, started_unix, format!("вход не найден: {}", input.source.display())));
                }
                copy_file(&input.source, &dest).map_err(|e| format!("скопировать вход {}: {e}", input.dest))?;
            }
        }
        // R-RUN-04: входы материализуются только для чтения.
        set_read_only(&dest);
    }

    let runtime = RuntimeKind::from_kind(&spec.runtime)?;
    let (program, extra) = match &spec.interpreter {
        Some(interpreter) if !interpreter.trim().is_empty() => (interpreter.clone(), Vec::new()),
        _ => {
            let (program, extra) = runtime.command();
            (program.to_owned(), extra.to_vec())
        }
    };
    let network_denied = spec.network == NetworkPolicy::Deny;
    let started = Instant::now();
    let mut command = Command::new(&program);
    command
        .args(&extra)
        .arg(&spec.entry)
        .current_dir(&work_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in &spec.env {
        command.env(key, value);
    }
    if network_denied {
        // Best-effort без ОС-песочницы: blackhole-прокси для типовых HTTP-клиентов
        // + явный флаг RDE_NETWORK=deny; в provenance фиксируется, что это best-effort.
        for key in ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"] {
            command.env(key, "http://127.0.0.1:9");
        }
        command.env("NO_PROXY", "").env("no_proxy", "");
        command.env("RDE_NETWORK", "deny");
    } else {
        command.env("RDE_NETWORK", "allow");
    }
    command.env("RDE_RUN_ID", &run_id).env("RDE_ARTIFACT_ID", &spec.artifact_id);
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
            return Ok(spawn_error(spec, &run_id, &run_dir, &work_dir, &stdout_log, &stderr_log, started_unix, error));
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
            terminate_tree(&mut child, Duration::from_secs(2));
            break None;
        }
        if let Some(timeout) = spec.timeout {
            if started.elapsed() >= timeout {
                timeout_hit = true;
                terminate_tree(&mut child, Duration::from_secs(2));
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
    // R-RUN-04: файлы вне объявленных выходов — ошибка, а не предупреждение.
    let extra_files = scan_extra_files(&work_dir, spec);
    let (final_status, exit_code, error) = if cancelled {
        (RunStatus::Cancelled, None, Some("отменено пользователем".to_owned()))
    } else if timeout_hit {
        (RunStatus::Timeout, None, Some("превышен таймаут".to_owned()))
    } else {
        match status {
            Some(status) if status.success() && missing.is_empty() && extra_files.is_empty() => {
                (RunStatus::Ok, status.code(), None)
            }
            Some(status) if status.success() && !missing.is_empty() => (
                RunStatus::Failed,
                status.code(),
                Some(format!("нет объявленных выходов: {}", missing.join(", "))),
            ),
            Some(status) if status.success() => (
                RunStatus::Failed,
                status.code(),
                Some(format!(
                    "лишние файлы вне объявленных outputs (R-RUN-04): {}",
                    extra_files.iter().take(10).cloned().collect::<Vec<_>>().join(", ")
                )),
            ),
            Some(status) => (RunStatus::Failed, status.code(), None),
            None => (RunStatus::SpawnError, None, Some("процесс не вернул статус".to_owned())),
        }
    };
    let finished_unix = now_unix();
    let outcome = RunOutcome {
        run_id: run_id.clone(),
        status: final_status,
        exit_code,
        duration_ms: duration.as_millis() as u64,
        stdout_log: stdout_log.clone(),
        stderr_log: stderr_log.clone(),
        work_dir: work_dir.clone(),
        outputs: outcomes,
        extra_files,
        network_denied,
        run_details_path: run_dir.join("run_details.json"),
        error,
    };
    // provenance: runDetails (R-ART-08) пишет rde-run; dag добавит subject/dependencies.
    let details = RunDetails {
        run_id: run_id.clone(),
        artifact_id: spec.artifact_id.clone(),
        runtime: spec.runtime.clone(),
        entry: spec.entry.clone(),
        interpreter: program.clone(),
        initiator: spec.initiator.clone(),
        platform: platform(),
        trusted: spec.trusted,
        network: if network_denied { "deny".to_owned() } else { "allow".to_owned() },
        network_denied_best_effort: network_denied,
        script_hash: spec.script_hash.clone(),
        env_lock_digest: spec.env_lock_digest.clone(),
        exit_code: outcome.exit_code,
        status: outcome.status,
        duration_ms: outcome.duration_ms,
        started_unix,
        finished_unix,
        stdout_log: stdout_log.display().to_string(),
        stderr_log: stderr_log.display().to_string(),
        stdout_digest: file_digest(&stdout_log),
        stderr_digest: file_digest(&stderr_log),
        work_dir: work_dir.display().to_string(),
        extra_files: outcome.extra_files.clone(),
    };
    if let Ok(json) = serde_json::to_vec_pretty(&details) {
        let _ = fs::write(&outcome.run_details_path, json);
    }
    let run_json = run_dir.join("run.json");
    if let Ok(json) = serde_json::to_vec_pretty(&outcome) {
        let _ = fs::write(run_json, json);
    }
    Ok(outcome)
}

/// `runDetails` provenance запуска (R-ART-08); `rde-dag` объединяет его с subject/dependencies.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RunDetails {
    pub run_id: String,
    pub artifact_id: String,
    pub runtime: String,
    pub entry: String,
    pub interpreter: String,
    pub initiator: String,
    pub platform: String,
    pub trusted: bool,
    /// `allow` | `deny` (сеть).
    pub network: String,
    /// true — сеть блокировалась best-effort (без ОС-песочницы).
    pub network_denied_best_effort: bool,
    pub script_hash: Option<String>,
    pub env_lock_digest: Option<String>,
    pub exit_code: Option<i32>,
    pub status: RunStatus,
    pub duration_ms: u64,
    pub started_unix: u64,
    pub finished_unix: u64,
    pub stdout_log: String,
    pub stderr_log: String,
    pub stdout_digest: Option<String>,
    pub stderr_digest: Option<String>,
    pub work_dir: String,
    #[serde(default)]
    pub extra_files: Vec<String>,
}

/// Платформа для provenance (`RDE_PLATFORM` может переопределить).
pub fn platform() -> String {
    std::env::var("RDE_PLATFORM").unwrap_or_else(|_| format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH))
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn file_digest(path: &Path) -> Option<String> {
    std::fs::read(path).ok().map(|bytes| rde_hash::sha256(&bytes).hex())
}

/// Сделать файл только для чтения (R-RUN-04: входы не должны меняться скриптом).
fn set_read_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = fs::metadata(path) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(0o444);
            let _ = fs::set_permissions(path, permissions);
        }
    }
    let _ = path;
}

/// Файлы workdir вне объявленных выходов/входов/скрипта (R-RUN-04).
fn scan_extra_files(work_dir: &Path, spec: &RunSpec) -> Vec<String> {
    let mut allowed: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    allowed.insert(spec.entry.clone());
    for input in &spec.inputs {
        allowed.insert(input.dest.clone());
    }
    for output in &spec.outputs {
        allowed.insert(output.clone());
    }
    let mut extra = Vec::new();
    let mut stack = vec![work_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            let rel = path.strip_prefix(work_dir).unwrap_or(&path).to_string_lossy().replace('\\', "/");
            if path.is_dir() {
                // Служебные каталоги Python/сборки не считаются загрязнением outputs.
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name == "__pycache__" || name.starts_with(".rde") {
                    continue;
                }
                stack.push(path);
                continue;
            }
            if allowed.contains(&rel) {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.ends_with(".pyc") || name.starts_with(".rde-tmp") {
                continue;
            }
            extra.push(rel);
        }
    }
    extra.sort();
    extra
}

/// Корректное завершение: SIGTERM группе → grace-ожидание → SIGKILL (R-RUN-05).
fn terminate_tree(child: &mut std::process::Child, grace: Duration) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        if pid > 0 {
            // SAFETY: отрицательный pid — сигнал всей процессной группе.
            unsafe {
                libc::kill(-pid, libc::SIGTERM);
            }
        }
        let deadline = std::time::Instant::now() + grace;
        while std::time::Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                Err(_) => break,
            }
        }
    }
    kill_tree(child);
}

// ── логи (R-RUN-08): чтение с байтового смещения, поиск, ретенция ───────────

/// Кусок лога, прочитанный с байтового смещения (для переподключения к логу).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogChunk {
    pub bytes: Vec<u8>,
    pub next_offset: u64,
    pub eof: bool,
}

/// Прочитать лог начиная с `offset` (не более `max_bytes`; `eof` — прочитан до конца файла).
pub fn read_log_from(path: &Path, offset: u64, max_bytes: usize) -> std::io::Result<LogChunk> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = File::open(path)?;
    let len = file.metadata()?.len();
    if offset >= len {
        return Ok(LogChunk { bytes: Vec::new(), next_offset: len, eof: true });
    }
    file.seek(SeekFrom::Start(offset))?;
    let take = ((len - offset) as usize).min(max_bytes);
    let mut bytes = vec![0u8; take];
    let read = file.read(&mut bytes)?;
    bytes.truncate(read);
    let next_offset = offset + read as u64;
    Ok(LogChunk { bytes, next_offset, eof: next_offset >= len })
}

/// Найденная строка лога.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogHit {
    pub line_no: usize,
    pub offset: u64,
    pub text: String,
}

/// Поиск по логу (регистронезависимо), не более `max_hits` строк.
pub fn search_log(path: &Path, query: &str, max_hits: usize) -> std::io::Result<Vec<LogHit>> {
    let text = fs::read_to_string(path)?;
    let needle = query.to_lowercase();
    let mut hits = Vec::new();
    let mut offset = 0u64;
    for (line_no, line) in text.lines().enumerate() {
        if needle.is_empty() || line.to_lowercase().contains(&needle) {
            hits.push(LogHit { line_no: line_no + 1, offset, text: line.to_owned() });
            if hits.len() >= max_hits {
                break;
            }
        }
        offset += line.len() as u64 + 1;
    }
    Ok(hits)
}

/// Ретенция запусков: оставить `keep_last` последних каталогов `.rde/runs/`.
/// Возвращает число удалённых запусков.
pub fn prune_runs(project_root: &Path, keep_last: usize) -> std::io::Result<usize> {
    let root = runs_root(project_root);
    let mut dirs: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in fs::read_dir(&root)?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            let modified = entry.metadata().and_then(|meta| meta.modified()).unwrap_or(std::time::UNIX_EPOCH);
            dirs.push((modified, path));
        }
    }
    dirs.sort_by(|a, b| b.0.cmp(&a.0));
    let mut removed = 0;
    for (_, path) in dirs.into_iter().skip(keep_last) {
        if fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

fn spawn_error(
    spec: &RunSpec,
    run_id: &str,
    run_dir: &Path,
    work_dir: &Path,
    stdout_log: &Path,
    stderr_log: &Path,
    started_unix: u64,
    error: String,
) -> RunOutcome {
    let _ = fs::create_dir_all(run_dir);
    let _ = File::create(stdout_log);
    let _ = File::create(stderr_log);
    let run_details_path = run_dir.join("run_details.json");
    let details = RunDetails {
        run_id: run_id.to_owned(),
        artifact_id: spec.artifact_id.clone(),
        runtime: spec.runtime.clone(),
        entry: spec.entry.clone(),
        interpreter: spec.interpreter.clone().unwrap_or_else(|| spec.runtime.clone()),
        initiator: spec.initiator.clone(),
        platform: platform(),
        trusted: spec.trusted,
        network: if spec.network == NetworkPolicy::Deny { "deny".to_owned() } else { "allow".to_owned() },
        network_denied_best_effort: spec.network == NetworkPolicy::Deny,
        script_hash: spec.script_hash.clone(),
        env_lock_digest: spec.env_lock_digest.clone(),
        exit_code: None,
        status: RunStatus::SpawnError,
        duration_ms: 0,
        started_unix,
        finished_unix: now_unix(),
        stdout_log: stdout_log.display().to_string(),
        stderr_log: stderr_log.display().to_string(),
        stdout_digest: file_digest(stdout_log),
        stderr_digest: file_digest(stderr_log),
        work_dir: work_dir.display().to_string(),
        extra_files: Vec::new(),
    };
    if let Ok(json) = serde_json::to_vec_pretty(&details) {
        let _ = fs::write(&run_details_path, json);
    }
    RunOutcome {
        run_id: run_id.to_owned(),
        status: RunStatus::SpawnError,
        exit_code: None,
        duration_ms: 0,
        stdout_log: stdout_log.to_path_buf(),
        stderr_log: stderr_log.to_path_buf(),
        work_dir: work_dir.to_path_buf(),
        outputs: Vec::new(),
        extra_files: Vec::new(),
        network_denied: spec.network == NetworkPolicy::Deny,
        run_details_path,
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
