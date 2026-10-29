//! Бандл math-шрифта формул: **STIX Two Math** (SIL OFL 1.1) — антиква Times-стиля со своей
//! таблицей `MATH` (ADR-0018).
//!
//! Зачем: формулы RDE рендерятся движком WordCraft, а он разрешает math-шрифт через цепочку
//! `resolve_math("Cambria Math")` → ... → `DejaVu Math TeX Gyre` (единственный math-шрифт в
//! окружении прототипа). DejaVu заметно отличается от Times New Roman, которым набран текст
//! документа, и формулы «выпадают» из вёрстки. Регистрируем в глобальном `FontDb` собственный
//! шрифт семейства `STIX Two Math` — он уже есть в `MATH_SUBSTITUTES` движка, поэтому подхватывается
//! без патча вендора: `layout/math.rs` вызывает `resolve_math("Cambria Math")` на каждой вёрстке
//! формулы, а цепочка замен находит установленное семейство с `MATH`.
//!
//! STIX Two Math выбран (а не TeX Gyre Termes Math) из-за лицензии и покрытия:
//! - SIL OFL 1.1 (TeX Gyre — GUST Font License, не OFL/Apache; в поставку не берём);
//! - полная кириллица (64/64 букв) — в импортированных MathType-формулах кириллица встречается
//!   (у TeX Gyre Termes Math её нет: 0/64), плюс греческий и все нужные математические символы;
//! - дизайн STIX — Times-совместимая антиква (глифы/пропорции близки к TNR), метрики 1000 upem.
//!
//! Шрифт встраивается в бинарник (`include_bytes!`), лицензия — тоже (требование OFL: текст
//! лицензии распространяется вместе со шрифтом). Регистрация идемпотентна (OnceLock).

use std::sync::OnceLock;

/// Семейство бандл-шрифта в `FontDb`; совпадает с именем в `MATH_SUBSTITUTES` WordCraft.
pub const MATH_FONT_FAMILY: &str = "STIX Two Math";
/// Имя файла шрифта в `assets/fonts/` (для документации/NOTICE).
pub const MATH_FONT_FILE: &str = "STIXTwoMath-Regular.otf";
/// Шрифт целиком (838 КБ): OFL разрешает встраивание и распространение вместе с продуктом.
const MATH_FONT_BYTES: &[u8] = include_bytes!("../assets/fonts/STIXTwoMath-Regular.otf");
/// Текст лицензии SIL OFL 1.1 из дистрибутива STIX (атрибуция по требованию OFL).
pub const MATH_FONT_LICENSE: &str = include_str!("../assets/fonts/STIXTwoMath-OFL.txt");
/// Правообладатель для NOTICE/«О прототипе».
pub const MATH_FONT_COPYRIGHT: &str = "Copyright 2001-2021 The STIX Fonts Project Authors (SIL Open Font License 1.1)";

/// Зарегистрировать шрифт в глобальном `FontDb` (один раз за процесс, идемпотентно).
///
/// Ошибок не бывает: при проблемах с шрифтом движок продолжит использовать прежний math-шрифт
/// (деградация без падений). `add_font` сам пропускает уже присутствующие семейство/начертание.
pub fn ensure_registered() {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| {
        // Аварийный откат/диагностика: `RDE_MATH_FONT=none|cambria|dejavu` отключает бандл —
        // формулы рендерятся прежним substitute-шрифтом WordCraft (DejaVu Math TeX Gyre).
        // Используется для честных замеров «до/после» и как страховка при проблемах со шрифтом.
        if std::env::var("RDE_MATH_FONT")
            .map(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "none" | "cambria" | "dejavu"))
            .unwrap_or(false)
        {
            return;
        }
        let _ = wordcraft_fonts::FontDb::global().add_font(MATH_FONT_BYTES.to_vec());
    });
}

/// Какое семейство реально используется для формул (для диагностики/тестов): если бандл
/// зарегистрирован и подхватился — `STIX Two Math`, иначе `DejaVu Math TeX Gyre`/иное.
pub fn resolved_math_family() -> String {
    wordcraft_fonts::math::resolve_math("Cambria Math").0.get().family.clone()
}

/// Доступен ли бандл-шрифт (регистрация прошла и family видна в FontDb).
pub fn bundled_font_available() -> bool {
    ensure_registered();
    wordcraft_fonts::FontDb::global().has_family(MATH_FONT_FAMILY)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Бандл: OFL-текст встроен, шрифт регистрируется и реально выбирается движком для формул.
    #[test]
    fn bundled_math_font_is_registered_and_resolved() {
        assert!(MATH_FONT_LICENSE.contains("SIL OPEN FONT LICENSE"), "лицензия OFL встроена");
        assert!(MATH_FONT_LICENSE.contains("STIX"), "атрибуция STIX в лицензии");
        assert!(MATH_FONT_BYTES.starts_with(b"OTTO"), "в бандле настоящий OpenType/CFF-шрифт");
        assert!(bundled_font_available(), "семейство {MATH_FONT_FAMILY} видно в FontDb");
        let resolved = resolved_math_family();
        assert_eq!(resolved, MATH_FONT_FAMILY, "формулы вёрстаются бандл-шрифтом (не DejaVu)");
        // Повторная регистрация идемпотентна и не ломает резолв.
        let _ = ensure_registered();
        assert_eq!(resolved_math_family(), MATH_FONT_FAMILY);
    }

    /// Покрытие глифов: математические операторы/символы, греческий, кириллица, скобки-куски
    /// и латиница — всё, что встречается в формулах (в т.ч. импортированных MathType), есть в
    /// бандл-шрифте. Это регрессия «не потерять глифы» без зависимости от внешних .docx.
    #[test]
    fn bundled_font_covers_formula_glyphs() {
        ensure_registered();
        let face = wordcraft_fonts::FontDb::global().face(MATH_FONT_FAMILY, "Regular");
        let mut missing: Vec<char> = Vec::new();
        // Операторы и отношения, интегралы/суммы (включая двойные/тройные), скобки-куски.
        for ch in "∫∬∭∮∯∰∑∏∐⋃⋂√∞∂∇≤≥≠≈≡±×÷·→←↔⇒⇐∈∉⊂⊃∪∩∀∃∄∅∠⊥∥−⋅∗∘⊕⊗⋆…".chars() {
            if !face.covers(ch) {
                missing.push(ch);
            }
        }
        // Скобки-куски U+239B..U+23AE (Word/MT Extra/Adobe Symbol при импорте MathType).
        for code in 0x239Bu32..=0x23AE {
            if let Some(ch) = char::from_u32(code)
                && !face.covers(ch)
            {
                missing.push(ch);
            }
        }
        // Греческий (строчный/заглавный) и кириллица — в формулах проекта бывают русские буквы.
        for ch in "αβγδεζηθικλμνξοπρστυφχψωΓΔΘΛΞΠΣΦΨΩ".chars() {
            if !face.covers(ch) {
                missing.push(ch);
            }
        }
        for ch in "АБВГДЕЁЖЗИЙКЛМНОПРСТУФХЦЧШЩЪЫЬЭЮЯабвгдеёжзийклмнопрстуфхцчшщъыьэюя".chars() {
            if !face.covers(ch) {
                missing.push(ch);
            }
        }
        // Латиница (обычная и курсивная math-альфа STIX), цифры.
        for ch in "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789".chars() {
            if !face.covers(ch) {
                missing.push(ch);
            }
        }
        assert!(missing.is_empty(), "бандл-шрифт не содержит глифов: {missing:?}");
    }

    /// Повторная регистрация идемпотентна: не падает, не создаёт дубликатов, family резолвится.
    #[test]
    fn registration_is_idempotent() {
        ensure_registered();
        ensure_registered();
        assert!(bundled_font_available());
        assert_eq!(resolved_math_family(), MATH_FONT_FAMILY);
        // Дублирования семейства нет: `fonts()` перечисляет каждое имя один раз.
        let families = wordcraft_fonts::FontDb::global().families();
        let count = families.iter().filter(|family| family.eq_ignore_ascii_case(MATH_FONT_FAMILY)).count();
        assert_eq!(count, 1, "семейство {MATH_FONT_FAMILY} ровно одно: {count}");
    }
}
