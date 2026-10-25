# Вендоренные шрифты оболочки RDE

Файлы лежат в репозитории, чтобы интерфейс не зависел от системных
`/usr/share/fonts` (детерминированная сборка, офлайн, кириллица везде).

| Файл | Роль в `FontDefinitions` | Источник |
|---|---|---|
| `DejaVuSans.ttf` | `rde-sans` — пропорциональный, первый в `FontFamily::Proportional` | `/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf` |
| `DejaVuSansMono.ttf` | `rde-mono` — моноширинный, первый в `FontFamily::Monospace` | `/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf` |

Именованные семьи WordCraft (`medium`/`semibold`) тоже привязаны к `rde-sans`:
WordCraft ставит на `semibold` глобальный `TextStyle::Heading`, и без привязки
заголовки окон RDE паникуют, когда активны шрифты оболочки.

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
