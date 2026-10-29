//! egui-виджет редактора формулы (P5, ADR-0021 §Решение 1): **один** canvas (без второго
//! `WordApp`), WYSIWYG-отрисовка тем же рендером, что документ, каретка/выделение, клавиатура,
//! палитры структур/символов и ввод LaTeX-команд.
//!
//! Как это устроено:
//! * модель — [`crate::edit::MathEditorState`] (дерево `MNode` + `MathPos` + выделение + undo);
//! * картинка — [`crate::layout::FormulaRender`] (layout+растр WordCraft, кэш [`RenderCache`]);
//! * каретка/выделение — оверлей egui по `MathLayout.slots` (точная геометрия документа);
//! * ввод — события egui → операции `math_edit` (Word-совместимое поведение: `a/b`→дробь,
//!   `\alpha`→α, Tab по слотам, стрелки/Backspace/Delete, undo/redo, clipboard LaTeX).

use egui::{Color32, FontId, Rect, Sense, TextStyle, pos2, vec2};

use crate::edit::{MathEditorState, Move};
use crate::layout::{FormulaRender, RenderCache};
use crate::palette;

/// Настройки отображения редактора.
#[derive(Clone, Debug)]
pub struct FormulaEditorOptions {
    /// Кегль формулы в pt (16–18 — читаемо; масштаб панели — `zoom`).
    pub pt: f32,
    /// Масштаб панели (1.0 = 100 %).
    pub zoom: f32,
    /// Показывать палитры структур/символов (можно свернуть).
    pub show_palette: bool,
    /// Минимальная высота области формулы, px.
    pub min_height: f32,
    /// Мигать кареткой (в тестах/скриншотах лучше выключить — детерминированный кадр).
    pub blink_caret: bool,
}

impl Default for FormulaEditorOptions {
    fn default() -> Self {
        Self { pt: 18.0, zoom: 1.0, show_palette: true, min_height: 96.0, blink_caret: true }
    }
}

/// Что случилось в кадре (панель синхронизирует LaTeX-поле/ошибки).
#[derive(Clone, Debug, Default)]
pub struct FormulaEditorResponse {
    /// Модель менялась в этом кадре.
    pub changed: bool,
    /// Канонический LaTeX модели (источник истины) — после правок.
    pub latex: String,
    /// Модель не сериализуется в LaTeX без потерь (нельзя записывать в источник).
    pub roundtrip_error: Option<String>,
}

/// Хранимое состояние виджета между кадрами: модель, кэш рендеров, фокус, выбранная палитра.
pub struct MathEditorUi {
    /// Модель/каретка/выделение/undo — источник истины редактора.
    pub state: MathEditorState,
    /// Кэш растров формул/превью палитры.
    pub cache: RenderCache,
    /// Настройки отображения.
    pub options: FormulaEditorOptions,
    /// Запросить фокус клавиатуры на следующем кадре (после открытия/клика).
    want_focus: bool,
    /// Индекс категории структур в палитре.
    palette_category: usize,
    /// Индекс набора символов.
    palette_symbol_set: usize,
    /// Поле ввода LaTeX-команды (автодополнение).
    command_input: String,
    /// Последний отрисованный кадр — для диагностики/тестов.
    pub last_rendered: Option<Rect>,
    /// Было ли полотно в фокусе клавиатуры в последнем кадре (диагностика/тесты).
    pub focused: bool,
    /// Где рисовалась каретка в последнем кадре (диагностика/тесты), в точках экрана.
    pub last_caret: Option<Rect>,
}

impl MathEditorUi {
    /// Создать виджет из LaTeX-источника.
    #[must_use]
    pub fn new(latex: &str, display: bool) -> Self {
        let mut state = MathEditorState::from_latex(latex, display);
        state.mark_applied();
        Self {
            state,
            cache: RenderCache::default(),
            options: FormulaEditorOptions::default(),
            want_focus: false,
            palette_category: 0,
            palette_symbol_set: 0,
            command_input: String::new(),
            last_rendered: None,
            focused: false,
            last_caret: None,
        }
    }

    /// Сменить источник извне (открыли другую формулу / правили поле LaTeX).
    /// Возвращает `false`, если LaTeX не разбирается (модель не меняется).
    pub fn set_source(&mut self, latex: &str, display: bool) -> bool {
        let changed = self.state.set_from_latex(latex);
        if changed {
            self.state.display = display;
            self.state.mark_applied();
            self.cache.clear();
        }
        changed
    }

    /// Синхронизировать модель с уже применённым источником (после «Применить»/открытия).
    pub fn sync_applied(&mut self, latex: &str, display: bool) {
        self.state.set_from_latex(latex);
        self.state.display = display;
        self.state.mark_applied();
        self.cache.clear();
    }

    /// Подготовить источник к записи в `formula.toml`: нормализовать непредставимые конструкции
    /// (`FracKind` вне binom, `Func` с составным именем, пустые хвостовые строки матриц), затем
    /// проверить roundtrip `parse(to_latex(model)) == model`. Возвращает канонический LaTeX.
    pub fn prepare_source(&mut self) -> Result<String, crate::source::RoundtripError> {
        crate::source::normalize(&mut self.state.model);
        self.state.pos = wordcraft_engine::doc::math_edit::clamp(&self.state.model, &self.state.pos);
        self.state.anchor = None;
        crate::source::check_roundtrip(&self.state.model)
    }

    /// Запросить фокус (например, при открытии панели/клике по формуле в тексте).
    pub fn request_focus(&mut self) {
        self.want_focus = true;
    }

    /// Показать редактор. Возвращает [`FormulaEditorResponse`].
    pub fn show(&mut self, ui: &mut egui::Ui) -> FormulaEditorResponse {
        let mut response = FormulaEditorResponse::default();
        let mut changed = false;

        // ── Formula canvas: рендер тем же конвейером, что документ ──────────────────────────
        let text_color = ui.visuals().strong_text_color();
        let accent = ui.visuals().selection.bg_fill;
        let ppp = (ui.ctx().pixels_per_point() * self.options.zoom).clamp(0.5, 8.0);
        let pt = (self.options.pt * self.options.zoom).clamp(6.0, 96.0);
        let key = self.state.latex_source();
        let render = if self.state.is_empty() {
            None
        } else {
            let model = self.state.model.clone();
            self.cache.get_or_render(&key, &model, self.state.display, pt, ppp)
        };

        let image_size = render.as_ref().map(|render| render.size_px() / ui.ctx().pixels_per_point()).unwrap_or(vec2(240.0, 44.0));
        let available = ui.available_width();
        let height = (image_size.y + 16.0).max(self.options.min_height);
        let (canvas_rect, canvas_response) = ui.allocate_exact_size(vec2(available, height), Sense::click_and_drag());
        let id = canvas_response.id;
        if self.want_focus {
            self.want_focus = false;
            canvas_response.request_focus();
        }
        if canvas_response.clicked() || canvas_response.drag_started() {
            // Клик по полотну забирает клавиатурный фокус (иначе ввод уходит в оболочку).
            canvas_response.request_focus();
            self.want_focus = false;
        }
        if canvas_response.hovered() {
            ui.output_mut(|output| output.cursor_icon = egui::CursorIcon::Text);
        }
        let focused = ui.memory(|memory| memory.has_focus(id));
        self.focused = focused;
        // КРИТИЧНО: по умолчанию egui использует стрелки/Tab/Esc для навигации ФОКУСА между
        // виджетами — «стрелки уводят активный элемент в другие кнопки». Освобождаем эти клавиши
        // для редактора (то же делает TextEdit своим focus-lock-фильтром).
        if focused {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    id,
                    egui::EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true },
                );
            });
        }
        let painter = ui.painter_at(canvas_rect);

        // Полотно: лёгкий фон и рамка, как у поля ввода.
        painter.rect_filled(canvas_rect, 6.0, ui.visuals().extreme_bg_color);
        let stroke_color = if focused { ui.visuals().selection.stroke.color } else { ui.visuals().widgets.noninteractive.bg_stroke.color };
        painter.rect_stroke(canvas_rect, 6.0, egui::Stroke::new(1.0, stroke_color), egui::StrokeKind::Inside);

        let origin = pos2(canvas_rect.left() + 10.0, canvas_rect.top() + 8.0);
        let mut formula_rect = Rect::from_min_size(origin, image_size);
        if let Some(render) = &render {
            formula_rect = Rect::from_min_size(origin, image_size);
            if let Some(texture) = ensure_texture(ui.ctx(), &key, render) {
                painter.image(texture.id(), formula_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), text_color);
            }
            // Выделение (полупрозрачный акцент) под кареткой.
            if let (Some(anchor), focus) = (self.state.anchor.clone(), &self.state.pos) {
                for rect in render.selection_rects(&anchor, focus) {
                    let scale = ui.ctx().pixels_per_point();
                    let scaled = Rect::from_min_size(origin + rect.min.to_vec2() / ui.ctx().pixels_per_point().max(f32::EPSILON) * scale, rect.size());
                    painter.rect_filled(scaled, 1.0, accent.gamma_multiply(0.35));
                }
            }
            // Каретка: видна, когда редактор в фокусе (опционально мигает).
            let blink = !self.options.blink_caret || (ui.input(|input| input.time) * 1.7).fract() < 0.6;
            self.last_caret = None;
            if focused
                && let Some(caret) = render.caret_rect(&self.state.pos)
            {
                let ppp = ui.ctx().pixels_per_point();
                let min = origin + caret.min.to_vec2() / ppp;
                let max = origin + caret.max.to_vec2() / ppp;
                let caret_ui = Rect::from_min_max(min, max);
                self.last_caret = Some(caret_ui);
                if blink {
                    painter.rect_filled(caret_ui, 0.5, accent);
                }
            }
        } else {
            painter.text(origin + vec2(4.0, 12.0), egui::Align2::LEFT_TOP, "Введите формулу: палитра ниже или LaTeX-команда (\\frac, \\sqrt, \\sum…)", FontId::proportional(13.0), ui.visuals().weak_text_color());
        }
        self.last_rendered = Some(formula_rect);

        // ── Мышь: клик/протяжка ставят каретку (hit-test по слотам MathLayout) ────────────────
        // Точка указателя переводится в координаты картинки как `(pointer - origin) * ppp`
        // (никакого прибавления экранного origin — иначе каретка всегда прыгала в конец строки),
        // а клик вне картинки клампится к ближайшему краю (справа/снизу — конец формулы).
        if canvas_response.clicked() || canvas_response.dragged() {
            let pointer = canvas_response
                .interact_pointer_pos()
                .or_else(|| ui.input(|input| input.pointer.interact_pos()));
            if let Some(pointer) = pointer {
                let offset = pointer - formula_rect.min;
                let ppp = ui.ctx().pixels_per_point().max(f32::EPSILON);
                match &render {
                    Some(render) => {
                        if let Some(pos) = render.hit_ui(offset, ppp) {
                            let extend = ui.input(|input| input.modifiers.shift) || canvas_response.dragged();
                            self.state.set_caret(pos, extend);
                        }
                    }
                    // Пустая формула: клик по полотну ставит каретку в начало.
                    None => self.state.set_caret(Default::default(), false),
                }
            }
        }

        // ── Клавиатура ──────────────────────────────────────────────────────────────────────
        if focused {
            // События читаем, не забирая: чужие (колесо и т.п.) не ломаем, а повторного ввода
            // нет — события существуют только в кадре, когда пришли.
            let events: Vec<egui::Event> = ui.input(|input| input.events.clone());
            let mut handled_nav = false;
            for event in events {
                match event {
                    egui::Event::Text(text) => {
                        if text.chars().all(|c| !c.is_control()) && !text.is_empty() {
                            self.state.type_text(&text);
                            changed = true;
                        }
                    }
                    egui::Event::Key { key, pressed: true, modifiers, .. } if self.handle_key(key, modifiers) => {
                        changed = true;
                        handled_nav = matches!(
                            key,
                            egui::Key::ArrowLeft | egui::Key::ArrowRight | egui::Key::ArrowUp | egui::Key::ArrowDown | egui::Key::Tab | egui::Key::Escape
                        );
                    }
                    _ => {}
                }
            }
            if handled_nav {
                // egui уже мог поставить навигацию фокуса по стрелке/Tab в начале кадра
                // (фильтр применяется только со следующего кадра) — отменяем её, чтобы
                // «активным элементом» не становились кнопки оболочки.
                ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
            }
        }

        // ── LaTeX-команда с автодополнением (MathType: набор команд с клавиатуры) ─────────────
        if self.options.show_palette {
            ui.add_space(6.0);
            self.command_palette(ui, &mut changed);
            ui.add_space(4.0);
            self.structure_palette(ui, &mut changed);
            self.symbol_palette(ui, &mut changed);
            if !self.state.structure_actions().is_empty() {
                self.structure_actions_bar(ui, &mut changed);
            }
            ui.add_space(2.0);
            let hint = "стрелки — каретка, Tab — следующий слот, Esc — на уровень выше, Ctrl+Z/Y — отмена/повтор, выделение мышью/Shift+стрелками";
            ui.label(egui::RichText::new(hint).small().color(ui.visuals().weak_text_color()));
        }

        response.changed = changed;
        response.latex = self.state.latex_source();
        // Ошибку показываем только если формула не представима в источнике ДАЖЕ после
        // нормализации (пустые хвостовые строки матриц/нестандартные дроби нормализуются при
        // «Применить» — это не ошибка пользователя).
        let mut prepared = self.state.model.clone();
        crate::source::normalize(&mut prepared);
        if let Err(error) = crate::source::check_roundtrip(&prepared) {
            response.roundtrip_error = Some(error.detail);
        }
        response
    }

    /// Обработать клавишу; возвращает `true`, если состояние изменилось.
    fn handle_key(&mut self, key: egui::Key, modifiers: egui::Modifiers) -> bool {
        use egui::Key;
        let command = modifiers.command;
        if command {
            match key {
                Key::Z if modifiers.shift => return self.state.redo(),
                Key::Z => return self.state.undo(),
                Key::Y => return self.state.redo(),
                Key::A => {
                    self.state.select_all();
                    return true;
                }
                Key::C => return false,
                _ => {}
            }
        }
        match key {
            Key::ArrowLeft => self.state.move_caret(Move::Left, modifiers.shift),
            Key::ArrowRight => self.state.move_caret(Move::Right, modifiers.shift),
            Key::ArrowUp => self.state.move_caret(Move::Up, modifiers.shift),
            Key::ArrowDown => self.state.move_caret(Move::Down, modifiers.shift),
            Key::Tab => self.state.move_caret(if modifiers.shift { Move::PrevSlot } else { Move::NextSlot }, false),
            Key::Home => self.state.move_caret(Move::Home, modifiers.shift),
            Key::End => self.state.move_caret(Move::End, modifiers.shift),
            Key::Backspace => self.state.backspace(),
            Key::Delete => self.state.delete_forward(),
            Key::Escape => {
                if self.state.has_selection() {
                    self.state.collapse_selection();
                } else {
                    self.state.escape_structure();
                }
            }
            _ => return false,
        }
        true
    }

    /// Строка ввода LaTeX-команды + подсказки автодополнения.
    fn command_palette(&mut self, ui: &mut egui::Ui, changed: &mut bool) {
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("LaTeX-команда").small().color(ui.visuals().weak_text_color()));
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.command_input)
                    .hint_text("\\frac, \\sqrt, \\alpha, \\sum… (Enter — вставить)")
                    .desired_width(280.0)
                    .font(TextStyle::Monospace),
            );
            let prefix: String = self.command_input.trim_start_matches('\\').to_owned();
            let suggestions = palette::complete_command(&prefix, 8);
            if !suggestions.is_empty() && !prefix.is_empty() {
                for suggestion in suggestions.iter().take(5) {
                    if ui.small_button(suggestion.name.to_string()).on_hover_text(&suggestion.hint).clicked() {
                        self.insert_command(suggestion.name);
                        changed_now(changed);
                        self.want_focus = true;
                    }
                }
            }
            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                let command = self.command_input.trim().trim_start_matches('\\').to_owned();
                if !command.is_empty() {
                    self.insert_command(&command);
                    self.command_input.clear();
                    changed_now(changed);
                    self.want_focus = true;
                }
            }
        });
    }

    /// Вставить команду по имени: шаблон структуры, символ или функция.
    fn insert_command(&mut self, name: &str) {
        // 1) шаблон структуры галереи (id с точкой: frac.stacked) — по id или имени шаблона;
        if wordcraft_engine::math_gallery::template(name).is_some() {
            self.state.insert_template(name);
            return;
        }
        // 2) символ из таблицы имён;
        if let Some(ch) = wordcraft_engine::doc::math_symbols::symbol(name) {
            self.state.insert_symbol(&ch.to_string());
            return;
        }
        // 3) функция/команда — вставляем как LaTeX-выражение (parse_latex сам разберётся).
        self.state.insert_latex(&format!("\\{name} "));
    }

    /// Палитра структур: категории + превью шаблонов (тем же рендером, что формула).
    fn structure_palette(&mut self, ui: &mut egui::Ui, changed: &mut bool) {
        egui::CollapsingHeader::new(egui::RichText::new("Структуры").strong()).id_salt("rde_math_structures").default_open(true).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (index, category) in palette::STRUCTURE_CATEGORIES.iter().enumerate() {
                    let selected = index == self.palette_category;
                    if ui.selectable_label(selected, category.label).clicked() {
                        self.palette_category = index;
                    }
                }
            });
            let category = palette::STRUCTURE_CATEGORIES.get(self.palette_category).copied().unwrap_or(palette::STRUCTURE_CATEGORIES[0]);
            let templates = palette::category_templates(category.gallery);
            let ppp = (ui.ctx().pixels_per_point() * self.options.zoom).clamp(0.5, 8.0);
            let preview_pt = (14.0 * self.options.zoom).clamp(8.0, 32.0);
            let text_color = ui.visuals().strong_text_color();
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for (id, _linear) in templates {
                    let label = wordcraft_engine::math_gallery::template(id).map(|template| template.label).unwrap_or(id);
                    let Some(latex) = palette::template_preview_latex(id) else { continue };
                    let model = crate::source::from_latex(&latex);
                    let Some(render) = self.cache.get_or_render(&latex, &model, false, preview_pt, ppp) else { continue };
                    let size = (render.size_px() / ui.ctx().pixels_per_point()) + vec2(10.0, 8.0);
                    let (rect, response) = ui.allocate_exact_size(vec2(size.x.max(28.0), 30.0), Sense::click());
                    let hovered = response.hovered();
                    if hovered {
                        ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill);
                    }
                    if let Some(texture) = ensure_texture(ui.ctx(), &format!("palette:{latex}"), &render) {
                        let image_rect = Rect::from_center_size(rect.center(), size - vec2(10.0, 8.0));
                        ui.painter().image(texture.id(), image_rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), text_color);
                    }
                    let response = response.on_hover_text(label);
                    if response.clicked() {
                        self.state.insert_template(id);
                        changed_now(changed);
                        self.want_focus = true;
                    }
                }
            });
        });
    }

    /// Палитра символов по наборам.
    fn symbol_palette(&mut self, ui: &mut egui::Ui, changed: &mut bool) {
        egui::CollapsingHeader::new(egui::RichText::new("Символы").strong()).id_salt("rde_math_symbols").default_open(false).show(ui, |ui| {
            let sets = palette::symbol_sets();
            if sets.is_empty() {
                return;
            }
            self.palette_symbol_set = self.palette_symbol_set.min(sets.len() - 1);
            egui::ComboBox::from_id_salt("rde_math_symbol_set").selected_text(sets[self.palette_symbol_set].0).show_ui(ui, |ui| {
                for (index, (label, _)) in sets.iter().enumerate() {
                    if ui.selectable_label(index == self.palette_symbol_set, *label).clicked() {
                        self.palette_symbol_set = index;
                    }
                }
            });
            let symbols = sets[self.palette_symbol_set].1;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                for ch in symbols.chars() {
                    let button = ui.add(egui::Button::new(egui::RichText::new(ch.to_string()).size(17.0)).frame(true).min_size(vec2(24.0, 24.0)));
                    if button.clicked() {
                        self.state.insert_symbol(&ch.to_string());
                        changed_now(changed);
                        self.want_focus = true;
                    }
                }
            });
        });
    }

    /// Матричный/структурный тулбар: действия над структурой вокруг каретки.
    fn structure_actions_bar(&mut self, ui: &mut egui::Ui, changed: &mut bool) {
        let actions = self.state.structure_actions();
        if actions.is_empty() {
            return;
        }
        egui::CollapsingHeader::new(egui::RichText::new("Структура вокруг каретки").strong()).id_salt("rde_math_struct_actions").default_open(false).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for (level, action, label) in actions {
                    if ui.small_button(label_localized(action, label)).clicked() {
                        self.state.apply_structure_action(level, action);
                        changed_now(changed);
                        self.want_focus = true;
                    }
                }
            });
        });
    }
}

fn changed_now(changed: &mut bool) {
    *changed = true;
}

/// Локализация подписей действий `math_edit::structure_actions` (часто встречающиеся).
fn label_localized<'a>(action: &str, fallback: &'a str) -> &'a str {
    match action {
        "frac.bar" => "Дробь: с чертой",
        "frac.skewed" => "Дробь: наклонная",
        "frac.linear" => "Дробь: линейная",
        "mat.addRow" => "Строка +",
        "mat.addCol" => "Столбец +",
        "mat.delRow" => "Строка −",
        "mat.delCol" => "Столбец −",
        "lim.undovr" => "Пределы: сверху/снизу",
        "lim.subsup" => "Пределы: справа",
        "delim.delete" => "Убрать скобки",
        "script.delete" => "Убрать скрипт",
        "rad.delete" => "Убрать корень",
        _ => fallback,
    }
}

/// Текстура для рендера формулы: загружается в egui по ключу (одна на рендер).
fn ensure_texture(ctx: &egui::Context, key: &str, render: &FormulaRender) -> Option<egui::TextureHandle> {
    let texture_id = egui::Id::new(("rde_math_formula_texture", key));
    if let Some(texture) = ctx.data(|data| data.get_temp::<egui::TextureHandle>(texture_id)) {
        return Some(texture);
    }
    let texture = ctx.load_texture(format!("rde-math:{key}"), render.image.clone(), egui::TextureOptions::LINEAR);
    ctx.data_mut(|data| data.insert_temp(texture_id, texture.clone()));
    Some(texture)
}

/// Подсказка-предупреждение для панели, если модель не сериализуется в LaTeX.
#[must_use]
pub fn roundtrip_warning(error: &str) -> String {
    format!("Формула не представима в LaTeX-источнике (правка не будет сохранена): {error}")
}

/// Цвет каретки/выделения по теме (для панели; используется в тестах контраста).
#[must_use]
pub fn caret_color(visuals: &egui::Visuals) -> Color32 {
    visuals.selection.bg_fill
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widget_state_roundtrip_and_undo() {
        let mut ui_state = MathEditorUi::new(r"x+1", true);
        assert_eq!(ui_state.state.latex_source(), "x+1");
        ui_state.state.type_text("=2");
        assert!(ui_state.state.latex_source().contains("2"));
        assert!(ui_state.state.undo());
        assert_eq!(ui_state.state.latex_source(), "x+1");
    }

    #[test]
    fn options_default_are_readable() {
        let options = FormulaEditorOptions::default();
        assert!(options.pt >= 16.0 && options.pt <= 22.0);
        assert!(options.show_palette);
    }
}
