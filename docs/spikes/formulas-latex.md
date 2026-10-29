# Спайки формул и величин (P0): нативный редактор, SVG-вектор, значения из артефактов

| Поле | Значение |
|---|---|
| Дата | 2026-10-26 |
| Статус | завершено (P0); решения — [ADR-0005](../adr/0005-formulas-and-values-as-project-objects.md), решения владельца — §6 |
| База | HEAD `ab7f409` (`fix(ui): узкий инспектор и одна линия панелей`), рабочее дерево чистое |
| Канон | [`docs/tz-03.md`](../tz-03.md) §4.2, §7, §8.3, §8.5, §9.4, §9.7, R-DOC-11…13, R-DOC-19…24, R-ART-09, R-TREE-01/02, NFR-LIC-02 |
| Окружение | Rust/cargo 1.99.1; poppler 26.01.0 (`pdftocairo`, `pdftotext`, `pdfimages`, `pdffonts`, `pdftoppm`); LibreOffice 26.2.6.3; target — локальный `rde/target` (≈23 ГБ свободно на старте) |
| Продуктовый код | **не менялся**; харнессы спайков — временные examples (исходники сохранены в [`harness/`](harness/), из воркспейса удалены) |

Спайки отвечают на три вопроса P0:

* **A (P0.2)** — можно ли из RDE-кода управлять **нативным** LaTeX-редактором WordCraft
  (`WordApp` + `equation_tab`) и встроить вкладку Equation в панель RDE без правок
  `vendor/wordcraft`;
* **B (P0.3)** — можно ли получить **векторную** SVG-картинку формулы и вставить её в docx
  так, чтобы LibreOffice/Word показывали вектор, а WordCraft — PNG-fallback;
* **C (P0.4)** — работает ли механика «величина = расширение константы со `source` из выхода
  артефакта»: разрешение значения, вставка в документ, устаревание и обновление.

---

## 1. Факты кодовой базы (P0.1)

Прочитано точечно; ниже — только то, что важно для ADR.

### 1.1. Модель проекта и величины (`rde-core`)

| Факт | Где |
|---|---|
| `ConstantValue { value: f64, unit, display, style }` — значение обязательное, других полей нет | [`manifest.rs:156-167`](../../crates/rde-core/src/manifest.rs#L156-L167) |
| `ArtifactManifest.values: BTreeMap<String, ConstantValue>` (только `type = "constants"`) | [`manifest.rs:202-204`](../../crates/rde-core/src/manifest.rs#L202-L204) |
| `InputSpec` — untagged enum: `Artifact{artifact,selector}`, `File`, `Note`, `Source`, `Constants{constants,selector}` | [`manifest.rs:86-112`](../../crates/rde-core/src/manifest.rs#L86-L112) |
| JSON Schema генерируется `schemars` из Rust-типов, валидация — `jsonschema`; `additionalProperties` не запрещены → **новые поля старый код не ломают** | [`manifest.rs:347-395`](../../crates/rde-core/src/manifest.rs#L347-L395) |
| Типы объектов: документ, артефакт, константы, источник, заметка, репозиторий; «служебные объекты» в дереве пока не моделируются | [`project.rs:14-47`](../../crates/rde-core/src/project.rs#L14-L47) |
| Загрузка: документы/источники/заметки/репо, затем артефакты топопорядком; лишние каталоги внутри объекта **игнорируются** (`load_kind` читает только известные подкаталоги `objects/*`) | [`project.rs:363-421`](../../crates/rde-core/src/project.rs#L363-L421) |
| DAG-рёбра — только `hashed`-группы артефактов; цикл-детект DFS-раскраской | [`project.rs:204-313`](../../crates/rde-core/src/project.rs#L204-L313) |
| `delete_object` переносит **весь каталог** объекта в `.rde/trash/` с метаданными (каскад для вложенных данных — бесплатно) | [`ops.rs:592-610`](../../crates/rde-core/src/ops.rs#L592-L610) |
| `write_artifact_manifest` перезаписывает `artifact.toml` из типизированной модели (`toml_edit` не подключён, §15-6) | [`ops.rs:262-274`](../../crates/rde-core/src/ops.rs#L262-L274) |
| `record_artifact_out_hash`/`clear_artifact_out_hash` — точечная правка ключа в TOML без потери остального текста (прецедент «кэш в манифесте») | [`ops.rs:464-518`](../../crates/rde-core/src/ops.rs#L464-L518) |
| `create_document_stub` создаёт `objects/documents/<id>/document.toml`, пакет собирает слой с движком | [`ops.rs:543-565`](../../crates/rde-core/src/ops.rs#L543-L565) |

### 1.2. Хеши, DAG, транзакции (`rde-hash`, `rde-dag`, `rde-store`)

| Факт | Где |
|---|---|
| `act_key = H("rde:act:1" | kind | schema_version | in_hash | script_hash? | params_hash | env_hash)` | [`rde-hash/src/lib.rs:380-396`](../../crates/rde-hash/src/lib.rs#L380-L396) |
| `in_hash` агрегирует только `hashed`-группы (`group_hash` по отсортированным `(target, out, selector)`) | [`rde-hash/src/lib.rs:278-332`](../../crates/rde-hash/src/lib.rs#L278-L332) |
| `out_hash` = Merkle `tree_hash`; JCS (RFC 8785) для `params_hash` | [`rde-hash/src/lib.rs:125-243,259-276`](../../crates/rde-hash/src/lib.rs#L259-L276) |
| Вход-константы хешируются `sha256(canonical_json(ConstantValue))` — **типизированные поля, не разрешённое значение** | [`rde-dag/src/lib.rs:334-361`](../../crates/rde-dag/src/lib.rs#L334-L361) |
| `act_key` compute-артефакта: `in_hash`+`script_hash`+`params_hash`+`env_hash`; данные/константы не активируются | [`rde-dag/src/lib.rs:377-456`](../../crates/rde-dag/src/lib.rs#L377-L456) |
| Статусы: `Fresh/Stale(reasons)/NeverRun/NotComputable`; причины по компонентам ключа + целостность выходов | [`rde-dag/src/lib.rs:127-199,471-584`](../../crates/rde-dag/src/lib.rs#L471-L584) |
| `update` — топопорядок compute, отдельная транзакция на артефакт (или batch), provenance, `out_hash` в манифест | [`rde-dag/src/lib.rs:1026-1359`](../../crates/rde-dag/src/lib.rs#L1026-L1359) |
| `rollback` меняет только корни `artifact/*`; документные корни — зона `rde-doc` (комментарий в коде) | [`rde-dag/src/lib.rs:1414-1508`](../../crates/rde-dag/src/lib.rs#L1414-L1508) |
| `resolve_insert_text` — URI `rde://artifact/<id>@<out_hash>/<selector>`, JSON pointer через `#` | [`rde-dag/src/lib.rs:1546-1578`](../../crates/rde-dag/src/lib.rs#L1546-L1578) |
| `resolve_insert_constant` — URI `rde://constants/<id>/<key>` → текст/`style`/hash из **литеральных** полей | [`rde-dag/src/lib.rs:1603-1634`](../../crates/rde-dag/src/lib.rs#L1603-L1634) |
| Store: CAS-блобы, деревья, tx с payload произвольного `kind`, `current` roots, snapshots, rollback O(1), pins/GC | [`rde-store/src/lib.rs`](../../crates/rde-store/src/lib.rs) (`RootSet` 111-155, `Pin` 198-207, `rollback` 815-855, `gc` 893) |

### 1.3. Документ и вставки (`rde-doc`, `rde-docx`)

| Факт | Где |
|---|---|
| `RdeInsert { id, block, kind: figure|caption|table|note|value, target, caption, number, hash, cached, selector, value_kind, para_id, style, format, run_style }` — реестр вставок в `rde:doc` | [`rde-doc/src/lib.rs:66-103`](../../crates/rde-doc/src/lib.rs#L66-L103) |
| `RdeDoc { schema_version="1", doc_id, template, body_hash, blocks, inserts, review, updated_unix }`; XML парсится/рендерится вручную | [`rde-doc/src/lib.rs:112-124,248-346`](../../crates/rde-doc/src/lib.rs#L112-L124) |
| `format_value(raw, "auto|raw|int|dec:N|sci:N|percent:N")` — форматирование значений (R-DOC-11) | [`rde-doc/src/lib.rs:1007-1075`](../../crates/rde-doc/src/lib.rs#L1007-L1075) |
| `add_insert` — `w:sdt` с `w:tag="rde:<id>"`/`w14:paraId`, кэш rich-text в теле и в `rde:doc` | [`rde-doc/src/lib.rs:1756-1798`](../../crates/rde-doc/src/lib.rs#L1756-L1798) |
| `update_insert_values` — замена текста `w:sdt` по id + обновление `cached`/`hash` | [`rde-doc/src/lib.rs:1808-1839`](../../crates/rde-doc/src/lib.rs#L1808-L1839) |
| `insert_is_stale(insert, expected_hash)` = `insert.hash != expected` | [`rde-doc/src/lib.rs:1841-1844`](../../crates/rde-doc/src/lib.rs#L1841-L1844) |
| `rasterize_svg` (resvg) — SVG→PNG fallback; SVG-рисунки при вставке **заменяются PNG** для DocView | [`rde-doc/src/lib.rs:1857-1891`](../../crates/rde-doc/src/lib.rs#L1857-L1891) |
| `add_figure_insert`/`update_figure_images` — `wp:inline` + `pic:pic` + `r:embed` media-части (`word/media/rde-figure-<id>.<ext>`) | [`rde-doc/src/lib.rs:2272-2414`](../../crates/rde-doc/src/lib.rs#L2272-L2414) |
| `document/<doc_id>` root, `commit_document` — транзакция store с корнем документа | [`rde-doc/src/lib.rs:1848-1851,2551-2572`](../../crates/rde-doc/src/lib.rs#L2551-L2572) |
| `update_document_transactional` — ценности/рисунки/таблицы → перенумерация подписей → refresh/save → diff → store-транзакция | [`rde-doc/src/lib.rs:2618-2661`](../../crates/rde-doc/src/lib.rs#L2618-L2661) |
| Формулы: `EquationSyntax{detect,parse}` (LaTeX при `\`, иначе linear), `EquationRequest`, `add_equation` (insert через модель WordCraft), `list_equations`, `renumber_equations`; нумерация — таблица 1×2 | [`equation.rs:37-305`](../../crates/rde-doc/src/equation.rs#L37-L305) |
| OMML-гейт: `OmmlStats`, `compare_omml` (счётчики `m:oMath`/`m:oMathPara`/`m:eqArr` + структурная sha256), `check_omml`, `FidelityReport::passed()` | [`rde-docx/src/lib.rs:53-79,239-309`](../../crates/rde-docx/src/lib.rs#L239-L309) |

### 1.4. Нативный редактор (`vendor/wordcraft`, публичные API без правок)

| Факт | Где |
|---|---|
| `WordApp::{new(session, services), run(id, params), execute(id, params), ui, doc_view_ui (feature rde-docview)}` | [`ui-egui/src/lib.rs:524,684,890,2353,8923`](../../vendor/wordcraft/crates/ui-egui/src/lib.rs#L890) |
| `pub mod equation_tab`; публичные `builtin_menu(ui, app)`, `insert_button(ui, app)`, `show(app, ui)`; галереи/символы из `math_gallery` | [`ui-egui/src/equation_tab.rs:120,150,264`](../../vendor/wordcraft/crates/ui-egui/src/equation_tab.rs#L120-L150) |
| Команды Equation: `insert.equation` (linear/latex/builtin/display/edit), `equation.edit` (только если каретка у формулы), `equation.exit/type/convert/inputFormat/normalText/insertStructure/insertSymbol/move/display/justify/number/set/get/gallery` | [`engine/src/cmd/equation.rs:24-79`](../../vendor/wordcraft/crates/engine/src/cmd/equation.rs#L24-L79) |
| LaTeX: `parse_latex`, `to_latex`; linear: `parse`/`to_linear`; правка дерева: `math_edit::{insert_text, build_up, autocorrect, each_run_mut, …}` | [`doc/src/math_latex.rs:99,746`](../../vendor/wordcraft/crates/doc/src/math_latex.rs#L99), [`doc/src/math_edit.rs`](../../vendor/wordcraft/crates/doc/src/math_edit.rs), [`doc/src/math_linear.rs:28`](../../vendor/wordcraft/crates/doc/src/math_linear.rs#L28) |
| PDF-экспорт — векторный `wordcraft_pdf::export`; публичная обёртка `io::save_bytes_opts("x.pdf", doc, opts)` | [`pdf/src/lib.rs:109,118`](../../vendor/wordcraft/crates/pdf/src/lib.rs#L109), [`engine/src/io.rs:357-380`](../../vendor/wordcraft/crates/engine/src/io.rs#L357-L380) |
| Галерея: 11 групп структур, 9 наборов символов, 14 готовых формул | [`engine/src/math_gallery.rs:47,507,520`](../../vendor/wordcraft/crates/engine/src/math_gallery.rs#L507-L520) |
| Превью формул для панели — `previews::math_texture(&mut WordApp, ctx, key, nodes, display, pt, size)` (используется самой вкладкой Equation) | [`ui-egui/src/previews.rs:316`](../../vendor/wordcraft/crates/ui-egui/src/previews.rs#L316) |
| **Ограничения**: writer OMML пишет маркер `#`-номера в `m:t`; reader docx берёт **только PNG-fallback** (`a:blip r:embed`) и не знает `asvg:svgBlip`; SVG-экспорта нет; content controls (`w:sdt`) моделью поддерживаются | [`docs/wordcraft-fork.md`](../wordcraft-fork.md), спайк B6 |

### 1.5. RDE-UI

| Факт | Где |
|---|---|
| Встроенный редактор: `EmbeddedEditor { app: wordcraft_ui_egui::WordApp, pages }` — целевая точка встраивания панели формулы | [`crates/rde-ui/src/lib.rs:383-387`](../../crates/rde-ui/src/lib.rs#L383-L387) |
| Создание `WordApp` из `.doc.rde` (extract → `open_path_in` → `Session::new`) | [`lib.rs:1933-1966`](../../crates/rde-ui/src/lib.rs#L1933-L1966) |
| Режим RDE (DocView): лента RDE → `ed.app.doc_view_ui(&mut child)`; в режиме WordCraft — `ed.app.ui(ui)` | [`lib.rs:4638-4680`](../../crates/rde-ui/src/lib.rs#L4638-L4680) |
| `run_editor_action(id)` — запуск команды WordCraft из RDE | [`lib.rs:2143-2147`](../../crates/rde-ui/src/lib.rs#L2143-L2147) |
| Headless-скриншоты: `cargo run -p rde-ui --example screenshot -- <png> [tab] [объект] [панель] [действие] [тема]` | [`crates/rde-ui/examples/screenshot.rs`](../../crates/rde-ui/examples/screenshot.rs) |

---

## 2. Спайк A — нативный редактор формулы из RDE

**Метод.** Временный example `crates/rde-ui/examples/spike_p0_equation.rs` (сохранён:
[`harness/spike_p0_equation.rs.txt`](harness/spike_p0_equation.rs.txt)). Харнесс — ровно тот класс,
что встроен в RDE: `Session::new(Document::new())` → `WordApp::new(session, Services::default())`;
команды выполняются `app.run(id, params)` — тем же путём, каким RDE вызывает
`run_editor_action`. Второй харнесс — `egui_kittest` (`wgpu`, offscreen) рисует панель
`equation_tab::{insert_button, show}` вне WordCraft-окна.

**Сценарий и фактические результаты:**

| # | Команда | Результат |
|---|---|---|
| A1 | `equation.edit` на пустом абзаце | `Err("not available: equation.edit: no equation at the caret")` — **создавать формулу этой командой нельзя** |
| A2 | `insert.equation` | `Ok({"display":true,"editing":true,"linear":"",...})` — display на пустой строке, режим правки включён |
| A3 | `equation.inputFormat {"format":"latex"}` | `Ok({"format":"latex"})` |
| A4 | `equation.type {"text":"\frac{a}{b}"}` | `Ok(linear="\frac{a}{b}")` — пока просто текст |
| A5 | `equation.type {"text":" "}` | `Ok(linear="a/b")` — пробел триггерит build-up LaTeX, дерево `m:f` |
| A6 | `equation.get` | `latex="\frac{a}{b}", linear="a/b", linearForm=false` |
| A7/A8 | `equation.convert → linear` / `→ professional` | A7 линейный текст `\frac{a}{b}`, A8 обратно дерево |
| A9 | `equation.get` | `latex="\frac{a}{b}", linear="a/b"` (круговой LaTeX↔linear без потерь) |
| A10 | `equation.exit` | каретка после формулы (`off=3`) |
| A11 | `equation.edit` **на формуле под каретой** | `Ok(editing=true, linear="a/b")` — открытие существующей формулы работает |
| A13 | `insert.equation {"builtin":"quadratic"}` | `Ok(linear="x=(−b±√(b^2−4ac))/2a")` — галерея доступна |
| A15 | `wordcraft_engine::io::save_path(docx)` | docx сохранён, 6 651 Б |
| A16 | OMML в `word/document.xml` | `m:oMath=3, m:oMathPara=1, m:f=2, m:num=2, m:den=2`; текст `a|b|x=|−b±|b|2|−4ac|2a` |

**Встраивание вкладки Equation в панель RDE (без правок vendor).**
`equation_tab::insert_button` и `equation_tab::show` — публичные функции с сигнатурой
`(&mut Ui, &mut WordApp)`, поэтому рисуются прямо из egui-панели RDE
(WordApp живёт в `EmbeddedEditor.app`). Проверено offscreen-рендером: обе функции компилируются и
рисуются в `egui_kittest`-харнессе, панель 1280×420 — на скриншоте
[`assets/p0-equation-tab.png`](assets/p0-equation-tab.png) видны split-button «Equation»,
группы Tools (Unicode/LaTeX/Convert/Normal Text), Symbols, Structures (11 галерей) и
Equation Options (Display/Inline/Number/Justification/Close Equation).

**Важно (найдено на спайке):** вкладка Equation использует семейства шрифтов `medium`/`semibold`
WordCraft; вне `WordApp::ui` их нужно установить самостоятельно, иначе паника
`FontFamily::Name("semibold") is not bound to any fonts`. Лечение — вызвать
`wordcraft_ui_egui::theme::install_fonts(ctx)` (панель RDE должна это делать до первого кадра
вкладки; `set_fonts` применяется со следующего кадра, поэтому первый кадр панели — без `show`).

**Какие команды нужны панели RDE** (минимальный набор, все уже есть):

* `insert.equation {latex}` — создать формулу из LaTeX-источника объекта (или `builtin`);
* `equation.edit` — открыть существующую формулу (каретка должна стоять на формуле);
* `equation.inputFormat {format}` — переключение LaTeX/Unicode (состояние в `Session.math_latex`);
* `equation.type`/`insertStructure`/`insertSymbol`/`move`/`backspace`/`enter` — ввод и правка;
* `equation.convert {to}` — linear↔professional (LaTeX-источник ↔ собранное дерево);
* `equation.get` — чтение `latex`/`linear`/`display` (источник истины объекта пишется из `latex`);
* `equation.display {value}`/`equation.justify`/`equation.number` — display/inline и оформление;
* `equation.exit` — выход из редактора.

**Чего не хватает (зафиксировано для P4; обходится в RDE без правок vendor):**

1. **Позиционирование каретки по `paraId`/URI объекта.** Команды «поставить каретку к формуле
   объекта» нет; `equation.edit` работает только «у каретки». Обход: RDE выставляет
   `app.session.sel` напрямую (поле публичное, `EmbeddedEditor.app` уже в руках RDE), либо
   `insert.equation` в каретку.
2. **OMML как данные объекта.** Нет команды «дай OMML формулы по якорю» / «вставь OMML» —
   кэш объекта собирается прогоном мини-документа WordCraft и извлечением `m:oMath` из
   `word/document.xml` (проверено A16), либо через `save_engine_document` + пост-обработку пакета
   (RDE и так владеет финальной записью пакета).
3. **Экспорт формулы в изображение** (SVG/PNG) — своей команды нет; SVG делается пайплайном
   спайка B (PDF → `pdftocairo`), либо это upstream-кандидат (SVG-export в WordCraft).
4. **Номер формулы как поле Word/`SEQ`** — нет (тем же gap'ом WordCraft, что и в фазе 1:
   `#`-номер пишется в `m:t`); каноническая нумерация RDE — таблица 1×2
   (`rde-doc/src/equation.rs`), она и остаётся.
5. **Стиль/шрифт формулы из объекта** — частично (`equation.normalText`), но для ГОСТ-оформления
   достаточно шрифта абзаца: `add_equation` уже подхватывает размер основного текста
   (`body_font_size`).

**Вывод A:** нативный LaTeX-редактор WordCraft **полностью доступен из RDE через публичный
`WordApp`** и встраивается в панель без единой правки `vendor/wordcraft` (пункт ADR-0001 §4 не
задействован). Для P4 нужен RDE-слой «объект формулы ↔ LaTeX-источник ↔ OMML-кэш ↔ вставка».

---

## 3. Спайк B — векторная SVG-формула в docx

**Метод.** Временный example `crates/rde-doc/examples/spike_p0_vector.rs` (сохранён:
[`harness/spike_p0_vector.rs.txt`](harness/spike_p0_vector.rs.txt)):

```
WordCraft mini-doc (OMML σ=F/A)
  → wordcraft_engine::io::save_bytes_opts("formula.pdf")        # krilla, вектор
  → pdftocairo -svg formula.pdf formula.svg                     # poppler 26.01
  → кроп bbox: pdftotext -bbox → viewBox/width/height SVG
  → resvg: SVG → PNG fallback
  → docx: <a:blip r:embed=PNG><a:extLst><a:ext uri="{96DA…}"><asvg:svgBlip r:embed=SVG/>
  → LibreOffice headless → PDF → pdfimages -list / pdffonts
```

**Замеры (все артефакты — в `../.cache-tmp/spikes/spike-p0/b/`, вне репозитория):**

| Шаг | Результат |
|---|---|
| B1. WordCraft PDF формулы | **4 970 Б**; `pdfimages -list` — **0 изображений**; `pdffonts` — 1 шрифт `FBIHLE+DejaVuMathTeXGyre-Regular` (CID TrueType, embedded subset) → формула уже **векторная** |
| B2. `pdftocairo -svg` (страница 612×792 pt) | 2 777 Б; `<image>=0`, `<path>=5`, `<text>=0`, `<use>=4`; глифы — путями, шрифты не нужны |
| B2b. Кроп до bbox формулы | bbox из `pdftotext -bbox`: `x=284.6 y=69.2 w=41.8 h=31.3` pt (+2 pt поля); SVG **2 848 Б**, `width=41.83pt height=31.26pt viewBox="284.6 69.2 41.83 31.26"` |
| B4. PNG-fallback (resvg 0.45) | **830 Б** (41.8×31.3 pt при дефолтном масштабе resvg) |
| B4. docx с svgBlip+PNG | **8 970 Б** (контрольный только-PNG — 7 759 Б) |
| B5. LO 26.2.6.3 → PDF, вариант **SVG+fallback** | PDF **14 874 Б**; `pdfimages -list` — **0 строк** (формулы-картинки нет, вектор); `pdffonts` — только `NotoSerif-Regular` (текст абзаца) |
| B5. LO, контрольный **только-PNG** | PDF 15 482 Б; `pdfimages -list` — **2 строки** (`image 56×42 gray jpeg` + `smask`) → растр на месте |
| B6. WordCraft читает docx с svgBlip | открыл, в модели **1 картинка** (PNG-fallback: reader берёт `r:embed`, `asvg:svgBlip` не понимает) |
| B6. WordCraft пересохраняет docx | **svgBlip в `document.xml`: 0**; svg-части в пакете **нет**; PNG-fallback остался (7 859 Б) |

Визуальная проверка вектора: [`assets/p0-svg-formula-libreoffice.png`](assets/p0-svg-formula-libreoffice.png)
— LO-рендер docx, формула `σ = F/A` отрисована векторно (в PDF нет растра, см. таблицу).

**Точный формат вставки (проверенный; числа — из собранного docx):**

```xml
<w:p><w:r><w:drawing><wp:inline distT="0" distB="0" distL="0" distR="0">
  <wp:extent cx="531251" cy="397053"/><wp:docPr id="1001" name="rde-formula-svg"/>
  <a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/picture">
    <pic:pic>
      <pic:nvPicPr><pic:cNvPr id="0" name="rde-formula-svg"/><pic:cNvPicPr/></pic:nvPicPr>
      <pic:blipFill>
        <a:blip r:embed="rIdRdePngFormula">
          <a:extLst>
            <a:ext uri="{96DAC541-7B7A-43D3-8B79-37D633B846F1}">
              <asvg:svgBlip
                xmlns:asvg="http://schemas.microsoft.com/office/drawing/2016/SVG/main"
                r:embed="rIdRdeSvgFormula"/>
            </a:ext>
          </a:extLst>
        </a:blip>
        <a:stretch><a:fillRect/></a:stretch>
      </pic:blipFill>
      <pic:spPr><a:xfrm><a:off x="0" y="0"/>
        <a:ext cx="531251" cy="397053"/></a:xfrm><a:prstGeom prst="rect"/></pic:spPr>
    </pic:pic>
  </a:graphicData></a:graphic>
</wp:inline></w:drawing></w:r></w:p>
```

Обязательные части пакета: `word/media/*.svg` + `word/media/*.png`; relationship типа
`…/relationships/image` на обе части (`ids` из `r:embed`); `[Content_Types].xml` —
`Default Extension="svg" ContentType="image/svg+xml"` (png обычно уже есть); `wp`/`a`/`pic`/`r`
объявлены в корне `word/document.xml` (WordCraft-writer объявляет их всегда), `asvg` — локально на
`svgBlip`. `cx/cy` — EMU от размера SVG в pt (1 pt = 12 700 EMU), поля bbox — 2 pt.

**Выводы и ограничения B:**

* **Вектор достижим без правок vendor**: OMML → WordCraft PDF (уже вектор) → `pdftocairo -svg`
  (путями) → SVG в docx как `asvg:svgBlip` + PNG-fallback. LO 26.2 **предпочитает** SVG и даёт
  вектор (0 растра); WordCraft и Word 2010/2013 показывают PNG-fallback (Word 2016+ — SVG).
* **Кроп SVG — правкой `viewBox`/`width`/`height`, не опциями poppler.** В проверенной версии
  poppler 26.01 `pdftocairo -svg -x/-y/-W/-H` дал лишь частичный сдвиг содержимого при неизменном
  размере страницы (header остался `612pt×792pt`), поэтому bbox берётся из `pdftotext -bbox`
  (поля +2 pt) и подставляется в `viewBox="x y w h"`/`width`/`height` — проверено: SVG 41,83×31,26 pt
  и корректный рендер (LO). Ограничение рецепта: bbox берётся из текстового слоя PDF; если в будущем
  рендер формулы станет «только пути без текста», нужен другой способ (ink-bbox по рендеру страницы
  или геометрия layout-модели WordCraft).
* **WordCraft-writer теряет SVG-расширение при пересохранении** (B6: `svgBlip`=0, svg-части нет,
  PNG-fallback остался). Решение владельца (2026-10-26): **фикс writer'а в форке** — P2.5:
  сохранять `a:extLst`/`asvg:svgBlip`, `word/media/*.svg`, relationship и Content_Types при
  open/save; обязательный регрессионный тест форка «svgBlip → open → save → сохранён»;
  `docs/wordcraft-fork.md`; коммит `rde/main`; отдельный коммит-пин субмодуля. Переприменение SVG
  в RDE отклонено (лечит симптом, а не причину). Это ключевой архитектурный факт для P3/P5.
* Word 2016+ официально поддерживает `asvg:svgBlip` (MS-ODRAWXML), Word 2010/2013 — PNG-fallback;
  **на стенде Word нет**, поэтому совместимость Word проверена только по спецификации, фактически
  проверены LibreOffice 26.2 (вектор) и WordCraft (fallback) — статус «не проверено в Word».
* Пайплайн зависит от **poppler CLI** (`pdftocairo`/`pdftotext`/`pdfimages`). Это продолжение уже
  принятого в v03 отклонения §15-2 (poppler CLI вместо pdfium) — не новая лицензионная проблема
  (внешний GPL-инструмент вызывается процессом, в бандл не линкуется), но для v0.2 при возврате к
  pdfium нужно перепроверить шаг PDF→SVG.

---

## 4. Спайк C — величина со `source` из артефакта

**Метод.** Временный example (тот же harness B; стадия `c`): копия `demo-project/` (11 объектов,
`.rde`-store, `out/`), затем:

1. `rde_dag::resolve_insert_text("rde://artifact/01M5VJ1JG039MMV97SZGY6V9EQ@dd7e…/out/stats.json#/stats/x1/mean")`
   → значение из выхода артефакта «Статистики реализаций»;
2. в `artifact.toml` констант добавляется `[values.x1_mean]` с полями `value/unit/display/style/source`
   (прототип расширения `ConstantValue`) → `load_project` проверяется на совместимость;
3. значение вставляется в `.doc.rde` как `RdeInsert kind="value"` (кэш rich-текста);
4. имитируется пересчёт артефакта (новый `out/stats.json` + новый `out_hash` в манифесте);
5. проверяются «устарело → обновить»: `insert_is_stale`, `update_insert_values`, store-транзакция;
6. проверяются хранение формулы в каталоге документа, каскад удаления, `act_key`-стабильность,
   детект цикла по фрагментам.

**Результаты:**

| # | Проверка | Результат |
|---|---|---|
| C1 | копия демо-проекта | 11 объектов, `issues=0` |
| C2 | разрешение `source` | `-0.0007597812500000007`, digest `dd7e3c10f7b3…` (= `out_hash` артефакта), rich-обозначение `x̄₁ = -0.0008 м` |
| C3 | `ConstantValue` + `source` (прототип) | `load_project`: **issues=0**, `values.x1_mean.display = "x̄₁"` — загрузчик принимает расширение; старое чтение поле `source` игнорирует (JSON Schema не запрещает доп. поля) |
| C4 | вставка значения в документ | `RdeInsert id="v12"`, `hash=dd7e3c10f7b3…`, `cached="x̄₁ = -0.0008 м"` |
| C5 | пересчёт артефакта | out_hash `dd7e3c10f7b3…` → `30fa03509725…`; новое значение `0.123456`; **`insert_is_stale = true`** |
| C6 | «обновить документ» | `update_insert_values` обновил `v12`, `cached="x̄₁ = 0.1235 м"`, старого значения в теле нет, новое есть; зафиксирована store-транзакция `01M5WR0KMSR6TDXA2AJRFW1CW3` |
| C7 | формула как объект (на момент спайка — в каталоге документа) | `objects/documents/<doc>/formulas/<fid>/formula.toml` создан; `load_project` — **issues=0** (лишний каталог внутри объекта игнорируется). Итоговое решение владельца — **project-level** `objects/formulas/<id>/`; проверка совместимости та же: старый загрузчик читает только известные виды `objects/*` и новый каталог игнорирует |
| C8 | прототип `act_key` формулы | стабилен при одинаковых входах (`a7a309ac5c69…` == повтор), **меняется** при изменении digest входа |
| C9 | детект цикла по фрагментам | `A→B→A` → `["A","B","A"]`; без цикла → `None` |
| C10 | удаление/каскад (на момент спайка — document-scope) | `delete_object(документ)` перенёс весь каталог в `.rde/trash/` вместе с формулой. Итоговое решение владельца — project-level: удаление документа формулы **не** затрагивает; удаление формулы — проверка использования в N документах; вставки без цели — статус «цель отсутствует» |

**Выводы C:**

* Механика «величина = расширение константы» **совместима с текущим загрузчиком**: новые поля
  (`symbol/subscript/format/source`) не ломают старый код (schemas без `additionalProperties:false`,
  serde игнорирует неизвестное), `value` остаётся кэшем/литералом.
* Устаревание и обновление значения в тексте **уже поддержаны** существующим механизмом вставок
  (`RdeInsert.hash` + `insert_is_stale` + `update_insert_values` + транзакция store); нужен только
  резолвер значения (source → текст) и включение source-digest в хеш константы/`act_key`.
* Формула как **project-level** объект (`objects/formulas/<id>/`, итоговое решение владельца):
  переиспользование в нескольких документах штатное; номер — документно-локальный (политика в
  объекте, фактический номер в `RdeInsert.number`); удаление — проверка «используется в N
  документах»; старый RDE новый каталог объектов игнорирует (та же проверка совместимости, что C7).
* Для графа: формулы — вычисляемые узлы с теми же примитивами (`in_hash`+`params_hash`+`env_hash`
  → `act_key`), цикл-детект по фрагментам реализуем DFS-раскраской (прототип `fragment_cycle`
  отработал); нужно расширить `Project::dag_edges`/цикл-детект на формулы.

---

## 5. Сводка для следующих фаз

| Фаза | Что подтверждено спайками | Что осталось решить/сделать |
|---|---|---|
| **P1** value | `ConstantValue` расширяемо; source→текст через `resolve_insert_text`; вставки/hash/stale/update работают | схема полей, `source`-резолвер и формат, включение source-digest в хеш, запись кэша `value` в манифест, централизованная замена обозначения |
| **P2** formula | `objects/formulas/<id>/` (project-level) — старый загрузчик каталог игнорирует; `act_key`-прототип; цикл-детект; OMML-извлечение (A16); SVG-пайплайн (B) | `FormulaManifest`+загрузка/валидация/циклы, компиляция OMML-кэша из LaTeX, SVG-кэш, статусы/причины, переиспользование в документах |
| **P2.5** writer fix | gap B6 измерен: WordCraft теряет `svgBlip`/svg-медиа при open→save | фикс `crates/docx` в форке (blip-extLst, svg-медиа, rels, Content_Types на чтении и записи), регрессионный тест форка, docs/wordcraft-fork.md, коммит `rde/main`, пин субмодуля |
| **P3** вставка | `RdeInsert`/`rde:doc` готовы к новому kind; пост-обработка пакета — рабочий механизм (sdt/paraId, как у рисунков) | kind `formula`, `render_mode omml|svg`, rewrite по paraId/tag, конфликт ручной правки (R-DOC-13), миграция демо |
| **P4** редактор — **реализовано (2026-10-29, [ADR-0020](../adr/0020-formula-editor-ui.md))** | `equation_tab::{insert_button,show}` публичны и рисуются в egui-панели; команды Equation доступны через `WordApp::run`; LaTeX-круг без потерь | панель «Формула» (LaTeX+предпросмотр `previews::math_texture`, display/inline, политика нумерации), нативная `equation_tab` на **scratch-`WordApp`** в отдельном окне (документ проекта не меняется), `equation.get` → поля панели; `formula.toml` — source-of-truth, кэши/`[state]` через ledger; вставка с номером по схеме (`numbered` из политики объекта) — см. §8 |
| **P5** вектор/экспорт | векторный PDF WordCraft; SVG-вставка `asvg:svgBlip`+PNG проверена в LO; гейты `pdfimages`/`pdffonts` воспроизводимы | export-mode «формулы как SVG», переприменение SVG после сохранения движком (gap B6), `pdfimages`-гейт «нет растровых формул», Word-проверка (Word на стенде нет), `.tex`-обмен |

---

## 6. Решения владельца (получены 2026-10-26)

| # | Вопрос | Решение |
|---|---|---|
| 1 | scope формул | **Project-level**: `objects/formulas/<ULID>/formula.toml` — как артефакты/константы; переиспользование между документами штатное; номер документно-локальный; удаление — проверка «используется в N документах»; вставки без цели — «цель отсутствует»; в дереве — группа «Формулы» в служебных, скрыта по умолчанию. |
| 2 | формат SVG-вставки | `asvg:svgBlip` + PNG-fallback (подтверждено) + **фикс WordCraft-writer в форке (P2.5)** вместо переприменения SVG в RDE; регрессионный тест форка, `docs/wordcraft-fork.md`, `rde/main`, отдельный пин субмодуля. |
| 3 | место формулы в дереве | Скрытые служебные объекты, группа «Формулы» (реализовывать, не переспрашивать). |
| 4 | `value` при `source` (константы) | Оба поля: optional `source` + обязательный `value`-кэш; схема `rde/artifact@1` без повышения; при `source` источник истины — артефакт. |
| 5 | миграция inline-формулы демо | Не вопрос: выполнить в рамках реализации явной командой/пересборкой демо, без молчаливого изменения при открытии. |

---

## 7. Воспроизведение

Окружение и команды (из каталога `rde/`; target — локальный, `.cargo/config.toml` на стенде
указывает read-only `/srv/yocto/rde-target`, поэтому `CARGO_TARGET_DIR` переопределён):

```bash
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_TARGET_DIR=/home/admin-pc/proj/rde-dev/rde/target
export UV_CACHE_DIR=/home/admin-pc/proj/rde-dev/.cache-tmp/uv-cache

# 1) харнессы спайков лежат как .txt; скопировать во временные examples:
cp docs/spikes/harness/spike_p0_equation.rs.txt crates/rde-ui/examples/spike_p0_equation.rs
cp docs/spikes/harness/spike_p0_vector.rs.txt   crates/rde-doc/examples/spike_p0_vector.rs

cargo run -q -p rde-ui --example spike_p0_equation -- /tmp/spike-a.docx /tmp/spike-a-panel.png
cargo run -q -p rde-doc --example spike_p0_vector -- /tmp/spike-p0 bc

# 2) LibreOffice в песочнице без записи в $HOME (иначе падает DeploymentException):
export HOME=/tmp/lo-home XDG_CACHE_HOME=/tmp/lo-home/.cache
soffice --headless --norestore -env:UserInstallation=file:///tmp/lo-profile \
  --convert-to pdf --outdir /tmp /tmp/spike-p0/b/formula-svg.docx
pdfimages -list /tmp/spike-p0/b/formula-svg.pdf     # ожидается 0 строк растра
pdffonts /tmp/spike-p0/b/formula-svg.pdf            # только шрифт текста абзаца
pdftocairo -svg /tmp/spike-p0/b/formula.pdf /tmp/formula.svg   # <image>=0

# 3) убрать временные examples после проверки
rm crates/rde-ui/examples/spike_p0_equation.rs crates/rde-doc/examples/spike_p0_vector.rs
```

Ограничения воспроизведения: Word на стенде отсутствует (проверка Word-совместимости
`asvg:svgBlip` — по спецификации MS-ODRAWXML, не фактическая); точные размеры/хеши в этой
таблице — снимок 2026-10-26 и могут отличаться при других версиях poppler/LO/WordCraft.

---

## 8. Реализация P4 (2026-10-29, ADR-0020)

Спайк A подтвердился на практике; правок `vendor/wordcraft` не понадобилось (пин прежний).

| Что проверено спайком | Как реализовано |
|---|---|
| A2/A3: `insert.equation {latex, display}` + `equation.inputFormat latex` на `Session::new(Document::default())` | «Профессиональный редактор формулы»: отдельный scratch-`WordApp` панели; текущий LaTeX вставляется в мини-документ, пользователь вводит структуры/символы/текст нативно |
| A6: `equation.get` → `latex`/`display` | кнопка «← Забрать LaTeX из редактора» переносит правку в поля панели; в `formula.toml` она попадает только по «Применить» |
| шрифты `medium`/`semibold` обязательны до первого кадра вкладки | `wordcraft_ui_egui::theme::install_fonts(ctx)` перед первым кадром + первый кадр без `equation_tab::show` (как в харнессе спайка) |
| предпросмотр — `previews::math_texture` | предпросмотр панели рендерится тем же путём (MathAtlas-кэш по latex+display), пустой LaTeX — подсказка |
| canvas WordCraft | рисуется в отдельном окне; пока он открыт, живой canvas документа заменяется растровым предпросмотром страниц — два canvas в одном кадре конфликтуют по фиксированному egui-id `wordcraft_canvas`/слоям (обнаружено при встраивании, задокументировано в ADR-0020 §3) |
| ограничение «каретка к формуле объекта» | не хачили: правка «в тексте документа поверх equation_tab» — следующий шаг; поток P4 — источник в панели + вставка/обновление |

Доказательства: [`docs/fidelity/p20-formula-editor.md`](../fidelity/p20-formula-editor.md),
скриншоты `docs/fidelity/gui-p20-formula-*.png`, тесты `rde-ui` `formula_editor_*` (4) и
`rde-core` `formula_write_tests` (4); LO-оракул номера `(2)` — `xMax=552.9 pt` при правом поле
555.3 pt.
