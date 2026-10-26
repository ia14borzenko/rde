//! Форматирование величин/констант (R-ART-09, R-DOC-11/23): общий код для `rde-doc`
//! (живые вставки в документ) и `rde-dag` (разрешение значений констант).
//!
//! Поведение форматов — из фазы 1 `rde_doc::format_value`: `auto|raw|int|dec:N|sci:N|percent:N`;
//! `raw` и нераспознанный текст возвращаются как есть, нечисловые/NaN/∞ — как есть.
//!
//! Обозначение единицы по ГОСТ 8.417-2024 (§9.7): значение и единица — через неразрывный пробел
//! ([`NBSP`]), единица прямым начертанием (начертание обеспечивается rich-стилем вставки,
//! [`constant_run_style`]).

/// Неразрывный пробел между значением и единицей (ГОСТ 8.417-2024, ТЗ §9.7).
pub const NBSP: char = '\u{00A0}';

/// Известен ли формат отображения значения (для валидации/подсказок).
pub fn is_known_format(format: &str) -> bool {
    let format = format.trim();
    if format.is_empty() || format == "auto" || format == "raw" || format == "int" {
        return true;
    }
    for prefix in ["dec", "sci", "percent"] {
        if let Some(rest) = format.strip_prefix(prefix) {
            if let Some(digits) = rest.strip_prefix(':') {
                return digits.parse::<usize>().is_ok();
            }
            return false;
        }
    }
    false
}

/// Отформатировать число по формату вставки (R-DOC-11): `auto|raw|int|dec:N|sci:N|percent:N`.
pub fn format_number(value: f64, format: &str) -> String {
    if !value.is_finite() {
        return format!("{value}");
    }
    let format = format.trim();
    let precision = |prefix: &str, default: usize| -> usize {
        format
            .strip_prefix(prefix)
            .and_then(|rest| rest.strip_prefix(':'))
            .and_then(|rest| rest.parse::<usize>().ok())
            .unwrap_or(default)
            .min(10)
    };
    match format {
        "" | "auto" => format_auto(value),
        "raw" => format_auto(value),
        "int" => format!("{value:.0}"),
        _ if format.starts_with("dec") => {
            let digits = precision("dec", 2);
            format!("{value:.digits$}")
        }
        _ if format.starts_with("sci") => {
            let digits = precision("sci", 3);
            let text = format!("{value:.digits$e}"); // 1.235e3
            match text.split_once('e') {
                Some((mantissa, exponent)) => {
                    let exponent: i32 = exponent.parse().unwrap_or(0);
                    format!("{mantissa}·10^{exponent}")
                }
                None => text,
            }
        }
        _ if format.starts_with("percent") => {
            let digits = precision("percent", 1);
            format!("{:.digits$} %", value * 100.0)
        }
        _ => format_auto(value),
    }
}

/// Формат `auto`: целые без `.0`, обычные числа — до 4 знаков без хвостовых нулей,
/// очень большие/малые — `m·10^e`.
fn format_auto(value: f64) -> String {
    if value == 0.0 {
        return "0".to_owned();
    }
    let abs = value.abs();
    if (1e-3..1e6).contains(&abs) {
        let mut text = format!("{value:.4}");
        while text.ends_with('0') && text.contains('.') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
        text
    } else {
        let text = format!("{value:.4e}");
        match text.split_once('e') {
            Some((mantissa, exponent)) => {
                let exponent: i32 = exponent.parse().unwrap_or(0);
                format!("{mantissa}·10^{exponent}")
            }
            None => text,
        }
    }
}

/// Отформатировать «сырое» текстовое значение (значение вставки/выхода артефакта) по формату.
/// Нечисловой текст возвращается как есть (как `rde_doc::format_value`).
pub fn format_raw(raw: &str, format: &str) -> String {
    let format = format.trim();
    if format.is_empty() || format == "raw" {
        return raw.to_owned();
    }
    let Ok(value) = raw.trim().parse::<f64>() else {
        return raw.to_owned();
    };
    if !value.is_finite() {
        return raw.to_owned();
    }
    format_number(value, format)
}

/// Текст величины для вставки/документа (R-DOC-23):
/// * есть `display` — обозначение как задано (`R_н`);
/// * иначе есть `symbol` — обозначение из `symbol`+`subscript` (`Rн`, rich-стиль подчёркивает);
/// * иначе — число по `format` + неразрывный пробел + единица.
pub fn constant_text(value: f64, unit: &str, display: &str, symbol: &str, subscript: &str, format: &str) -> String {
    if !display.is_empty() {
        return display.to_owned();
    }
    let designation = format!("{symbol}{subscript}");
    if !designation.is_empty() {
        return designation;
    }
    let number = format_number(value, format);
    if unit.is_empty() {
        number
    } else {
        format!("{number}{NBSP}{unit}")
    }
}

/// Rich-стиль обозначения величины для docx-вставки (R-ART-09): `italic`/`italic-subscript`.
/// Явный `style` величины важнее; иначе подстрочный индекс делает `italic-subscript`,
/// символ без индекса — `italic`; если обозначения нет — пусто (обычный текст).
pub fn constant_run_style(style: &str, symbol: &str, subscript: &str) -> String {
    if !style.is_empty() {
        return style.to_owned();
    }
    if !subscript.is_empty() {
        return "italic-subscript".to_owned();
    }
    if !symbol.is_empty() {
        return "italic".to_owned();
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_formats_match_phase_1_behavior() {
        assert_eq!(format_number(12.4, "auto"), "12.4");
        assert_eq!(format_number(12.0, "auto"), "12");
        assert_eq!(format_number(0.00075978125, "auto"), "7.5978·10^-4");
        assert_eq!(format_number(12.4, "int"), "12");
        assert_eq!(format_number(12.3456, "dec:2"), "12.35");
        assert_eq!(format_number(1234.5, "percent:1"), "123450.0 %");
        assert_eq!(format_number(1.235e3, "sci:3"), "1.235·10^3");
        assert_eq!(format_number(12.4, "raw"), "12.4");
        assert_eq!(format_number(12.4, "unknown"), "12.4");
    }

    #[test]
    fn format_raw_keeps_text() {
        assert_eq!(format_raw("42.5", "dec:1"), "42.5");
        assert_eq!(format_raw("важно", "dec:1"), "важно");
        assert_eq!(format_raw("42.55", "raw"), "42.55");
        assert_eq!(format_raw("0.5", "percent:0"), "50 %");
    }

    #[test]
    fn constant_text_prefers_designation() {
        assert_eq!(constant_text(12.4, "Ом", "R_н", "R", "н", "auto"), "R_н");
        assert_eq!(constant_text(12.4, "Ом", "", "R", "н", "auto"), "Rн");
        assert_eq!(constant_text(12.4, "Ом", "", "", "", "auto"), "12.4\u{00A0}Ом");
        assert_eq!(constant_text(12.4, "", "", "", "", "auto"), "12.4");
        assert_eq!(constant_run_style("", "R", "н"), "italic-subscript");
        assert_eq!(constant_run_style("", "R", ""), "italic");
        assert_eq!(constant_run_style("", "", ""), "");
        assert_eq!(constant_run_style("italic", "R", "н"), "italic");
    }

    #[test]
    fn known_formats_validation() {
        for format in ["", "auto", "raw", "int", "dec:2", "sci:3", "percent:1"] {
            assert!(is_known_format(format), "{format}");
        }
        for format in ["dec", "dec:", "sci:x", "glory"] {
            assert!(!is_known_format(format), "{format}");
        }
    }
}
