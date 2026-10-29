//! `rde-math-editor` — нативный WYSIWYG-редактор формул RDE (P5, ADR-0021).
//!
//! Архитектура (вариант «WordCraft math engine + собственный UI», ADR-0021 §Решение 1):
//!
//! * [`source`] — LaTeX-кодек источника: `parse_latex` (вендор) + **собственный roundtrip-safe**
//!   сериализатор модели в канонический LaTeX (`check_roundtrip` перед записью в `formula.toml`);
//! * [`layout`] — вёрстка и растр формулы тем же конвейером, что документ (WordCraft
//!   `layout::math` + `render::render_area`, STIX): картинка для egui + геометрия каретки/слотов;
//! * [`edit`] — модель редактирования: состояние (модель + `MathPos` + selection + undo/redo),
//!   операции ввода/навигации/удаления/структур на примитивах `wordcraft_doc::math_edit`;
//! * [`palette`] — палитры MathType-класса: структуры (`math_gallery`, 222 шаблона), наборы
//!   символов, LaTeX-команды с автодополнением;
//! * [`widget`] — egui-виджет редактора: один canvas, каретка/выделение, клавиатура, палитры,
//!   LaTeX-команды, контекстная подсказка — без второго `WordApp` (ADR-0020 §3 больше не актуален).
//!
//! LaTeX остаётся источником истины: виджет правит модель и отдаёт «Применить» канонический LaTeX
//! через [`source::check_roundtrip`]; валидация перед записью — `rde_doc::check_formula_latex`
//! (P5 Phase 1) + roundtrip здесь.

pub mod edit;
pub mod layout;
pub mod palette;
pub mod source;
pub mod widget;

pub use edit::MathEditorState;
pub use layout::{FormulaRender, RenderCache};
