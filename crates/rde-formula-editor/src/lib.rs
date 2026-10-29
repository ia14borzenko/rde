//! Окно «настоящего» визуального LaTeX-редактора формул RDE (P6, ADR-0022).
//!
//! Владелец заменил собственный WYSIWYG-редактор P5 (`rde-math-editor`) открытым визуальным
//! редактором уровня MathType и разрешил webview. На стенде нет dev-заголовков WebKitGTK и нет
//! `sudo`, поэтому встроенный `wry`-webview невозможен; выбран эквивалентный путь «отдельное окно
//! браузера» (как работает сам MathType) — RDE запускает установленный Chrome/Chromium/Edge в
//! режиме приложения (`--app=`) на локальной странице, а общается с ней по loopback-HTTP мосту.
//!
//! Состав крейта:
//! - [`assets`] — встроенные (`include_bytes!`) ассеты страницы и MathLive+шрифтов KaTeX: рантайм
//!   полностью офлайновый, сеть не используется (ТЗ §1.4);
//! - [`protocol`] — JSON-протокол моста (состояние: LaTeX/тема/зум; события: apply/cancel/zoom);
//! - [`server`] — минимальный HTTP/1.0-сервер на `std`, только `127.0.0.1`, токен, CSP, лимиты;
//! - [`host`] — поиск/запуск браузера, жизненный цикл окна, настройки (зум) между открытиями.
//!
//! LaTeX остаётся **источником истины** (`formula.toml.latex`): редактор возвращает строку LaTeX,
//! валидация и запись — прежний конвейер RDE (`apply_formula_edit`/`create_formula_action`,
//! кэши OMML/SVG/PNG, ledger). Крейт не зависит от `rde-doc`: его легко тестировать без проекта.

#![deny(missing_docs)]

pub mod assets;
pub mod host;
pub mod protocol;
pub mod server;

pub use assets::{ASSETS, Asset, lookup as asset, total_bytes as asset_total_bytes};
pub use host::{
    BrowserLaunch, EditorConfig, EditorError, EditorPrefs, EditorWindow, default_prefs_path, default_profile_dir,
    detect_launch, display_available, load_prefs, save_prefs,
};
pub use protocol::{EditorEvent, EditorState, EditorTheme, ApiReply, ApplyRequest, PrefsRequest, clamp_zoom};
pub use server::Bridge;

/// Версия встроенного MathLive (пин; обновляется вместе с ассетами и проверяется тестами).
pub const MATHLIVE_VERSION: &str = "0.111.1";

/// Краткое описание окна для «О программе»/диагностики.
#[must_use]
pub fn about() -> String {
    format!(
        "Окно визуального редактора формул RDE (P6): MathLive {MATHLIVE_VERSION} (MIT), \
         встроенных ассетов {} КБ, офлайн",
        asset_total_bytes() / 1024
    )
}

#[cfg(test)]
mod about_tests {
    use super::*;

    /// Диагностическая строка «О программе»: версия MathLive, лицензия и размер бандла — то, что
    /// нужно для отчёта/поддержки (ассеты зафиксированы тестами `assets::tests`).
    #[test]
    fn about_mentions_mathlive_version_license_and_size() {
        let text = about();
        assert!(text.contains(MATHLIVE_VERSION), "версия MathLive в описании: {text}");
        assert!(text.contains("MIT"), "лицензия в описании: {text}");
        assert!(text.contains("офлайн"), "офлайновость в описании: {text}");
        assert!(asset_total_bytes() > 1_000_000, "ассеты в бандле: {}", asset_total_bytes());
    }
}
