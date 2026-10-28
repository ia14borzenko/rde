//! MTEF-дерево → LaTeX (порт `makeLatex` из mtef-go v2.0, Apache-2.0, с дополнениями).
//!
//! Шаблоны MTEF (selector из MS «MTEF v5»): скобки (0–9), корень (10), дроби (11), под/надчёркивания
//! (12–13), стрелки (14), большие операторы (15–22), пределы (23), горизонтальные скобки (24–25),
//! деление (26), индексы (27–29), бра-кет (30), векторы (31), тильда/шляпа/дуга (32–34), статус (35),
//! зачёркивание (36), рамка (37), а также PILE (многострочная строка) и MATRIX.
//!
//! `mtef-go` не умел большие операторы (`∫`, `∑`, …) и часть шаблонов — здесь они реализованы;
//! неизвестный шаблон даёт [`MtefError::UnsupportedTemplate`], и импорт оставляет preview-картинку.

use super::chars::{CHARS, CHARS_MATH, SPECIAL};
use super::records::CharNode;
use super::tree::Ast;
use super::MtefError;

/// Стили набора MathType (`FONT_STYLE_DEF` индексы из MTEF v5): типографский стиль символа.
const FN_TEXT: u8 = 1;
const FN_MTEXTRA: u8 = 11;
const FN_SPACE: u8 = 24;

/// Собрать LaTeX из корневого списка объектов.
pub(super) fn emit(nodes: &[Ast]) -> Result<String, MtefError> {
    let mut out = String::new();
    emit_list(nodes, &mut out)?;
    Ok(out)
}

fn emit_list(nodes: &[Ast], out: &mut String) -> Result<(), MtefError> {
    for node in nodes {
        emit_node(node, out)?;
    }
    Ok(())
}

fn emit_children(children: &[Ast]) -> Result<String, MtefError> {
    let mut out = String::new();
    emit_list(children, &mut out)?;
    Ok(out)
}

/// LaTeX одного узла; пишет в `out`.
fn emit_node(node: &Ast, out: &mut String) -> Result<(), MtefError> {
    match node {
        Ast::Line { children, .. } => emit_list(children, out)?,
        Ast::Char(c) => out.push_str(&char_latex(c)),
        Ast::Pile { children } => emit_pile(children, out)?,
        Ast::Matrix { cols, children } => emit_matrix(*cols, children, out)?,
        Ast::Embell { kind, children } => emit_embell(*kind, children, out)?,
        Ast::Tmpl { selector, variation, children } => emit_template(*selector, *variation, children, out)?,
    }
    Ok(())
}

// ── символы ────────────────────────────────────────────────────────────────

/// LaTeX одного символа MTEF: по таблице MTCode→LaTeX, иначе — сам символ (с экранированием).
fn char_latex(c: &CharNode) -> String {
    let style = c.typeface.wrapping_sub(128);
    let mathmode = matches!(style, FN_MTEXTRA | FN_SPACE);
    let ch = char::from_u32(u32::from(c.mtcode)).unwrap_or('\u{FFFD}');
    let table = if mathmode { &CHARS_MATH[..] } else { &CHARS[..] };
    let mapped = lookup(table, c.mtcode);
    let mut out = match mapped {
        Some(latex) => latex.to_owned(),
        None => match special(ch) {
            Some(latex) => latex.to_owned(),
            None => escape_char(ch),
        },
    };
    if style == FN_TEXT {
        out = format!("\\mathrm{{{out}}}");
    }
    out
}

/// Двоичный поиск в отсортированной таблице `MTCode → LaTeX`.
fn lookup(table: &[(u32, &'static str)], mtcode: u16) -> Option<&'static str> {
    let code = u32::from(mtcode);
    table.binary_search_by_key(&code, |(key, _)| *key).ok().and_then(|index| table.get(index)).map(|(_, latex)| *latex)
}

/// Экранирование LaTeX-спецсимволов из таблицы `SPECIAL`.
fn special(ch: char) -> Option<&'static str> {
    let mut buf = [0u8; 4];
    let text = ch.encode_utf8(&mut buf);
    SPECIAL.iter().find(|(key, _)| *key == text).map(|(_, latex)| *latex)
}

/// Символ, которого нет в таблицах: экранируем LaTeX-спецсимволы, остальное — как есть
/// (Unicode-символы WordCraft рисует сам).
fn escape_char(ch: char) -> String {
    match ch {
        '\\' => "\\backslash ".to_owned(),
        '#' | '$' | '%' | '&' | '_' | '{' | '}' => format!("\\{ch}"),
        // `~` и `^` в математике — не текст: даём безопасные эквиваленты.
        '~' => "\\sim ".to_owned(),
        '^' => "\\hat{}".to_owned(),
        c if c.is_control() => String::new(),
        c => c.to_string(),
    }
}

// ── embellishments (акценты) ───────────────────────────────────────────────

/// Акцент над/под символом. Для dot/hat/bar дерево уже переставило узел перед символом, поэтому это
/// LaTeX-префикс; остальные embellishments печатаются как есть.
fn emit_embell(kind: u8, children: &[Ast], out: &mut String) -> Result<(), MtefError> {
    let prefix = match kind {
        2 => "\\dot ",
        3 => "\\ddot ",
        4 => "\\dddot ",
        5 => "'",
        6 => "''",
        7 => "'",
        8 => "\\tilde ",
        9 => "\\hat ",
        10 => "\\not ",
        11 => "\\overset{\\rightarrow}{}",
        12 => "\\overset{\\leftarrow}{}",
        13 => "\\overset{\\leftrightarrow}{}",
        16 => "\\bar ",
        17 => "\\bar ",
        18 => "'''",
        _ => "",
    };
    out.push_str(prefix);
    // Если у акцента есть содержимое (широкий акцент) — ставим его аргументом.
    if !children.is_empty() {
        let inner = emit_children(children)?;
        if !inner.trim().is_empty() {
            out.push('{');
            out.push_str(&inner);
            out.push('}');
        }
    }
    Ok(())
}

// ── PILE / MATRIX ──────────────────────────────────────────────────────────

/// PILE — вертикальный стек строк. Одна строка печатается как есть; несколько — `array` с `\\`,
/// чтобы WordCraft рисовал многострочную формулу столбиком.
fn emit_pile(children: &[Ast], out: &mut String) -> Result<(), MtefError> {
    let mut rows: Vec<String> = Vec::new();
    for child in children {
        rows.push(emit_children(std::slice::from_ref(child))?);
    }
    if rows.len() <= 1 {
        if let Some(row) = rows.first() {
            out.push_str(row);
        }
        return Ok(());
    }
    out.push_str("\\begin{array}{l} ");
    out.push_str(&rows.join(" \\\\ "));
    out.push_str(" \\end{array}");
    Ok(())
}

/// MATRIX — сетка `rows × cols` (`array` с `&` и `\\`). Первый ребёнок — служебный пустой LINE
/// (см. `records.rs`); далее идут ячейки построчно. Один столбец — многострочная формула (по левому
/// краю, как в MathType), больше — настоящая матрица по центру.
fn emit_matrix(cols: u8, children: &[Ast], out: &mut String) -> Result<(), MtefError> {
    let cols = usize::from(cols.max(1));
    let align = if cols == 1 { "l" } else { "c" };
    let cells: Vec<String> = children
        .iter()
        .skip(1) // служебный LINE-маркер
        .map(|child| emit_children(std::slice::from_ref(child)))
        .collect::<Result<_, _>>()?;
    out.push_str(&format!("\\begin{{array}}{{{align}}} "));
    for (index, cell) in cells.iter().enumerate() {
        out.push_str(cell);
        if index % cols == cols - 1 && index != cells.len().saturating_sub(1) {
            out.push_str(" \\\\ ");
        } else if index % cols != cols - 1 {
            out.push_str(" & ");
        }
    }
    out.push_str(" \\end{array}");
    Ok(())
}

// ── шаблоны ────────────────────────────────────────────────────────────────

fn child_slot(children: &[Ast], index: usize) -> Result<String, MtefError> {
    children.get(index).map_or_else(|| Ok(String::new()), |child| emit_children(std::slice::from_ref(child)))
}

fn emit_template(selector: u8, variation: u16, children: &[Ast], out: &mut String) -> Result<(), MtefError> {
    match selector {
        // Скобки: angle, paren, brace, brack, bar, dbar, floor, ceiling, open-brack.
        0..=9 => emit_fence(selector, children, out)?,
        // Корень: main + показатель степени (пустой — квадратный).
        10 => {
            let main = child_slot(children, 0)?;
            let index = child_slot(children, 1)?;
            if index.trim().is_empty() {
                out.push_str(&format!("\\sqrt{{{main}}}"));
            } else {
                out.push_str(&format!("\\sqrt[{index}]{{{main}}}"));
            }
        }
        // Дроби: variation 0x0002 — «слэш» (2R/c), иначе обычная дробь.
        11 => {
            let numerator = child_slot(children, 0)?;
            let denominator = child_slot(children, 1)?;
            if variation & 0x0002 != 0 {
                out.push_str(&format!("{{{numerator}}}/{{{denominator}}}"));
            } else {
                out.push_str(&format!("\\frac{{{numerator}}}{{{denominator}}}"));
            }
        }
        // Под-/надчёркивание.
        12 => out.push_str(&format!("\\underline{{{}}}", child_slot(children, 0)?)),
        13 => out.push_str(&format!("\\overline{{{}}}", child_slot(children, 0)?)),
        // Стрелка: верхний текст над стрелкой (направление по variation), нижний — под.
        14 => emit_arrow(variation, children, out)?,
        // Большие операторы (BigOpBox): main, lower, upper, operator.
        15..=22 => emit_big_op(selector, children, out)?,
        // Пределы.
        23 => {
            let main = child_slot(children, 0)?;
            let lower = child_slot(children, 1)?;
            let upper = child_slot(children, 2)?;
            if upper.trim().is_empty() {
                out.push_str(&format!("\\underset{{{lower}}}{{{main}}}"));
            } else {
                out.push_str(&format!("\\overset{{{upper}}}{{\\underset{{{lower}}}{{{main}}}}}"));
            }
        }
        // Горизонтальные скобки: variation 0x0001 — сверху, иначе снизу.
        24 | 25 => {
            let main = child_slot(children, 0)?;
            let over = variation & 0x0001 != 0;
            let command = match (selector, over) {
                (24, true) => "overbrace",
                (24, false) => "underbrace",
                (_, true) => "overbracket",
                (_, false) => "underbracket",
            };
            out.push_str(&format!("\\{command}{{{main}}}"));
        }
        // Деление в столбик: приближение — обычная строка (визуально отличается, помечается отчётом).
        26 => out.push_str(&child_slot(children, 0)?),
        // Индексы: sub, sup; variation 0x0001 — значки перед базой.
        27..=29 => emit_scripts(selector, variation, children, out)?,
        // Бра-кет: левая/правая часть по variation.
        30 => emit_dirac(variation, children, out)?,
        // Вектор: main + variation (направление, над/под).
        31 => emit_vector(variation, children, out)?,
        32 => out.push_str(&format!("\\tilde{{{}}}", child_slot(children, 0)?)),
        33 => {
            let main = child_slot(children, 0)?;
            let accent = child_slot(children, 1)?;
            if accent.trim().is_empty() {
                out.push_str(&format!("\\hat{{{main}}}"));
            } else {
                out.push_str(&format!("\\overset{{{accent}}}{{{main}}}"));
            }
        }
        34 => out.push_str(&format!("\\overparen{{{}}}", child_slot(children, 0)?)),
        35 => out.push_str(&emit_children(children)?),
        36 => out.push_str(&format!("\\overline{{{}}}", child_slot(children, 0)?)),
        37 => out.push_str(&format!("\\fbox{{{}}}", child_slot(children, 0)?)),
        _ => return Err(MtefError::UnsupportedTemplate { selector, variation }),
    }
    Ok(())
}

/// Скобки (ParBoxClass): main, left, right; открытая сторона — точка.
fn emit_fence(selector: u8, children: &[Ast], out: &mut String) -> Result<(), MtefError> {
    let main = child_slot(children, 0)?;
    let left = child_slot(children, 1)?;
    let right = child_slot(children, 2)?;
    let (left, right) = (fence_char(selector, left.trim(), true), fence_char(selector, right.trim(), false));
    // Фигурные скобки MathType строит как многострочный список — как и mtef-go, рисуем array.
    if selector == 2 {
        out.push_str(&format!("\\left{left} \\begin{{array}}{{l}} {main} \\end{{array}} \\right{right}"));
    } else {
        out.push_str(&format!("\\left{left} {main} \\right{right}"));
    }
    Ok(())
}

/// Символ скобки для `\left`/`\right`: пустая сторона — точка, `<`/`>` — `\langle`/`\rangle`.
fn fence_char(selector: u8, given: &str, left: bool) -> String {
    if given.is_empty() {
        return ".".to_owned();
    }
    match given {
        "<" => "\\langle".to_owned(),
        ">" => "\\rangle".to_owned(),
        "|" => "|".to_owned(),
        "||" => "\\|".to_owned(),
        "/" => "/".to_owned(),
        _ => {
            // Если MathType не прислал символ — берём по типу скобки.
            let default = match (selector, left) {
                (0, true) => "\\langle",
                (0, false) => "\\rangle",
                (1, true) => "(",
                (1, false) => ")",
                (2, true) => "\\{",
                (2, false) => "\\}",
                (3, true) => "[",
                (3, false) => "]",
                (4, _) => "|",
                (5, _) => "\\|",
                (6, true) => "\\lfloor",
                (6, false) => "\\rfloor",
                (7, true) => "\\lceil",
                (7, false) => "\\rceil",
                (8, true) => "[",
                (8, false) => "]",
                _ => ".",
            };
            default.to_owned()
        }
    }
}

/// Стрелка с надписью сверху/снизу.
fn emit_arrow(variation: u16, children: &[Ast], out: &mut String) -> Result<(), MtefError> {
    let top = child_slot(children, 0)?;
    let bottom = child_slot(children, 1)?;
    let points_left = variation & 0x0010 != 0 && variation & 0x0020 == 0;
    let arrow = if points_left { "\\leftarrow" } else { "\\rightarrow" };
    if bottom.trim().is_empty() {
        out.push_str(&format!("\\overset{{{top}}}{{{arrow}}}"));
    } else {
        out.push_str(&format!("\\overset{{{top}}}{{\\underset{{{bottom}}}{{{arrow}}}}}"));
    }
    Ok(())
}

/// Большие операторы (интеграл, сумма, произведение, объединение…).
fn emit_big_op(selector: u8, children: &[Ast], out: &mut String) -> Result<(), MtefError> {
    let main = child_slot(children, 0)?;
    let lower = child_slot(children, 1)?;
    let upper = child_slot(children, 2)?;
    let operator = child_slot(children, 3)?;
    let op = match selector {
        15 => "\\int".to_owned(),
        16 => "\\sum".to_owned(),
        17 => "\\prod".to_owned(),
        18 => "\\coprod".to_owned(),
        19 => "\\bigcup".to_owned(),
        20 => "\\bigcap".to_owned(),
        // 21/22 — «интеграл-стиль» и «сумма-стиль» с собственным оператором.
        _ => {
            if operator.trim().is_empty() {
                "\\int".to_owned()
            } else {
                operator.trim().to_owned()
            }
        }
    };
    out.push_str(&op);
    if !lower.trim().is_empty() {
        out.push_str(&format!("_{{{lower}}}"));
    }
    if !upper.trim().is_empty() {
        out.push_str(&format!("^{{{upper}}}"));
    }
    out.push(' ');
    out.push_str(&main);
    Ok(())
}

/// Под- и надстрочные индексы; `variation & 0x0001` — значки перед базой (prescripts).
fn emit_scripts(selector: u8, variation: u16, children: &[Ast], out: &mut String) -> Result<(), MtefError> {
    let sub = child_slot(children, 0)?;
    let sup = child_slot(children, 1)?;
    let precedes = variation & 0x0001 != 0;
    if precedes {
        out.push_str("{}");
    }
    let emit_sub = selector == 27 || selector == 29;
    let emit_sup = selector == 28 || selector == 29;
    if emit_sub && !sub.trim().is_empty() {
        out.push_str(&format!("_{{{sub}}}"));
    }
    if emit_sup && !sup.trim().is_empty() {
        out.push_str(&format!("^{{{sup}}}"));
    }
    Ok(())
}

/// Бра-кет (`⟨ψ|`): левая и правая черты по variation.
fn emit_dirac(variation: u16, children: &[Ast], out: &mut String) -> Result<(), MtefError> {
    let content = emit_children(children)?;
    if variation & 0x0001 != 0 {
        out.push_str("\\langle ");
    }
    out.push_str(&content);
    if variation & 0x0002 != 0 || variation & 0x0001 != 0 {
        out.push_str(" \\rangle");
    }
    Ok(())
}

/// Вектор: arrow над/под базой, направление по variation.
fn emit_vector(variation: u16, children: &[Ast], out: &mut String) -> Result<(), MtefError> {
    let main = child_slot(children, 0)?;
    let under = variation & 0x0004 != 0;
    let harpoon = variation & 0x0008 != 0;
    let left = variation & 0x0001 != 0 && variation & 0x0002 == 0;
    let arrow = match (harpoon, left) {
        (false, false) => "\\rightarrow",
        (false, true) => "\\leftarrow",
        (true, false) => "\\rightharpoonup",
        (true, true) => "\\leftharpoonup",
    };
    let command = if under { "\\underset" } else { "\\overset" };
    out.push_str(&format!("{command}{{{arrow}}}{{{main}}}"));
    Ok(())
}
