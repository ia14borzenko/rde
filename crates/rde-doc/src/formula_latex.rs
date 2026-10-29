//! Валидация LaTeX-источника формулы (P5, ADR-0021 §Решение 2).
//!
//! Зачем: в P4 «профессиональный редактор» (нативная вкладка Equation WordCraft) отдавал LaTeX
//! через `equation.get`, и `sync_formula_from_native` копировал его в источник **без проверки**.
//! После конвертаций/навигации `equation.get` может вернуть не-LaTeX: Unicode-символы (`√`, `∑`),
//! плейсхолдеры линейного формата (`▒`, `█`, `⬚`) и не-LaTeX-структуру — и «Применить» записывал
//! это в `formula.toml` (инцидент A итерации P5). Здесь — единая точка проверки, которую обязаны
//! проходить все пути «получить LaTeX от чужого редактора → записать в источник».
//!
//! Правило (ADR-0021): ни один путь не пишет в `formula.toml` LaTeX, который
//! 1) содержит плейсхолдеры/Unicode-заменители LaTeX-команд;
//! 2) пуст; и по возможности 3) **молча** меняет структуру формулы — вызывающий код обязан
//! показать, что структура изменилась (это разрешено только явным действием пользователя).
//!
//! Модуль — часть LaTeX-first архитектуры: в Phase 2 здесь же появится собственный
//! roundtrip-safe сериализатор модели в LaTeX (вендорный `to_latex` не полностью таков — спайк
//! `docs/spikes/latex-editor.md` §3.3).

use wordcraft_engine::doc::math::{Arg, MNode};
use wordcraft_engine::doc::math_latex::{parse_latex, to_latex};

/// Плейсхолдеры линейного формата и Unicode-заменители LaTeX-команд: в источнике истины их быть
/// не может (LaTeX пишет `\sqrt`, `\sum`, `\int`). Пара (символ, пояснение для пользователя).
const FORBIDDEN_CHARS: [(char, &str); 6] = [
    ('▒', "плейсхолдер линейного формата"),
    ('█', "плейсхолдер линейного формата"),
    ('⬚', "плейсхолдер линейного формата"),
    ('√', "Unicode-корень вместо \\sqrt"),
    ('∑', "Unicode-сумма вместо \\sum"),
    ('∫', "Unicode-интеграл вместо \\int"),
];

/// Результат проверки кандидата в источник формулы.
#[derive(Clone, Debug, PartialEq)]
pub struct LatexCheck {
    /// Модель, в которую разобран кандидат (то, что реально попадёт в документ).
    pub model: Arg,
    /// Структура кандидата совпадает с текущим источником (семантическое сравнение AST).
    pub equivalent: bool,
    /// Человеческая сводка структуры кандидата (для сообщения «структура изменилась»).
    pub summary: String,
    /// Некритичные предупреждения (например, «структура изменилась — это явное действие»).
    pub warnings: Vec<String>,
}

/// Проверить кандидат `candidate` против текущего источника `source`.
///
/// `source` может быть пустым (режим создания формулы): тогда эквивалентность не проверяется,
/// но кандидат обязан быть валидным LaTeX без плейсхолдеров.
///
/// Возвращает `Err` с понятным текстом, если кандидат **нельзя** писать в `formula.toml`;
/// `Ok(LatexCheck)` — если можно (при `equivalent == false` вызывающий код обязан явно показать
/// пользователю, что структура меняется — только явное действие «Применить»/«Забрать LaTeX»).
pub fn check_formula_latex(source: &str, candidate: &str) -> Result<LatexCheck, String> {
    let candidate = candidate.trim();
    if candidate.is_empty() {
        return Err("LaTeX пуст".to_owned());
    }
    if let Some((ch, reason)) = forbidden_char(candidate) {
        return Err(format!("в LaTeX есть символ {ch:?} ({reason}) — это признак линейного формата, а не LaTeX-источника"));
    }
    let source = source.trim();
    let model = parse_latex(candidate);
    if model.is_empty() {
        return Err("LaTeX не разбирается в модель формулы (пустая модель)".to_owned());
    }
    // Кандидат не должен быть «эхом» линейного формата: канонический LaTeX из модели не должен
    // снова содержать односимвольные Unicode-заменители (они попадают в модель как Run).
    let canonical = to_latex(&model);
    if let Some((ch, reason)) = forbidden_char(&canonical) {
        return Err(format!("LaTeX нормализуется в строку с {ch:?} ({reason}) — источник не LaTeX-first"));
    }
    let source_model = parse_latex(source);
    let equivalent = !source.is_empty() && source_model == model;
    let mut warnings = Vec::new();
    if !source.is_empty() && !equivalent {
        warnings.push(format!("структура формулы изменится: было [{}], станет [{}]", structure_summary(&source_model), structure_summary(&model)));
    }
    let summary = structure_summary(&model);
    Ok(LatexCheck { model, equivalent, summary, warnings })
}

/// Первый запрещённый символ в строке (и причина) — для сообщений и для быстрых проверок.
pub fn forbidden_char(latex: &str) -> Option<(char, &'static str)> {
    for ch in latex.chars() {
        if let Some((_, reason)) = FORBIDDEN_CHARS.iter().find(|(bad, _)| *bad == ch) {
            return Some((ch, reason));
        }
    }
    None
}

/// Есть ли в строке признаки линейного формата (плейсхолдеры/Unicode-команды).
pub fn is_linear_latex(latex: &str) -> bool {
    forbidden_char(latex).is_some()
}

/// Короткая сводка структуры модели: «дроби 2, скрипты 3, …» (для UI/логов и диффа).
pub fn structure_summary(model: &[MNode]) -> String {
    let mut counts: [usize; 8] = [0; 8];
    fn walk(nodes: &[MNode], counts: &mut [usize; 8]) {
        for node in nodes {
            match node {
                MNode::Frac { num, den, .. } => {
                    counts[0] += 1;
                    walk(num, counts);
                    walk(den, counts);
                }
                MNode::Script { base, sub, sup, .. } => {
                    counts[1] += 1;
                    walk(base, counts);
                    walk(sub, counts);
                    walk(sup, counts);
                }
                MNode::Rad { deg, e, .. } => {
                    counts[2] += 1;
                    walk(deg, counts);
                    walk(e, counts);
                }
                MNode::Matrix { rows, .. } => {
                    counts[3] += 1;
                    for row in rows {
                        for cell in row {
                            walk(cell, counts);
                        }
                    }
                }
                MNode::Nary { sub, sup, e, .. } => {
                    counts[4] += 1;
                    walk(sub, counts);
                    walk(sup, counts);
                    walk(e, counts);
                }
                MNode::Delim { elems, .. } => {
                    counts[5] += 1;
                    for elem in elems {
                        walk(elem, counts);
                    }
                }
                MNode::EqArr { rows } => {
                    counts[6] += 1;
                    for row in rows {
                        walk(row, counts);
                    }
                }
                MNode::Acc { e, .. }
                | MNode::Bar { e, .. }
                | MNode::GroupChr { e, .. }
                | MNode::BorderBox { e, .. }
                | MNode::Boxed { e }
                | MNode::Phant { e, .. } => {
                    counts[7] += 1;
                    walk(e, counts);
                }
                MNode::Func { name, e } => {
                    walk(name, counts);
                    walk(e, counts);
                }
                MNode::Lim { e, lim, .. } => {
                    walk(e, counts);
                    walk(lim, counts);
                }
                MNode::Run(_) => {}
            }
        }
    }
    walk(model, &mut counts);
    let labels = ["дроби", "скрипты", "корни", "матрицы", "суммы/интегралы", "скобки", "выравнивания", "акценты"];
    let mut parts = Vec::new();
    for (label, count) in labels.iter().zip(counts.iter()) {
        if *count > 0 {
            parts.push(format!("{label} {count}"));
        }
    }
    if parts.is_empty() { "только символы".to_owned() } else { parts.join(", ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Реальный демо-источник (HEAD `formula-tab`): σ-формула.
    const SIGMA: &str = r"\sigma =\sqrt{\frac{1}{N-1}\sum_{i=1}^{N} {({x}_{i}}-\bar{x}{)}^{2}}";
    /// Испорченная строка из рабочего дерева владельца (P5, инцидент A): смесь LaTeX и линейного
    /// формата (`√`, `▒`, вывернутые скобки). В `formula.toml` она попасть не должна.
    const CORRUPTED: &str =
        r"\sigma =√(1/(N-1)\sum_{(} {i}=1{)}^{2N}▒(({x}_{i})-x{\bar{)}}^{2})";

    #[test]
    fn corrupted_native_latex_is_rejected() {
        let error = check_formula_latex(SIGMA, CORRUPTED).expect_err("мусор из equation.get отклонён");
        assert!(error.contains("√") && error.contains("плейсхолдер") || error.contains("Unicode"), "{error}");
        assert!(forbidden_char(CORRUPTED).is_some());
    }

    #[test]
    fn unchanged_source_is_equivalent() {
        let check = check_formula_latex(SIGMA, SIGMA).expect("валидный LaTeX");
        assert!(check.equivalent, "та же формула — эквивалентна");
        assert!(check.warnings.is_empty());
        // Нормализация пробелов не ломает эквивалентность (AST-сравнение, не строк).
        let spaced = r"  \sigma  =  \sqrt{\frac{1}{N-1}\sum_{i=1}^{N} {({x}_{i}}-\bar{x}{)}^{2}}  ";
        assert!(check_formula_latex(SIGMA, spaced).expect("LaTeX с пробелами").equivalent);
    }

    #[test]
    fn intentional_structure_edit_is_allowed_with_warning() {
        let edited = r"\sigma =\sqrt{\frac{1}{N-2}\sum_{i=1}^{N} {({x}_{i}}-\bar{x}{)}^{2}}";
        let check = check_formula_latex(SIGMA, edited).expect("валидная правка допустима");
        assert!(!check.equivalent, "правка меняет структуру");
        assert!(check.warnings.iter().any(|warning| warning.contains("изменится")), "{:?}", check.warnings);
    }

    #[test]
    fn empty_or_linear_only_candidates_are_rejected() {
        assert!(check_formula_latex(SIGMA, "   ").is_err());
        assert!(check_formula_latex(SIGMA, r"\sqrt{x}▒").is_err());
        assert!(check_formula_latex("", r"\frac{1}{2}").expect("новая формула").model.len() == 1);
        assert!(is_linear_latex(r"\sum_{i=1}^{N}▒i"));
    }

    #[test]
    fn structure_summary_reads_like_a_human_diff() {
        let model = parse_latex(SIGMA);
        let summary = structure_summary(&model);
        assert!(summary.contains("дроби"), "{summary}");
        assert!(summary.contains("корни"), "{summary}");
        assert!(summary.contains("суммы/интегралы"), "{summary}");
    }
}
