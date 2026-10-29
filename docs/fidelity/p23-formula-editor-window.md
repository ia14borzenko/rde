# P6 — отчёт: визуальный LaTeX-редактор формул в отдельном окне (MathLive) + SVG-вставки

| Поле | Значение |
|---|---|
| Итерация | `latex-editor-window` (P6), 2026-10-31 |
| Вход | HEAD `e71c5e4` (тег `import-number-restore`), workspace **629 passed / 0 failed**, worktree чистый |
| Выход | workspace-тесты — см. §3; release-бинарь `/srv/yocto/rde-target/release/rde` |
| Решения | [ADR-0022](../adr/0022-latex-editor-window.md) (принят); [ADR-0021](../adr/0021-latex-math-editor.md) — superseded в части редактора (P5); ADR-0005 §5.1 (запрет webview) — снят решением владельца |
| Спайк | [`docs/spikes/latex-editor-window.md`](../spikes/latex-editor-window.md) (Research Gate: ограничение стенда, матрица, POC, лицензии, LO-оракул baseline) |
| Продукт | новый crate `crates/rde-formula-editor`; интеграция в `rde-ui` (панель «Формула»); `rde-doc` (метрики вёрстки + подъём номера SVG-вставки); P5-crate `rde-math-editor` **удалён**; `vendor/wordcraft` **не менялся** |
| Фото-доказательства | [`gui-p23-editor-window-dark.png`](gui-p23-editor-window-dark.png), [`…-light.png`](gui-p23-editor-window-light.png), [`…-dark-matrix.png`](gui-p23-editor-window-dark-matrix.png), [`gui-p23-formula-panel.png`](gui-p23-formula-panel.png) (+light), [`gui-p23-formula-svg-document.png`](gui-p23-formula-svg-document.png) |

## 1. Что сделано

### 1.1 Окно визуального редактора (MathType-класс) — `crates/rde-formula-editor`

Владелец заменил собственный WYSIWYG-редактор P5 на «настоящий открытый визуальный LaTeX-редактор
уровня MathType» и разрешил webview. На стенде встроенный `wry`/WebKitGTK невозможен (нет
`webkit2gtk-4.1.pc`/GTK-dev, `sudo` запрещён — спайк §1), поэтому реализован эквивалентный путь
владельца «отдельное окно на рабочем столе, как MathType»:

* **MathLive 0.111.1 (MIT) офлайн** — JS/CSS/20 шрифтов KaTeX (MIT) + наша страница редактора
  (`editor.html/js/css`) встроены в бинарник `include_bytes!` (**1 143 422 B ≈ 1,09 МиБ**),
  отдаются локальным HTTP-мостом; никакого CDN/сети в рантайме.
* **Окно браузера в режиме приложения**: RDE находит Chrome/Chromium/Brave/Edge (Linux/Windows/
  macOS списки; `RDE_FORMULA_EDITOR_BROWSER` — переопределение), запускает
  `--app=http://127.0.0.1:<порт>/?t=<токен>` с изолированным профилем и флагами
  `--disable-background-networking/…`; fallback — браузер по умолчанию (`xdg-open`/`open`/`start`);
  нет браузера — понятная ошибка в панели (LaTeX-поле остаётся рабочим).
* **Мост JS↔Rust на `std`** (без async/новых зависимостей): `GET /api/state` (LaTeX/display/title/
  theme/zoom/project), `POST /api/apply {latex}`, `/api/cancel`, `/api/prefs {zoom}`; события через
  `mpsc`, опрос в каждом кадре RDE. Безопасность: loopback-only bind, одноразовый токен 16 байт
  CSPRNG (constant-time), проверка `Host` (anti-DNS-rebinding), лимиты 512 КиБ/32 КиБ, таймауты,
  `nosniff`/`no-store`, CSP страницы `default-src 'self'; connect-src 'self'; …`.
* **UX MathLive**: визуальные структуры/палитры (встроенная виртуальная клавиатура: числа/символы/
  латиница/греческий), undo/redo, LaTeX-копипаст, `Tab` между слотами, авто-дроби (`/`), степени,
  индексы, `\alpha`-автозамены; наша шапка — заголовок формулы, display/inline, зум 100–200 %
  (сохраняется в `~/.config/rde/formula-editor.toml`), LaTeX-панель источника, «Применить»/«Отмена»,
  горячие клавиши (Ctrl+Enter / Esc / Ctrl+K / Ctrl+Shift+L / Ctrl+±). Тема — dark/light синхронно с
  RDE; тёмная палитра виртуальной клавиатуры на CSS-переменных MathLive.
* **Жизненный цикл**: «Применить» → LaTeX в RDE → окно гасится (kill process group chrome);
  «Отмена»/закрытие крестиком/падение браузера → без изменений (событие `Cancelled`); тема RDE
  транслируется в открытое окно; после закрытия ресурсы освобождаются, профиль переиспользуется.

### 1.2 Панель «Формула» — интеграция и удаление P5

* Кнопка **«Открыть редактор формул…»** (при открытом окне — «Закрыть окно»), LaTeX-поле —
  источник истины, **предпросмотр формулы из кэшей** (PNG из CAS → ink-mask с тинтом темы), статус
  актуальности/вставок, display/inline, нумерация, режим вставки, «Применить/Вставить/Обновить
  документ» — прежний конвейер без изменения семантики (ledger/CAS/`body_hash`/Fresh-Stale,
  right-numbered, `number_source`/`suffix`/crossrefs, импорт MathType).
* «Применить» в окне — **явное действие**: LaTeX проходит `rde_doc::check_formula_latex` (запрет
  Unicode-мусора/плейсхолдеров линейного формата — safety P5) → `apply_formula_edit`/
  `create_formula_action` (нормализация, `compile_formula` → OMML/SVG/PNG, ledger) → окно закрыто.
  Никакого автосинка из окна нет.
* **P5-виджет удалён из UI**; crate `rde-math-editor` (модель/кодек/виджет, 23 теста) **удалён из
  workspace и репозитория** (история — git). Демо-хелперы/скриншот-действия обновлены
  (`formula-editor-v2`, `formula-editor-fraction/matrix/palette`, новый `formula-insert-svg`).

### 1.3 SVG-вставки: основной режим display + метрики/baseline

* Режим вставки по умолчанию в панели — **`svg`** (OMML остаётся опцией; существующие OMML-проекты
  не ломаются, переключение режима не теряет LaTeX).
* SVG для документа рендерит **сам RDE** прежним конвейером (`compile_formula`: WordCraft mini-doc →
  векторный PDF → `pdftocairo -svg` → кроп по ink-bbox, глифы путями, PNG-fallback; visual parity со
  STIX-документом), т.е. требование «рендер SVG вызывается внутри RDE» выполнено.
* Добавлены **метрики вёрстки**: `rde_doc::formula_metrics(latex, display) → FormulaMetrics
  { width_pt, ascent_pt, descent_pt }` (из `MathLayout` WordCraft). Для display+SVG номер `(N)`
  поднимается `<w:position>` на `descent + SVG_CROP_PAD_PT`, чтобы его базовая линия совпала с
  базовой линией формулы (низ `w:drawing` в WordprocessingML = базовая линия строки; LO-оракул —
  спайк §4). Для OMML-вставок подъём не нужен (0).
* Inline-формулы по умолчанию — **OMML** (нативный baseline); SVG inline возможен, но имеет
  ограничение OOXML (низ картинки = базовая линия → нижние индексы приподняты) — явная подсказка в
  панели. Кэш-схема/`FORMULA_RENDERER_VERSION` не менялись (байты кэшей те же; метрики считаются из
  LaTeX при вставке) — старые проекты не инвалидируются.

## 2. Доказательства (скриншоты)

| Скриншот | Что показывает |
|---|---|
| [`gui-p23-editor-window-dark.png`](gui-p23-editor-window-dark.png) | **Окно редактора** (тёмная тема RDE): заголовок формулы, display/проект, зум 110 %, визуальная формула σ=F/A (не код), MathType-класс: **виртуальная клавиатура** (цифры/символы/латиница/греческий), LaTeX-панель источника, «Применить»/«Отмена» |
| [`gui-p23-editor-window-light.png`](gui-p23-editor-window-light.png) | То же в светлой теме RDE (тема передаётся мостом) |
| [`gui-p23-editor-window-dark-matrix.png`](gui-p23-editor-window-dark-matrix.png) | Матрица 2×2, загруженная из LaTeX в визуальный редактор |
| [`gui-p23-formula-panel.png`](gui-p23-formula-panel.png) / [`…-light.png`](gui-p23-formula-panel-light.png) | Панель «Формула» после интеграции: кнопка **«Открыть редактор формул…»**, предпросмотр формулы из кэшей, LaTeX-поле, display/inline, нумерация, **вставка SVG-вектор** по умолчанию |
| [`gui-p23-formula-svg-document.png`](gui-p23-formula-svg-document.png) | Документ с **SVG-вставкой display-формулы**: формула по центру, **номер `(1)` справа на её базовой линии** (right-numbered табами, без таблицы) — WordCraft-рендер PNG-fallback из SVG-кэша |

Скриншоты окна сняты **headless** (Chrome `--headless=new` + CDP, `scripts/verify-formula-editor.sh`);
RDE-панель/документ — egui_kittest (`examples/screenshot`).

## 3. Тесты

* Вход: **629 passed / 0 failed**.
* Выход: полный `./scripts/build.sh test --workspace` — **632 passed / 0 failed** (2 ignored —
  ручные перф-замеры); новые тесты:
  * `rde-formula-editor` — **24** (17 unit: protocol/assets/host/server/описание окна; 7 integration:
    реальный loopback-мост — страница/CSP/ассеты, токен/Host, apply/cancel-события, зум, состояние
    из RDE, лимиты, остановка моста; + lifecycle окна с фейковым браузером);
  * `rde-ui` — **+3** (`formula_panel_offers_visual_editor_window_and_source_field`,
    `formula_editor_window_apply_writes_source` — сквозь мост: POST `/api/apply` → запись
    `formula.toml`, `formula_editor_window_reports_missing_browser`); P5-тесты (3) удалены вместе с
    виджетом;
  * `rde-doc` — **+3** (`formula_metrics_match_layout`, `svg_numbered_insert_raises_number_to_math_baseline`,
    `perf_compile_formula_and_metrics` — ignored); P5-crate `rde-math-editor` (23 теста) удалён.
* Продуктовые регрессы: formula/import/numbering/migration/ledger тесты зелёные; OMML-режим,
  импорт MathType, right-numbered/`number_source`/`suffix`/crossrefs — без регрессов.

## 4. Замеры (этот стенд; release, если не указано)

| Метрика | Значение | Как измерено |
|---|---|---|
| Встроенные ассеты окна | **1 143 422 B ≈ 1,09 МиБ** (MathLive 843 924 + CSS 16 217 + 20×woff2 259 792 + страница 23 489) | `assets_total_bytes()`/инвентаризация тестом |
| Release-бинарь `rde` | **130 680 440 B** (+923 504 B ≈ +0,88 МиБ к P5-бинарю 129 756 936 B) | `stat` до/после сборки |
| Готовность окна (страница MathLive + мост) | **~0,93–1,02 с холодно**, **~0,77–0,80 с тепло** | `performance.now()` в странице (`scripts/formula-editor-cdp.js`) |
| Применение LaTeX (страница → мост) | **~2 мс** (POST) + ≤40 мс опрос моста в RDE | `performance.now()` вокруг fetch; опрос 40 мс (example) |
| Рендер SVG-формулы (`compile_formula`: OMML+SVG+PNG, poppler) | **см. [`p23-perf.md`](p23-perf.md)** | `cargo test -p rde-doc --release --lib formula::tests::perf -- --ignored` |
| Метрики вёрстки (`formula_metrics`) | **см. [`p23-perf.md`](p23-perf.md)** | там же |
| CSP/JS-ошибки страницы | **0 / 0** | CDP `Log`/`Runtime` |
| Офлайн | все запросы — loopback; внешних сетевых загрузок нет | grep бандла + CDP Network (спайк §3) |

## 5. LO-оракул baseline (подробно — спайк §4)

* Синтетический `.docx` + LO 26.2.6.3 → PDF (300 dpi): низ inline-картинки = базовая линия текста
  (76,60 vs 76,32 pt, расхождение 0,28 pt — растеризация).
* `<w:position w:val="-10"/>` на run с картинкой **не сдвигает** картинку в LO → inline SVG-baseline
  исправить нельзя (ограничение OOXML); `w:position` на **текст** работает → номер `(N)` поднимается.
* Рендер документа подтверждает: `gui-p23-formula-svg-document.png` — номер `(1)` на одной базовой
  линии с формулой. Word на стенде отсутствует — Word-рендер `unverified` (LO-оракул).

## 6. Ограничения (честно)

1. **Word не проверен** — на стенде нет Word; SVG/`svgBlip`/номер проверены LO-оракулом и
   WordCraft-рендером (`unverified` для Word).
2. **Windows/Wayland** — списки браузеров и пути запуска заложены, ручной проверки нет (на стенде
   нет графической сессии; headless-эквивалент проверен на Linux/Chrome).
3. **Inline SVG** — фундаментальное ограничение WordprocessingML (низ рисунка = базовая линия):
   inline по умолчанию OMML; SVG inline даёт «приподнятые» нижние индексы (подсказка в панели).
4. **Стиль окна ≠ документ**: MathLive рисует KaTeX-типографикой; документ — STIX/WordCraft
   (официальной подмены math-шрифта MathLive нет). Документ остаётся визуально паритетным прежнему
   конвейеру.
5. **Зависимость от браузера**: без Chrome/Chromium/Edge окно не откроется (fallback — браузер по
   умолчанию); панель при этом полностью работоспособна.
6. **Кастомизация MathType-палитр под ГОСТ, матричный тулбар, MathML-копипаст** — не в этой
   итерации (MathLive-база готова, P2-бэклог).

## 7. Как проверить руками (на рабочем столе)

1. Запустить RDE, открыть демо-проект → вкладка «Формула» → выбрать формулу → **«Открыть редактор
   формул…»** → откроется отдельное окно (Chrome app-mode) с визуальной формулой.
2. В окне: кликнуть по дроби/скобкам, ввести `x^2`, `\alpha`, открыть ⌨ (виртуальная клавиатура),
   `Tab` — следующий слот, `Ctrl+Z` — undo; **«Применить»** → окно закроется, в панели LaTeX
   обновится, статус станет «актуальна» после «Обновить документ».
3. «Отмена»/крестик окна → источник не меняется (сравнить LaTeX в панели).
4. В панели выбрать «Вставить в документ» (режим SVG) → «Обновить документ» → в документе формула
   по центру с номером `(2)` справа; номер на базовой линии формулы.
5. Переключить тему RDE (Настройки → тема) при открытом окне — окно сменит тему.
6. Проверить OMML-режим: вставка OMML по-прежнему работает (переключить «Вставка: OMML»).
7. Ошибка: ввести заведомо невалидный LaTeX (`\frac{`) → «Применить» → ошибка в панели, источник
   не испорчен.

## 8. Риски

* Внешний браузер как зависимость UX (митигация: fallback + понятная ошибка; окно можно заменить
  на встроенный webview без изменения моста/страницы — границы крейта это позволяют).
* Word-непроверенность SVG (митигация: прежний проверенный `svgBlip`-путь P0/P3 + LO-оракул).
* MathLive-обновления (пин 0.111.1, ассеты/лицензии фиксируются тестом; обновление — отдельной
  задачей с ревизией лицензий).

## 9. Дальше

Следующая итерация — **PERF-1**: оптимизация RDE по [`docs/perf-backlog.md`](../perf-backlog.md)
(владелец просил напомнить после формул). Отложенное по формулам: P2-кастомизация палитр/матричный
тулбар, Word-проверка, Windows/Wayland-ручная проверка.
