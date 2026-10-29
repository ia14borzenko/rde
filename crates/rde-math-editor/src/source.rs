//! LaTeX-кодек источника формулы (P5, ADR-0021 §Решение 2).
//!
//! Источник истины RDE — `formula.toml.latex`, а модель редактирования — OMML-дерево
//! `wordcraft_engine::doc::math` (то же, что у документа). Здесь:
//!
//! * [`from_latex`] — LaTeX → модель (вендорный `math_latex::parse_latex`; он никогда не падает);
//! * [`to_latex`] — модель → **канонический roundtrip-safe** LaTeX: собственный сериализатор,
//!   потому что вендорный `to_latex` не восстанавливается своим же парсером в 9/316 случаях
//!   (спайк [`docs/spikes/latex-editor.md`] §3.3: `\binom`→`\genfrac`, пробелы, array colspec,
//!   пустые строки матриц);
//! * [`check_roundtrip`] — проверка `parse(to_latex(model)) == model` перед записью в источник;
//! * [`normalize`] — приведение конструкций, которые нельзя выразить в LaTeX нашим парсером
//!   (`FracKind::NoBar/Skewed/Linear` вне `\binom`, `Delim::sep` с несколькими элементами) к
//!   представимому виду — с сохранением вида (binom остаётся binom, см. `Delim`-паттерн).
//!
//! Правила: сериализатор не пишет Unicode-плейсхолдеры (`▒`, `█`, `⬚`); все команды — те, что
//! понимает парсер; результат обязан пере-парситься в ту же модель (тесты `tests/roundtrip.rs`).

use wordcraft_engine::doc::math::{Arg, ColJc, FracKind, LimLoc, MNode, MRun, MScr, MSty, ScriptKind};
use wordcraft_engine::doc::math_linear::FUNCTION_NAMES;
use wordcraft_engine::doc::math_latex::parse_latex;
use wordcraft_engine::doc::math_symbols::name_of;

/// Имена символов, которые парсер трактует как команды, а не как косвенно-экранированный символ:
/// их нельзя выводить через `name_of` (иначе LaTeX-источник поменяет смысл).
const NOT_SYMBOL_NAMES: &[&str] = &["sqrt", "matrix", "of", "above", "below", "box", "rect"];

/// Разобрать LaTeX-источник в модель формулы. Никогда не падает (пустая строка — пустая модель).
#[must_use]
pub fn from_latex(latex: &str) -> Arg {
    parse_latex(latex)
}

/// Ошибка roundtrip: источник не может быть канонически сериализован так, чтобы вернуться в модель.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundtripError {
    /// Что сериализатор выдал (для отладки/сообщения).
    pub latex: String,
    /// В чём расхождение (человеческим языком).
    pub detail: String,
}

/// Проверить, что модель сериализуется в LaTeX, который парсится обратно в **ту же** модель.
pub fn check_roundtrip(model: &[MNode]) -> Result<String, RoundtripError> {
    let latex = to_latex(model);
    if latex.is_empty() && !model.is_empty() {
        return Err(RoundtripError { latex, detail: "сериализатор выдал пустой LaTeX для непустой модели".to_owned() });
    }
    let back = from_latex(&latex);
    if back != model {
        return Err(RoundtripError {
            latex,
            detail: format!("LaTeX не восстанавливается в ту же модель: было [{}], стало [{}]", node_brief(model), node_brief(&back)),
        });
    }
    Ok(latex)
}

fn node_brief(model: &[MNode]) -> String {
    fn walk(nodes: &[MNode], counts: &mut (usize, usize)) {
        for node in nodes {
            counts.0 += 1;
            match node {
                MNode::Frac { num, den, .. } => {
                    walk(num, counts);
                    walk(den, counts);
                }
                MNode::Script { base, sub, sup, .. } => {
                    walk(base, counts);
                    walk(sub, counts);
                    walk(sup, counts);
                }
                MNode::Rad { deg, e, .. } => {
                    walk(deg, counts);
                    walk(e, counts);
                }
                MNode::Nary { sub, sup, e, .. } => {
                    walk(sub, counts);
                    walk(sup, counts);
                    walk(e, counts);
                }
                MNode::Delim { elems, .. } => elems.iter().for_each(|e| walk(e, counts)),
                MNode::Func { name, e } => {
                    walk(name, counts);
                    walk(e, counts);
                }
                MNode::Lim { e, lim, .. } => {
                    walk(e, counts);
                    walk(lim, counts);
                }
                MNode::Acc { e, .. }
                | MNode::Bar { e, .. }
                | MNode::BorderBox { e, .. }
                | MNode::Boxed { e }
                | MNode::GroupChr { e, .. }
                | MNode::Phant { e, .. } => walk(e, counts),
                MNode::Matrix { rows, .. } => rows.iter().flat_map(|row| row.iter()).for_each(|cell| walk(cell, counts)),
                MNode::EqArr { rows } => rows.iter().for_each(|row| walk(row, counts)),
                MNode::Run(_) => counts.1 += 1,
            }
        }
    }
    let mut counts = (0usize, 0usize);
    walk(model, &mut counts);
    format!("узлов {}, run {}", counts.0, counts.1)
}

/// Канонический LaTeX модели (roundtrip-safe для модели, пришедшей из [`from_latex`]).
#[must_use]
pub fn to_latex(model: &[MNode]) -> String {
    let mut out = String::new();
    nodes(model, &mut out, 0);
    out.trim().to_owned()
}

/// Проверить, что модель не содержит конструкций, не представимых в LaTeX-источнике.
/// Возвращает человекочитаемые предупреждения (для UI/логов); запись блокирует `check_roundtrip`.
pub fn unrepresentable(model: &Arg) -> Vec<String> {
    let mut warnings = Vec::new();
    fn walk(nodes: &[MNode], top: bool, warnings: &mut Vec<String>) {
        for (index, node) in nodes.iter().enumerate() {
            match node {
                MNode::Frac { kind, num, den } => {
                    if !top && *kind != FracKind::Bar {
                        warnings.push("дробь без черты/наклонная нормализуется в обычную (`\\frac`)".to_owned());
                    }
                    walk(num, false, warnings);
                    walk(den, false, warnings);
                }
                MNode::Run(run) => {
                    if run.text.chars().any(|c| matches!(c, '▒' | '█' | '⬚' | '\u{FFFC}')) {
                        warnings.push("в тексте есть плейсхолдер линейного формата".to_owned());
                    }
                }
                MNode::Delim { elems, .. } => elems.iter().for_each(|e| walk(e, false, warnings)),
                MNode::Script { base, sub, sup, .. } => {
                    walk(base, false, warnings);
                    walk(sub, false, warnings);
                    walk(sup, false, warnings);
                }
                MNode::Rad { deg, e, .. } => {
                    walk(deg, false, warnings);
                    walk(e, false, warnings);
                }
                MNode::Nary { sub, sup, e, .. } => {
                    walk(sub, false, warnings);
                    walk(sup, false, warnings);
                    walk(e, false, warnings);
                }
                MNode::Func { name, e } => {
                    walk(name, false, warnings);
                    walk(e, false, warnings);
                }
                MNode::Lim { e, lim, .. } => {
                    walk(e, false, warnings);
                    walk(lim, false, warnings);
                }
                MNode::Acc { e, .. }
                | MNode::Bar { e, .. }
                | MNode::BorderBox { e, .. }
                | MNode::Boxed { e }
                | MNode::GroupChr { e, .. }
                | MNode::Phant { e, .. } => walk(e, false, warnings),
                MNode::Matrix { rows, .. } => rows.iter().flat_map(|row| row.iter()).for_each(|cell| walk(cell, false, warnings)),
                MNode::EqArr { rows } => rows.iter().for_each(|row| walk(row, false, warnings)),
            }
            let _ = index;
        }
    }
    walk(model, true, &mut warnings);
    warnings.sort();
    warnings.dedup();
    warnings
}

/// Привести модель к виду, представимому в LaTeX (до сериализации; идемпотентно).
///
/// Что нормализуется (честно фиксируется в ADR-0021 как ограничение MVP):
/// * `FracKind::{NoBar, Skewed, Linear}` вне `\binom`-паттерна → `Bar` (обычная `\frac`);
/// * `Delim` с несколькими элементами и `sep` сохраняется (парсер понимает `\middle`);
/// * `Run` с плейсхолдерами не трогаем — их отклонит `check_roundtrip`/валидация источника.
pub fn normalize(model: &mut Arg) {
    normalize_arg(model, false);
    dissolve_complex_functions(model);
    drop_trailing_empty_rows(model);
}

/// `Func` с составным именем (`sin^{-1}` и т.п.) не выражается LaTeX-командой: разворачиваем в
/// последовательность (имя, аргумент) — визуально то же, семантика `m:func` теряется (ограничение MVP).
fn dissolve_complex_functions(nodes: &mut Arg) {
    let mut index = 0;
    while index < nodes.len() {
        let dissolved = match &nodes[index] {
            MNode::Func { name, e } => {
                let simple = matches!(name.as_slice(), [MNode::Run(run)] if run.sty == Some(MSty::Plain) && FUNCTION_NAMES.contains(&run.text.as_str()));
                if simple {
                    None
                } else {
                    let mut name = name.clone();
                    dissolve_complex_functions(&mut name);
                    let mut body = e.clone();
                    dissolve_complex_functions(&mut body);
                    Some((name, body))
                }
            }
            _ => None,
        };
        if let Some((name, body)) = dissolved {
            nodes.splice(index..=index, name.into_iter().chain(body));
            index += 1;
        } else {
            index += 1;
        }
    }
    for node in nodes.iter_mut() {
        match node {
            MNode::Frac { num, den, .. } => {
                dissolve_complex_functions(num);
                dissolve_complex_functions(den);
            }
            MNode::Script { base, sub, sup, .. } => {
                dissolve_complex_functions(base);
                dissolve_complex_functions(sub);
                dissolve_complex_functions(sup);
            }
            MNode::Rad { deg, e, .. } => {
                dissolve_complex_functions(deg);
                dissolve_complex_functions(e);
            }
            MNode::Nary { sub, sup, e, .. } => {
                dissolve_complex_functions(sub);
                dissolve_complex_functions(sup);
                dissolve_complex_functions(e);
            }
            MNode::Delim { elems, .. } => elems.iter_mut().for_each(dissolve_complex_functions),
            MNode::Lim { e, lim, .. } => {
                dissolve_complex_functions(e);
                dissolve_complex_functions(lim);
            }
            MNode::Acc { e, .. }
            | MNode::Bar { e, .. }
            | MNode::BorderBox { e, .. }
            | MNode::Boxed { e }
            | MNode::GroupChr { e, .. }
            | MNode::Phant { e, .. } => dissolve_complex_functions(e),
            MNode::Matrix { rows, .. } => rows.iter_mut().flat_map(|row| row.iter_mut()).for_each(dissolve_complex_functions),
            MNode::EqArr { rows } => rows.iter_mut().for_each(dissolve_complex_functions),
            MNode::Run(_) | MNode::Func { .. } => {}
        }
    }
}

/// Убрать хвостовые пустые строки матриц/выравниваний: парсер не сохраняет «висячий» `\\`
/// (ограничение MVP: пустые хвостовые строки не представимы в LaTeX-источнике).
fn drop_trailing_empty_rows(nodes: &mut Arg) {
    for node in nodes.iter_mut() {
        match node {
            MNode::Matrix { rows, .. } => {
                while rows.len() > 1 && rows.last().is_some_and(|row| row.iter().all(Vec::is_empty)) {
                    rows.pop();
                }
                for row in rows.iter_mut() {
                    for cell in row.iter_mut() {
                        drop_trailing_empty_rows(cell);
                    }
                }
            }
            MNode::EqArr { rows } => {
                while rows.len() > 1 && rows.last().is_some_and(Vec::is_empty) {
                    rows.pop();
                }
                rows.iter_mut().for_each(drop_trailing_empty_rows);
            }
            MNode::Frac { num, den, .. } => {
                drop_trailing_empty_rows(num);
                drop_trailing_empty_rows(den);
            }
            MNode::Script { base, sub, sup, .. } => {
                drop_trailing_empty_rows(base);
                drop_trailing_empty_rows(sub);
                drop_trailing_empty_rows(sup);
            }
            MNode::Rad { deg, e, .. } => {
                drop_trailing_empty_rows(deg);
                drop_trailing_empty_rows(e);
            }
            MNode::Nary { sub, sup, e, .. } => {
                drop_trailing_empty_rows(sub);
                drop_trailing_empty_rows(sup);
                drop_trailing_empty_rows(e);
            }
            MNode::Delim { elems, .. } => elems.iter_mut().for_each(drop_trailing_empty_rows),
            MNode::Func { name, e } => {
                drop_trailing_empty_rows(name);
                drop_trailing_empty_rows(e);
            }
            MNode::Lim { e, lim, .. } => {
                drop_trailing_empty_rows(e);
                drop_trailing_empty_rows(lim);
            }
            MNode::Acc { e, .. }
            | MNode::Bar { e, .. }
            | MNode::BorderBox { e, .. }
            | MNode::Boxed { e }
            | MNode::GroupChr { e, .. }
            | MNode::Phant { e, .. } => drop_trailing_empty_rows(e),
            MNode::Run(_) => {}
        }
    }
}

fn normalize_arg(nodes: &mut Arg, binomial: bool) {
    for node in nodes.iter_mut() {
        match node {
            MNode::Frac { kind, num, den } => {
                if !binomial {
                    *kind = FracKind::Bar;
                }
                normalize_arg(num, false);
                normalize_arg(den, false);
            }
            MNode::Delim { elems, .. } => {
                let binomial_delim = matches!(elems.as_slice(), [one] if is_binomial_body(one));
                for elem in elems.iter_mut() {
                    normalize_arg(elem, binomial_delim);
                }
            }
            MNode::Script { base, sub, sup, .. } => {
                normalize_arg(base, false);
                normalize_arg(sub, false);
                normalize_arg(sup, false);
            }
            MNode::Rad { deg, e, .. } => {
                normalize_arg(deg, false);
                normalize_arg(e, false);
            }
            MNode::Nary { sub, sup, e, .. } => {
                normalize_arg(sub, false);
                normalize_arg(sup, false);
                normalize_arg(e, false);
            }
            MNode::Func { name, e } => {
                normalize_arg(name, false);
                normalize_arg(e, false);
            }
            MNode::Lim { e, lim, .. } => {
                normalize_arg(e, false);
                normalize_arg(lim, false);
            }
            MNode::Acc { e, .. }
            | MNode::Bar { e, .. }
            | MNode::BorderBox { e, .. }
            | MNode::Boxed { e }
            | MNode::GroupChr { e, .. }
            | MNode::Phant { e, .. } => normalize_arg(e, false),
            MNode::Matrix { rows, .. } => {
                for row in rows.iter_mut() {
                    for cell in row.iter_mut() {
                        normalize_arg(cell, false);
                    }
                }
            }
            MNode::EqArr { rows } => rows.iter_mut().for_each(|row| normalize_arg(row, false)),
            MNode::Run(_) => {}
        }
    }
}

/// `\binom{n}{k}` разбирается как `Delim('(',')', elems=[[Frac NoBar]])` — узнаём паттерн, чтобы
/// сериализовать обратно в `\binom`, а не терять дробь без черты.
fn is_binomial_body(elems: &[MNode]) -> bool {
    matches!(elems, [MNode::Frac { kind: FracKind::NoBar, .. }])
}

// ───────────────────────────── сериализация ────────────────────────────────────────────────

fn nodes(model: &[MNode], out: &mut String, depth: usize) {
    if depth > 128 {
        return;
    }
    for node in model {
        node_to_latex(node, out, depth + 1);
    }
}

fn braced(arg: &[MNode], out: &mut String, depth: usize) {
    out.push('{');
    nodes(arg, out, depth);
    out.push('}');
}

fn run_to_latex(run: &MRun, out: &mut String) {
    if run.nor {
        out.push_str("\\text{");
        for c in run.text.chars() {
            match c {
                '{' | '}' | '%' | '$' | '#' | '&' | '_' | '~' => {
                    out.push('\\');
                    out.push(c);
                }
                '\\' => out.push_str("\\backslash{}"),
                c => out.push(c),
            }
        }
        out.push('}');
        return;
    }
    let (open, close) = match (run.sty, run.scr) {
        (_, MScr::DoubleStruck) => ("\\mathbb{", "}"),
        (_, MScr::Script) => ("\\mathcal{", "}"),
        (_, MScr::Fraktur) => ("\\mathfrak{", "}"),
        (_, MScr::SansSerif) => ("\\mathsf{", "}"),
        (_, MScr::Monospace) => ("\\mathtt{", "}"),
        (Some(MSty::Bold), _) => ("\\mathbf{", "}"),
        (Some(MSty::BoldItalic), _) => ("\\boldsymbol{", "}"),
        (Some(MSty::Plain), _) if run.text.chars().any(|c| c.is_alphabetic()) => ("\\mathrm{", "}"),
        _ => ("", ""),
    };
    out.push_str(open);
    for c in run.text.chars() {
        escape_run_char(c, out);
    }
    out.push_str(close);
}

fn escape_run_char(c: char, out: &mut String) {
    match c {
        // Обычный пробел вне `\text{}` LaTeX не читает (пропускается токенайзером) — пишем
        // control-space `\ `, который парсер превращает в Run(" ") (roundtrip).
        ' ' => out.push_str("\\ "),
        '\t' => out.push_str("\\ "),
        '{' | '}' | '%' | '$' | '#' | '&' | '_' => {
            out.push('\\');
            out.push(c);
        }
        '\\' => out.push_str("\\backslash "),
        c if c.is_ascii() => out.push(c),
        c => match name_of(c) {
            Some(name) if !NOT_SYMBOL_NAMES.contains(&name) => {
                out.push('\\');
                out.push_str(name);
                out.push(' ');
            }
            _ => out.push(c),
        },
    }
}

fn command_for_char(c: char) -> Option<&'static str> {
    name_of(c).filter(|name| !NOT_SYMBOL_NAMES.contains(name))
}

fn node_to_latex(node: &MNode, out: &mut String, depth: usize) {
    match node {
        MNode::Run(run) => run_to_latex(run, out),
        MNode::Frac { kind: FracKind::Bar, num, den } => {
            out.push_str("\\frac");
            braced(num, out, depth);
            braced(den, out, depth);
        }
        // Прочие виды дробей в источнике не представимы — нормализуются `normalize`; если всё же
        // дошли сюда, пишем обычную дробь (визуально черта появится — фиксируется в ADR).
        MNode::Frac { num, den, .. } => {
            out.push_str("\\frac");
            braced(num, out, depth);
            braced(den, out, depth);
        }
        MNode::Script { kind, base, sub, sup } => {
            if *kind == ScriptKind::Pre {
                out.push_str("{}_");
                braced(sub, out, depth);
                out.push('^');
                braced(sup, out, depth);
                braced(base, out, depth);
                return;
            }
            braced(base, out, depth);
            if matches!(kind, ScriptKind::Sub | ScriptKind::SubSup) {
                out.push('_');
                braced(sub, out, depth);
            }
            if matches!(kind, ScriptKind::Sup | ScriptKind::SubSup) {
                out.push('^');
                braced(sup, out, depth);
            }
        }
        MNode::Rad { deg, deg_hide, e } => {
            out.push_str("\\sqrt");
            if !*deg_hide && !deg.is_empty() {
                out.push('[');
                nodes(deg, out, depth);
                out.push(']');
            }
            braced(e, out, depth);
        }
        MNode::Nary { chr, lim_loc, sub_hide, sup_hide, sub, sup, e, .. } => {
            match command_for_char(*chr) {
                Some(name) => {
                    out.push('\\');
                    out.push_str(name);
                    out.push(' ');
                }
                None => out.push(*chr),
            }
            match lim_loc {
                Some(LimLoc::UndOvr) => out.push_str("\\limits"),
                Some(LimLoc::SubSup) if !wordcraft_engine::doc::math::is_integral(*chr) => out.push_str("\\nolimits"),
                _ => {}
            }
            if !*sub_hide && !sub.is_empty() {
                out.push('_');
                braced(sub, out, depth);
            }
            if !*sup_hide && !sup.is_empty() {
                out.push('^');
                braced(sup, out, depth);
            }
            braced(e, out, depth);
        }
        MNode::Delim { beg, end, sep, elems, .. } => {
            // `\binom{n}{k}` — особый паттерн парсера: Delim('(',')') с одной дробью без черты.
            if matches!((beg, end), (Some('('), Some(')')))
                && matches!(elems.as_slice(), [one] if is_binomial_body(one))
                && sep.is_none()
                && let [MNode::Frac { num, den, .. }] = elems[0].as_slice()
            {
                out.push_str("\\binom");
                braced(num, out, depth);
                braced(den, out, depth);
                return;
            }
            out.push_str("\\left");
            out.push_str(&delimiter_latex(*beg));
            let middle = match sep {
                Some(c) => format!("\\middle{}", delimiter_latex(Some(*c))),
                None => "\\middle|".to_owned(),
            };
            for (index, elem) in elems.iter().enumerate() {
                if index > 0 {
                    out.push_str(&middle);
                }
                nodes(elem, out, depth);
            }
            out.push_str("\\right");
            out.push_str(&delimiter_latex(*end));
        }
        MNode::Func { name, e } => {
            // Функция с пределами (`\lim_{…}`, `\max_{…}`): парсер собирает имя как `Lim`, а всё
            // вместе — `Func{name:[Lim]}`; обратно пишется командой имени с `_`-пределом и телом.
            if let [MNode::Lim { upper: false, e: limit_name, lim }] = name.as_slice()
                && let [MNode::Run(run)] = limit_name.as_slice()
                && run.sty == Some(MSty::Plain)
                && FUNCTION_NAMES.contains(&run.text.as_str())
            {
                out.push('\\');
                out.push_str(&run.text);
                out.push('_');
                braced(lim, out, depth);
                if e.len() == 1 && matches!(e[0], MNode::Run(_)) {
                    nodes(e, out, depth);
                } else {
                    braced(e, out, depth);
                }
                return;
            }
            // Простая именованная функция (`\sin x`) — специальная форма парсера: имя как команда,
            // тело — следующий атом/группа без скобок.
            let simple = match name.as_slice() {
                [MNode::Run(run)] if !run.nor && run.sty == Some(MSty::Plain) && FUNCTION_NAMES.contains(&run.text.as_str()) => Some(run.text.as_str()),
                _ => None,
            };
            match simple {
                Some(fname) => {
                    out.push('\\');
                    out.push_str(fname);
                    out.push(' ');
                    if e.len() == 1 && matches!(e[0], MNode::Run(_)) {
                        nodes(e, out, depth);
                    } else {
                        braced(e, out, depth);
                    }
                }
                None => {
                    out.push_str("\\operatorname{");
                    let mut name_latex = String::new();
                    nodes(name, &mut name_latex, depth);
                    // В `\operatorname{}` текст читается как обычный: снимаем обёртки \mathrm{}.
                    let trimmed = name_latex.trim();
                    let unmath = trimmed.strip_prefix("\\mathrm{").and_then(|rest| rest.strip_suffix('}')).unwrap_or(trimmed);
                    out.push_str(unmath);
                    out.push('}');
                    braced(e, out, depth);
                }
            }
        }
        MNode::Lim { upper, e, lim } => {
            let simple = matches!(e.as_slice(), [MNode::Run(run)] if run.sty == Some(MSty::Plain) && FUNCTION_NAMES.contains(&run.text.as_str()));
            if simple && !*upper {
                let [MNode::Run(run)] = e.as_slice() else { return };
                out.push('\\');
                out.push_str(&run.text);
                out.push('_');
                braced(lim, out, depth);
            } else {
                out.push_str(if *upper { "\\overset" } else { "\\underset" });
                braced(lim, out, depth);
                braced(e, out, depth);
            }
        }
        MNode::Acc { chr, e } => {
            let command = match chr {
                '\u{302}' => "hat",
                '\u{303}' => "tilde",
                '\u{307}' => "dot",
                '\u{308}' => "ddot",
                '\u{20DB}' => "dddot",
                '\u{30C}' => "check",
                '\u{306}' => "breve",
                '\u{301}' => "acute",
                '\u{300}' => "grave",
                '\u{305}' | '\u{AF}' => "bar",
                '\u{20D6}' => "overleftarrow",
                '\u{20E1}' => "overleftrightarrow",
                _ => "vec",
            };
            out.push('\\');
            out.push_str(command);
            braced(e, out, depth);
        }
        MNode::Bar { top, e } => {
            out.push_str(if *top { "\\overline" } else { "\\underline" });
            braced(e, out, depth);
        }
        MNode::GroupChr { chr, top, e } => {
            let command = match chr {
                '⏜' => "overparen",
                '⏝' => "underparen",
                '⎴' => "overbracket",
                '⎵' => "underbracket",
                _ if *top => "overbrace",
                _ => "underbrace",
            };
            out.push('\\');
            out.push_str(command);
            braced(e, out, depth);
        }
        MNode::BorderBox { e, .. } => {
            out.push_str("\\boxed");
            braced(e, out, depth);
        }
        MNode::Boxed { e } => braced(e, out, depth),
        MNode::Matrix { rows, col_jc } => {
            let spec: Option<String> = if col_jc.is_empty() {
                None
            } else {
                Some(col_jc.iter().map(|jc| match jc { ColJc::Left => 'l', ColJc::Right => 'r', ColJc::Center => 'c' }).collect())
            };
            if let Some(spec) = &spec {
                out.push_str("\\begin{array}{");
                out.push_str(spec);
                out.push('}');
            } else {
                out.push_str("\\begin{matrix}");
            }
            for (row_index, row) in rows.iter().enumerate() {
                if row_index > 0 {
                    out.push_str("\\\\");
                }
                for (cell_index, cell) in row.iter().enumerate() {
                    if cell_index > 0 {
                        out.push('&');
                    }
                    nodes(cell, out, depth);
                }
            }
            if spec.is_some() {
                out.push_str("\\end{array}");
            } else {
                out.push_str("\\end{matrix}");
            }
        }
        MNode::EqArr { rows } => {
            // `cases`/`aligned` — EqArr со строками, склеенными `&`-run'ами; распознаём для
            // красивого и roundtrip-точного LaTeX.
            out.push_str("\\begin{aligned}");
            for (index, row) in rows.iter().enumerate() {
                if index > 0 {
                    out.push_str("\\\\");
                }
                nodes(row, out, depth);
            }
            out.push_str("\\end{aligned}");
        }
        MNode::Phant { show: false, zero_wid: false, zero_asc: false, zero_desc: false, e } => {
            out.push_str("\\phantom");
            braced(e, out, depth);
        }
        MNode::Phant { e, .. } => {
            // Прочие виды `m:phant` (скрытые по одной оси) парсер не выражает — рисуем как обычную
            // группу (нормализация; фиксируется как ограничение MVP).
            braced(e, out, depth);
        }
    }
}

fn delimiter_latex(delim: Option<char>) -> String {
    match delim {
        None => ".".to_owned(),
        Some('{') => "\\{".to_owned(),
        Some('}') => "\\}".to_owned(),
        Some('‖') => "\\|".to_owned(),
        Some('|') => "|".to_owned(),
        Some(c) if c.is_ascii() => c.to_string(),
        Some(c) => match command_for_char(c) {
            Some(name) => format!("\\{name} "),
            None => c.to_string(),
        },
    }
}

/// Проверить, является ли LaTeX-строка пригодной к записи в источник: разбирается в непустую модель
/// и сериализуется обратно без потерь.
pub fn validate_source_latex(latex: &str) -> Result<(), String> {
    let model = from_latex(latex);
    if model.is_empty() {
        return Err("LaTeX пуст или не разбирается в модель формулы".to_owned());
    }
    check_roundtrip(&model).map(|_| ()).map_err(|error| error.detail)
}
