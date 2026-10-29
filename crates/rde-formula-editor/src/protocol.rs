//! Протокол JS↔Rust локального моста (P6, ADR-0022 §3.1).
//!
//! Все сообщения — JSON поверх loopback-HTTP (`127.0.0.1:<случайный порт>`), авторизация —
//! одноразовый токен в query-параметре `t`. Окно редактора (страница MathLive) и RDE обмениваются
//! только **данными**: LaTeX-строка, тема, зум, заголовок, признак display/inline. Никакого
//! исполнения кода/команд из страницы в RDE нет: допустимые действия перечислены в [`EditorEvent`].
//!
//! Безопасность (NFR-SEC): сервер слушает только loopback, проверяет `Host` (защита от DNS
//! rebinding), размер тела ограничен, токен сравнивается за постоянное время, страница получает
//! строгий CSP (только собственные ассеты, без сети).

use serde::{Deserialize, Serialize};

/// Тема оформления окна редактора: следует теме RDE (dark/light).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum EditorTheme {
    /// Тёмная тема (по умолчанию — как в RDE).
    #[default]
    Dark,
    /// Светлая тема.
    Light,
}

impl EditorTheme {
    /// CSS-значение для страницы редактора (`data-theme`).
    #[must_use]
    pub fn as_css(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    /// Тема по строке настроек RDE (`dark` | `light` | `system`): системная трактуется как тёмная
    /// (RDE по умолчанию тёмный; при смене темы окно обновляется через `/api/state`).
    #[must_use]
    pub fn from_settings(theme: &str) -> Self {
        if theme.eq_ignore_ascii_case("light") { Self::Light } else { Self::Dark }
    }
}

/// Зум окна редактора: 100–200 % (шаг 10). Вне диапазона — клампится.
#[must_use]
pub fn clamp_zoom(zoom: u32) -> u32 {
    zoom.clamp(100, 200)
}

/// Состояние редактора, которое видит страница (`GET /api/state`) и которое RDE обновляет.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EditorState {
    /// Заголовок формулы (для шапки окна; может быть пустым у новой формулы).
    pub title: String,
    /// Канонический LaTeX формулы — источник истины (`formula.toml.latex`).
    pub latex: String,
    /// `true` — display-формула (отдельная строка), `false` — inline.
    pub display: bool,
    /// Тема оформления окна.
    pub theme: EditorTheme,
    /// Зум 100–200 %.
    pub zoom: u32,
    /// Отображаемое имя проекта (шапка окна; необязательно).
    pub project: String,
}

impl Default for EditorState {
    fn default() -> Self {
        Self { title: String::new(), latex: String::new(), display: true, theme: EditorTheme::Dark, zoom: 100, project: String::new() }
    }
}

/// События страницы редактора, которые забирает RDE (единственный канал воздействия на продукт).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum EditorEvent {
    /// Пользователь нажал «Применить»: LaTeX готов к записи в `formula.toml`.
    Applied {
        /// LaTeX-источник из редактора (как есть, без нормализации — её делает RDE).
        latex: String,
    },
    /// Пользователь нажал «Отмена»/закрыл окно: изменений нет.
    Cancelled,
    /// Окно сообщает новый зум (для сохранения между открытиями).
    Zoom {
        /// Новый зум 100–200 %.
        zoom: u32,
    },
}

/// `POST /api/apply` — тело запроса.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplyRequest {
    /// LaTeX-источник из визуального редактора.
    pub latex: String,
}

/// `POST /api/prefs` — тело запроса (настройки окна, переживают закрытие).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PrefsRequest {
    /// Новый зум 100–200 %.
    pub zoom: u32,
}

/// JSON-ответ моста: единый конверт (успех/ошибка + полезная нагрузка).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiReply<T> {
    /// `true` — запрос выполнен.
    pub ok: bool,
    /// Читаемая ошибка (русский текст для окна редактора).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub error: String,
    /// Полезная нагрузка (для `/api/state` — состояние).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T> ApiReply<T> {
    /// Успешный ответ с данными.
    #[must_use]
    pub fn ok(data: T) -> Self {
        Self { ok: true, error: String::new(), data: Some(data) }
    }

    /// Успешный ответ без данных.
    #[must_use]
    pub fn done() -> Self
    where
        T: Default,
    {
        Self { ok: true, error: String::new(), data: None }
    }

    /// Ошибка с сообщением (данных нет).
    #[must_use]
    pub fn fail(error: impl Into<String>) -> Self {
        Self { ok: false, error: error.into(), data: None }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_parsing_and_css() {
        assert_eq!(EditorTheme::from_settings("light"), EditorTheme::Light);
        assert_eq!(EditorTheme::from_settings("dark"), EditorTheme::Dark);
        assert_eq!(EditorTheme::from_settings("system"), EditorTheme::Dark);
        assert_eq!(EditorTheme::Dark.as_css(), "dark");
        assert_eq!(EditorTheme::Light.as_css(), "light");
    }

    #[test]
    fn zoom_is_clamped_to_100_200() {
        assert_eq!(clamp_zoom(50), 100);
        assert_eq!(clamp_zoom(150), 150);
        assert_eq!(clamp_zoom(999), 200);
    }

    #[test]
    fn event_json_is_stable() {
        let applied = EditorEvent::Applied { latex: "\\frac{1}{2}".to_owned() };
        let json = serde_json::to_string(&applied).expect("сериализация");
        assert_eq!(json, r#"{"event":"applied","latex":"\\frac{1}{2}"}"#);
        let back: EditorEvent = serde_json::from_str(&json).expect("разбор");
        assert_eq!(back, applied);
        let cancel: EditorEvent = serde_json::from_str(r#"{"event":"cancelled"}"#).expect("отмена");
        assert_eq!(cancel, EditorEvent::Cancelled);
    }

    #[test]
    fn api_reply_envelope() {
        let reply = ApiReply::ok(EditorState::default());
        let json = serde_json::to_value(&reply).expect("json");
        assert_eq!(json["ok"], true);
        assert!(json.get("error").is_none(), "пустая ошибка не сериализуется");
        let fail: ApiReply<EditorState> = ApiReply::fail("нет связи");
        assert_eq!(serde_json::to_value(fail).expect("json")["error"], "нет связи");
    }
}
