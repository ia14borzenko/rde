# Спайк P5: движок LaTeX-редактора формул (Research Gate)

| Поле | Значение |
|---|---|
| Дата | 2026-10-29, итерация `math-editor` (P5) |
| Статус | завершено; решение — [ADR-0021](../adr/0021-latex-math-editor.md) |
| База | HEAD `3e21164` (тег `formula-tab`), рабочее дерево чистое (после восстановления фикстур) |
| Канон | [`docs/tz-03.md`](../tz-03.md) §4 (NFR-PERF-01), §0.4 (NFR-LIC-01/02), §5.7.5/§16; ADR-0005 (§5.1 webview-запрет), ADR-0018 (STIX/Word-вёрстка), ADR-0020 (редактор P4) |
| Окружение | Rust/cargo **1.99.1**; egui/egui_kittest **0.36.2**; target `/srv/yocto/rde-target`; Linux; release-замеры на этом железе |
| Продуктовый код | **не менялся** в рамках спайков; harness-исходники — в [`harness/`](harness/), зависимости спайков — только в `.cache-tmp/latex-editor-spikes/` (не коммитятся) |

Спайки отвечают на вопросы §2.1 ТЗ сессии: чем нативно (без webview) делать WYSIWYG-редактор
структурных формул с LaTeX-источником истины, паритетом с MathType по UX и визуальным паритетом
с документом.

---

## 1. Методика

- Каждый кандидат проверялся по **первоисточникам**: распакованный исходник crate из локального
  registry-cache (`~/.cargo/registry/src`), `Cargo.toml`, `LICENSE`/`NOTICE`, README, публичный API,
  живой `cargo build/run`; метаданные — crates.io API (`downloads`, даты, версии).
- Замеры — `Instant` вокруг вызовов, N=200 итераций (latex-rust) / N=20 (WordCraft-путь), **release**;
  одинаковые входные формулы: простая, демо-σ, «сложная» из импорта MathType, матрица.
- Рендер-паритет — одна и та же σ-формула отрисована кандидатом (`latex-rust`) и документным
  конвейером WordCraft: [`assets/p5-latex-rust-sigma.png`](assets/p5-latex-rust-sigma.png),
  [`assets/p5-wordcraft-sigma.png`](assets/p5-wordcraft-sigma.png).
- Ограничение честности: Word на стенде нет, fidelity-гейты — LO-оракул; здесь сравнивается только
  рендер собственных движков.

## 2. Кандидаты: факты и лицензии

### 2.1 `latex-rust` 3.0.1 — pure-Rust LaTeX-**рендерер** (не редактор)

| Факт | Значение | Источник |
|---|---|---|
| Версия/даты | 3.0.1, создан 2026-09-02, обновлён 2026-10-15; 12 версий | crates.io API |
| Скачивания | **6 695** (recent 6 695) | crates.io API |
| Лицензия | **MIT OR Apache-2.0**; шрифт STIX Two Math — SIL OFL 1.1 (в комплекте) | `Cargo.toml`, crate `fonts/stix-two-math`, `NOTICE` |
| MSRV / зависимости | rust **1.99**; `skrifa =0.39.0`, `egui =0.36.2` (feature `egui`), `tiny-skia` (feature `png`) | `Cargo.toml` |
| Размер | `.crate` **0.9 МБ** (тянет skrifa/read-fonts/font-types + опционально tiny-skia); рантайм-зависимостей/сети нет | registry-cache |
| Что умеет | parse LaTeX-math → типизированный AST (`MathNode`, 30+ вариантов), TeX-faithful layout (`MathBox`), SVG/PNG/egui-shapes; матрицы/cases/aligned, скобки, суммы/интегралы, акценты, `\mathbb/\mathcal/\mathfrak`, `\text` | исходник + спайк |
| Чего **не** умеет | **нет source spans** в AST; нет модели редактирования/каретки/слотов/undo/выделения; нет hit-test; `MathBox` — только габариты/глифы (нет привязки box→AST-узел); visual parity с документом — свой TeX-лейаут | спайк §3.1 |

Вывод: годный **рендер**, но редактор на нём пришлось бы писать с нуля (модель, layout слотов,
каретка, hit-test, выделение, клавиатура) — это наибольший объём работ из всех вариантов.

### 2.2 `mathtex-editor-core`/`-session`/`-keymap` 0.4.1 + `mathtex` 0.2.0 — структурный редактор

| Факт | Значение | Источник |
|---|---|---|
| Версии/даты | editor-core 0.4.1 (2026-10-16, 7 версий, **160** скачиваний); движок `mathtex` 0.2.0 (2026-09-27) | crates.io API |
| Лицензия «шапки» | MIT OR Apache-2.0 (editor-core, session, keymap, ir, engine, facade) | `Cargo.toml` |
| **Лицензионный блокер** | `mathtex-portable-engine-generated` — машинный перевод исходников **TeX/eTeX/XeTeX** (`texlive-source` commit d92f5f98…): в LICENSE — «all rights reserved», «Copying of this file is authorized only if … you make absolutely no changes to your copy», унаследованные ограничения TeX-производных; поле `license-file`, не OSI-permissive → **нельзя в бандл MIT/Apache (NFR-LIC-01/02)** | `THIRD-PARTY-LICENSES.json`; выдержка — [`harness/p5-mathtex-generated-license-excerpt.txt`](harness/p5-mathtex-generated-license-excerpt.txt) |
| Деплой-блокер | редактор требует **`format.pkg`**, испечённый из TeX Live под **конкретную сборку движка** («A format belongs to the engine build that baked it»); на стенде **TeX Live не установлен** (`kpsewhich/tex` отсутствуют) → нужна тяжёлая воспроизводимая сборка формата + шрифты (Latin Modern/OFL и др.) | README `mathtex-engine`, `src/format.rs` |
| Что умеет (UX — лучший из готовых) | структурное дерево, каретка/selection/undo/clipboard, keymap с авто-коррекцией (`a/b`→дробь), ~190 backslash-слов, geometry каретки **со спанами LaTeX (byte ranges)**, hit-test, шаблоны, документ как versioned JSON | README editor-core, `src/lib.rs` |
| Импорт LaTeX | **нет** LaTeX→модель (только экспорт `to_tex_with`); у нас `formula.toml.latex` — источник истины → нужен свой конвертер | README/исходник |

Вывод: технически самый «готовый MathType-класс», но **лицензия движка и `format.pkg`-цепочка
несовместимы с NFR-LIC-01/02/NFR-DEP-* и офлайн-поставкой** без смены модели поставки; отклонён.

### 2.3 WordCraft math engine (в `vendor/`) + **свой UI** — документный движок как ядро

| Факт | Значение | Источник |
|---|---|---|
| Лицензия | в репозитории WordCraft (MIT/Apache-2.0), уже в бандле RDE | `vendor/wordcraft`, ADR-0001 |
| Модель | OMML-дерево `wordcraft_doc::math`: `MNode`/`Arg`/`Math` (+`parse_linear`/`to_linear`) | `crates/doc/src/math.rs` |
| Редактирование | `wordcraft_doc::math_edit`: `MathPos`(path,off), `units`, `insert_text`, `insert_nodes`, `delete_back/forward`, `move_left/right/vertical`, `home_end`, `next_placeholder`, `autocorrect`, `build_up`, `structure_actions`, `apply_structure`, `set_limits`, `tidy` — это **те же примитивы, что у `equation.*`** | `crates/doc/src/math_edit.rs` |
| LaTeX | `math_latex::{parse_latex, to_latex}`: parse — есть (LaTeX→модель), to_latex — 90 %-й, но **не полностью roundtrip-safe** (§3.3) | `crates/doc/src/math_latex.rs` |
| Вёрстка и геометрия | `layout::math::layout_equation` → `MathLayout { width, ascent, descent, items, slots }`; `slot(path,off)` — точка каретки (x, baseline y, extent a/d), `hit(x,y)` — хит-тест, `arg_extent(path)` — габариты аргумента | `crates/layout/src/math.rs` |
| Растр | `render::render_area(&doc, page, x,y,w,h, ppp, &opts)` — тот же vello_cpu-растеризатор и STIX-шрифт, что у документа (`RenderOptions::display.placeholders` рисует пустые слоты) | `crates/render`, `ui-egui/src/previews.rs:math_image` |
| Палитры | `engine::math_gallery`: **222** шаблона структур (дроби/корни/скрипты/суммы/пределы/интегралы/скобки/акценты/матрицы/cases/выравнивания), **8** наборов символов, **14** готовых формул | `crates/engine/src/math_gallery.rs` |
| Доступ | всё — **публичные** элементы через `wordcraft_engine` (реэкспорты `doc`/`layout`/`render`); в `rde-ui`/`rde-doc` уже подключён `wordcraft-engine` → **новых зависимостей нет** | `crates/engine/src/lib.rs` |
| Минусы | это движок «в стиле Word Equations»; вендор-зависимость (форк-политика ADR-0001 §4); свой рендер/геометрия редактора — на нас; визуальный паритет — **по построению** (тот же конвейер) |  |

### 2.4 Webview-кандидаты (MathLive/MathQuill/KaTeX/MathJax)

Не прототипировались: **webview/JS запрещены решением владельца** (ADR-0005 §5.1, «Webview/wry не
используются нигде»); ADR-допустимое исключение — только по явному «да» владельца. Спайки 2.1–2.3
показали, что нативный путь даёт паритет за обозримый объём → **вопрос владельцу в этой итерации не
требуется** (явление §1.4 ТЗ: спрашивать только если паритет достижим исключительно webview).

### 2.5 Внешние GPL-редакторы (LyX, TeXmacs, KLatexFormula, LibreOffice Math)

GPL/MPL — **не в бандл**, только внешние референсы UX/оракулы вёрстки (как и было в ТЗ); для
продуктового редактора не рассматриваются.

## 3. Спайки и измерения

### 3.1 `latex-rust`: parse/layout/egui-shapes (release, N=200, этот стенд)

| Формула | parse, µs | layout, µs | egui shapes, µs | shapes |
|---|---|---|---|---|
| `x=Vt` | 3,66 | 10,11 | 84,09 | 4 |
| σ-демо (дроби/корень/сумма) | 11,88 | 88,36 | 191,58 | 22 |
| сложная (MathType-импорт) | 15,18 | 240,15 | 490,13 | 32 |
| матрица 2×2 | 5,11 | 60,30 | 41,40 | 6 |
| cases | 8,39 | 63,43 | 165,42 | 15 |
| акценты | 7,03 | 33,22 | 120,31 | 14 |
| `\text{…}` | 3,21 | 15,19 | 73,83 | 7 |

Вывод по перфу: полный цикл «строка → shapes» ≈ 0,1–0,7 мс release — бюджет §3.4 (≤16 мс на
нажатие) выдерживается. **Но** геометрии каретки/слотов нет: «каретка по префиксам» (рендер
подстрок) не работает — префикс пере-вёрстывается, размеры скриптов/структур меняются, позиция
каретки из этого не выводится; нужен собственный layout + маппинг AST→box (у `MathBox` нет ссылок
на узлы AST). Плюс визуально это **другой TeX-лейаут**, не документный (см. §3.4).

### 3.2 WordCraft-путь: вёрстка+растр (тот же вызов, что будет у редактора)

`spike_math_engine.rs` собирает scratch-документ с одной формулой, зовёт
`layout::layout` → `MathLayout` (для каретки) и `render::render_area` (для картинки);
на каждой итерации — полный цикл (Document+Paragraph+layout+render+to_straight).

| Формула | debug, мс/цикл | **release, µs/цикл** | габариты (pt) | слотов каретки |
|---|---|---|---|---|
| `x=Vt` | 1,46–1,69 | **168,8** | 41,7×8,9 | — |
| σ-демо | 2,91–2,95 | **218,7** | 144,4×35,5 | **46** |
| сложная (MathType-импорт) | 3,28–3,49 | **252,7** | 227,0×31,1 | — |
| матрица 2×2 | 1,79–2,04 | **142,1** | 37,2×25,9 | — |

Полный цикл «модель → layout → растр → ColorImage» в release — **0,14–0,25 мс** (для σ-формулы при
ppp=4 (ретина) — 0,22 мс). В редакторе растр пересчитывается **не каждый кадр**, а только при
изменении модели/зума/темы (кэш текстуры) → бюджет нажатия (≤16 мс) выдерживается с запасом.

Геометрия каретки работает: `MathLayout.slots` для σ-формулы даёт 46 позиций с путём
`[(node,arg),…]`, offset, `x`, `y` (базовая линия, вверх), `a`/`d` (высота/глубина) — этого
достаточно для отрисовки каретки и для `hit()` (клик → ближайший слот). `arg_extent(path)` — для
подсветки аргументов.

### 3.3 Roundtrip LaTeX→модель→LaTeX (корпус 316 случаев)

Корпус: демо-σ, 62 импортированные MathType-формулы, 222 шаблона Equation-галереи, 31 конструкция
чек-листа P0. `parse_latex` → `to_latex` → `parse_latex`, сравнение AST.

| Результат | Число | Что именно |
|---|---|---|
| структурный roundtrip без потерь | **307/316** | — |
| расхождения | 9 | (1) пустые строки матриц/`aligned` у шаблонов галереи (модель→LaTeX теряет trailing-пустые строки); (2) `\ ` (control space) и запятая+пробел: `to_latex` пишет пробел, parse его съедает → в AST теряется `Space`; (3) `\begin{array}{cc}` → `\begin{matrix}` (потеря colspec); (4) `\binom` → `\genfrac{}{}{0pt}{}{n}{k}`, который `parse_latex` **не понимает** → мусор при повторном парсе |
| идемпотентность `to_latex` | 308/316 | те же 8 случаев + склейка пробелов вокруг `\ne`/`,` |

Вывод: вендорный `to_latex` нельзя использовать как сериализатор источника истины **без правок**.
Решение (ADR-0021): собственный roundtrip-safe сериализатор в RDE (`MNode`→LaTeX) с
нормативными тестами `parse(serialize(model)) == model` на корпусе; вендорный `parse_latex`
используется для импорта LaTeX, вендорный `to_latex` остаётся только для совместимости
(сопоставление клика по формуле).

### 3.4 Сравнение рендера (Spike 5)

Одна σ-формула, два конвейера:

- [`assets/p5-latex-rust-sigma.png`](assets/p5-latex-rust-sigma.png) — `latex-rust` 3.0.1:
  пределы суммы **справа** (`∑^N_{i=1}`), свой TeX-лейаут, величины/кернинги отличаются.
- [`assets/p5-wordcraft-sigma.png`](assets/p5-wordcraft-sigma.png) — документный конвейер WordCraft
  (STIX Two Math, vello_cpu): пределы **сверху/снизу** (Word-канон display-формул, ADR-0018),
  те же глифы/метрики, что в документе.

Оба рендера «красивые», но **разные**: при выборе `latex-rust` редактор показывал бы формулу не так,
как она выглядит в документе → нужен либо второй рендер (паритет vs скорость), либо обоснованная
разница. WordCraft-путь даёт паритет **по построению** (один код вёрстки/растра/шрифта).

### 3.5 Release-замеры WordCraft-пути (итог)

`cargo test -p rde-doc --release --test spike_math_engine`: полный цикл (scratch-Document +
`layout::layout` + `render_area` + `to_straight`) — **142–253 µs** на формулу (см. §3.2); roundtrip
316 случаев — **1,17 мс суммарно** (3,7 µs/случай). Это подтверждает: пересчёт растра на каждое
изменение модели допустим (кэш — по `(source, display, pt, ppp, theme)`; можно добавить ленивость,
если понадобится).

### 3.6 Операции редактирования

Спайк подтвердил на модели: `insert_text` в каретку, `move_left/right/vertical`,
`next_placeholder` (Tab по слотам), `delete_back/forward`, `structure_actions` →
`apply_structure` (дроби/скобки/лимиты/матрицы), `build_up`/`autocorrect` (Word-подобный ввод),
`set_limits` (переключение пределов `\sum`), undo — снапшот дерева `Arg` (клон дёшев: формулы
маленькие). Галерея: 222 шаблона, 8 наборов символов, 14 готовых формул — покрывают P0/P1-чек-лист
MathType.

## 4. Матрица критериев (1 — плохо, 5 — отлично)

| Критерий | latex-rust | mathtex-editor | **WordCraft + свой UI** | webview MathLive |
|---|---|---|---|---|
| Лицензия/бандл (NFR-LIC) | 5 (MIT/Apache, OFL-шрифт в комплекте) | **1** (TeX-производная, non-standard) | **5** (уже в бандле) | 4 (MIT, но webview-стек) |
| Зрелость/поддержка | 3 (новый, 6,7k скачиваний) | 2 (160 скачиваний, 0.4.x) | 5 (наш вендор, тесты/CI) | 5 |
| Совместимость (Rust 1.99/egui 0.36.2/MSRV) | 4 (MSRV 1.99 ok; `skrifa =0.39` новый для нас) | 3 (нужен bake-пайплайн) | **5** (уже зависимость) | 3 (wry/webkit2gtk) |
| Функциональность (структурный ввод/undo/…) | 1 (только рендер) | 5 | **4** (примитивы есть, UI пишем) | 5 |
| LaTeX-first (импорт/экспорт/roundtrip) | 5/5 parse, но наш сериализатор всё равно нужен | экспорт есть, импорта нет | **4** (parse есть; сериализатор — свой) | 5 |
| Геометрия каретки/слотов/hit-test | 1 (нет) | 5 (спаны LaTeX) | **5** (`slots`/`hit`/`arg_extent`) | 5 |
| Visual parity с документом/STIX | 2 (другой TeX-лейаут) | 2 (свой TeX-лейаут) | **5** (тот же конвейер) | 2 |
| Производительность | **5** (0,1–0,7 мс/цикл) | 3 (TeX-движок, format) | 4 (1,5–3,5 мс debug; release — §3.5) | 3 (webview) |
| Объём внедрения | 2 (редактор с нуля) | 3 (конвертер+format+UI) | **4** (UI+кодек+интеграция) | 3 (+webview-архитектура) |
| Риски (лицензии/vendor/…) | 3 (новая зависимость) | **1** (лицензия/сборка) | **4** (вендор-зависимость, но уже принята ADR-0001) | **1** (запрет владельца) |

**Победитель: WordCraft math engine + собственный UI** (вариант (c) из §2.1 ТЗ).

## 5. Целевая архитектура (эскиз, детали — ADR-0021)

- **Модель**: `wordcraft_doc::math::{Arg, MNode}` + `math_edit::MathPos` (path/off) — та же модель,
  что в документе.
- **Источник истины**: `formula.toml.latex`. Открытие: `math_latex::parse_latex`. Сохранение: **свой**
  `to_latex` (roundtrip-safe, тесты на корпусе) + валидация `parse(serialize) == model` и запрет
  Unicode-мусора (`▒`, `█`, `√` в тексте run’ов и т.п.).
- **Вёрстка/рендер**: scratch-документ с одной формулой → `layout::layout` (берём `MathLayout` из
  `ParaLayout.maths`) → `render::render_area` (ink-mask, тинт темой) → `egui::TextureHandle`; кэш по
  `(latex-модель, display, pt, zoom, ppp)`. Каретка/выделение — оверлей egui по `MathLayout.slots`.
- **Ввод**: клавиатура → `math_edit` (type/delete/move/Tab/backspace), структурные шаблоны из
  `engine::math_gallery`, палитры символов из `SYMBOL_SETS`, LaTeX-команды через `build_up`/
  `autocorrect` + автодополнение по `math_symbols`; undo/redo — стек снапшотов `Arg`.
- **Интеграция**: панель вкладки «Формула» (как в P4), «Применить» → `update_formula_source` →
  `compile_formula`/ledger (существующий конвейер OMML/SVG), «Вставить»/«Обновить документ» — без
  изменений; клик по формуле в тексте — существующий путь.
- **Зависимости**: новых нет; всё через публичные API `wordcraft-engine` (реэкспорты doc/layout/render).

## 6. Риски / открытые вопросы

1. Вендорный `to_latex` не roundtrip-safe (§3.3) → свой сериализатор; риск — неполное покрытие
   экзотических конструкций MathType-импорта; митигация: корпусные тесты, fallback «не менять
   источник, если roundtrip не подтверждён».
2. Публичность/стабильность вендорных API: `MathLayout.slots`, `layout_equation`, `render_area` —
   публичны и используются; при обновлении пина вендора API может измениться (правило ADR-0001 §4,
   без прямых правок `vendor/`).
3. Растр на каждое изменение модели: release-замер §3.5; при необходимости — асинхронный/ленивый
   пересчёт (показывать последний кадр).
4. Визуальный паритет «по построению» — но при редактировании формула рисуется *в панели*, а не в
   документе: масштаб/зум панели могут отличаться (обоснованно; предпросмотр совпадает с документом
   при том же pt).
5. «Клон MS Equations»: движок под капотом тот же, что у WordCraft-редактора, но **UX/клавиши/
   палитры — свои, MathType-класса**; вендорская вкладка Equation в UI RDE не используется и будет
   удалена (ADR-0020 §5.2 → ADR-0021).
6. Лицензии: новых ассетов/зависимостей нет; STIX Two Math уже в NOTICE (ADR-0018). NOTICE не меняется.

## 7. Артефакты спайков

| Артефакт | Где |
|---|---|
| Исходник спайка `latex-rust` (release-замеры, PNG/SVG, покрытие конструкций) | [`harness/spike_p5_latex_rust.rs.txt`](harness/spike_p5_latex_rust.rs.txt) |
| Исходник спайка WordCraft-пути (roundtrip-корпус, замеры, slots/hit, edit-ops, галерея) | [`harness/spike_p5_wordcraft_math.rs.txt`](harness/spike_p5_wordcraft_math.rs.txt) |
| Выдержка LICENSE `mathtex-portable-engine-generated` | [`harness/p5-mathtex-generated-license-excerpt.txt`](harness/p5-mathtex-generated-license-excerpt.txt) |
| Рендер σ-формулы: кандидат / документный конвейер | [`assets/p5-latex-rust-sigma.png`](assets/p5-latex-rust-sigma.png), [`assets/p5-wordcraft-sigma.png`](assets/p5-wordcraft-sigma.png) |
| Временные crate’ы/распаковки (не коммитятся) | `.cache-tmp/latex-editor-spikes/` |

## 8. Вывод (Research Gate)

1. **Выбран путь (c)**: нативный редактор на модели/примитивах/вёрстке/растре WordCraft + собственный
   MathType-подобный UI; LaTeX — источник истины, сериализатор — собственный, roundtrip-тесты.
2. Нативных альтернатив с готовым UX и чистой лицензией нет: `latex-rust` — только рендер (и не
   документный), `mathtex-editor` — лицензионный/деплой-блокер, webview — запрет владельца.
3. Паритет с MathType достижим поэтапно: P0 (дроби/корни/скрипты/пределы/скобки/матрицы/cases/
   акценты/операторы/палитры/клавиатура/undo/clipboard/LaTeX) — MVP; P1 (матричный тулбар,
   выравнивания, swap/delete структур, MathML/OMML-копипаст, нумерация) — следующая очередь.
4. Решение и план фаз — [ADR-0021](../adr/0021-latex-math-editor.md).
