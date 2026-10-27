//! MCP-инструкции сервера: карта проекта + политика + правила чтения (≤32 KiB).
//!
//! Инструкции формирует хост ([`RdeHost::instructions`]) — только он знает проект; этот модуль
//! отвечает за протокольное ограничение: лимит 32 KiB (ограничение DSH, S16) и стабильность
//! (одинаковый проект → одинаковый текст: никакого времени/случайности, R-AGT-08).

use crate::error::HostError;
use crate::host::RdeHost;
use crate::MAX_INSTRUCTIONS_BYTES;

/// Маркер, который добавляется при усечении инструкций.
pub const CLAMP_MARKER: &str = "\n\n[RDE: инструкции усечены до 32 KiB — используйте инструменты/ресурсы для деталей]";

/// Уложить текст инструкций в лимит [`MAX_INSTRUCTIONS_BYTES`] по границе UTF-8.
pub fn clamp(text: &str) -> String {
    if text.len() <= MAX_INSTRUCTIONS_BYTES {
        return text.to_owned();
    }
    let budget = MAX_INSTRUCTIONS_BYTES.saturating_sub(CLAMP_MARKER.len());
    let mut end = budget.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut clamped = String::with_capacity(end + CLAMP_MARKER.len());
    clamped.push_str(&text[..end]);
    clamped.push_str(CLAMP_MARKER);
    clamped
}

/// Получить инструкции у хоста и привести к протокольному лимиту.
///
/// Пустой ответ хоста — ошибка `RDE_NOT_READY` (карта проекта обязательна для R-MCP-05).
pub fn for_host(host: &dyn RdeHost) -> Result<String, HostError> {
    let text = host.instructions()?;
    if text.trim().is_empty() {
        return Err(HostError::not_ready("хост вернул пустые MCP-инструкции"));
    }
    Ok(clamp(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::MockHost;

    #[test]
    fn short_instructions_are_not_touched() {
        let text = "Проект: демо\nПолитика: read-only";
        assert_eq!(clamp(text), text);
    }

    #[test]
    fn long_instructions_are_clamped_to_32kib_on_char_boundary() {
        // Многострочный UTF-8 текст (кириллица — 2 байта на символ).
        let text = "карта проекта: объект\n".repeat(4_000);
        assert!(text.len() > MAX_INSTRUCTIONS_BYTES);
        let clamped = clamp(&text);
        assert!(clamped.len() <= MAX_INSTRUCTIONS_BYTES, "{} > {}", clamped.len(), MAX_INSTRUCTIONS_BYTES);
        assert!(clamped.ends_with(CLAMP_MARKER.trim_end()));
        // Усечение по границе символа: строка остаётся валидным UTF-8.
        assert!(std::str::from_utf8(clamped.as_bytes()).is_ok());
    }

    #[test]
    fn instructions_from_host_are_stable() {
        let host = MockHost::default();
        let first = for_host(&host).unwrap();
        let second = for_host(&host).unwrap();
        assert_eq!(first, second, "инструкции должны быть детерминированными");
        assert!(first.contains("RDE"));
    }

    #[test]
    fn empty_instructions_are_rejected() {
        let host = MockHost { instructions: "   ".to_owned(), ..MockHost::default() };
        let error = for_host(&host).expect_err("пустые инструкции — RDE_NOT_READY");
        assert_eq!(error.code, crate::error::RdeErrorCode::NotReady);
    }
}
