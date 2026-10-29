# Итерация `formula-tab` — редактор формул на вкладке ленты «Формула», клик по формуле в тексте, фикс скролла предпросмотра

| Поле | Значение |
|---|---|
| Дата | 2026-10-29 (продолжение P4, ADR-0020, §5.2 «Уточнение») |
| База | тег `formula-editor` (`1d297cc`), дерево чистое |
| Канон | [ADR-0020](../adr/0020-formula-editor-ui.md) §2/§4/§5.2, ADR-0005 P4, ADR-0018/0019, R-DOC-21 фаза 2 |
| Тесты | workspace **592 passed / 0 failed** (`rde-ui` 143; +2 регресс-теста итерации) |
| Vendor | `vendor/wordcraft` не менялся |
| Скриншоты | `gui-p21-formula-tab.png`, `gui-p21-formula-tab-empty.png`, `gui-p21-formula-tab-create.png`, `gui-p21-formula-tab-native.png`, `gui-p21-preview-scroll.png` |

## 1. Что изменилось по итогам приёмки P4

1. **Редактор формулы — вкладка ленты «Формула»** (в верхней панели RDE рядом с «Файл/Правка/
   Главная»), а не плавающее окно: вкладка заменяет группы ленты панелью редактора на всю ширину
   (статус, заголовок, LaTeX-источник, display/inline, политика нумерации, параметры вставки,
   предпросмотр, две строки действий). Пустое состояние — выбор формулы проекта, создание новой,
   подсказка про клик в тексте. Панель адаптивна к ширине центра: две колонки + две строки кнопок
   фиксированной ширины, длинные подписи обрезаются (полные — в подсказке).
2. **Клик по формуле в тексте автоматически открывает редактор** на этой вкладке: WordCraft на клик
   в уравнение входит в `equation.edit`; RDE вызывает `equation.get`, сопоставляет LaTeX с
   project-level формулой (нормализация пробелов; `resolved_latex` из ledger — для формул с
   входами/фрагментами), выходит из инлайн-правки (`equation.exit`, документ не меняется) и
   открывает вкладку «Формула» на объекте (центр — «Документ», режим RDE). Формулы вне реестра
   остаются в нативном режиме (принимаются миграцией ADR-0019).
3. **Нативная вкладка Equation** — по-прежнему отдельное окно («Проф. редактор…»): canvas
   WordCraft не уживается с живым canvas документа в одном кадре (фиксированный egui-id/слои).
4. **Починен скролл предпросмотра страниц:** демо-offset применялся как `vertical_scroll_offset`
   каждый кадр и сбрасывал прокрутку на 0 — колесо мыши и ползунок не работали. Теперь демо-offset
   задаётся только для скриншотов, а в интерактивном режиме прокруткой владеет `ScrollArea`.

## 2. Скриншоты

| Скриншот | Что видно |
|---|---|
| [`gui-p21-formula-tab.png`](gui-p21-formula-tab.png) | вкладка «Формула» ленты: статус «актуальна · вставок 2 · устаревших 0», заголовок, LaTeX-источник, display/inline, нумерация, вставка OMML/позиция/номер, предпросмотр σ, две строки действий |
| [`gui-p21-formula-tab-empty.png`](gui-p21-formula-tab-empty.png) | пустое состояние вкладки: «Формула не выбрана», «Выбрать формулу проекта…», «+ Создать формулу», подсказка «кликните формулу в тексте» |
| [`gui-p21-formula-tab-create.png`](gui-p21-formula-tab-create.png) | создание формулы: поля + предпросмотр на верхней панели (светлая тема) |
| [`gui-p21-formula-tab-native.png`](gui-p21-formula-tab-native.png) | «Профессиональный редактор формулы»: нативная вкладка Equation WordCraft + canvas, документ проекта не меняется |
| [`gui-p21-preview-scroll.png`](gui-p21-preview-scroll.png) | **фикс скролла:** режим «Предпросмотр страниц», демо-offset = 0, колесо мыши прокрутило на страницу 2/3 |

Команды съёмки:

```bash
cd /home/admin-pc/proj/rde-dev/rde
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=60 \
  /srv/yocto/rde-target/debug/examples/screenshot docs/fidelity/gui-p21-formula-tab.png \
  document "" 0 formula-editor dark
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=45 \
  /srv/yocto/rde-target/debug/examples/screenshot docs/fidelity/gui-p21-formula-tab-empty.png \
  document "" 0 formula-tab-empty light
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=45 \
  /srv/yocto/rde-target/debug/examples/screenshot docs/fidelity/gui-p21-formula-tab-create.png \
  document "" 0 formula-create-dialog light
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=80 \
  /srv/yocto/rde-target/debug/examples/screenshot docs/fidelity/gui-p21-formula-tab-native.png \
  document "" 0 formula-editor-native dark tall
# скролл предпросмотра колесом (delta<0 — вниз); демо-offset не используется
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=40 RDE_SCREENSHOT_WHEEL=-320 \
  /srv/yocto/rde-target/debug/examples/screenshot docs/fidelity/gui-p21-preview-scroll.png \
  document "" 0 "" light "" preview
```

## 3. Тесты (+2, workspace 592/0)

| Тест | Что подтверждает |
|---|---|
| `preview_scrolls_with_mouse_wheel` | в режиме «Предпросмотр страниц» при `demo_preview_scroll = 0` колесо мыши над страницами даёт `preview_scroll_offset > 0` (раньше форсированный offset каждый кадр сбрасывал прокрутку — колесо и ползунок «не работали») |
| `formula_ribbon_tab_opens_editor_from_formula_click_in_text` | вкладка «Формула» есть на ленте и показывает пустое состояние; каретка WordCraft на объекте-уравнении демо-документа + `open_formula_editor_from_canvas_click` открывает редактор именно этой формулы, переключает центр на «Документ», режим — RDE, вкладку — «Формула»; на панели видно поле «LaTeX-источник» |
| `every_ribbon_tab_has_groups_and_slots` (обновлён) | вкладок десять (добавлена «Формула»), у каждой есть группа и слот |

Существующие тесты P4/P3/P2, MathType-импорта и фиделити остались зелёными; `rde-ui` — 143.

## 4. Ограничения

- Клик по формуле открывает редактор для формул **реестра** (`rde://formula/…`); формулы вне
  реестра остаются в нативном режиме правки WordCraft (их принимают миграцией, ADR-0019).
- Клик по SVG-вставке формулы (это картинка, не OMML) редактор не открывает — только по OMML-телу;
  ограничение прежнее (SVG-режим — «печатная» вставка).
- Нативная вкладка Equation — отдельное окно (техническое ограничение canvas, ADR-0020 §3).
- Правка «в тексте документа» поверх `equation.edit` не делается: RDE выходит из инлайн-правки,
  источник истины — `formula.toml` (см. ADR-0020 §5.2/§6).
- Word на стенде отсутствует — вёрстка `unverified` (LO-оракул прежний, ADR-0020 §4).
