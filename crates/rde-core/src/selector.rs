//! Селекторы выходов артефакта (tz-02 §7.1): относительный путь или glob.
//!
//! Вход артефакта может указывать селектор выхода другого артефакта (`out/*.csv`,
//! `out/realizations/**/*.dat`). Здесь — единая реализация сопоставления и раскрытия
//! селектора в список файлов; её используют `rde-core` (проверка наличия входов) и
//! `rde-dag` (in_hash и материализация входов запуска), чтобы семантика не разъезжалась.
//!
//! Поддерживаемые шаблоны (минимальный POSIX-glob без скобочных классов и экранирования):
//! - `*` — любая последовательность символов внутри одного сегмента пути (кроме `/`);
//! - `?` — ровно один символ внутри сегмента;
//! - `**` как отдельный сегмент — ноль и более сегментов пути.
//!
//! Скрытые файлы/каталоги (имя начинается с `.`) при раскрытии glob пропускаются:
//! это временные файлы RDE и служебные каталоги.

use std::fs;
use std::path::{Path, PathBuf};

/// Файл, выбранный селектором: абсолютный путь и путь относительно базы селектора.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectorMatch {
    pub path: PathBuf,
    pub rel: String,
}

/// Есть ли в селекторе glob-метасимволы.
pub fn is_glob(selector: &str) -> bool {
    selector.contains('*') || selector.contains('?')
}

/// Сопоставить относительный путь шаблону-глобу.
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match_segments(&pat, &segs)
}

fn match_segments(pat: &[&str], path: &[&str]) -> bool {
    match pat.split_first() {
        None => path.is_empty(),
        Some((&"**", rest)) => {
            // `**` — ноль сегментов...
            if match_segments(rest, path) {
                return true;
            }
            // ...или один и более: пробуем «съесть» сегмент и остаться на том же `**`.
            match path.split_first() {
                Some((_, tail)) => match_segments(pat, tail),
                None => false,
            }
        }
        Some((segment, rest)) => match path.split_first() {
            Some((name, tail)) => match_segment(segment, name) && match_segments(rest, tail),
            None => false,
        },
    }
}

/// Классическое сопоставление с `*`/`?` внутри одного сегмента (итеративное, без рекурсии).
fn match_segment(pattern: &str, text: &str) -> bool {
    let pat: Vec<char> = pattern.chars().collect();
    let txt: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while ti < txt.len() {
        if pi < pat.len() && (pat[pi] == '?' || pat[pi] == txt[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < pat.len() && pat[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(star_pi) = star {
            pi = star_pi + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < pat.len() && pat[pi] == '*' {
        pi += 1;
    }
    pi == pat.len()
}

/// Раскрыть селектор относительно `base` в отсортированный по `rel` список файлов.
///
/// - точный путь: файл должен существовать, иначе `Err`;
/// - glob: обход `base` (скрытые файлы/каталоги пропускаются), пустой результат — `Err`.
pub fn expand_selector(base: &Path, selector: &str) -> Result<Vec<SelectorMatch>, String> {
    let selector = selector.trim();
    if selector.is_empty() {
        return Err("пустой селектор выхода".to_owned());
    }
    if !is_glob(selector) {
        let path = base.join(selector);
        if !path.is_file() {
            return Err(format!("файл `{selector}` не найден"));
        }
        return Ok(vec![SelectorMatch { path, rel: selector.to_owned() }]);
    }
    let mut out = Vec::new();
    let mut stack: Vec<(PathBuf, String)> = vec![(base.to_path_buf(), String::new())];
    while let Some((dir, rel_dir)) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with('.') {
                continue;
            }
            let rel = if rel_dir.is_empty() { name.to_owned() } else { format!("{rel_dir}/{name}") };
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, rel));
            } else if glob_match(selector, &rel) {
                out.push(SelectorMatch { path, rel });
            }
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    if out.is_empty() {
        return Err(format!("селектор `{selector}` не совпал ни с одним файлом"));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches_segments_and_wildcards() {
        assert!(glob_match("out/*.csv", "out/a.csv"));
        assert!(!glob_match("out/*.csv", "out/sub/a.csv"), "`*` не переходит через `/`");
        assert!(glob_match("out/**/*.csv", "out/sub/a.csv"));
        assert!(glob_match("out/**/*.csv", "out/a.csv"), "`**` матчит ноль сегментов");
        assert!(glob_match("out/**", "out/a/b/c.png"));
        assert!(glob_match("out/?.png", "out/a.png"));
        assert!(!glob_match("out/?.png", "out/ab.png"));
        assert!(glob_match("a*b?c", "axxbxc"));
        assert!(!glob_match("out/a.csv", "out/b.csv"));
    }

    #[test]
    fn expand_selector_handles_files_globs_and_errors() {
        let base = std::env::temp_dir().join(format!("rde-sel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("out/sub")).unwrap();
        fs::write(base.join("out/a.csv"), "a").unwrap();
        fs::write(base.join("out/sub/b.csv"), "b").unwrap();
        fs::write(base.join("out/sub/c.png"), "c").unwrap();
        fs::write(base.join("out/.hidden.csv"), "h").unwrap();

        let exact = expand_selector(&base, "out/a.csv").unwrap();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].rel, "out/a.csv");

        let all_csv = expand_selector(&base, "out/*.csv").unwrap();
        assert_eq!(all_csv.len(), 1, "скрытые файлы пропускаются");
        let recursive = expand_selector(&base, "out/**/*.csv").unwrap();
        assert_eq!(recursive.iter().map(|m| m.rel.as_str()).collect::<Vec<_>>(), vec!["out/a.csv", "out/sub/b.csv"]);

        assert!(expand_selector(&base, "out/missing.csv").is_err());
        assert!(expand_selector(&base, "out/*.dat").is_err());
        let _ = fs::remove_dir_all(&base);
    }
}
