//! Модель редактирования формулы (P5, ADR-0021 §Решение 1): состояние (дерево `MNode`, `MathPos`,
//! выделение, undo/redo) и операции поверх примитивов `wordcraft_doc::math_edit` — тех же, что у
//! команд `equation.*` WordCraft.
//!
//! 
//!
//! Это даёт Word-совместимую структурную правку без второго `WordApp`: ввод, навигация, удаление,
//! структуры из галереи, авто-коррекция (`a/b`→дробь, `\alpha`→α) и undo.
//!
//! LaTeX-источник: [`MathEditorState::latex_source`] отдаёт канонический LaTeX (собственный
//! сериализатор [`crate::source`]); запись — только через проверку roundtrip.

use std::cmp::Ordering;
use std::time::Instant;

use wordcraft_engine::doc::math::MNode;
use wordcraft_engine::doc::math_edit::{self as me, TypeStyle};
/// Позиция каретки в модели (аргумент `path` + смещение в units) — публичный тип API редактора.
pub use wordcraft_engine::doc::math_edit::MathPos;

use crate::source;

/// Глубина undo-истории (формулы маленькие — снапшоты дешёвые).
const UNDO_LIMIT: usize = 200;
/// Окно, в котором подряд идущий ввод сливается в один шаг undo.
const TYPING_GROUP: std::time::Duration = std::time::Duration::from_millis(800);

/// Снимок модели для undo/redo.
#[derive(Clone, Debug, PartialEq)]
struct Snapshot {
    model: Vec<MNode>,
    pos: MathPos,
    anchor: Option<MathPos>,
}

/// Состояние редактора формулы.
#[derive(Clone, Debug)]
pub struct MathEditorState {
    /// OMML-модель формулы (источник отображения; LaTeX — сериализация).
    pub model: Vec<MNode>,
    /// Каретка: аргумент (path) + смещение в units.
    pub pos: MathPos,
    /// Начало выделения; `None` — выделения нет.
    pub anchor: Option<MathPos>,
    /// `true` — display-формула (пределы сверху/снизу, крупные операторы), `false` — inline.
    pub display: bool,
    /// Последний сохранённый (применённый) LaTeX — для проверки «есть неприменённые правки».
    baseline: String,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    /// Вид последней операции — для слияния подряд идущего ввода в один undo-шаг.
    last_action: Option<ActionKind>,
    last_action_at: Option<Instant>,
    /// Текст режима `\text{}` (обычный текст вместо математики).
    pub normal_text: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionKind {
    Typing,
    Structure,
    Delete,
    Other,
}

/// Направление навигации каретки.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
    NextSlot,
    PrevSlot,
    Home,
    End,
}

impl MathEditorState {
    /// Создать состояние из LaTeX-источника (существующая формула: каретка в конце).
    #[must_use]
    pub fn from_latex(latex: &str, display: bool) -> Self {
        let model = source::from_latex(latex);
        let pos = MathPos::new(Vec::new(), me::units(&model));
        Self {
            model,
            pos,
            anchor: None,
            display,
            baseline: latex.trim().to_owned(),
            undo: Vec::new(),
            redo: Vec::new(),
            last_action: None,
            last_action_at: None,
            normal_text: false,
        }
    }

    /// Пустой редактор (новая формула).
    #[must_use]
    pub fn empty(display: bool) -> Self {
        Self::from_latex("", display)
    }

    /// Заменить модель из LaTeX (например, пользователь правил поле LaTeX-источника).
    /// Возвращает `false`, если источник пуст/не разбирается в непустую модель.
    pub fn set_from_latex(&mut self, latex: &str) -> bool {
        let model = source::from_latex(latex);
        if model.is_empty() && !latex.trim().is_empty() {
            return false;
        }
        self.model = model;
        self.pos = MathPos::new(Vec::new(), me::units(&self.model));
        self.anchor = None;
        self.undo.clear();
        self.redo.clear();
        self.last_action = None;
        true
    }

    /// Канонический LaTeX текущей модели (источник истины для `formula.toml`).
    #[must_use]
    pub fn latex_source(&self) -> String {
        source::to_latex(&self.model)
    }

    /// Проверить, что модель сериализуется без потерь (перед записью в источник).
    pub fn validate(&self) -> Result<String, source::RoundtripError> {
        source::check_roundtrip(&self.model)
    }

    /// Есть ли неприменённые правки относительно `baseline`.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.latex_source() != self.baseline
    }

    /// Зафиксировать текущий источник как сохранённый (после «Применить»/открытия).
    pub fn mark_applied(&mut self) {
        self.baseline = self.latex_source();
    }

    /// Модель пуста (пустая формула).
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.model.is_empty()
    }

    /// Есть ли непустое выделение.
    #[must_use]
    pub fn has_selection(&self) -> bool {
        self.anchor.clone().is_some_and(|anchor| anchor != self.pos)
    }

    /// Границы выделения в документном порядке: (начало, конец).
    #[must_use]
    pub fn selection_bounds(&self) -> Option<(MathPos, MathPos)> {
        let anchor = self.anchor.as_ref()?.clone();
        if anchor == self.pos {
            return None;
        }
        if compare_positions(&self.model, &anchor, &self.pos) == Ordering::Greater {
            Some((self.pos.clone(), anchor))
        } else {
            Some((anchor, self.pos.clone()))
        }
    }

    // ─────────────────────────── операции редактирования ───────────────────────────────────

    /// Ввести текст (буквы/цифры/команды). Повторяет поведение `equation.type` WordCraft:
    /// авто-коррекция `\alpha` и build-up `a/b`/`x^2` на пробеле или знаке отношения.
    /// Можно ли безопасно запускать WordCraft build-up на текущей позиции.
    ///
    /// `math_edit::build_up` перелинеаризует и заново парсит аргумент от последнего «форматного»
    /// узла до каретки. Для линейно набираемого текста (один Run) это безопасно, но если в
    /// аргументе уже есть готовые структуры (корень/сумма/дробь/скобки), их линейный образ не
    /// гарантированно эквивалентен: «пробел» молча перестраивал открытую LaTeX-формулу
    /// (`\sqrt{...}`/`ar{x}` превращались в скрытые `\left.\left(...`). Разрешаем build-up
    /// только когда аргумент каретки целиком — текстовые Run (именно тот случай, ради которого
    /// build-up и нужен: `a/b`→дробь, `x^2`→надстрочный).
    fn can_build_up(&self) -> bool {
        let Some(argument) = me::arg_at(&self.model, &self.pos.path) else {
            return false;
        };
        argument.iter().all(|node| matches!(node, MNode::Run(_)))
    }

    pub fn type_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.begin(ActionKind::Typing);
        self.delete_selection();
        let style = TypeStyle { nor: self.normal_text };
        let mut pos = self.pos.clone();
        for c in text.chars().take(10_000) {
            let relation = matches!(wordcraft_engine::doc::math::math_class(c), wordcraft_engine::doc::math::MClass::Rel);
            if !style.nor && (c == ' ' || c == '\t' || relation) {
                let mut changed = false;
                if let Some(next) = me::autocorrect(&mut self.model, &pos) {
                    pos = next;
                    changed = true;
                }
                // build-up как в WordCraft Equation (UnicodeMath): `a/b`→дробь, `x^2`→надстрочный.
                // LaTeX-команды вводятся строкой «LaTeX-команда» ниже панели (автодополнение).
                // Только для линейного набора: готовые структуры build-up не трогает (иначе
                // пробел/оператор в конце формулы перестраивал её — см. can_build_up).
                if self.can_build_up()
                    && let Some(next) = me::build_up(&mut self.model, &pos, false)
                {
                    pos = next;
                    changed = true;
                }
                if c == ' ' && changed {
                    continue;
                }
            }
            pos = me::insert_text(&mut self.model, &pos, &c.to_string(), style);
        }
        self.pos = pos;
        self.anchor = None;
        self.after_edit();
    }

    /// Вставить символ (палитра символов): как `equation.insertSymbol`.
    pub fn insert_symbol(&mut self, symbol: &str) {
        if symbol.is_empty() {
            return;
        }
        self.begin(ActionKind::Other);
        self.delete_selection();
        let style = TypeStyle { nor: self.normal_text };
        self.pos = me::insert_text(&mut self.model, &self.pos, symbol, style);
        self.anchor = None;
        self.after_edit();
    }

    /// Вставить шаблон структуры из галереи (`math_gallery`): дробь/корень/сумма/матрица/…
    /// После вставки каретка — в первый пустой слот структуры (как в Word).
    pub fn insert_template(&mut self, template_id: &str) -> bool {
        let Some(template) = wordcraft_engine::math_gallery::template(template_id) else {
            return false;
        };
        let nodes = template.nodes();
        self.insert_nodes(nodes)
    }

    /// Вставить готовую модель (LaTeX/структуру) в каретку.
    pub fn insert_nodes(&mut self, nodes: Vec<MNode>) -> bool {
        if nodes.is_empty() {
            return false;
        }
        self.begin(ActionKind::Structure);
        self.delete_selection();
        self.pos = me::insert_nodes(&mut self.model, &self.pos, nodes);
        self.anchor = None;
        // Каретка — внутрь первого пустого слота вставленной структуры, иначе в конец.
        self.after_edit();
        true
    }

    /// Вставить LaTeX-выражение (например, из автодополнения команд).
    pub fn insert_latex(&mut self, latex: &str) -> bool {
        let nodes = source::from_latex(latex);
        self.insert_nodes(nodes)
    }

    /// Backspace: удалить выделение или символ/структуру слева.
    pub fn backspace(&mut self) {
        if self.has_selection() {
            self.begin(ActionKind::Delete);
            self.delete_selection();
            self.after_edit();
            return;
        }
        self.begin(ActionKind::Delete);
        self.pos = me::delete_back(&mut self.model, &self.pos);
        self.anchor = None;
        self.after_edit();
    }

    /// Delete: удалить выделение или символ/структуру справа.
    pub fn delete_forward(&mut self) {
        if self.has_selection() {
            self.begin(ActionKind::Delete);
            self.delete_selection();
            self.after_edit();
            return;
        }
        self.begin(ActionKind::Delete);
        self.pos = me::delete_forward(&mut self.model, &self.pos);
        self.anchor = None;
        self.after_edit();
    }

    /// Переместить каретку; `extend` — с расширением выделения (Shift+стрелки).
    pub fn move_caret(&mut self, direction: Move, extend: bool) {
        let next = match direction {
            Move::Left => me::move_left(&self.model, &self.pos),
            Move::Right => me::move_right(&self.model, &self.pos),
            Move::Up => me::move_vertical(&self.model, &self.pos, true),
            Move::Down => me::move_vertical(&self.model, &self.pos, false),
            Move::NextSlot => me::next_placeholder(&self.model, &self.pos, false),
            Move::PrevSlot => me::next_placeholder(&self.model, &self.pos, true),
            Move::Home => Some(me::home_end(&self.model, &self.pos, false)),
            Move::End => Some(me::home_end(&self.model, &self.pos, true)),
        };
        let Some(next) = next else { return };
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.pos.clone());
            }
        } else {
            self.anchor = None;
        }
        self.pos = next;
        self.last_action = None;
    }

    /// Перейти к позиции (клик мышью/хит-тест); `extend` — Shift+клик.
    pub fn set_caret(&mut self, pos: MathPos, extend: bool) {
        if extend {
            if self.anchor.is_none() {
                self.anchor = Some(self.pos.clone());
            }
        } else {
            self.anchor = None;
        }
        self.pos = pos;
        self.last_action = None;
    }

    /// Выделить всё (Ctrl+A): весь верхний аргумент.
    pub fn select_all(&mut self) {
        self.anchor = Some(MathPos::new(Vec::new(), 0));
        self.pos = MathPos::new(Vec::new(), me::units(&self.model));
        self.last_action = None;
    }

    /// Свернуть выделение (Esc): каретка остаётся, выделение снимается.
    pub fn collapse_selection(&mut self) {
        self.anchor = None;
        self.last_action = None;
    }

    /// Выйти из структуры на уровень выше (Esc при пустом/структурном слоте): как Word.
    pub fn escape_structure(&mut self) {
        self.anchor = None;
        if let Some(((node_index, _), parent_path)) = self.pos.path.split_last() {
            let parent_path = parent_path.to_vec();
            if let Some(parent) = me::arg_at(&self.model, &parent_path)
                && let Some(start) = parent.get(*node_index).map(|_| me::node_start(parent, *node_index))
            {
                self.pos = MathPos::new(parent_path, start + 1);
            }
        }
        self.last_action = None;
    }

    /// Действия над структурой вокруг каретки (матричный тулбар/контекстное меню):
    /// `(level, action, label)` — innermost first.
    #[must_use]
    pub fn structure_actions(&self) -> Vec<(usize, &'static str, &'static str)> {
        me::structure_actions(&self.model, &self.pos)
    }

    /// Применить действие над структурой (например, `frac.bar`, `mat.addRow`, `delim.delete`).
    pub fn apply_structure_action(&mut self, level: usize, action: &str) -> bool {
        self.begin(ActionKind::Structure);
        match me::apply_structure(&mut self.model, &self.pos, level, action) {
            Some(pos) => {
                self.pos = pos;
                self.anchor = None;
                self.after_edit();
                true
            }
            None => {
                self.undo.pop();
                false
            }
        }
    }

    /// Переключить подписи/надписи пределов у n-арного оператора в каретке (`\sum`).
    pub fn toggle_limits(&mut self, stacked: bool) -> bool {
        let Some(((node_index, _), parent_path)) = self.pos.path.split_last() else { return false };
        let parent_path = parent_path.to_vec();
        let node_index = *node_index;
        let Some(parent) = me::arg_at_mut(&mut self.model, &parent_path) else { return false };
        let Some(node) = parent.get_mut(node_index) else { return false };
        let mut wrapper = vec![std::mem::replace(node, MNode::Run(wordcraft_engine::doc::math::MRun::new("")))];
        me::set_limits(&mut wrapper, Some(if stacked { wordcraft_engine::doc::math::LimLoc::UndOvr } else { wordcraft_engine::doc::math::LimLoc::SubSup }));
        if let Some(restored) = wrapper.pop() {
            *node = restored;
            true
        } else {
            false
        }
    }

    /// Undo (Ctrl+Z).
    pub fn undo(&mut self) -> bool {
        let Some(snapshot) = self.undo.pop() else { return false };
        self.redo.push(self.snapshot());
        self.apply_snapshot(snapshot);
        self.last_action = None;
        true
    }

    /// Redo (Ctrl+Y / Ctrl+Shift+Z).
    pub fn redo(&mut self) -> bool {
        let Some(snapshot) = self.redo.pop() else { return false };
        self.undo.push(self.snapshot());
        self.apply_snapshot(snapshot);
        self.last_action = None;
        true
    }

    /// Можно ли отменить/повторить.
    #[must_use]
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    #[must_use]
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    // ─────────────────────────────── внутреннее ────────────────────────────────────────────

    fn snapshot(&self) -> Snapshot {
        Snapshot { model: self.model.clone(), pos: self.pos.clone(), anchor: self.anchor.clone() }
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.model = snapshot.model;
        self.pos = me::clamp(&self.model, &snapshot.pos);
        self.anchor = snapshot.anchor;
    }

    /// Начать операцию: снять undo-снапшот (с учётом слияния подряд идущего ввода).
    fn begin(&mut self, kind: ActionKind) {
        let now = Instant::now();
        let merge = kind == ActionKind::Typing
            && self.last_action == Some(ActionKind::Typing)
            && self.last_action_at.is_some_and(|at| now.duration_since(at) < TYPING_GROUP);
        if !merge {
            self.undo.push(self.snapshot());
            if self.undo.len() > UNDO_LIMIT {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
        self.last_action = Some(kind);
        self.last_action_at = Some(now);
    }

    fn after_edit(&mut self) {
        self.pos = me::clamp(&self.model, &self.pos);
        if self.anchor.clone().is_some_and(|anchor| me::clamp(&self.model, &anchor) != anchor) {
            self.anchor = None;
        }
    }

    /// Удалить текущее выделение (в документном порядке).
    fn delete_selection(&mut self) {
        let Some((start, end)) = self.selection_bounds() else { return };
        // Удаляем с конца к началу: удаление справа не сдвигает позиции до начала.
        self.pos = end.clone();
        let mut guard = 0usize;
        while compare_positions(&self.model, &self.pos, &start) == Ordering::Greater {
            let before = self.pos.clone();
            self.pos = me::delete_back(&mut self.model, &self.pos);
            if self.pos == before {
                // Не смогли удалить (пусто на пути) — выходим, чтобы не зациклиться.
                break;
            }
            guard += 1;
            if guard > 10_000 {
                break;
            }
        }
        self.pos = me::clamp(&self.model, &start);
        self.anchor = None;
    }
}

/// Аргумент по пути каретки, начиная со среза верхнего уровня (аналог `me::arg_at` для `&[MNode]`).
fn argument_at<'a>(root: &'a [MNode], path: &[(usize, usize)]) -> Option<&'a [MNode]> {
    let mut nodes: &[MNode] = root;
    for (node_index, child_index) in path {
        let node = nodes.get(*node_index)?;
        let child = me::child(node, *child_index)?;
        nodes = child.as_slice();
    }
    Some(nodes)
}

/// Документный порядок двух позиций каретки (для выделения/удаления выделения).
///
/// Позиция — аргумент (path) + смещение. Если пути расходятся, порядок задаёт первый различающийся
/// шаг `(индекс узла, индекс аргумента)`; если один путь — префикс другого, позиция в аргументе
/// сравнивается со стартом вложенного узла.
#[must_use]
pub fn compare_positions(root: &[MNode], a: &MathPos, b: &MathPos) -> Ordering {
    let common = {
        let mut common = 0;
        while common < a.path.len() && common < b.path.len() && a.path[common] == b.path[common] {
            common += 1;
        }
        common
    };
    if common == a.path.len() && common == b.path.len() {
        return a.off.cmp(&b.off);
    }
    if common == a.path.len() {
        // `a` — каретка в предке; сравниваем со стартом узла, в который уходит `b`.
        let (node_index, _) = b.path[common];
        if let Some(parent) = argument_at(root, &a.path) {
            let start = me::node_start(parent, node_index);
            return if a.off <= start { Ordering::Less } else { Ordering::Greater };
        }
        return Ordering::Less;
    }
    if common == b.path.len() {
        let (node_index, _) = a.path[common];
        if let Some(parent) = argument_at(root, &b.path) {
            let start = me::node_start(parent, node_index);
            return if b.off <= start { Ordering::Greater } else { Ordering::Less };
        }
        return Ordering::Greater;
    }
    let (node_a, child_a) = a.path[common];
    let (node_b, child_b) = b.path[common];
    node_a.cmp(&node_b).then(child_a.cmp(&child_b))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIGMA: &str = r"\sigma =\sqrt{\frac{1}{N-1}\sum_{i=1}^{N} {({x}_{i}}-\bar{x}{)}^{2}}";

    #[test]
    fn typing_does_not_corrupt_structure() {
        let mut state = MathEditorState::from_latex(SIGMA, true);
        let before = state.latex_source();
        for _ in 0..20 {
            state.move_caret(Move::Left, false);
            state.move_caret(Move::Right, false);
        }
        assert_eq!(state.latex_source(), before, "навигация не меняет формулу");
        state.type_text("+1");
        let after = state.latex_source();
        assert!(after.contains("+1"), "{after}");
        assert!(state.undo());
        assert_eq!(state.latex_source(), before, "undo возвращает формулу");
    }

    #[test]
    fn build_up_makes_fraction_and_autocorrect_makes_greek() {
        let mut state = MathEditorState::empty(true);
        state.type_text("a/b ");
        assert!(state.latex_source().contains("\\frac"), "a/b + пробел → дробь: {}", state.latex_source());
        let mut greek = MathEditorState::empty(true);
        greek.type_text("\\alpha ");
        assert!(greek.latex_source().contains("α") || greek.latex_source().contains("\\alpha"), "{}", greek.latex_source());
    }

    #[test]
    fn templates_insert_and_move_to_first_slot() {
        let mut state = MathEditorState::empty(true);
        assert!(state.insert_template("frac.stacked"));
        assert!(state.latex_source().contains("\\frac"), "{}", state.latex_source());
        state.type_text("1");
        state.move_caret(Move::NextSlot, false);
        state.type_text("2");
        let source = state.latex_source();
        assert!(source.contains("1") && source.contains("2"), "{source}");
        assert!(state.validate().is_ok(), "roundtrip: {:?}", state.validate().err());
    }

    #[test]
    fn selection_orders_and_deletes_across_arguments() {
        let mut state = MathEditorState::from_latex(SIGMA, true);
        state.select_all();
        assert!(state.has_selection());
        let (start, end) = state.selection_bounds().expect("выделение");
        assert_eq!(start.off, 0);
        assert_eq!(end.off, me::units(&state.model));
        state.backspace();
        assert!(state.model.is_empty(), "выделение удалено: {}", state.latex_source());
        assert!(state.undo());
        assert!(!state.model.is_empty(), "undo восстановил формулу");
    }

    #[test]
    fn compare_positions_document_order() {
        let model = source::from_latex(r"a+\frac{b}{c}");
        // a (off 1) < дробь (off 2) < числитель (path [(2,0)]) < знаменатель.
        let a = MathPos::new(Vec::new(), 1);
        let frac = MathPos::new(Vec::new(), 2);
        let num = MathPos::new(vec![(2, 0)], 0);
        let den = MathPos::new(vec![(2, 1)], 0);
        assert_eq!(compare_positions(&model, &a, &frac), Ordering::Less);
        assert_eq!(compare_positions(&model, &a, &num), Ordering::Less);
        assert_eq!(compare_positions(&model, &num, &den), Ordering::Less);
        assert_eq!(compare_positions(&model, &frac, &num), Ordering::Less);
        assert_eq!(compare_positions(&model, &den, &frac), Ordering::Greater);
    }

    #[test]
    fn move_vertical_keeps_model_unchanged() {
        let mut state = MathEditorState::from_latex(SIGMA, true);
        let before = state.latex_source();
        for _ in 0..5 {
            state.move_caret(Move::Up, false);
            state.move_caret(Move::Down, false);
            state.move_caret(Move::NextSlot, false);
        }
        assert_eq!(state.latex_source(), before);
    }
}
