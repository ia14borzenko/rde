//! Регресс-тесты ввода WYSIWYG-редактора формулы (P5): клик ставит каретку в место клика, а не в
//! конец строки; стрелки двигают каретку и не «уводят» клавиатурный фокус в кнопки оболочки.
//!
//! Баги-предшественники (найдены владельцем на живой панели «Формула»):
//! * `hit-test` прибавлял экранную позицию полотна к локальной координате → ближайшим слотом всегда
//!   оказывался последний → каретка всегда в конце формулы;
//! * виджет не ставил `focus_lock_filter`, поэтому egui использовал стрелки/Tab для навигации
//!   **фокуса** между виджетами: стрелка «активировала» другие кнопки интерфейса.

use egui_kittest::Harness;
use rde_math_editor::widget::{FormulaEditorOptions, MathEditorUi};

fn widget(latex: &str) -> MathEditorUi {
    let mut widget = MathEditorUi::new(latex, false);
    widget.options.blink_caret = false;
    // Палитры в этих тестах не нужны: проверяем ввод в полотно.
    widget.options = FormulaEditorOptions { blink_caret: false, show_palette: false, ..widget.options };
    widget.request_focus();
    widget
}

fn harness(widget: MathEditorUi) -> Harness<'static, MathEditorUi> {
    Harness::builder()
        .with_size(egui::vec2(600.0, 200.0))
        .with_max_steps(50_000)
        .build_ui_state(|ui, state: &mut MathEditorUi| {
            state.show(ui);
        }, widget)
}

fn click_at(harness: &mut Harness<'static, MathEditorUi>, pos: egui::Pos2) {
    harness.event(egui::Event::PointerMoved(pos));
    for pressed in [true, false] {
        harness.event(egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        });
    }
    harness.step();
}

fn press(harness: &mut Harness<'static, MathEditorUi>, key: egui::Key, modifiers: egui::Modifiers) {
    harness.event(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers });
    harness.step();
}

/// Клик в левой части формулы ставит каретку в начало, в правой — в конец (а не «всегда в конец»).
#[test]
fn click_places_caret_at_clicked_position() {
    let mut harness = harness(widget(r"x+y+z")); // units = 5
    for _ in 0..4 {
        harness.step();
    }
    let rect = harness.state().last_rendered.expect("формула отрисована");
    assert!(rect.width() > 20.0, "габарит формулы: {rect:?}");

    // Клик у левого края формулы → каретка в начале.
    click_at(&mut harness, rect.left_center() + egui::vec2(1.0, 0.0));
    harness.step();
    let left_off = harness.state().state.pos.off;
    assert!(left_off <= 1, "клик слева ставит каретку в начало, а не в конец (off={left_off})");

    // Клик у правого края формулы → каретка в конце.
    let rect = harness.state().last_rendered.expect("формула");
    click_at(&mut harness, rect.right_center() - egui::vec2(1.0, 0.0));
    harness.step();
    let right_off = harness.state().state.pos.off;
    assert!(right_off >= 4, "клик справа ставит каретку в конец (off={right_off}, units=5)");
}

/// Клик в любом месте полотна (в т.ч. далеко от формулы) не оставляет каретку в конце:
/// справа/снизу — конец, слева/сверху — начало (как в текстовом поле).
#[test]
fn click_outside_formula_clamps_to_edges() {
    let mut harness = harness(widget(r"x+y+z"));
    for _ in 0..4 {
        harness.step();
    }
    // Клик заведомо правее формулы, но внутри полотна.
    let rect = harness.state().last_rendered.expect("формула");
    let canvas_right = egui::pos2(rect.right() + 120.0, rect.center().y);
    click_at(&mut harness, canvas_right);
    harness.step();
    assert!(harness.state().state.pos.off >= 4, "клик правее формулы — каретка в конец");

    // Клик в левом верхнем углу полотна (формула начинается с отступом) — в начало.
    let rect = harness.state().last_rendered.expect("формула");
    let canvas_corner = egui::pos2(rect.left() - 6.0, rect.top() - 4.0);
    click_at(&mut harness, canvas_corner);
    harness.step();
    assert!(harness.state().state.pos.off <= 1, "клик левее/выше формулы — каретка в начало: {:?}", harness.state().state.pos);
}

/// Стрелки двигают каретку редактора и НЕ отдают фокус кнопкам оболочки: серия нажатий
/// продолжает двигать каретку (до фикса фокус «слетал» после первого нажатия).
#[test]
fn arrow_keys_move_caret_and_keep_focus() {
    let mut harness = harness(widget(r"x+y+z"));
    for _ in 0..4 {
        harness.step();
    }
    let rect = harness.state().last_rendered.expect("формула");
    click_at(&mut harness, rect.right_center() - egui::vec2(1.0, 0.0));
    // Дать egui кадр на установку focus-lock-фильтра (как в живом UI).
    harness.step();
    harness.step();
    let start = harness.state().state.pos.off;
    assert!(start >= 4, "исходная позиция в конце (off={start})");

    let mut expected = start;
    for step in 1..=3_u32 {
        expected -= 1;
        press(&mut harness, egui::Key::ArrowLeft, egui::Modifiers::NONE);
        let off = harness.state().state.pos.off;
        assert_eq!(off, expected, "нажатие {step}: стрелка влево двигает каретку в редакторе, фокус не ушёл в оболочку");
    }
}

/// Tab двигает каретку по пустым слотам структуры (дробь: числитель → знаменатель),
/// а не фокус оболочки.
#[test]
fn tab_moves_between_slots_not_ui_focus() {
    let mut harness = harness(widget(r"\frac{}{}"));
    for _ in 0..4 {
        harness.step();
    }
    let rect = harness.state().last_rendered.expect("формула");
    click_at(&mut harness, rect.left_center() + egui::vec2(1.0, 0.0));
    harness.step();
    harness.step();
    let before = harness.state().state.pos.clone();
    press(&mut harness, egui::Key::Tab, egui::Modifiers::NONE);
    let after = harness.state().state.pos.clone();
    assert_ne!(before, after, "Tab переводит каретку на следующий слот структуры, а не фокус оболочки: {before:?}");
}

/// Backspace в каретке удаляет символ, а не «переключает активный элемент».
#[test]
fn backspace_edits_formula() {
    let mut harness = harness(widget(r"x+y+z"));
    for _ in 0..4 {
        harness.step();
    }
    let rect = harness.state().last_rendered.expect("формула");
    click_at(&mut harness, rect.right_center() - egui::vec2(1.0, 0.0));
    harness.step();
    harness.step();
    let before = harness.state().state.latex_source();
    press(&mut harness, egui::Key::Backspace, egui::Modifiers::NONE);
    let after = harness.state().state.latex_source();
    assert_ne!(before, after, "Backspace меняет формулу: {before} -> {after}");
    assert!(!after.ends_with('z'), "удалён последний символ: {after}");
}

/// Каретка заполненной матрицы имеет геометрию (иначе она «невидима» — регресс: каретка в конце).
#[test]
fn caret_rect_exists_for_filled_matrix() {
    let mut widget = MathEditorUi::new("", false);
    widget.state.insert_template("mat.paren");
    for (index, ch) in "1234".chars().enumerate() {
        if index > 0 {
            widget.state.move_caret(rde_math_editor::edit::Move::NextSlot, false);
        }
        widget.state.type_text(&ch.to_string());
    }
    let model = widget.state.model.clone();
    let key = widget.state.latex_source();
    let render = widget.cache.get_or_render(&key, &model, widget.state.display, 18.0, 1.5).expect("рендер матрицы");
    let caret = render.caret_rect(&widget.state.pos);
    eprintln!("pos={:?} caret={:?} image={:?}", widget.state.pos, caret, render.size_px());
    assert!(caret.is_some(), "каретка должна рисоваться для текущей позиции {:?}", widget.state.pos);
}

/// Пробел в конце уже структурированной формулы не перестраивает её (баг владельца: σ-формула
/// после пробела превращалась в `\left.\left(...`-вариант с «уехавшей» вёрсткой).
#[test]
fn space_at_end_does_not_restructure_formula() {
    use rde_math_editor::edit::{MathEditorState, MathPos};
    let source = r"\sigma = \sqrt{\frac{1}{N-1}\sum_{i=1}^{N}(x_i-\bar{x})^2}";
    let mut state = MathEditorState::from_latex(source, true);
    let before_model = state.model.clone();
    let units = wordcraft_engine::doc::math_edit::units(&state.model);
    state.set_caret(MathPos::new(Vec::new(), units), false);
    state.type_text(" ");
    let after = state.latex_source();
    assert!(!after.contains(r"\left."), r"пробел не должен порождать скрытые \left.: {after}");
    assert!(after.contains(r"\sqrt{"), "структура формулы сохранена: {after}");
    assert!(after.contains(r"\bar{x}"), "акценты сохранены: {after}");
    assert!(after.contains(r"\sum"), "сумма сохранена: {after}");
    // Модель не перестроена: убрали добавленный пробельный Run(ы) — получили исходную модель.
    let mut trimmed = state.model.clone();
    while let Some(wordcraft_engine::doc::math::MNode::Run(run)) = trimmed.last() {
        if run.text.trim().is_empty() {
            trimmed.pop();
        } else {
            break;
        }
    }
    assert_eq!(trimmed, before_model, "пробел в конце только добавляет пробел, но не меняет структуру");
}

/// Оператор (`+`) в конце формулы тоже не должен перезапускать build-up и перестраивать модель.
#[test]
fn operator_at_end_appends_without_restructuring() {
    use rde_math_editor::edit::{MathEditorState, MathPos};
    let source = r"\sigma = \sqrt{x^2 + y^2}";
    let mut state = MathEditorState::from_latex(source, true);
    let units = wordcraft_engine::doc::math_edit::units(&state.model);
    state.set_caret(MathPos::new(Vec::new(), units), false);
    state.type_text("+");
    let after = state.latex_source();
    assert!(!after.contains(r"\left."), r"оператор не перестраивает формулу: {after}");
    assert!(after.contains(r"\sqrt{"), "корень сохранён: {after}");
    assert!(after.ends_with('+'), "оператор добавлен в конец: {after}");
}

/// А линейный набор по-прежнему собирает структуры: `a/b` + пробел → дробь.
#[test]
fn linear_typing_still_builds_structures() {
    use rde_math_editor::edit::MathEditorState;
    let mut state = MathEditorState::empty(true);
    state.type_text("a/b");
    state.type_text(" ");
    let latex = state.latex_source();
    assert!(latex.contains(r"\frac"), "линейный набор a/b + пробел строит дробь: {latex}");
}
