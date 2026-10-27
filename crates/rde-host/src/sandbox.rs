//! Песочница путей: всё, что приходит из протокола, обязано остаться внутри проекта (A5).
//!
//! Алгоритм (лексическая проверка + канонизация существующих префиксов + `starts_with`):
//! 1. отказ абсолютным путям, `file://`, UNC, Windows-дискам, `..` — `RDE_PATH_ESCAPE`;
//! 2. `fs::canonicalize` — симлинки раскрываются (симлинк наружу тоже отсекается);
//! 3. канонический путь обязан начинаться с канонического корня проекта;
//! 4. `.rde/**` закрыт для чтения инструментами (`RDE_APPROVAL_DENIED`): аудит/индекс —
//!    внутреннее состояние RDE, наружу не отдаётся (A5).
//!
//! Корень канонизируется один раз при старте хоста и наружу не печатается.

use std::path::{Component, Path, PathBuf};

use rde_mcp::HostError;

/// Проверка путей проекта.
#[derive(Clone, Debug)]
pub struct PathSandbox {
    root: PathBuf,
}

impl PathSandbox {
    /// Песочница над каноническим корнем проекта.
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Корень проекта (уже канонизирован).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Разрешить относительный путь внутри проекта; ошибки — `RDE_PATH_ESCAPE`/`RDE_NOT_FOUND`.
    pub fn resolve(&self, rel_path: &str) -> Result<PathBuf, HostError> {
        let rel_path = rel_path.trim();
        if rel_path.is_empty() {
            return Err(HostError::invalid_argument("путь не может быть пустым"));
        }
        if looks_like_escape(rel_path) {
            return Err(HostError::path_escape("абсолютные пути, file://, UNC и Windows-диски запрещены"));
        }
        let candidate = Path::new(rel_path);
        for component in candidate.components() {
            match component {
                Component::Normal(_) | Component::CurDir => {}
                Component::ParentDir => {
                    return Err(HostError::path_escape("`..` в путях проекта запрещён"));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(HostError::path_escape("путь должен быть относительным путём проекта"));
                }
            }
        }
        let joined = self.root.join(rel_path);
        // `.rde/**` закрыт даже если такого файла ещё нет (аудит создаётся лениво): не даём
        // клиенту ни читать, ни зондировать внутреннее состояние.
        if joined.strip_prefix(&self.root).ok().and_then(|relative| relative.components().next())
            .is_some_and(|first| first.as_os_str() == ".rde")
        {
            return Err(rde_denied());
        }
        let canonical = std::fs::canonicalize(&joined).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                HostError::not_found(format!("файл `{rel_path}` не найден в проекте"))
            } else {
                HostError::not_ready(format!("файл `{rel_path}` недоступен: {error}"))
            }
        })?;
        if !canonical.starts_with(&self.root) {
            return Err(HostError::path_escape(format!("путь `{rel_path}` выходит за корень проекта")));
        }
        let relative = canonical.strip_prefix(&self.root).map_err(|_| HostError::path_escape("путь вне проекта"))?;
        if relative.components().next().is_some_and(|first| first.as_os_str() == ".rde") {
            return Err(rde_denied());
        }
        Ok(canonical)
    }

    /// Относительный POSIX-путь внутри проекта для ответа клиенту.
    pub fn rel_posix(&self, path: &Path) -> String {
        let relative = path.strip_prefix(&self.root).unwrap_or(path);
        let mut out = String::new();
        for component in relative.components() {
            if !out.is_empty() {
                out.push('/');
            }
            out.push_str(&component.as_os_str().to_string_lossy());
        }
        out
    }
}

fn rde_denied() -> HostError {
    HostError::approval_denied("каталог `.rde` закрыт для чтения инструментами").with_hint(
        "внутреннее состояние RDE (store/аудит) не отдаётся внешним клиентам; читайте объекты проекта",
    )
}

fn looks_like_escape(path: &str) -> bool {
    path.starts_with('/')
        || path.starts_with('\\')
        || path.contains("://")
        || path.starts_with("file:")
        || (path.len() > 1 && path.as_bytes()[1] == b':')
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn project_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-host-sandbox-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("objects")).unwrap();
        std::fs::write(dir.join("rde.toml"), "schema = \"rde/project@1\"\n").unwrap();
        std::fs::create_dir_all(dir.join(".rde").join("agent")).unwrap();
        std::fs::write(dir.join(".rde").join("agent").join("audit.log"), "{}\n").unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn resolves_existing_file_inside_project() {
        let root = project_dir("ok");
        let sandbox = PathSandbox::new(root.clone());
        let path = sandbox.resolve("rde.toml").unwrap();
        assert_eq!(path, root.join("rde.toml"));
        assert_eq!(sandbox.rel_posix(&path), "rde.toml");
        assert_eq!(sandbox.rel_posix(&root.join("objects").join("x.toml")), "objects/x.toml");
    }

    #[test]
    fn rejects_parent_dir_absolute_and_urls() {
        let root = project_dir("escape");
        let sandbox = PathSandbox::new(root);
        for path in ["../etc/passwd", "/etc/passwd", "objects/../../x", "file:///etc/passwd", "\\\\server\\share", "C:\\Windows\\win.ini"] {
            let error = sandbox.resolve(path).expect_err(path);
            assert_eq!(error.code, rde_mcp::RdeErrorCode::PathEscape, "{path}: {error}");
        }
    }

    #[test]
    fn rejects_missing_file_as_not_found() {
        let root = project_dir("missing");
        let sandbox = PathSandbox::new(root);
        let error = sandbox.resolve("nope.toml").expect_err("нет файла");
        assert_eq!(error.code, rde_mcp::RdeErrorCode::NotFound);
    }

    #[test]
    fn rejects_rde_internal_state() {
        let root = project_dir("rde-dir");
        let sandbox = PathSandbox::new(root);
        let error = sandbox.resolve(".rde/agent/audit.log").expect_err(".rde закрыт");
        assert_eq!(error.code, rde_mcp::RdeErrorCode::ApprovalDenied);
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlink_escaping_the_project() {
        let root = project_dir("symlink");
        let outside = std::env::temp_dir().join(format!("rde-host-outside-{}.txt", std::process::id()));
        std::fs::write(&outside, "secret").unwrap();
        let link = root.join("escape-link");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&outside, &link).unwrap();
        let sandbox = PathSandbox::new(root);
        let error = sandbox.resolve("escape-link").expect_err("симлинк наружу запрещён");
        assert_eq!(error.code, rde_mcp::RdeErrorCode::PathEscape);
        let _ = std::fs::remove_file(&outside);
    }
}
