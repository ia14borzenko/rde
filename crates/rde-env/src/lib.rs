//! RDE execution environments (P3): окружения артефактов и `rde.lock` (tz-02 §5.4, R-RUN-02/07).
//!
//! Задача слоя — детерминированно описать окружение запуска и дать digest, который входит в
//! `env_hash` (rde-hash §7.2). Окружения управляются внешними менеджерами: `uv` для Python
//! (per-project venv + `uv.lock`), `pnpm` для Node, `pixi` для R; RDE их не реализует сам
//! (ADR-0002), а разрешает и фиксирует.
//!
//! `rde.lock` (TOML в корне проекта) — запись разрешённых окружений: версия интерпретатора,
//! lock-файл зависимостей и его digest. Он коммитится в git и показывает, в каком окружении
//! получены выходы; `env_hash` считается из **фактического** окружения, а расхождение с
//! `rde.lock` — это «изменилось окружение» в статусе артефакта.
//!
//! Режимы (`runtime.env`):
//! - `project` — управляемое окружение проекта (Python: `uv venv .venv`, lock-файл `uv.lock`);
//! - `system` — системный интерпретатор (выбор пользователя, R-RUN-02);
//! - `artifact` — окружение артефакта (пока как `project`; отдельные venv — позже).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Имя файла записи окружений в корне проекта.
pub const RDE_LOCK_FILE: &str = "rde.lock";
/// Схема `rde.lock`.
pub const SCHEMA_LOCK: &str = "rde/lock@1";

/// Запись окружения одного runtime.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeLock {
    /// `python` | `node` | `r` | `shell`.
    pub runtime: String,
    /// `project` | `system` | `artifact`.
    pub mode: String,
    /// Исполняемый файл (путь внутри проекта или имя в PATH).
    pub interpreter: String,
    /// Версия интерпретатора (`3.14.4`, `v22.1.0`, …).
    #[serde(default)]
    pub version: String,
    /// Менеджер окружения (`uv`, `pnpm`, `pixi`).
    #[serde(default)]
    pub manager: Option<String>,
    #[serde(default)]
    pub manager_version: Option<String>,
    /// Lock-файл зависимостей относительно корня проекта.
    #[serde(default)]
    pub lock_file: Option<String>,
    /// sha256 содержимого lock-файла.
    #[serde(default)]
    pub lock_digest: Option<String>,
    /// Платформа, на которой разрешено окружение.
    #[serde(default)]
    pub platform: String,
    /// Digest записи (вход `env_hash`).
    #[serde(default)]
    pub digest: String,
}

impl RuntimeLock {
    /// Пересчитать `digest` записи (канонический JSON без самого digest-поля).
    pub fn refresh_digest(&mut self) -> Result<String, String> {
        self.digest = compute_digest(self)?;
        Ok(self.digest.clone())
    }
}

/// `rde.lock` — запись окружений проекта.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvLock {
    pub schema: String,
    #[serde(default)]
    pub updated_unix: u64,
    /// runtime → запись (например, `python`).
    #[serde(default)]
    pub envs: BTreeMap<String, RuntimeLock>,
}

impl Default for EnvLock {
    fn default() -> Self {
        Self { schema: SCHEMA_LOCK.to_owned(), updated_unix: 0, envs: BTreeMap::new() }
    }
}

/// Платформенная строка (OS-ARCH); RDE может переопределить через `RDE_PLATFORM`.
pub fn platform() -> String {
    std::env::var("RDE_PLATFORM").unwrap_or_else(|_| format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH))
}

/// Digest записи окружения: sha256 канонического JSON (JCS) без поля `digest`.
pub fn compute_digest(lock: &RuntimeLock) -> Result<String, String> {
    let mut value = serde_json::to_value(lock).map_err(|e| format!("json окружения: {e}"))?;
    if let Some(object) = value.as_object_mut() {
        object.remove("digest");
    }
    let canonical = rde_hash::canonical_json(&value)?;
    Ok(rde_hash::sha256(canonical.as_bytes()).hex())
}

/// Разрешить окружение для runtime артефакта (без изменения проекта).
pub fn resolve(project_root: &Path, runtime: &str, mode: &str) -> Result<RuntimeLock, String> {
    let mode = if mode.trim().is_empty() { "project" } else { mode.trim() };
    // Абсолютный путь: команды `uv`/интерпретатор запускаются с cwd = корень проекта.
    let project_root = project_root.canonicalize().unwrap_or_else(|_| project_root.to_path_buf());
    let project_root = project_root.as_path();
    let mut lock = match runtime {
        "python" | "python3" => resolve_python(project_root, mode)?,
        "node" => resolve_node(project_root, mode)?,
        "r" => resolve_r(project_root, mode)?,
        "shell" | "sh" | "bash" => resolve_shell()?,
        other => return Err(format!("runtime `{other}` пока не поддерживается (python/shell/node/r)")),
    };
    lock.platform = platform();
    lock.refresh_digest()?;
    Ok(lock)
}

fn resolve_python(project_root: &Path, mode: &str) -> Result<RuntimeLock, String> {
    let mut lock = RuntimeLock {
        runtime: "python".to_owned(),
        mode: mode.to_owned(),
        interpreter: String::new(),
        version: String::new(),
        manager: None,
        manager_version: None,
        lock_file: None,
        lock_digest: None,
        platform: String::new(),
        digest: String::new(),
    };
    if mode != "system" {
        // Управляемое окружение: uv venv в проекте (idempotent, без сети).
        if let Some(uv) = find_tool("uv") {
            let venv = project_root.join(".venv");
            if !venv.join("bin/python").is_file() {
                let status = Command::new(&uv)
                    .args(["venv", "--quiet"])
                    .arg(&venv)
                    .current_dir(project_root)
                    .status()
                    .map_err(|e| format!("uv venv: {e}"))?;
                if !status.success() {
                    return Err(format!("uv venv завершился с кодом {:?}", status.code()));
                }
            }
            lock.interpreter = ".venv/bin/python".to_owned();
            lock.manager = Some("uv".to_owned());
            lock.manager_version = tool_version(&uv);
        }
    }
    if lock.interpreter.is_empty() {
        // Системный интерпретатор (или fallback, если uv недоступен).
        lock.mode = if mode == "system" { "system".to_owned() } else { "system".to_owned() };
        lock.interpreter = std::env::var("RDE_PYTHON").unwrap_or_else(|_| "python3".to_owned());
    }
    let program = interpreter_path(project_root, &lock.interpreter);
    lock.version = tool_version(&program).unwrap_or_default();
    // Lock-файл зависимостей: uv.lock (Python), если есть.
    let uv_lock = project_root.join("uv.lock");
    if uv_lock.is_file() {
        lock.lock_file = Some("uv.lock".to_owned());
        lock.lock_digest = std::fs::read(&uv_lock).ok().map(|bytes| rde_hash::sha256(&bytes).hex());
    }
    if lock.version.is_empty() {
        return Err(format!(
            "не удалось определить версию интерпретатора `{}` (проверьте Python/uv)",
            lock.interpreter
        ));
    }
    Ok(lock)
}

fn resolve_node(project_root: &Path, mode: &str) -> Result<RuntimeLock, String> {
    let mut lock = RuntimeLock {
        runtime: "node".to_owned(),
        mode: mode.to_owned(),
        interpreter: "node".to_owned(),
        version: tool_version(Path::new("node")).unwrap_or_default(),
        manager: None,
        manager_version: None,
        lock_file: None,
        lock_digest: None,
        platform: String::new(),
        digest: String::new(),
    };
    if let Some(pnpm) = find_tool("pnpm") {
        lock.manager = Some("pnpm".to_owned());
        lock.manager_version = tool_version(&pnpm);
    }
    let lock_file = project_root.join("pnpm-lock.yaml");
    if lock_file.is_file() {
        lock.lock_file = Some("pnpm-lock.yaml".to_owned());
        lock.lock_digest = std::fs::read(&lock_file).ok().map(|bytes| rde_hash::sha256(&bytes).hex());
    }
    if lock.version.is_empty() {
        return Err("node не найден в PATH (выберите системный интерпретатор или установите компонент)".to_owned());
    }
    Ok(lock)
}

fn resolve_r(project_root: &Path, mode: &str) -> Result<RuntimeLock, String> {
    let mut lock = RuntimeLock {
        runtime: "r".to_owned(),
        mode: mode.to_owned(),
        interpreter: "Rscript".to_owned(),
        version: tool_version(Path::new("Rscript")).unwrap_or_default(),
        manager: None,
        manager_version: None,
        lock_file: None,
        lock_digest: None,
        platform: String::new(),
        digest: String::new(),
    };
    if let Some(pixi) = find_tool("pixi") {
        lock.manager = Some("pixi".to_owned());
        lock.manager_version = tool_version(&pixi);
    }
    let lock_file = project_root.join("pixi.lock");
    if lock_file.is_file() {
        lock.lock_file = Some("pixi.lock".to_owned());
        lock.lock_digest = std::fs::read(&lock_file).ok().map(|bytes| rde_hash::sha256(&bytes).hex());
    }
    if lock.version.is_empty() {
        return Err("Rscript не найден в PATH (R — опциональный компонент, R-RUN-01)".to_owned());
    }
    Ok(lock)
}

fn resolve_shell() -> Result<RuntimeLock, String> {
    let mut lock = RuntimeLock {
        runtime: "shell".to_owned(),
        mode: "system".to_owned(),
        interpreter: "sh".to_owned(),
        version: String::new(),
        manager: None,
        manager_version: None,
        lock_file: None,
        lock_digest: None,
        platform: String::new(),
        digest: String::new(),
    };
    // У `sh` обычно нет `--version`; фиксируем путь и платформу — этого достаточно для v1.
    lock.version = which("sh").unwrap_or_else(|| "sh".to_owned());
    Ok(lock)
}

fn interpreter_path(project_root: &Path, interpreter: &str) -> PathBuf {
    let path = Path::new(interpreter);
    if path.is_absolute() || interpreter.contains('/') {
        project_root.join(interpreter)
    } else {
        path.to_path_buf()
    }
}

/// Найти инструмент в PATH (возвращает полный путь).
pub fn find_tool(name: &str) -> Option<PathBuf> {
    which(name).map(PathBuf::from)
}

fn which(name: &str) -> Option<String> {
    let output = Command::new("sh").args(["-c", &format!("command -v {name}")]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if path.is_empty() {
        None
    } else {
        Some(path)
    }
}

/// Версия инструмента: `--version` (первая строка, первое «похожее на версию» слово).
pub fn tool_version(program: &Path) -> Option<String> {
    let output = Command::new(program).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let line = text.lines().next().unwrap_or("").trim();
    let word = line
        .split_whitespace()
        .find(|word| word.chars().next().is_some_and(|ch| ch.is_ascii_digit()))
        .unwrap_or(line);
    Some(word.trim_start_matches('v').to_owned())
}

/// Прочитать `rde.lock` проекта (None — файла нет).
pub fn load(project_root: &Path) -> Option<EnvLock> {
    let path = project_root.join(RDE_LOCK_FILE);
    let text = std::fs::read_to_string(path).ok()?;
    toml::from_str(&text).ok()
}

/// Записать `rde.lock` проекта.
pub fn save(project_root: &Path, lock: &EnvLock) -> Result<(), String> {
    let path = project_root.join(RDE_LOCK_FILE);
    let text = toml::to_string_pretty(lock).map_err(|e| format!("rde.lock: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("записать {}: {e}", path.display()))
}

/// Обновить запись окружения в `rde.lock` (после успешного запуска). `true` — файл изменился.
pub fn update_entry(project_root: &Path, entry: &RuntimeLock) -> Result<bool, String> {
    let mut lock = load(project_root).unwrap_or_default();
    if lock.schema != SCHEMA_LOCK {
        lock.schema = SCHEMA_LOCK.to_owned();
    }
    let previous = lock.envs.get(&entry.runtime).cloned();
    if previous.as_ref() == Some(entry) {
        return Ok(false);
    }
    lock.envs.insert(entry.runtime.clone(), entry.clone());
    lock.updated_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    save(project_root, &lock)?;
    Ok(true)
}

/// Записанный в `rde.lock` digest окружения runtime (для сравнения/диагностики).
pub fn recorded_digest(project_root: &Path, runtime: &str) -> Option<String> {
    load(project_root)?.envs.get(runtime).map(|entry| entry.digest.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-env-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn system_python_resolves_with_version_and_stable_digest() {
        let root = tmp("python");
        let lock = resolve(&root, "python", "system").unwrap();
        assert_eq!(lock.runtime, "python");
        assert_eq!(lock.mode, "system");
        assert!(!lock.version.is_empty(), "версия python определена: {lock:?}");
        assert_eq!(lock.digest.len(), 64);
        let again = resolve(&root, "python", "system").unwrap();
        assert_eq!(lock, again, "digest окружения детерминирован");
        assert_eq!(lock.platform, platform());
        // Смена режима меняет digest (вход env_hash).
        let project_mode = resolve(&root, "python", "project").unwrap();
        assert_ne!(lock.digest, project_mode.digest, "mode входит в env_hash");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn shell_and_unknown_runtime() {
        let root = tmp("shell");
        let shell = resolve(&root, "shell", "system").unwrap();
        assert_eq!(shell.runtime, "shell");
        assert!(!shell.version.is_empty());
        assert!(resolve(&root, "cobol", "project").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn rde_lock_roundtrip_and_change_detection() {
        let root = tmp("lock");
        let entry = resolve(&root, "python", "system").unwrap();
        assert!(update_entry(&root, &entry).unwrap(), "первая запись создаёт rde.lock");
        assert!(!update_entry(&root, &entry).unwrap(), "повтор не меняет файл");
        let loaded = load(&root).unwrap();
        assert_eq!(loaded.schema, SCHEMA_LOCK);
        assert_eq!(loaded.envs.get("python"), Some(&entry));
        assert_eq!(recorded_digest(&root, "python"), Some(entry.digest.clone()));
        // Изменение записи (например, версия) обновляет файл.
        let mut changed = entry.clone();
        changed.version = "0.0.0-test".to_owned();
        changed.refresh_digest().unwrap();
        assert!(update_entry(&root, &changed).unwrap());
        assert_ne!(recorded_digest(&root, "python"), Some(entry.digest));
        // rde.lock — валидный TOML с человекочитаемыми полями.
        let text = std::fs::read_to_string(root.join(RDE_LOCK_FILE)).unwrap();
        assert!(text.contains("schema = \"rde/lock@1\""), "{text}");
        assert!(text.contains("[envs.python]"), "{text}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn digest_ignores_digest_field_and_is_jcs_stable() {
        let root = tmp("digest");
        let mut entry = resolve(&root, "shell", "system").unwrap();
        let original = entry.digest.clone();
        entry.digest = "deadbeef".to_owned();
        assert_eq!(compute_digest(&entry).unwrap(), original, "digest не входит в собственный digest");
        let _ = std::fs::remove_dir_all(&root);
    }
}
