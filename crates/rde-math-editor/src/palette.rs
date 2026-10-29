//! Палитры MathType-класса для редактора формулы (P5, ADR-0021 §Решение 1):
//! структуры (`math_gallery`: 222 шаблона), наборы символов, LaTeX-команды с автодополнением.
//!
//! Шаблон — линейный формат WordCraft (`⬚` = пустой слот); [`template_preview_latex`] даёт
//! LaTeX-вид для превью кнопки палитры (рендерится тем же конвейером, что формула).

/// Категория структур: галерея WordCraft + русская подпись (UI RDE — русский).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StructureCategory {
    /// Идентификатор галереи (`fraction`, `radical`, …).
    pub gallery: &'static str,
    /// Подпись категории для UI.
    pub label: &'static str,
}

/// Категории структур в порядке MathType-подобной палитры (P0 → P1).
pub const STRUCTURE_CATEGORIES: &[StructureCategory] = &[
    StructureCategory { gallery: "fraction", label: "Дроби" },
    StructureCategory { gallery: "script", label: "Индексы" },
    StructureCategory { gallery: "radical", label: "Корни" },
    StructureCategory { gallery: "integral", label: "Интегралы" },
    StructureCategory { gallery: "largeOperator", label: "Суммы" },
    StructureCategory { gallery: "bracket", label: "Скобки" },
    StructureCategory { gallery: "function", label: "Функции" },
    StructureCategory { gallery: "accent", label: "Акценты" },
    StructureCategory { gallery: "limitLog", label: "Пределы" },
    StructureCategory { gallery: "operator", label: "Операторы" },
    StructureCategory { gallery: "matrix", label: "Матрицы" },
];

/// Шаблоны категории: `(id, линейный формат шаблона)`.
#[must_use]
pub fn category_templates(gallery: &str) -> Vec<(&'static str, &'static str)> {
    wordcraft_engine::math_gallery::STRUCTURES
        .iter()
        .filter(|structure| structure.id == gallery)
        .flat_map(|structure| structure.sections.iter().flat_map(|(_, templates)| templates.iter().map(|template| (template.id, template.linear))))
        .collect()
}

/// LaTeX-вид шаблона для превью кнопки палитры.
///
/// Линейный формат WordCraft (`⬚`, `■(…&…)`, `∑_⬚^⬚▒`) не является LaTeX, поэтому превью
/// строится из **модели** шаблона: `Template::nodes()` → наш roundtrip-safe сериализатор.
#[must_use]
pub fn template_preview_latex(template_id: &str) -> Option<String> {
    let template = wordcraft_engine::math_gallery::template(template_id)?;
    let mut nodes = template.nodes();
    crate::source::normalize(&mut nodes);
    Some(crate::source::to_latex(&nodes))
}

/// Готовая формула из `math_gallery::BUILT_INS`: (id, latex-источник для `formula.toml`).
#[must_use]
pub fn builtin_latex() -> Vec<(&'static str, String)> {
    wordcraft_engine::math_gallery::BUILT_INS
        .iter()
        .map(|(id, _label, linear)| {
            let mut nodes = wordcraft_engine::doc::math::parse_linear(linear);
            crate::source::normalize(&mut nodes);
            (*id, crate::source::to_latex(&nodes))
        })
        .collect()
}

/// Набор символов с русской подписью: `(подпись, строка символов)`.
#[must_use]
pub fn symbol_sets() -> Vec<(&'static str, &'static str)> {
    wordcraft_engine::math_gallery::SYMBOL_SETS
        .iter()
        .map(|(label, symbols)| (symbol_set_label_ru(label), *symbols))
        .collect()
}

fn symbol_set_label_ru(label: &str) -> &'static str {
    match label {
        "Basic Math" => "Основные",
        "Greek Letters" => "Греческие",
        "Letter-Like Symbols" => "Буквоподобные",
        "Operators" => "Операторы",
        "Arrows" => "Стрелки",
        "Negated Relations" => "Отрицания",
        "Scripts" => "Скрипт/фрактура",
        "Geometry" => "Геометрия",
        _ => "Символы",
    }
}

/// Подсказка для автодополнения LaTeX-команд.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSuggestion {
    /// Имя команды без обратной косой черты (`alpha`, `frac`, `sum`).
    pub name: &'static str,
    /// Что делает/что вставит (для подсказки под списком).
    pub hint: String,
    /// LaTeX, который вставится при выборе (для структур — шаблон).
    pub insert_latex: String,
}

/// Все известные LaTeX-команды для автодополнения: символы, функции, шаблоны структур.
///
/// Список строится один раз (первый вызов) и кэшируется: ~символы + ~30 функций + 222 шаблона.
#[must_use]
pub fn latex_commands() -> &'static [CommandSuggestion] {
    use std::sync::OnceLock;
    static COMMANDS: OnceLock<Vec<CommandSuggestion>> = OnceLock::new();
    COMMANDS.get_or_init(|| {
        let mut commands: Vec<CommandSuggestion> = Vec::new();
        for (name, ch) in wordcraft_engine::doc::math_symbols::SYMBOLS {
            commands.push(CommandSuggestion { name, hint: format!("символ {ch}"), insert_latex: ch.to_string() });
        }
        for function in wordcraft_engine::doc::math_linear::FUNCTION_NAMES {
            commands.push(CommandSuggestion { name: function, hint: "функция (\\sin x, \\lim…)".to_owned(), insert_latex: format!("\\{function} ") });
        }
        for structure in wordcraft_engine::math_gallery::STRUCTURES {
            for (_section, templates) in structure.sections {
                for template in *templates {
                    commands.push(CommandSuggestion {
                        name: template.id,
                        hint: format!("шаблон: {}", template.label),
                        insert_latex: template_preview_latex(template.id).unwrap_or_default(),
                    });
                }
            }
        }
        commands.sort_by(|a, b| a.name.cmp(b.name));
        commands.dedup_by(|a, b| a.name == b.name);
        commands
    })
}

/// Автодополнение по префиксу имени команды (без `\`): до `limit` штук.
#[must_use]
pub fn complete_command(prefix: &str, limit: usize) -> Vec<&'static CommandSuggestion> {
    let prefix = prefix.trim_start_matches('\\');
    if prefix.is_empty() {
        return Vec::new();
    }
    latex_commands().iter().filter(|command| command.name.starts_with(prefix)).take(limit.max(1)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_category_has_templates_and_previews() {
        let mut total = 0;
        for category in STRUCTURE_CATEGORIES {
            let templates = category_templates(category.gallery);
            assert!(!templates.is_empty(), "категория {} без шаблонов", category.gallery);
            for (id, _linear) in templates {
                total += 1;
                let latex = template_preview_latex(id).unwrap_or_else(|| panic!("превью {id}"));
                assert!(!latex.is_empty(), "{id}: пустой LaTeX превью");
            }
        }
        assert!(total >= 100, "шаблонов структур: {total}");
    }

    #[test]
    fn commands_autocomplete() {
        let completions = complete_command("alp", 10);
        assert!(completions.iter().any(|command| command.name == "alpha"), "{completions:?}");
        assert!(complete_command("", 10).is_empty(), "пустой префикс не даёт списка");
    }

    #[test]
    fn symbol_sets_have_russian_labels() {
        let sets = symbol_sets();
        assert_eq!(sets.len(), 8);
        assert!(sets.iter().any(|(label, _)| *label == "Операторы"));
    }
}
