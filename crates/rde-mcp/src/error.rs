//! Машиночитаемые ошибки MCP-слоя RDE (`RDE_*`).
//!
//! Ошибка инструмента/ресурса — это [`HostError`]: стабильный код [`RdeErrorCode`],
//! человекочитаемое сообщение и (опционально) подсказка. Такие ошибки возвращаются клиенту
//! как результат `tools/call` (`isError: true`, `structuredContent.error.code = RDE_*`), а не
//! как «непрозрачная» ошибка JSON-RPC — так агент может на них реагировать (R-MCP-03).
//!
//! Тексты ошибок стабильны и не содержат путей вне проекта/секретов (A5): хост отвечает за
//! санитизацию сообщений, слой лишь доносит их до клиента.

use std::fmt;

use serde::{Deserialize, Serialize};

/// Код ошибки RDE (машиночитаемый, стабильный; сериализуется строкой `RDE_*`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RdeErrorCode {
    /// Объект/документ/файл/инструмент не найден внутри проекта.
    #[serde(rename = "RDE_NOT_FOUND")]
    NotFound,
    /// Невалидные аргументы инструмента (не парсятся, нарушают контракт).
    #[serde(rename = "RDE_INVALID_ARGUMENT")]
    InvalidArgument,
    /// Значение вне допустимого диапазона/лимита (размер ответа, пагинация).
    #[serde(rename = "RDE_OUT_OF_RANGE")]
    OutOfRange,
    /// Путь выходит за корень проекта (абсолютный путь, `..`, симлинк, UNC/file://).
    #[serde(rename = "RDE_PATH_ESCAPE")]
    PathEscape,
    /// Документ ещё не в формате `.doc.rde` (нужна явная миграция; авто-миграции нет).
    #[serde(rename = "RDE_DOC_NOT_MIGRATED")]
    DocNotMigrated,
    /// Действие требует approval пользователя (мутации/опасные операции).
    #[serde(rename = "RDE_APPROVAL_REQUIRED")]
    ApprovalRequired,
    /// Действие запрещено политикой/пользователем (fail-closed).
    #[serde(rename = "RDE_APPROVAL_DENIED")]
    ApprovalDenied,
    /// Операция не поддерживается в текущей версии/режиме (M1 — read-only).
    #[serde(rename = "RDE_UNSUPPORTED")]
    Unsupported,
    /// Превышен лимит вызовов в минуту (`McpPolicy::rate_limit_per_min`).
    #[serde(rename = "RDE_RATE_LIMITED")]
    RateLimited,
    /// Проект/хранилище/документ ещё не готовы к операции.
    #[serde(rename = "RDE_NOT_READY")]
    NotReady,
    /// Инструмент не уложился в таймаут (`McpPolicy::tool_timeout_ms`).
    #[serde(rename = "RDE_TIMEOUT")]
    Timeout,
    /// Операция не выполнена: занято (идёт запуск артефактов/конкурентная мутация).
    #[serde(rename = "RDE_BUSY")]
    Busy,
    /// Конфликт состояния: файл изменён вне RDE (устаревший `expected_sha256`/ручная правка).
    #[serde(rename = "RDE_CONFLICT")]
    Conflict,
    /// Job запуска артефактов не найден (неизвестный job_id или перезапуск процесса).
    #[serde(rename = "RDE_JOB_NOT_FOUND")]
    JobNotFound,
    /// Внутренняя ошибка (panic в инструменте, повреждённые данные, ошибка ФС).
    #[serde(rename = "RDE_INTERNAL")]
    Internal,
}

impl RdeErrorCode {
    /// Строковое представление кода (то, что уходит клиенту).
    pub fn as_str(self) -> &'static str {
        match self {
            RdeErrorCode::NotFound => "RDE_NOT_FOUND",
            RdeErrorCode::InvalidArgument => "RDE_INVALID_ARGUMENT",
            RdeErrorCode::OutOfRange => "RDE_OUT_OF_RANGE",
            RdeErrorCode::PathEscape => "RDE_PATH_ESCAPE",
            RdeErrorCode::DocNotMigrated => "RDE_DOC_NOT_MIGRATED",
            RdeErrorCode::ApprovalRequired => "RDE_APPROVAL_REQUIRED",
            RdeErrorCode::ApprovalDenied => "RDE_APPROVAL_DENIED",
            RdeErrorCode::Unsupported => "RDE_UNSUPPORTED",
            RdeErrorCode::RateLimited => "RDE_RATE_LIMITED",
            RdeErrorCode::NotReady => "RDE_NOT_READY",
            RdeErrorCode::Timeout => "RDE_TIMEOUT",
            RdeErrorCode::Busy => "RDE_BUSY",
            RdeErrorCode::Conflict => "RDE_CONFLICT",
            RdeErrorCode::JobNotFound => "RDE_JOB_NOT_FOUND",
            RdeErrorCode::Internal => "RDE_INTERNAL",
        }
    }
}

impl fmt::Display for RdeErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Ошибка хоста RDE: код, сообщение, опциональная подсказка.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostError {
    /// Машиночитаемый код (`RDE_*`).
    pub code: RdeErrorCode,
    /// Стабильное человекочитаемое сообщение (без путей вне проекта и секретов).
    pub message: String,
    /// Подсказка: что сделать, чтобы ошибка ушла.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl HostError {
    /// Создать ошибку с кодом и сообщением.
    pub fn new(code: RdeErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), hint: None }
    }

    /// Добавить подсказку.
    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// `RDE_NOT_FOUND`.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::NotFound, message)
    }

    /// `RDE_INVALID_ARGUMENT`.
    pub fn invalid_argument(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::InvalidArgument, message)
    }

    /// `RDE_OUT_OF_RANGE`.
    pub fn out_of_range(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::OutOfRange, message)
    }

    /// `RDE_PATH_ESCAPE`.
    pub fn path_escape(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::PathEscape, message)
    }

    /// `RDE_DOC_NOT_MIGRATED`.
    pub fn doc_not_migrated(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::DocNotMigrated, message)
    }

    /// `RDE_APPROVAL_REQUIRED`.
    pub fn approval_required(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::ApprovalRequired, message)
    }

    /// `RDE_APPROVAL_DENIED`.
    pub fn approval_denied(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::ApprovalDenied, message)
    }

    /// `RDE_UNSUPPORTED`.
    pub fn unsupported(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::Unsupported, message)
    }

    /// `RDE_RATE_LIMITED`.
    pub fn rate_limited(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::RateLimited, message)
    }

    /// `RDE_NOT_READY`.
    pub fn not_ready(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::NotReady, message)
    }

    /// `RDE_TIMEOUT`.
    pub fn timeout(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::Timeout, message)
    }

    /// `RDE_BUSY`.
    pub fn busy(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::Busy, message)
    }

    /// `RDE_CONFLICT`.
    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::Conflict, message)
    }

    /// `RDE_JOB_NOT_FOUND`.
    pub fn job_not_found(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::JobNotFound, message)
    }

    /// `RDE_INTERNAL`.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(RdeErrorCode::Internal, message)
    }

    /// JSON-представление для `structuredContent` результата инструмента.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "code": self.code.as_str(),
            "message": self.message,
            "hint": self.hint,
        })
    }
}

impl fmt::Display for HostError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)?;
        if let Some(hint) = &self.hint {
            write!(f, " (подсказка: {hint})")?;
        }
        Ok(())
    }
}

impl std::error::Error for HostError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_serialize_as_stable_strings() {
        assert_eq!(serde_json::to_value(RdeErrorCode::PathEscape).unwrap(), serde_json::json!("RDE_PATH_ESCAPE"));
        assert_eq!(RdeErrorCode::DocNotMigrated.to_string(), "RDE_DOC_NOT_MIGRATED");
    }

    #[test]
    fn host_error_json_shape_is_stable() {
        let error = HostError::not_found("нет объекта").with_hint("проверьте id");
        let json = error.to_json();
        assert_eq!(json["code"], "RDE_NOT_FOUND");
        assert_eq!(json["message"], "нет объекта");
        assert_eq!(json["hint"], "проверьте id");
        // Сериализация самого типа — та же схема (используется в DTO/аудите).
        let value = serde_json::to_value(&error).unwrap();
        assert_eq!(value["code"], "RDE_NOT_FOUND");
    }

    #[test]
    fn hint_is_omitted_when_absent() {
        let json = serde_json::to_value(HostError::internal("x")).unwrap();
        assert!(json.get("hint").is_none());
    }
}
