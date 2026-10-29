//! Вёрстка и растр формулы **тем же конвейером, что документ** (P5, ADR-0021 §Решение 1):
//! scratch-документ с одной формулой → `layout::layout` → `MathLayout` (каретка/слоты) →
//! `render::render_area` (vello_cpu + STIX, ADR-0018) → ink-mask для egui-текстуры.
//!
//! Это даёт **visual parity by construction**: в панели редактора формула выглядит так же, как в
//! документе (тот же код вёрстки, тот же шрифт, тот же растеризатор), но без второго `WordApp`.
//!
//! Геометрия: координаты `MathLayout.slot` — точки в pt, y вверх от базовой линии. Наружу отдаём
//! локальные пиксели картинки (`0,0` — левый верх), чтобы рисовать каретку/выделение поверх текстуры.

use std::collections::HashMap;
use std::sync::Arc;

use egui::{ColorImage, Pos2, Rect, Vec2};
use wordcraft_engine::doc::math::{Arg, Math, to_linear};
use wordcraft_engine::doc::props::{Align, CharProps, LineSpacing};
use wordcraft_engine::doc::{Document, InlineObject, Paragraph, para_block};
use wordcraft_engine::layout::Placed;
use wordcraft_engine::layout::math::MathLayout;

use wordcraft_engine::doc::math_edit::{MathPos, arg_at, units};

/// Готовый рендер формулы: картинка (ink-mask), вёрстка (слоты каретки) и масштаб растра.
#[derive(Clone)]
pub struct FormulaRender {
    /// Пиксели: белый с alpha=покрытие (тинтуется цветом темы при отрисовке).
    pub image: ColorImage,
    /// Вёрстка уравнения: слоты каретки/хит-тест (pt, y вверх от базовой линии).
    pub layout: MathLayout,
    /// Ширина/высота формулы в pt.
    pub width_pt: f32,
    pub height_pt: f32,
    /// Подъём над базовой линией, pt.
    pub ascent_pt: f32,
    /// Масштаб растра: пикселей на pt по X/Y (может чуть отличаться из-за округления).
    pub scale_x: f32,
    pub scale_y: f32,
    /// Поля вокруг формулы в растре, pt (по 1 pt с каждой стороны): нужны для точной
    /// привязки каретки/кликов к картинке.
    pub pad_pt: f32,
}

impl FormulaRender {
    /// Размер картинки в пикселях.
    #[must_use]
    pub fn size_px(&self) -> Vec2 {
        Vec2::new(self.image.width() as f32, self.image.height() as f32)
    }

    /// Расстояние от верхнего края картинки до базовой линии формулы, pt (с учётом поля).
    #[must_use]
    pub fn baseline_from_top_pt(&self) -> f32 {
        self.ascent_pt + self.pad_pt
    }

    /// Прямоугольник каретки в локальных пикселях картинки.
    #[must_use]
    pub fn caret_rect(&self, pos: &MathPos) -> Option<Rect> {
        let slot = self.layout.slot(&pos.path, pos.off)?;
        let baseline = self.baseline_from_top_pt();
        let x = (slot.x + self.pad_pt) * self.scale_x;
        // Толщина каретки 1.5 px, но не меньше 1.
        let width = (self.scale_x * 1.2).max(1.0);
        let top_px = (baseline - slot.y - slot.a) * self.scale_y;
        let bottom_px = (baseline - slot.y + slot.d) * self.scale_y;
        // Пустой слот бывает нулевой высоты — рисуем «строчную» высоту, чтобы каретка была видна.
        let line_px = self.scale_y * 11.0;
        let (top_px, bottom_px) = if (bottom_px - top_px).abs() < line_px {
            let base = (baseline - slot.y) * self.scale_y;
            (base - line_px * 0.75, base + line_px * 0.25)
        } else {
            (top_px, bottom_px)
        };
        Some(Rect::from_min_max(Pos2::new(x, top_px), Pos2::new(x + width, bottom_px)))
    }

    /// Хит-тест: точка в локальных пикселях картинки → ближайшая позиция каретки.
    #[must_use]
    pub fn hit(&self, local: Pos2) -> Option<MathPos> {
        let x_pt = local.x / self.scale_x.max(f32::EPSILON) - self.pad_pt;
        let y_pt = self.baseline_from_top_pt() - local.y / self.scale_y.max(f32::EPSILON);
        let slot = self.layout.hit(x_pt, y_pt)?;
        Some(MathPos::new(slot.path.clone(), slot.off))
    }

    /// Хит-тест по смещению указателя от левого верхнего угла картинки в UI-точках.
    /// Клик за пределами картинки клампится к её краю: клик справа/снизу — в конец формулы,
    /// слева/сверху — в начало (как в текстовом поле), это ожидаемое поведение мыши.
    #[must_use]
    pub fn hit_ui(&self, offset_points: egui::Vec2, pixels_per_point: f32) -> Option<MathPos> {
        let ppp = pixels_per_point.max(f32::EPSILON);
        let size = self.size_px();
        let local = offset_points * ppp;
        self.hit(Pos2::new(local.x.clamp(0.0, size.x), local.y.clamp(0.0, size.y)))
    }

    /// Прямоугольники подсветки выделения (для selection в одном аргументе).
    ///
    /// MVP-ограничение: подсветка показывается, когда якорь и каретка — в одном аргументе (частый
    /// случай Shift+стрелок внутри строки); через границы структур каретка двигается, подсветка
    /// аргументов — P1 (см. ADR-0021 §Риски).
    #[must_use]
    pub fn selection_rects(&self, anchor: &MathPos, focus: &MathPos) -> Vec<Rect> {
        if anchor == focus || anchor.path != focus.path {
            return Vec::new();
        }
        let lo = anchor.off.min(focus.off);
        let hi = anchor.off.max(focus.off);
        let mut slots: Vec<_> = self.layout.slots.iter().filter(|slot| slot.path == anchor.path && slot.off >= lo && slot.off <= hi).collect();
        slots.sort_by(|a, b| a.x.total_cmp(&b.x));
        let mut rects = Vec::new();
        for pair in slots.windows(2) {
            let (left, right) = (pair[0], pair[1]);
            if right.x <= left.x {
                continue;
            }
            let top_pt = left.y + left.a;
            let bottom_pt = left.y - left.d;
            let baseline = self.baseline_from_top_pt();
            let x0 = (left.x + self.pad_pt) * self.scale_x;
            let x1 = (right.x + self.pad_pt) * self.scale_x;
            let y0 = (baseline - top_pt) * self.scale_y;
            let y1 = (baseline - bottom_pt) * self.scale_y;
            rects.push(Rect::from_min_max(Pos2::new(x0, y0.min(y1)), Pos2::new(x1, y0.max(y1))));
        }
        // Полное выделение пустого аргумента (lo == hi) или одного слота: тонкая плашка у каретки.
        if rects.is_empty()
            && let Some(slot) = self.layout.slot(&anchor.path, lo)
        {
            let baseline = self.baseline_from_top_pt();
            let x0 = (slot.x + self.pad_pt) * self.scale_x;
            let width = (self.scale_x * 6.0).max(3.0);
            let y0 = (baseline - slot.y - slot.a) * self.scale_y;
            let y1 = (baseline - slot.y + slot.d) * self.scale_y;
            rects.push(Rect::from_min_max(Pos2::new(x0, y0.min(y1)), Pos2::new(x0 + width, y0.max(y1))));
        }
        rects
    }

    /// Расширить выделение до границ структуры вокруг каретки (двойной клик/выбор аргумента).
    #[must_use]
    pub fn argument_extent(&self, pos: &MathPos) -> Option<Rect> {
        let extent = self.layout.arg_extent(&pos.path)?;
        let (x0, x1, up, down) = extent;
        let baseline = self.baseline_from_top_pt();
        let top = (baseline - up) * self.scale_y;
        let bottom = (baseline + down) * self.scale_y;
        Some(Rect::from_min_max(Pos2::new((x0 + self.pad_pt) * self.scale_x, top), Pos2::new((x1 + self.pad_pt) * self.scale_x, bottom)))
    }
}

/// Отрисовать формулу: `model` в стиле `display`, кегль `pt`, растр `ppp` пикселей на pt.
///
/// `ppp` — обычно `ctx.pixels_per_point() * zoom`: чем больше, тем чётче текст в панели.
#[must_use]
pub fn render_formula(model: &Arg, display: bool, pt: f32, ppp: f32) -> Option<FormulaRender> {
    if model.is_empty() || !pt.is_finite() || !ppp.is_finite() || pt <= 0.0 || ppp <= 0.0 {
        return None;
    }
    let pt = pt.clamp(4.0, 96.0);
    let ppp = ppp.clamp(0.5, 8.0);
    let math_layout = layout_model(model, display, pt)?;
    let width_pt = math_layout.width.max(0.01);
    let ascent_pt = math_layout.ascent.max(0.01);
    let descent_pt = math_layout.descent.max(0.0);
    let height_pt = ascent_pt + descent_pt;
    // Растр: рисуем ровно габарит формулы (чуть больше по высоте — чтобы не срезать выносные).
    let pad_pt = 1.0;
    let area_x = -pad_pt;
    let area_top = -ascent_pt - pad_pt;
    let area_w = width_pt + pad_pt * 2.0;
    let area_h = height_pt + pad_pt * 2.0;
    let image = rasterize(model, display, pt, ppp, area_x, area_top, area_w, area_h, ascent_pt + pad_pt)?;
    let width = image.width() as f32;
    let height = image.height() as f32;
    Some(FormulaRender {
        image,
        layout: math_layout,
        width_pt,
        height_pt,
        ascent_pt,
        scale_x: width / area_w,
        scale_y: height / area_h,
        pad_pt,
    })
}

/// Сверстать модель в `MathLayout` (слоты каретки/габариты) без растеризации.
fn layout_model(model: &Arg, display: bool, pt: f32) -> Option<MathLayout> {
    let (document, page) = layout_document(model, display, pt)?;
    let mut math_layout = None;
    for placed in &page.items {
        if let Placed::Lines { para, .. } = placed
            && let Some((_, layout)) = para.maths.first()
        {
            math_layout = Some(layout.as_ref().clone());
            break;
        }
    }
    let _ = &document;
    math_layout
}

/// Собрать scratch-документ с одной формулой и разложить его.
fn layout_document(model: &Arg, display: bool, pt: f32) -> Option<(Document, wordcraft_engine::layout::Page)> {
    let props = CharProps { size: Some(pt), ..Default::default() };
    let mut paragraph = Paragraph::with_text("", props.clone());
    paragraph.props.align = Some(Align::Left);
    paragraph.props.space_before = Some(0.0);
    paragraph.props.space_after = Some(0.0);
    paragraph.props.line_spacing = Some(LineSpacing::Multiple(1.0));
    let equation = InlineObject::Equation { linear: to_linear(model), display, math: Math { nodes: model.clone(), ..Default::default() } };
    paragraph.insert_object(0, equation, &props).ok()?;
    let mut document = Document::new();
    document.body = vec![para_block(paragraph)];
    // Широкое «полотно»: формула в редакторе не должна переноситься (масштаб задаёт панель).
    document.last_section.page_w = 4000.0;
    document.last_section.page_h = 4000.0;
    document.last_section.margin_left = 0.0;
    document.last_section.margin_right = 0.0;
    document.last_section.margin_top = 0.0;
    document.last_section.margin_bottom = 0.0;
    let layout = wordcraft_engine::layout::layout(&document, &mut wordcraft_engine::layout::LayoutCache::new(), &Default::default());
    let page = layout.pages.first()?.clone();
    Some((document, page))
}

#[allow(clippy::too_many_arguments)]
fn rasterize(
    model: &Arg,
    display: bool,
    pt: f32,
    ppp: f32,
    area_x: f32,
    area_top: f32,
    area_w: f32,
    area_h: f32,
    baseline_in_area_pt: f32,
) -> Option<ColorImage> {
    let (document, page) = layout_document(model, display, pt)?;
    // Позиция формулы на странице: первая кластерная позиция первой строки.
    let mut origin_x = 0.0;
    let mut baseline_y = 0.0;
    let mut found = false;
    for placed in &page.items {
        if let Placed::Lines { para, x, y, .. } = placed
            && !para.maths.is_empty()
        {
            let line = para.lines.first()?;
            origin_x = *x + line.xs.first().copied().unwrap_or(0.0);
            baseline_y = *y + line.baseline;
            found = true;
            break;
        }
    }
    if !found {
        return None;
    }
    let _ = baseline_in_area_pt;
    let mut options = wordcraft_engine::render::RenderOptions::default();
    // Редактор показывает пустые слоты структур (видимые места ввода), как Equation WordCraft.
    options.display.placeholders = true;
    let image = wordcraft_engine::render::render_area(
        &document,
        &page,
        origin_x + area_x,
        baseline_y + area_top,
        area_w,
        area_h,
        ppp,
        &options,
    );
    // В растре формула чёрная на белой странице; в панели рисуем ink-mask, тинтуя цветом темы.
    let straight = image.to_straight();
    let mut color = ColorImage::from_rgba_unmultiplied([image.width as usize, image.height as usize], &straight);
    ink_mask(&mut color);
    Some(color)
}

/// Чёрный текст → белый ink-mask с alpha=покрытие (тинтуется цветом темы при отрисовке).
fn ink_mask(image: &mut ColorImage) {
    for pixel in &mut image.pixels {
        let [r, g, b, a] = pixel.to_srgba_unmultiplied();
        let luminance = (u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114) / 1000;
        let ink = 255u32.saturating_sub(luminance.min(255)) * u32::from(a) / 255;
        *pixel = egui::Color32::from_white_alpha(u8::try_from(ink).unwrap_or(u8::MAX));
    }
}

/// Кэш рендеров формулы: ключ — канонический LaTeX + display + кегль + масштаб.
#[derive(Default)]
pub struct RenderCache {
    map: HashMap<String, Arc<FormulaRender>>,
    order: Vec<String>,
}

impl RenderCache {
    /// Ёмкость кэша (формул): редактор держит одну-две, запас на переключение.
    const CAPACITY: usize = 32;

    /// Получить рендер из кэша или отрисовать (ключ — канонический LaTeX модели).
    pub fn get_or_render(&mut self, key: &str, model: &Arg, display: bool, pt: f32, ppp: f32) -> Option<Arc<FormulaRender>> {
        let cache_key = format!("{key}|{display}|{pt:.2}|{ppp:.2}");
        if let Some(render) = self.map.get(&cache_key) {
            return Some(render.clone());
        }
        let render = Arc::new(render_formula(model, display, pt, ppp)?);
        self.map.insert(cache_key.clone(), render.clone());
        self.order.push(cache_key);
        while self.order.len() > Self::CAPACITY {
            let oldest = self.order.remove(0);
            self.map.remove(&oldest);
        }
        Some(render)
    }

    /// Есть ли готовый рендер (без отрисовки).
    #[must_use]
    pub fn peek(&self, key: &str, display: bool, pt: f32, ppp: f32) -> Option<Arc<FormulaRender>> {
        self.map.get(&format!("{key}|{display}|{pt:.2}|{ppp:.2}")).cloned()
    }

    /// Сбросить кэш (формула изменилась / сменился масштаб).
    pub fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

/// Количество units в модели — маленький хелпер для тестов/подсказок (то же, что у `math_edit`).
#[must_use]
pub fn model_units(model: &Arg) -> usize {
    units(model)
}

/// Аргумент модели по пути каретки (для палитры «что сейчас вокруг»).
#[must_use]
pub fn argument_at<'a>(model: &'a Arg, pos: &MathPos) -> Option<&'a Arg> {
    arg_at(model, &pos.path)
}
