# Вендоренные шрифты оболочки RDE

Файлы лежат в репозитории, чтобы интерфейс не зависел от системных
`/usr/share/fonts` (детерминированная сборка, офлайн, кириллица везде).

| Файл | Роль в `FontDefinitions` | Источник |
|---|---|---|
| `DejaVuSans.ttf` | `rde-sans` — пропорциональный, первый в `FontFamily::Proportional` | `/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf` |
| `DejaVuSansMono.ttf` | `rde-mono` — моноширинный, первый в `FontFamily::Monospace` | `/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf` |
| `RdeIcons.ttf` | `rde-icons` — сабсет 21 иконки Phosphor (PUA-кодпоинты `IC_*` в `src/lib.rs`); в Proportional/medium/semibold ставится первым (`ensure_icon_fonts`), т.к. WordCraft при открытии редактора подменяет шрифты | `@phosphor-icons/web@2.1.2` → `src/regular/Phosphor.ttf`, сабсет через `fontTools.subset`. Лицензия MIT, `PHOSPHOR-LICENSE.txt` (© 2023 Phosphor Icons) |
| `DejaVuSans-Bold-subset.ttf` | полужирное начертание для семьи `semibold` (заголовки секций инспектора) | `/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf`, сабсет U+0020–007E, U+00A0–00FF, U+0400–04FF и знаки пунктуации (`fontTools.subset`). Лицензия та же, что у DejaVu |

Иконки: Phosphor (MIT) — лицензия подтверждена по `LICENSE` в репозитории
`phosphor-icons/core`. Вендорится только используемое (21 глиф, 488 КБ полного
шрифта → ~5 КБ сабсета). Lucide (ISC) не вендорился: набор не нужен второй раз.

Именованные семьи WordCraft (`medium`/`semibold`) тоже привязаны: `medium` начинается с
`rde-sans`, `semibold` — с полужирного `rde-bold`. WordCraft ставит на `semibold`
глобальный `TextStyle::Heading`, и без привязки заголовки окон RDE паникуют.
Заголовки секций инспектора берут `semibold` через `bold_family(ctx)`: это делается
только со второго кадра (флаг `FONTS_READY_ID`), потому что `set_fonts` применяется в
конце первого кадра; в первом кадре — обычный Proportional.

Покрытие кириллицы (проверено `fontTools`): оба файла содержат все 96
символов U+0400–U+045F, включая Ё/ё.

Лицензия: DejaVu Fonts — Copyright (c) 2003 by Bitstream, Inc., изменения
DejaVu — public domain; распространение (включая вендоринг) разрешено
(см. `/usr/share/doc/fonts-dejavu-core/copyright`, лицензия Bitstream Vera).
Полный текст — в `LICENSE-MIT`/`LICENSE-APACHE` корня репозитория не входит;
условия шрифтов — permissive, указание авторства выше достаточно.

Шрифты документа (Aptos/TNR) живут в WordCraft-DocView — здесь не дублируются.
Типошкала оболочки (фаза 1): FS_DISPLAY 18 / HEADING 16 / BODY 13 /
BUTTON 12 / SMALL 12 / CAPTION 11 / MONO 11.5.

Глифы строк таблиц (P4): U+FFFC (OBJECT REPLACEMENT CHARACTER, якорь inline-объекта
WordCraft) не печатается — в таблицах структуры и вставок он удаляется функцией
`display_text` (только при показе, данные не меняются). После открытия редактора WordCraft
подменяет шрифты оболочки на Inter/Source*, а в них U+FFFC нет → квадрат. Все остальные
символы строк формата вставок (`·`, `—`, `σ`, кириллица) покрыты и DejaVu, и Inter.
Проверка: тест `structure_and_insert_strings_have_glyphs` (rde-ui, ttf-parser).
