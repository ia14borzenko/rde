//! Настройки приложения (R-SET-01/04): отдельный от проекта конфиг пользователя.
//!
//! Уровни по ТЗ §5.14: **приложение** (темы, язык, лимиты, автор по умолчанию, MCP/агенты) и
//! **проект** (`rde.toml`: имя, шаблон, авторы, дефолтные режимы — переносится с проектом).
//! Здесь — только app-level настройки; они не попадают в git проекта.
//!
//! Расположение: `$RDE_CONFIG_DIR/settings.toml`, иначе `$XDG_CONFIG_HOME/rde/settings.toml`,
//! иначе `~/.config/rde/settings.toml`; в portable-режиме — `.rde/app-settings.toml` рядом с
//! проектом (если задан `RDE_PORTABLE=1`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Схема файла настроек.
pub const SETTINGS_SCHEMA: &str = "rde/settings@1";

/// Настройки приложения.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppSettings {
    pub schema: String,
    /// Язык интерфейса: `ru` | `en` (R-UI-06, каркас i18n).
    #[serde(default = "default_language")]
    pub language: String,
    /// Тема: `dark` | `light` (пока поддерживается dark).
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Ретенция запусков (логи/provenance), R-RUN-08.
    #[serde(default = "default_keep_runs")]
    pub keep_runs: usize,
    /// Окно отката: сколько последних транзакций держать (R-ART-05).
    #[serde(default = "default_keep_txs")]
    pub keep_rollback_txs: usize,
    /// Автор по умолчанию (для git-коммитов и комментариев): имя/email.
    #[serde(default)]
    pub author_name: String,
    #[serde(default)]
    pub author_email: String,
    /// Показывать ли служебные объекты (R-TREE-01).
    #[serde(default)]
    pub show_service_objects: bool,
    /// Разрешить внешним агентам подключаться к RDE по MCP streamable HTTP (M4.1, R-MCP-02):
    /// по умолчанию выключено; сервер поднимается только на loopback с bearer-токеном.
    #[serde(default)]
    pub allow_external_mcp: bool,
    /// Разрешить внешним агентам **изменять проект** через MCP — каждое действие подтверждается
    /// диалогом GUI (M4.2, `ApprovalHook`), с чекпоинтом и аудитом. По умолчанию выключено:
    /// без него доступ только на чтение (fail-closed, R-MCP-05).
    #[serde(default)]
    pub allow_agent_mutations: bool,
    /// Разрешить внешним агентам **запускать скрипты проекта** (`artifact_run`/`run_cancel`,
    /// M4.2b): каждое действие — через GUI-диалог; действует только для `trusted`-проекта
    /// (R-RUN-06, эффективный грант вычисляется при старте сервера). По умолчанию выключено.
    #[serde(default)]
    pub allow_agent_run_script: bool,
    /// Разрешить внешним агентам **git-коммиты** (`git_commit`, M4.2b): каждое действие — через
    /// GUI-диалог с сообщением/автором; `.rde/**` не коммитится. По умолчанию выключено.
    #[serde(default)]
    pub allow_agent_git_commit: bool,
    /// M4.3 (R-AGT-01): разрешить **встроенного ACP-агента** (чат в панели «Агент»).
    /// По умолчанию выключено (fail-closed): процесс агента стартует только по явному «Новый чат».
    #[serde(default)]
    pub allow_builtin_agent: bool,
    /// Команда ACP-агента (по умолчанию `dsh`; допустим любой ACP-совместимый агент).
    #[serde(default = "default_agent_program")]
    pub agent_program: String,
    /// Аргументы команды агента (для DSH — `--profile acp`).
    #[serde(default = "default_agent_args")]
    pub agent_args: Vec<String>,
    /// Предпочтительная модель агента (`configId = model`); пусто — из `configOptions` DSH.
    /// Провайдер/ключи RDE **не дублирует** — они живут в профиле агента (R-AGT-01).
    #[serde(default)]
    pub agent_model: String,
    /// Предпочтительный уровень размышлений (`configId = reasoning_effort`); пусто — из DSH.
    #[serde(default)]
    pub agent_reasoning_effort: String,
}

fn default_language() -> String {
    "ru".to_owned()
}
fn default_theme() -> String {
    "dark".to_owned()
}
fn default_keep_runs() -> usize {
    200
}
fn default_keep_txs() -> usize {
    20
}
fn default_agent_program() -> String {
    "dsh".to_owned()
}
fn default_agent_args() -> Vec<String> {
    vec!["--profile".to_owned(), "acp".to_owned()]
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            schema: SETTINGS_SCHEMA.to_owned(),
            language: default_language(),
            theme: default_theme(),
            keep_runs: default_keep_runs(),
            keep_rollback_txs: default_keep_txs(),
            author_name: String::new(),
            author_email: String::new(),
            show_service_objects: false,
            allow_external_mcp: false,
            allow_agent_mutations: false,
            allow_agent_run_script: false,
            allow_agent_git_commit: false,
            allow_builtin_agent: false,
            agent_program: default_agent_program(),
            agent_args: default_agent_args(),
            agent_model: String::new(),
            agent_reasoning_effort: String::new(),
        }
    }
}

impl AppSettings {
    /// Проверить/нормализовать значения (язык, тема, лимиты).
    pub fn normalize(&mut self) {
        if self.language != "ru" && self.language != "en" {
            self.language = default_language();
        }
        if self.theme != "dark" && self.theme != "light" {
            self.theme = default_theme();
        }
        self.keep_runs = self.keep_runs.clamp(10, 10_000);
        self.keep_rollback_txs = self.keep_rollback_txs.clamp(1, 1_000);
        self.schema = SETTINGS_SCHEMA.to_owned();
    }
}

/// Каталог конфигурации RDE.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RDE_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    if let Some(dir) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(dir).join("rde");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join(".config/rde")
}

/// Путь к файлу настроек приложения.
pub fn settings_path() -> PathBuf {
    config_dir().join("settings.toml")
}

/// Путь к portable-настройкам проекта (если включён `RDE_PORTABLE=1`).
pub fn portable_settings_path(project_root: &Path) -> Option<PathBuf> {
    if std::env::var_os("RDE_PORTABLE").is_some() {
        Some(project_root.join(".rde/app-settings.toml"))
    } else {
        None
    }
}

/// Прочитать настройки (defaults, если файла нет или он повреждён).
pub fn load() -> AppSettings {
    load_from(&settings_path())
}

/// Прочитать настройки из конкретного пути.
pub fn load_from(path: &Path) -> AppSettings {
    let mut settings = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str::<AppSettings>(&text).ok())
        .unwrap_or_default();
    settings.normalize();
    settings
}

/// Сохранить настройки (создаёт каталог при необходимости).
pub fn save(settings: &AppSettings) -> Result<PathBuf, String> {
    save_to(settings, &settings_path())
}

/// Сохранить настройки в конкретный путь.
pub fn save_to(settings: &AppSettings, path: &Path) -> Result<PathBuf, String> {
    let mut settings = settings.clone();
    settings.normalize();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("создать {}: {e}", parent.display()))?;
    }
    let text = toml::to_string_pretty(&settings).map_err(|e| format!("settings.toml: {e}"))?;
    std::fs::write(path, text).map_err(|e| format!("записать {}: {e}", path.display()))?;
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-settings-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn defaults_roundtrip_and_normalize() {
        let dir = tmp("roundtrip");
        let path = dir.join("settings.toml");
        let settings = load_from(&path);
        assert_eq!(settings.language, "ru");
        assert_eq!(settings.keep_runs, 200);
        assert!(!settings.allow_external_mcp, "внешние агенты по умолчанию запрещены (fail-closed)");
        assert!(!settings.allow_agent_mutations, "мутации агента по умолчанию запрещены (fail-closed)");
        // M4.2b: отдельные гранты run/git по умолчанию выключены (fail-closed).
        assert!(!settings.allow_agent_run_script, "запуск скриптов агентом по умолчанию запрещён");
        assert!(!settings.allow_agent_git_commit, "git-коммиты агентом по умолчанию запрещены");
        save_to(&settings, &path).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded, settings);
        // M4.2b: старые settings.toml без новых ключей читаются (аддитивная миграция) → false.
        let old_text = std::fs::read_to_string(&path)
            .unwrap()
            .lines()
            .filter(|line| !line.starts_with("allow_agent_run_script") && !line.starts_with("allow_agent_git_commit"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&path, &old_text).unwrap();
        let old_loaded = load_from(&path);
        assert!(!old_loaded.allow_agent_run_script, "старый файл: run_script = false");
        assert!(!old_loaded.allow_agent_git_commit, "старый файл: git_commit = false");
        // Включённые гранты переживают сохранение/чтение (аддитивные поля сериализуются).
        let mut granted = AppSettings { allow_agent_run_script: true, allow_agent_git_commit: true, ..AppSettings::default() };
        granted.normalize();
        save_to(&granted, &path).unwrap();
        let granted_loaded = load_from(&path);
        assert!(granted_loaded.allow_agent_run_script);
        assert!(granted_loaded.allow_agent_git_commit);
        // Некорректные значения нормализуются.
        let mut bad = settings.clone();
        bad.language = "de".into();
        bad.keep_runs = 1;
        bad.normalize();
        assert_eq!(bad.language, "ru");
        assert_eq!(bad.keep_runs, 10);
        // Старый settings.toml без поля allow_external_mcp → false (аддитивная миграция).
        std::fs::write(&path, "schema = \"rde/settings@1\"\nlanguage = \"ru\"\n").unwrap();
        assert!(!load_from(&path).allow_external_mcp);
        assert!(!load_from(&path).allow_agent_mutations);
        // Повреждённый файл → defaults, не паника.
        std::fs::write(&path, "not toml [").unwrap();
        assert_eq!(load_from(&path).language, "ru");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn portable_path_respects_env() {
        let dir = tmp("portable");
        // Без RDE_PORTABLE portable-путь не используется.
        if std::env::var_os("RDE_PORTABLE").is_none() {
            assert!(portable_settings_path(&dir).is_none());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M4.3 (R-AGT-01): настройки встроенного ACP-агента аддитивны и fail-closed.
    #[test]
    fn builtin_agent_settings_are_additive_and_fail_closed() {
        let dir = tmp("builtin-agent");
        let path = dir.join("settings.toml");
        let settings = load_from(&path);
        assert!(!settings.allow_builtin_agent, "встроенный агент по умолчанию выключен");
        assert_eq!(settings.agent_program, "dsh", "дефолт — DSH (S16)");
        assert_eq!(settings.agent_args, vec!["--profile".to_owned(), "acp".to_owned()]);
        assert!(settings.agent_model.is_empty(), "модель — из configOptions DSH");
        assert!(settings.agent_reasoning_effort.is_empty());
        save_to(&settings, &path).unwrap();
        assert_eq!(load_from(&path), settings);
        // Старый settings.toml без новых ключей читается без миграции (аддитивные поля).
        std::fs::write(&path, "schema = \"rde/settings@1\"\nlanguage = \"ru\"\n").unwrap();
        let old = load_from(&path);
        assert!(!old.allow_builtin_agent, "старый файл: встроенный агент выключен");
        assert_eq!(old.agent_program, "dsh");
        assert_eq!(old.agent_args.len(), 2);
        // Включение и параметры переживают сохранение/чтение.
        let mut enabled = AppSettings {
            allow_builtin_agent: true,
            agent_program: "/opt/dsh".to_owned(),
            agent_args: vec!["--profile".to_owned(), "acp".to_owned(), "--verbose".to_owned()],
            agent_model: "deepseek-v4-pro".to_owned(),
            agent_reasoning_effort: "max".to_owned(),
            ..AppSettings::default()
        };
        enabled.normalize();
        save_to(&enabled, &path).unwrap();
        let loaded = load_from(&path);
        assert!(loaded.allow_builtin_agent);
        assert_eq!(loaded.agent_program, "/opt/dsh");
        assert_eq!(loaded.agent_args.len(), 3);
        assert_eq!(loaded.agent_model, "deepseek-v4-pro");
        assert_eq!(loaded.agent_reasoning_effort, "max");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
