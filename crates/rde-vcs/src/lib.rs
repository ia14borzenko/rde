//! RDE git-интеграция (P5, R-VCS-01/02): init/status/commit/revert/log/diff и генерация
//! служебных файлов проекта (`.gitignore`, `.gitattributes`).
//!
//! Git — единственный владелец операций с репозиторием (ADR-0002): UI/агент/MCP вызывают этот
//! слой, а не дёргают git сами. Выходы артефактов не коммитятся (`.gitignore`), их идентичность
//! фиксируется `out_hash` в `artifact.toml` (R-VCS-02); `.doc.rde` — бинарный пакет, поэтому в
//! `.gitattributes` он помечен `binary` (сравнение версий — через rde:doc/diff, не git).
//!
//! Используется CLI `git` (системный): RDE не реализует git-формат сам. Если git не установлен,
//! все операции возвращают понятную ошибку, а UI показывает «git недоступен».

use std::path::{Path, PathBuf};
use std::process::Command;

/// Ошибка git-слоя.
#[derive(Debug)]
pub struct VcsError(pub String);

impl std::fmt::Display for VcsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for VcsError {}

pub type Result<T> = std::result::Result<T, VcsError>;

fn error(message: impl Into<String>) -> VcsError {
    VcsError(message.into())
}

/// Запись истории git.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitEntry {
    pub id: String,
    pub summary: String,
    pub author: String,
    pub time: String,
}

/// Состояние репозитория проекта.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GitStatus {
    /// git установлен и команда доступна.
    pub available: bool,
    /// Каталог проекта находится внутри git-репозитория.
    pub is_repo: bool,
    /// Ветка (или `HEAD` в detached-состоянии).
    pub branch: String,
    /// Корень найденного репозитория (может быть выше каталога проекта).
    pub repo_root: PathBuf,
    /// Изменённые/удалённые файлы проекта (относительно корня проекта).
    pub changed: Vec<String>,
    /// Новые (неотслеживаемые) файлы проекта.
    pub untracked: Vec<String>,
    pub last_commit: Option<CommitEntry>,
    /// Сколько коммитов локально впереди upstream (если upstream настроен).
    pub ahead: Option<u32>,
}

impl GitStatus {
    /// Есть ли незакоммиченные изменения (включая новые файлы).
    pub fn is_dirty(&self) -> bool {
        !self.changed.is_empty() || !self.untracked.is_empty()
    }

    /// Короткая подпись для статус-бара.
    pub fn summary_ru(&self) -> String {
        if !self.available {
            return "git: недоступен".to_owned();
        }
        if !self.is_repo {
            return "git: не инициализирован".to_owned();
        }
        if self.is_dirty() {
            format!("{} · изменений: {}", self.branch, self.changed.len() + self.untracked.len())
        } else {
            format!("{} · чисто", self.branch)
        }
    }
}

/// Результат commit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitResult {
    pub id: String,
    pub summary: String,
    /// Файлов в коммите.
    pub files: usize,
}

/// Доступен ли `git` в системе.
pub fn git_available() -> bool {
    Command::new("git").arg("--version").output().map(|output| output.status.success()).unwrap_or(false)
}

fn run(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| error(format!("git не запускается: {e} (установите git)")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
        return Err(error(format!("git {}: {stderr}", args.join(" "))));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Содержимое `.gitignore` проекта RDE (выходы/локальное состояние не коммитятся).
pub const PROJECT_GITIGNORE: &str = "# Локальное состояние RDE (CAS, журнал, корни, запуски)\n/.rde/\n# Экспорты и кэши\n/exports/\n# Выходы артефактов (идентичность — out_hash в artifact.toml, R-VCS-02)\nobjects/*/*/out/\n__pycache__/\n.venv/\nuv.lock\n";

/// Содержимое `.gitattributes`: пакеты документов — бинарные.
pub const PROJECT_GITATTRIBUTES: &str = "# RDE: пакеты документов бинарные (сравнение — через rde:doc/diff)\n*.doc.rde binary\n*.docx binary\n*.rde.zip binary\n";

/// Создать недостающие служебные файлы проекта (`.gitignore`, `.gitattributes`).
/// Существующие файлы не перезаписываются. Возвращает список созданных файлов.
pub fn ensure_project_files(root: &Path) -> Result<Vec<String>> {
    let mut created = Vec::new();
    let gitignore = root.join(".gitignore");
    if !gitignore.is_file() {
        std::fs::write(&gitignore, PROJECT_GITIGNORE).map_err(|e| error(format!("записать .gitignore: {e}")))?;
        created.push(".gitignore".to_owned());
    }
    let gitattributes = root.join(".gitattributes");
    if !gitattributes.is_file() {
        std::fs::write(&gitattributes, PROJECT_GITATTRIBUTES)
            .map_err(|e| error(format!("записать .gitattributes: {e}")))?;
        created.push(".gitattributes".to_owned());
    }
    Ok(created)
}

/// Инициализировать репозиторий проекта, если его ещё нет. Возвращает `true`, если создан.
pub fn init(root: &Path) -> Result<bool> {
    if !git_available() {
        return Err(error("git недоступен: установите git или отключите VCS-функции"));
    }
    ensure_project_files(root)?;
    let status = status(root)?;
    if status.is_repo {
        return Ok(false);
    }
    run(root, &["init", "-q"])?;
    // Начальный коммит: манифесты/служебные файлы проекта.
    let _ = run(root, &["add", "-A", "."])?;
    let _ = run(
        root,
        &[
            "-c",
            "user.name=RDE",
            "-c",
            "user.email=rde@localhost",
            "commit",
            "-q",
            "-m",
            "project init (RDE)",
        ],
    )?;
    Ok(true)
}

fn parse_log_line(line: &str) -> Option<CommitEntry> {
    // Формат: <hash>|<summary>|<author>|<time>
    let mut parts = line.splitn(4, '|');
    let id = parts.next()?.trim().to_owned();
    let summary = parts.next()?.trim().to_owned();
    let author = parts.next()?.trim().to_owned();
    let time = parts.next().unwrap_or("").trim().to_owned();
    if id.is_empty() {
        return None;
    }
    Some(CommitEntry { id, summary, author, time })
}

/// Состояние git-репозитория проекта.
pub fn status(root: &Path) -> Result<GitStatus> {
    let mut status = GitStatus::default();
    if !git_available() {
        return Ok(status);
    }
    status.available = true;
    let inside = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(root)
        .output()
        .map(|output| output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true")
        .unwrap_or(false);
    if !inside {
        return Ok(status);
    }
    status.is_repo = true;
    status.repo_root = run(root, &["rev-parse", "--show-toplevel"])?.trim().into();
    status.branch = run(root, &["rev-parse", "--abbrev-ref", "HEAD"]).map(|value| value.trim().to_owned()).unwrap_or_else(|_| "HEAD".to_owned());
    // Изменения только по каталогу проекта.
    if let Ok(porcelain) = run(root, &["status", "--porcelain", "--", "."]) {
        for line in porcelain.lines() {
            let code = line.get(0..2).unwrap_or("  ").to_owned();
            let path = line.get(3..).unwrap_or("").trim().trim_matches('"').to_owned();
            if path.is_empty() {
                continue;
            }
            if code.starts_with("??") {
                status.untracked.push(path);
            } else {
                status.changed.push(path);
            }
        }
    }
    if let Ok(log) = run(root, &["log", "-1", "--pretty=format:%h|%s|%an|%ad", "--date=short"]) {
        status.last_commit = parse_log_line(log.trim());
    }
    if let Ok(counts) = run(root, &["rev-list", "--left-right", "--count", "HEAD...@{upstream}"]) {
        let ahead = counts.split_whitespace().next().and_then(|value| value.parse::<u32>().ok());
        status.ahead = ahead;
    }
    Ok(status)
}

/// История коммитов (новые первыми).
pub fn log(root: &Path, limit: usize) -> Result<Vec<CommitEntry>> {
    if !git_available() {
        return Err(error("git недоступен"));
    }
    let count = format!("-{limit}");
    let output = run(root, &["log", &count, "--pretty=format:%h|%s|%an|%ad", "--date=short"])?;
    Ok(output.lines().filter_map(parse_log_line).collect())
}

/// Закоммитить изменения проекта. Возвращает результат или ошибку, если коммитить нечего.
pub fn commit(root: &Path, message: &str, author_name: &str, author_email: &str) -> Result<CommitResult> {
    if message.trim().is_empty() {
        return Err(error("сообщение коммита не может быть пустым"));
    }
    let status = status(root)?;
    if !status.available {
        return Err(error("git недоступен"));
    }
    if !status.is_repo {
        return Err(error("каталог проекта не является git-репозиторием: сначала «git init»"));
    }
    ensure_project_files(root)?;
    run(root, &["add", "-A", "."])?;
    let staged = run(root, &["diff", "--cached", "--name-only"])?;
    let files = staged.lines().filter(|line| !line.trim().is_empty()).count();
    if files == 0 {
        return Err(error("нечего коммитить: нет изменений"));
    }
    let name = if author_name.trim().is_empty() { "RDE" } else { author_name.trim() };
    let email = if author_email.trim().is_empty() { "rde@localhost" } else { author_email.trim() };
    run(
        root,
        &[
            "-c",
            &format!("user.name={name}"),
            "-c",
            &format!("user.email={email}"),
            "commit",
            "-q",
            "-m",
            message,
        ],
    )?;
    let last = run(root, &["log", "-1", "--pretty=format:%h|%s|%an|%ad", "--date=short"])?;
    let entry = parse_log_line(last.trim()).ok_or_else(|| error("git не вернул последний коммит"))?;
    Ok(CommitResult { id: entry.id, summary: entry.summary, files })
}

/// Отменить незакоммиченные правки одного файла (восстановить из HEAD).
pub fn revert_file(root: &Path, path: &str) -> Result<()> {
    if path.trim().is_empty() {
        return Err(error("пустой путь файла"));
    }
    run(root, &["restore", "--source=HEAD", "--", path.trim()])
        .map(|_| ())
        .map_err(|e| error(format!("git restore {path}: {e}")))
}

/// Сводка diff незакоммиченных изменений (`git diff --stat` + новые файлы).
pub fn diff_stat(root: &Path) -> Result<String> {
    let status = status(root)?;
    if !status.is_repo {
        return Err(error("не git-репозиторий"));
    }
    let mut text = run(root, &["diff", "--stat", "--", "."])?;
    if !status.untracked.is_empty() {
        if !text.trim().is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str(&format!("новые файлы: {}", status.untracked.join(", ")));
    }
    Ok(text.trim_end().to_owned())
}

/// Проверка рассинхрона для «Проверить и починить» (R-SET-03): человекочитаемая строка.
pub fn check_sync(root: &Path) -> Result<String> {
    let status = status(root)?;
    if !status.available {
        return Ok("git недоступен (VCS-функции выключены)".to_owned());
    }
    if !status.is_repo {
        return Ok("проект не в git: история версий не ведётся".to_owned());
    }
    let mut lines = vec![format!("git: ветка {}, репозиторий {}", status.branch, status.repo_root.display())];
    if let Some(last) = &status.last_commit {
        lines.push(format!("последний коммит: {} {} ({})", last.id, last.summary, last.time));
    }
    if status.is_dirty() {
        lines.push(format!("незакоммиченные изменения: {} (изменено {}, новых {})", status.changed.len() + status.untracked.len(), status.changed.len(), status.untracked.len()));
    } else {
        lines.push("незакоммиченных изменений нет".to_owned());
    }
    if let Some(ahead) = status.ahead {
        if ahead > 0 {
            lines.push(format!("локально впереди upstream на {ahead} коммитов"));
        }
    }
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-vcs-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn git_available_or_skip() -> bool {
        if git_available() {
            true
        } else {
            eprintln!("git недоступен — тест пропущен");
            false
        }
    }

    #[test]
    fn project_files_are_generated_once() {
        let root = tmp("files");
        let created = ensure_project_files(&root).unwrap();
        assert_eq!(created, vec![".gitignore".to_owned(), ".gitattributes".to_owned()]);
        let ignore = std::fs::read_to_string(root.join(".gitignore")).unwrap();
        assert!(ignore.contains("/.rde/") && ignore.contains("objects/*/*/out/"), "{ignore}");
        let attributes = std::fs::read_to_string(root.join(".gitattributes")).unwrap();
        assert!(attributes.contains("*.doc.rde binary"), "{attributes}");
        // Повторный вызов ничего не перезаписывает.
        assert!(ensure_project_files(&root).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn init_status_commit_log_revert_roundtrip() {
        if !git_available_or_skip() {
            return;
        }
        let root = tmp("repo");
        std::fs::write(root.join("rde.toml"), "schema = \"rde/project@1\"\n").unwrap();
        let before = status(&root).unwrap();
        assert!(before.available);
        assert!(!before.is_repo);
        assert!(init(&root).unwrap(), "git init создаёт репозиторий");
        assert!(!init(&root).unwrap(), "повторный init ничего не делает");
        let current = status(&root).unwrap();
        assert!(current.is_repo);
        assert!(!current.branch.is_empty());
        assert!(current.last_commit.is_some(), "начальный коммит создан");

        // Новая правка → dirty; commit → clean; log содержит запись.
        std::fs::write(root.join("note.txt"), "v1").unwrap();
        let dirty = status(&root).unwrap();
        assert!(dirty.is_dirty());
        assert!(dirty.untracked.contains(&"note.txt".to_owned()), "{dirty:?}");
        let result = commit(&root, "add note", "Илья", "ilunix@pc.ubuntu").unwrap();
        assert!(!result.id.is_empty());
        assert_eq!(result.files, 1, "{result:?}");
        assert!(!status(&root).unwrap().is_dirty());
        let history = log(&root, 10).unwrap();
        assert_eq!(history[0].summary, "add note");
        assert_eq!(history[0].author, "Илья");

        // revert_file возвращает содержимое из HEAD.
        std::fs::write(root.join("note.txt"), "v2").unwrap();
        assert_eq!(diff_stat(&root).unwrap().contains("note.txt"), true);
        revert_file(&root, "note.txt").unwrap();
        assert_eq!(std::fs::read_to_string(root.join("note.txt")).unwrap(), "v1");
        assert!(!status(&root).unwrap().is_dirty());
        // check_sync — человекочитаемый отчёт.
        let sync = check_sync(&root).unwrap();
        assert!(sync.contains("git:"), "{sync}");
        // Пустой коммит — ошибка.
        assert!(commit(&root, "empty", "", "").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }
}
