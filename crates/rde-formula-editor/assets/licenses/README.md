# Лицензии и происхождение бандл-ассетов `rde-formula-editor`

Каталог `assets/` — офлайн-ассеты редактора формул RDE. Всё скачано через npm **во время подготовки** и после этого
не редактировалось (минифицированный JS побайтово совпадает с пакетом). Во время работы продукта сеть не нужна.

## Итог по лицензиям

| Компонент | Происхождение | Лицензия (SPDX) |
|---|---|---|
| MathLive (JS + CSS) | npm-пакет `mathlive@0.111.1` | **MIT** — Arno Gourdol |
| Шрифты `KaTeX_*.woff2` (20 файлов) | из того же пакета `mathlive@0.111.1/package/fonts/`; побайтово идентичны `katex@0.19.1/package/dist/fonts/` | **MIT** — Khan Academy / авторы KaTeX |
| Тексты лицензий | `mathlive@0.111.1/package/LICENSE.txt`, `katex@0.19.1/package/LICENSE` | MIT (тексты, не код) |

SPDX-идентификатор обоих компонентов: `MIT`. Копирайты: `Copyright (c) 2017 - present Arno Gourdol` (MathLive),
`Copyright (c) 2013-2020 Khan Academy and other contributors` (KaTeX, включая шрифты).
Тексты лицензий лежат рядом, в `licenses/LICENSE-mathlive.txt` и `licenses/LICENSE-katex-fonts.txt` — не изменялись.

**Версия JS закреплена:** `mathlive@0.111.1`, `mathlive.min.js` =
`51e9ab5c4739209a2951a550042dcecb99ad3047269c1d791d09d7d613226eae` (843 924 B).
Версия видна и в шапке бандла: `/** MathLive 0.111.1 */`.

## Содержимое каталога

Суммарно **25 payload-файлов из npm, 1 122 139 B** (без учёта этого `README.md` и без содержимого `editor/`,
которое добавляет владелец продукта; вместе с этим README — ≈1.13 МБ, на диске ~1.2 MiB из-за блоков по 4 KiB).

| Путь | Что это | Источник | SPDX | Байт |
|---|---|---|---:|---:|
| `editor/` | каталог страницы редактора — наполняется владельцем продукта (`editor.html/js/css`), не из npm; в суммы ниже не входит | — | — | — |
| `mathlive/` | каталог | npm `mathlive@0.111.1` | MIT | 860 141 |
| `mathlive/mathlive.min.js` | UMD-бандл, глобал `MathLive` (`package.json` → `main`) | `package/mathlive.min.js` | MIT | 843 924 |
| `mathlive/mathlive-static.css` | `@font-face` для всех 20 гарнитур + стили вёрстки (`--ML__static-fonts:true`) | `package/mathlive-static.css` | MIT | 13 205 |
| `mathlive/mathlive-fonts.css` | только `@font-face` (дублирует начало `mathlive-static.css`) | `package/mathlive-fonts.css` | MIT | 3 012 |
| `mathlive/fonts/` | каталог, 20 × `KaTeX_*.woff2` | `package/fonts/` | MIT (KaTeX) | 259 792 |
| `mathlive/fonts/KaTeX_AMS-Regular.woff2` | шрифт AMS (`\mathbb`, `\mathfrak`-набор AMS) | npm `mathlive@0.111.1` | MIT (KaTeX) | 28 076 |
| `mathlive/fonts/KaTeX_Caligraphic-Bold.woff2` | каллиграфика, bold | npm `mathlive@0.111.1` | MIT (KaTeX) | 6 912 |
| `mathlive/fonts/KaTeX_Caligraphic-Regular.woff2` | каллиграфика, regular | npm `mathlive@0.111.1` | MIT (KaTeX) | 6 908 |
| `mathlive/fonts/KaTeX_Fraktur-Bold.woff2` | фрактура, bold | npm `mathlive@0.111.1` | MIT (KaTeX) | 11 348 |
| `mathlive/fonts/KaTeX_Fraktur-Regular.woff2` | фрактура, regular | npm `mathlive@0.111.1` | MIT (KaTeX) | 11 316 |
| `mathlive/fonts/KaTeX_Main-Bold.woff2` | основной текст, bold | npm `mathlive@0.111.1` | MIT (KaTeX) | 25 324 |
| `mathlive/fonts/KaTeX_Main-BoldItalic.woff2` | основной текст, bold italic | npm `mathlive@0.111.1` | MIT (KaTeX) | 16 780 |
| `mathlive/fonts/KaTeX_Main-Italic.woff2` | основной текст, italic | npm `mathlive@0.111.1` | MIT (KaTeX) | 16 988 |
| `mathlive/fonts/KaTeX_Main-Regular.woff2` | основной текст, regular | npm `mathlive@0.111.1` | MIT (KaTeX) | 26 272 |
| `mathlive/fonts/KaTeX_Math-BoldItalic.woff2` | математический курсив, bold italic | npm `mathlive@0.111.1` | MIT (KaTeX) | 16 400 |
| `mathlive/fonts/KaTeX_Math-Italic.woff2` | математический курсив, italic | npm `mathlive@0.111.1` | MIT (KaTeX) | 16 440 |
| `mathlive/fonts/KaTeX_SansSerif-Bold.woff2` | sans-serif, bold | npm `mathlive@0.111.1` | MIT (KaTeX) | 12 216 |
| `mathlive/fonts/KaTeX_SansSerif-Italic.woff2` | sans-serif, italic | npm `mathlive@0.111.1` | MIT (KaTeX) | 12 028 |
| `mathlive/fonts/KaTeX_SansSerif-Regular.woff2` | sans-serif, regular | npm `mathlive@0.111.1` | MIT (KaTeX) | 10 344 |
| `mathlive/fonts/KaTeX_Script-Regular.woff2` | script/рукописный | npm `mathlive@0.111.1` | MIT (KaTeX) | 9 644 |
| `mathlive/fonts/KaTeX_Size1-Regular.woff2` | крупные скобки/операторы, size1 | npm `mathlive@0.111.1` | MIT (KaTeX) | 5 468 |
| `mathlive/fonts/KaTeX_Size2-Regular.woff2` | крупные скобки/операторы, size2 | npm `mathlive@0.111.1` | MIT (KaTeX) | 5 208 |
| `mathlive/fonts/KaTeX_Size3-Regular.woff2` | крупные скобки/операторы, size3 | npm `mathlive@0.111.1` | MIT (KaTeX) | 3 624 |
| `mathlive/fonts/KaTeX_Size4-Regular.woff2` | крупные скобки/операторы, size4 | npm `mathlive@0.111.1` | MIT (KaTeX) | 4 928 |
| `mathlive/fonts/KaTeX_Typewriter-Regular.woff2` | `\mathtt`, моноширинный | npm `mathlive@0.111.1` | MIT (KaTeX) | 13 568 |
| `licenses/` | каталог | — | — | 2 206 |
| `licenses/LICENSE-mathlive.txt` | MIT-текст MathLive (Arno Gourdol) | `mathlive@0.111.1/package/LICENSE.txt`, без изменений | MIT | 1 099 |
| `licenses/LICENSE-katex-fonts.txt` | MIT-текст KaTeX (Khan Academy и др.) — лицензия на шрифты | `katex@0.19.1/package/LICENSE`, без изменений | MIT | 1 107 |
| `licenses/README.md` | этот файл (≈8 КБ; в сумму 1 122 139 B не входит) | — | — | ≈8 KB |

## Проверки, выполненные при сборке ассетов

- Все 23 payload-файла (JS, 2 × CSS, 20 шрифтов) сверены `sha256sum` с содержимым npm-архива — расхождений нет.
- 20 из 20 `KaTeX_*.woff2` побайтово совпадают с `katex@0.19.1/package/dist/fonts/*.woff2` → MIT KaTeX применим напрямую.
- Все `url(fonts/KaTeX_*.woff2)` из CSS указывают на существующие файлы: 20 OK, 0 отсутствующих.
- В CSS/JS нет абсолютных путей, `file://` и `../` — только относительные ссылки, пригодные для встраивания в бинарь.
- Офлайн-рендер проверен в headless Chrome при заблокированном DNS: формула `\sigma=\frac{F}{A}` отрисовывается,
  скриншот побайтово совпадает с прогоном без блокировки; попыток сетевых запросов (`fetch`/`XHR`/`Image`) — 0.
- Внешних URL, требующих сети в рантайме, нет. В бандле есть только: XML-неймспейсы `www.w3.org` (не загружаются),
  две строки `console.error` со ссылками на npm и `esm.run` (только текст сообщения) и относительные `url(fonts/…)`.
- Посторонних файлов нет: не приложены `sounds/`, `*.map`, `types/`, `mathlive.js`/`mathlive.mjs`, `mathlive-ssr.min.mjs`.

Примечание: `mathlive-static.css` содержит `mathlive-fonts.css` целиком в качестве префикса, поэтому подключать
оба файла не обязательно — достаточно `mathlive-static.css` (при подключении обоих `document.fonts.size` = 40 вместо 20,
это безвредно). Оба файла оставлены, т.к. оба указаны в требуемой раскладке.

Важно для интеграции: `mathlive-static.css` заканчивается правилом `:root{--ML__static-fonts:true}`. Именно это правило
отключает в JS динамическую загрузку шрифтов (ветку с `new FontFace` и разрешением URL через `fontsDirectory`).
Если подключать только `mathlive.min.js` без этого CSS, MathLive пойдёт по динамическому пути — там есть
`fetch(scriptUrl, {method:"HEAD"})` (запрос к собственному URL скрипта, не в интернет) и попытка подгрузить
`*.woff2` по `fontsDirectory`. С `mathlive-static.css` этот путь не выполняется вообще.
