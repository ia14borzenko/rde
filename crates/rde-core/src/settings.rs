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
        save_to(&settings, &path).unwrap();
        let loaded = load_from(&path);
        assert_eq!(loaded, settings);
        // Некорректные значения нормализуются.
        let mut bad = settings.clone();
        bad.language = "de".into();
        bad.keep_runs = 1;
        bad.normalize();
        assert_eq!(bad.language, "ru");
        assert_eq!(bad.keep_runs, 10);
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
}
