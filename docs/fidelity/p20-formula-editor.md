# P4 — редактор формул в GUI: доказательства (итерация `formula-editor`, ADR-0020)

| Поле | Значение |
|---|---|
| Дата | 2026-10-29 |
| База | HEAD `61255fe` (тег `import-numbered-formulas`), дерево чистое |
| Канон | [ADR-0020](../adr/0020-formula-editor-ui.md), ADR-0005 P4, ADR-0018/0019, R-DOC-21 фаза 2 |
| Vendor | `vendor/wordcraft` не менялся (пин `33213040`) |
| Тесты | workspace **590 passed / 0 failed** (было 582; +8); новые `formula_editor_*` (4) + `rde-core` `formula_write_tests` (4) |
| Скриншоты | `gui-p20-formula-create.png`, `gui-p20-formula-editor.png`, `gui-p20-formula-native.png`, `gui-p20-formula-inspector.png`, `gui-p20-formula-insert-numbered.png` |

> **Уточнение (2026-10-29, итерация `formula-tab`):** по итогам приёмки панель редактора переехала
> с плавающего окна на **вкладку ленты «Формула»**, клик по формуле в тексте открывает её
> автоматически, и починен скролл предпросмотра страниц. Скриншоты этой страницы показывают первую
> (оконную) версию панели; актуальные — в отчёте
> [`p21-formula-tab.md`](p21-formula-tab.md) (`docs/fidelity/gui-p21-*.png`).


> **Исторический документ.** Отчёт итерации P4; нативный «проф. редактор» (equation_tab-окно),
> предпросмотр scratch-`WordApp` и скриншоты `gui-p2x-*native*` **выведены** в P5 «math-editor»
> (2026-10-29, [ADR-0021](../adr/0021-latex-math-editor.md)): редактор заменён собственным
> WYSIWYG-редактором MathType-класса, LaTeX-источник валидируется. Актуальные скриншоты и
> отчёт — [`p22-formula-editor.md`](p22-formula-editor.md), [`gui-p22-*`](gui-p22-formula-editor.png).

## 1. Что проверялось

Acceptance (ADR-0020/промпт итерации):

1. **Создание** формулы из GUI: диалог с LaTeX + предпросмотр + политика нумерации; валидация
   компиляцией; объект, кэши OMML/SVG/PNG в CAS, `[state]` в манифесте, статус Fresh; невалидный
   LaTeX — ошибка и ничего не записано; дедуп по `latex.trim()`.
2. **Правка** source-of-truth: title/latex/display/нумерация без потери `inputs`/`fragments`/`[state]`;
   кэши/`act_key` пересчитаны; вставки Stale; «Обновить документ» пересобирает тела, сохраняя
   номер/пунктуацию/`number_source` (ADR-0018/0019), `body_hash`-конфликт не перезаписывается.
3. **Вставка** из панели/дерева/инспектора: display+auto → right-numbered вёрстка табами (без
   `<w:tbl>`), номер по схеме; display+none/inline → без номера.
4. **Нативная вкладка Equation** WordCraft (спайк A) встроена без правок vendor; документ проекта не
   меняется; fallback не понадобился.
5. Ничего не сломано: 582 → **590 тестов** (0 failed), существующие зелёные; MathType-импорт/демо/фиделити не тронуты.

## 2. Скриншоты (headless `egui_kittest`, wgpu offscreen)

| Скриншот | Что видно |
|---|---|
| [`gui-p20-formula-create.png`](gui-p20-formula-create.png) | диалог «Новая формула»: заголовок, LaTeX-источник, display/inline, политика нумерации, **предпросмотр** формулы σ (рендер вкладки Equation, шрифт STIX), режим вставки OMML/SVG, позиция и **превью номера `(3)`** |
| [`gui-p20-formula-editor.png`](gui-p20-formula-editor.png) | панель «Формула» (тёмная тема): статус «актуальна · вставок 2 · устаревших 0», `id`, кнопка «Открыть документ на вставке», LaTeX, предпросмотр, кнопки «Применить/Вставить в документ/Обновить документ/Закрыть» |
| [`gui-p20-formula-native.png`](gui-p20-formula-native.png) | «Профессиональный редактор формулы»: нативная вкладка Equation WordCraft (Tools/Symbols/Structures/Equation Options) + canvas scratch-документа с формулой σ; документ проекта не меняется |
| [`gui-p20-formula-inspector.png`](gui-p20-formula-inspector.png) | инспектор формулы: тип/режим/нумерация/latex, кнопки **«Редактировать»** и **«Вставить»**, статус, act_key, «где вставлено» |
| [`gui-p20-formula-insert-numbered.png`](gui-p20-formula-insert-numbered.png) | вставка формулы σ из GUI в демо-документ: в конце страницы **right-numbered** `σ = √(1/(N−1) Σ(xᵢ−x̄)²)  (2)` (номер справа, без таблицы); журнал «Вставка v14 (formula, omml) добавлена в документ» |

Команды съёмки (пути — как в отчёте итерации):

```bash
cd /home/admin-pc/proj/rde-dev/rde
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=45 \
  /srv/yocto/rde-target/debug/examples/screenshot docs/fidelity/gui-p20-formula-create.png \
  document "" 0 formula-create-dialog light
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=45 \
  /srv/yocto/rde-target/debug/examples/screenshot docs/fidelity/gui-p20-formula-editor.png \
  document "" 0 formula-editor dark
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=80 \
  /srv/yocto/rde-target/debug/examples/screenshot docs/fidelity/gui-p20-formula-native.png \
  document "" 0 formula-editor-native dark tall
RDE_PROJECT=demo-project RDE_SCREENSHOT_STEPS=30 \
  /srv/yocto/rde-target/debug/examples/screenshot docs/fidelity/gui-p20-formula-inspector.png \
  document "" 0 formula-inspector light
# вставка с номером — на копии демо (исходный demo-project не изменяется)
cp -a demo-project/. /tmp/p4-insert/ && rm -rf /tmp/p4-insert/.rde /tmp/p4-insert/exports
RDE_PROJECT=/tmp/p4-insert RDE_SCREENSHOT_STEPS=160 \
  /srv/yocto/rde-target/debug/examples/screenshot docs/fidelity/gui-p20-formula-insert-numbered.png \
  document "" 0 formula-insert light "" preview 1100
```

## 3. Проверки тестами

`cargo test -p rde-ui formula_editor` — 4 теста (poppler-зависимые честно пропускаются, если
`pdftocairo`/`pdftotext` нет):

| Тест | Что подтверждает |
|---|---|
| `formula_editor_creates_formula_with_caches_and_validates_latex` | невалидный LaTeX (`{{{{`) и пустые поля → ошибка, объектов нет; валидная → объект + `[state]` (`act_key` 64 hex, OMML/SVG хэши) + кэши в CAS + статус Fresh; дедуп по LaTeX не создаёт дубль; политика `none`/inline сохраняется; панель создания рендерится в headless-харнессе без паники |
| `formula_editor_edit_stales_inserts_and_document_update_rebuilds` | правка LaTeX/заголовка → `apply_formula_edit` пишет манифест, пересчитывает кэши; формула Fresh, **вставки Stale**; «Обновить документ» → `number`/`number_text`/`suffix`/`number_source` сохранены, `hash` (act_key) вставки обновлён, `<w:tbl>` не добавлен, `<w:tabs>`/`m:oMath`/`rde:formula:` на месте |
| `formula_editor_inserts_numbered_formula_and_respects_numbering_policy` | вставка из панели: +1 formula-вставка с `target=rde://formula/<id>@…`, номер назначен, OMML-режим, right-numbered табами, без новой таблицы; смена схемы на «по разделам» перенумеровывает; формула с политикой `none` вставляется **без номера** |
| `formula_editor_native_equation_tab_renders_and_does_not_touch_project_document` | нативная вкладка Equation на scratch-`WordApp` рендерится в headless-харнессе (шрифты WordCraft, первый кадр без `show`), `equation.get` отдаёт LaTeX, байты `.doc.rde` до/после идентичны |

`cargo test -p rde-core` — `ops::formula_write_tests` (4): политика нумерации при создании; правка
сохраняет `inputs`/`fragments`/`[state]` и грузится загрузчиком; невалидные поля не перезаписывают
файл; запись с чужой `schema` отклоняется, tmp-файл убирается, прежний манифест цел.

## 4. LO-оракул вставки с номером

```bash
cargo run -p rde-doc --example migrate-numbered -- /tmp/p4-insert --export-only --export /tmp/p4-check.docx
HOME=/tmp/lo-home XDG_CACHE_HOME=/tmp/lo-home/.cache soffice --headless --norestore \
  -env:UserInstallation=file:///tmp/lo-profile-p4 --convert-to pdf --outdir /tmp/p4-out /tmp/p4-check.docx
pdftotext -bbox /tmp/p4-out/p4-check.pdf - | grep -E "\((1|2)\)"
```

Фактический результат (номера — у правого поля текста, как в ADR-0018):

```
<word xMin="537.900000" ... xMax="552.900000" ...>(1)</word>   # существующая вставка демо
<word xMin="537.900000" ... xMax="552.900000" ...>(2)</word>   # новая вставка из GUI
```

Правое поле текста — 555.3 pt (A4, поля 30/15/20/20 мм); номер `(2)` — `xMax = 552.9 pt`.
Word на стенде отсутствует → Word-часть `unverified` (LO + ECMA-376).

## 5. Ограничения (честно)

- **Правка «в тексте документа»** (каретка к формуле вставки + `equation.edit`) не сделана: API
  WordCraft не умеет ставить каретку к формуле объекта; безопасный поток — плагин источника
  `formula.toml` + вставка/обновление. Следующий шаг.
- **Нативный редактор — на scratch-документе**, не связан кареткой с документом; документ проекта в
  это время показывается растровым предпросмотром (два canvas WordCraft в одном кадре конфликтуют по
  egui-id/слоям; эксперименты — в ADR-0020 §3).
- `inputs`/`fragments` в GUI не редактируются (сохраняются при правке источника; задаются файлом/
  агентом); ручная нумерация `numbering.label` (R-DOC-15) в UI не выведена.
- Удаление используемой формулы — generic `delete_object` (корзина) без проверки «используется в N
  документах»; вставки без цели показываются статусом «цель отсутствует».
- Word `unverified` (нет на стенде): `m:oMathPara`+табы и `m:mathPr/mathFont` — по спецификации и LO.
