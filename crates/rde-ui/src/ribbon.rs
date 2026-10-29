//! Лента RDE по образцу WordCraft: вкладки сверху, под каждой — группы команд с подписями снизу.
//!
//! Этот модуль — только данные: какие команды на какой вкладке и в какой группе. Отрисовка и
//! диспетчеризация (`Cmd` → действие) — в `lib.rs` (`ribbon_ui`, `run_cmd`). Бывшее меню
//! «Файл/Правка/Документ/Проект/Вид/Агент/Помощь» не удалено логически: его пункты описаны в
//! `MENU_MODEL`, и тест ниже требует, чтобы каждый из них был на какой-то вкладке ленты.

use super::{
    IC_ADD, IC_ARTIFACT, IC_CHECK, IC_CODE, IC_CONST, IC_DOC, IC_DOWNLOAD, IC_EXPORT, IC_INSPECT, IC_NOTE, IC_OPEN, IC_RENAME,
    IC_REPO, IC_SAVE, IC_SEARCH, IC_SOURCE, IC_TREE, IC_UPDATE,
};

/// Команда оболочки. Одна точка входа для ленты, строки режимов и (бывшего) меню.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Cmd {
    // Файл
    NewProject,
    OpenProject,
    SaveProject,
    ExportDocx,
    OpenWordCopy,
    ExportProject,
    ImportReview,
    /// Импорт чужого `.docx` как нового объекта-документа (R-DOC-39, минимальная версия).
    ImportDocx,
    /// Импорт цитат BibTeX/RIS/CSL-JSON → объекты-источники проекта (rde-sources).
    ImportCitations,
    // Правка
    Undo,
    Redo,
    Find,
    // Главная
    NewObject,
    UpdateDocument,
    ToggleAutoUpdate,
    RollbackDocument,
    RollbackArtifacts,
    // Вставка (семь видов)
    InsertValue,
    InsertFigure,
    InsertTable,
    InsertConstant,
    InsertCrossref,
    InsertCitation,
    InsertDate,
    InsertFormula,
    // Документ
    /// P4: создать project-level формулу из GUI (диалог LaTeX + предпросмотр).
    NewFormula,
    /// P4: открыть панель редактора выбранной формулы проекта.
    EditFormula,
    MigrateFormulas,
    ApplyGost,
    InsertBibliography,
    Zoom100,
    PageWidth,
    // Проект
    CheckProject,
    UpdateAll,
    Settings,
    AppSettings,
    ExportDiagnostics,
    GitInit,
    GitCommit,
    GitHistory,
    BackupProject,
    RestoreBackup,
    // Вид
    ToggleTree,
    ToggleAgent,
    ToggleLogs,
    ToggleInspector,
    /// Показывать служебные объекты (формулы) в дереве (R-TREE-01/02, P3).
    ToggleServiceObjects,
    // Агент / Помощь
    Audit,
    /// M4.4: окно ревью правок агента (diff-UI suggested edits).
    AgentSuggestions,
    About,
    // Масштаб — на ленте (вкладка «Документ», группа «Масштаб», слот `Slot::Zoom`): из строки
    // вкладок центра он убран (§19-7).
    ZoomOut,
    ZoomIn,
    // Единственная команда, которая не на ленте: контролы предпросмотра страниц (лента в этом
    // режиме не показывается, поэтому кнопка живёт в панели предпросмотра).
    UpdateInserts,
}

#[cfg(test)]
impl Cmd {
    /// Полный реестр команд диспетчера (лента + панельные контролы). Тест ниже требует, чтобы
    /// каждая команда была достижима из GUI (на ленте) или явно исключена с причиной — так новая
    /// команда не может «молча» остаться без кнопки.
    pub(crate) const ALL: &'static [Cmd] = &[
        Cmd::NewProject,
        Cmd::OpenProject,
        Cmd::SaveProject,
        Cmd::ExportDocx,
        Cmd::OpenWordCopy,
        Cmd::ExportProject,
        Cmd::ImportReview,
        Cmd::ImportDocx,
        Cmd::ImportCitations,
        Cmd::Undo,
        Cmd::Redo,
        Cmd::Find,
        Cmd::NewObject,
        Cmd::UpdateDocument,
        Cmd::ToggleAutoUpdate,
        Cmd::RollbackDocument,
        Cmd::RollbackArtifacts,
        Cmd::InsertValue,
        Cmd::InsertFigure,
        Cmd::InsertTable,
        Cmd::InsertConstant,
        Cmd::InsertCrossref,
        Cmd::InsertCitation,
        Cmd::InsertDate,
        Cmd::InsertFormula,
        Cmd::NewFormula,
        Cmd::EditFormula,
        Cmd::MigrateFormulas,
        Cmd::ApplyGost,
        Cmd::InsertBibliography,
        Cmd::Zoom100,
        Cmd::PageWidth,
        Cmd::ZoomOut,
        Cmd::ZoomIn,
        Cmd::CheckProject,
        Cmd::UpdateAll,
        Cmd::Settings,
        Cmd::AppSettings,
        Cmd::ExportDiagnostics,
        Cmd::GitInit,
        Cmd::GitCommit,
        Cmd::GitHistory,
        Cmd::BackupProject,
        Cmd::RestoreBackup,
        Cmd::ToggleTree,
        Cmd::ToggleAgent,
        Cmd::ToggleLogs,
        Cmd::ToggleInspector,
        Cmd::ToggleServiceObjects,
        Cmd::Audit,
        Cmd::AgentSuggestions,
        Cmd::About,
        Cmd::UpdateInserts,
    ];
}

/// Вкладки ленты (порядок — как в WordCraft: файл, правка, главная, вставка, …).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum RibbonTab {
    File,
    Edit,
    Home,
    /// P4 (ADR-0020): полноценный редактор формулы в ленте — открывается по клику на формулу
    /// в тексте/дереве/инспекторе; заменяет группы ленты панелью редактора.
    Formula,
    Insert,
    Document,
    Project,
    View,
    Agent,
    Help,
}

impl RibbonTab {
    pub(crate) const ALL: [RibbonTab; 10] = [
        RibbonTab::File,
        RibbonTab::Edit,
        RibbonTab::Home,
        RibbonTab::Formula,
        RibbonTab::Insert,
        RibbonTab::Document,
        RibbonTab::Project,
        RibbonTab::View,
        RibbonTab::Agent,
        RibbonTab::Help,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            RibbonTab::File => "Файл",
            RibbonTab::Edit => "Правка",
            RibbonTab::Home => "Главная",
            RibbonTab::Formula => "Формула",
            RibbonTab::Insert => "Вставка",
            RibbonTab::Document => "Документ",
            RibbonTab::Project => "Проект",
            RibbonTab::View => "Вид",
            RibbonTab::Agent => "Агент",
            RibbonTab::Help => "Помощь",
        }
    }
}

/// Слот группы. Кнопки команд, переключатели видимости/состояния и два особых блока, у которых
/// есть своя логика отрисовки в `lib.rs`.
#[derive(Clone, Copy)]
pub(crate) enum Slot {
    /// Большая кнопка: иконка (может быть пустой), подпись в 1–2 строки, подсказка.
    Button { cmd: Cmd, icon: &'static str, label: &'static str, tip: &'static str, primary: bool },
    /// Переключатель с состоянием (видимость панели, автообновление).
    Toggle { cmd: Cmd, icon: &'static str, label: &'static str, tip: &'static str },
    /// Компактная иконка без подписи; полное название — в подсказке (и в доступной подписи).
    Icon { cmd: Cmd, icon: &'static str, tip: &'static str },
    /// Позиция следующей вставки + «сброс» + счётчик вставок (состояние — в `RdeApp`).
    InsertPosition,
    /// Кластер масштаба (§19-7): «−», текущий %, «+», «Сброс», «По ширине». Отрисовка — в
    /// `lib.rs` (`ribbon_slot_ui`), потому что процент — динамическое состояние редактора.
    Zoom,
    /// Текст без команды (состояние агента: v0.3, не подключён).
    Note(&'static str),
    /// Полноценный редактор формулы на вкладке «Формула» (P4/ADR-0020): LaTeX-источник,
    /// предпросмотр, display/inline, политика нумерации, «Применить»/«Вставить».
    /// Отрисовка — `RdeApp::formula_ribbon_editor` (вкладка заменяет группы ленты).
    FormulaEditor,
}

impl Slot {
    /// Команда слота, если слот её запускает (для тестов полноты; отрисовка берёт `cmd` из полей).
    #[cfg(test)]
    pub(crate) fn cmd(self) -> Option<Cmd> {
        match self {
            Slot::Button { cmd, .. } | Slot::Toggle { cmd, .. } | Slot::Icon { cmd, .. } => Some(cmd),
            Slot::InsertPosition | Slot::Zoom | Slot::Note(_) | Slot::FormulaEditor => None,
        }
    }
}

/// Группа команд внутри вкладки: слоты слева направо и подпись снизу.
pub(crate) struct Group {
    pub(crate) title: &'static str,
    pub(crate) slots: &'static [Slot],
}

const INSERT_SLOTS: &[Slot] = &[
    Slot::Icon { cmd: Cmd::InsertValue, icon: IC_ARTIFACT, tip: "Вставить значение артефакта…" },
    Slot::Icon { cmd: Cmd::InsertFigure, icon: IC_DOC, tip: "Вставить рисунок из артефакта…" },
    Slot::Icon { cmd: Cmd::InsertTable, icon: IC_TREE, tip: "Вставить таблицу из CSV-выхода…" },
    Slot::Icon { cmd: Cmd::InsertConstant, icon: IC_CONST, tip: "Вставить константу проекта…" },
    Slot::Icon { cmd: Cmd::InsertCrossref, icon: IC_CODE, tip: "Вставить перекрёстную ссылку на рисунок/таблицу…" },
    Slot::Icon { cmd: Cmd::InsertCitation, icon: IC_SOURCE, tip: "Вставить цитату на источник…" },
    Slot::Icon { cmd: Cmd::InsertDate, icon: IC_NOTE, tip: "Вставить дату…" },
    Slot::Icon { cmd: Cmd::InsertFormula, icon: IC_CONST, tip: "Вставить формулу проекта (OMML / SVG)…" },
];

/// Группы вкладки. Порядок групп и слотов — порядок отрисовки.
pub(crate) fn ribbon_groups(tab: RibbonTab) -> &'static [Group] {
    match tab {
        RibbonTab::File => &[
            Group {
                title: "Сохранение",
                slots: &[
                    Slot::Button {
                        cmd: Cmd::SaveProject,
                        icon: IC_SAVE,
                        label: "Сохранить\nпроект",
                        tip: "Сохранить проект: записать документ в .doc.rde (RDE — владелец сохранения)",
                        primary: false,
                    },
                    Slot::Button {
                        cmd: Cmd::ExportDocx,
                        icon: IC_EXPORT,
                        label: "Экспорт\n.docx",
                        tip: "Экспорт .docx: собрать чистый .docx из .doc.rde",
                        primary: false,
                    },
                    Slot::Button {
                        cmd: Cmd::OpenWordCopy,
                        icon: IC_OPEN,
                        label: "Открыть копию\nв Word",
                        tip: "Открыть копию в Word: экспорт .docx в exports/ и открытие системным обработчиком",
                        primary: false,
                    },
                ],
            },
            Group {
                title: "Проект",
                slots: &[
                    Slot::Button { cmd: Cmd::NewProject, icon: IC_ADD, label: "Новый\nпроект…", tip: "Новый проект…", primary: false },
                    Slot::Button { cmd: Cmd::OpenProject, icon: IC_OPEN, label: "Открыть\nпроект…", tip: "Открыть проект…", primary: false },
                    Slot::Button {
                        cmd: Cmd::ExportProject,
                        icon: IC_EXPORT,
                        label: "Экспорт\n.rde.zip",
                        tip: "Экспорт проекта (.rde.zip)",
                        primary: false,
                    },
                ],
            },
            Group {
                title: "Обмен с Word",
                slots: &[
                    Slot::Button {
                        cmd: Cmd::ImportDocx,
                        icon: IC_DOWNLOAD,
                        label: "Импорт\n.docx…",
                        tip: "Импорт .docx: новый объект-документ «[IMPORTED] …» (исходник не изменяется, отчёт — в журнале)",
                        primary: false,
                    },
                    Slot::Button {
                        cmd: Cmd::ImportReview,
                        icon: IC_DOWNLOAD,
                        label: "Импорт ревью\nиз Word…",
                        tip: "Импорт ревью: .docx после правок в Word становится новой версией документа (выбор файла; пусто — свежий из exports/)",
                        primary: false,
                    },
                ],
            },
            Group {
                title: "Источники",
                slots: &[Slot::Button {
                    cmd: Cmd::ImportCitations,
                    icon: IC_SOURCE,
                    label: "Импорт\nцитат…",
                    tip: "Импорт цитат BibTeX/RIS/CSL-JSON: создать объекты-источники проекта с библиографией",
                    primary: false,
                }],
            },
        ],
        RibbonTab::Edit => &[
            Group {
                title: "Отмена",
                slots: &[
                    Slot::Button { cmd: Cmd::Undo, icon: "", label: "Отменить", tip: "Отменить (Ctrl+Z)", primary: false },
                    Slot::Button { cmd: Cmd::Redo, icon: "", label: "Повторить", tip: "Повторить (Ctrl+Y)", primary: false },
                ],
            },
            Group {
                title: "Поиск",
                slots: &[Slot::Button { cmd: Cmd::Find, icon: IC_SEARCH, label: "Найти в\nпроекте…", tip: "Найти в проекте…", primary: false }],
            },
        ],
        RibbonTab::Home => &[
            Group {
                title: "Объект",
                slots: &[
                    Slot::Button { cmd: Cmd::NewObject, icon: IC_ADD, label: "+ Объект", tip: "Новый объект проекта", primary: false },
                    Slot::Button { cmd: Cmd::Find, icon: IC_SEARCH, label: "Поиск", tip: "Поиск по объектам (Ctrl+F)", primary: false },
                ],
            },
            Group {
                title: "Обновление",
                slots: &[
                    Slot::Button {
                        cmd: Cmd::UpdateDocument,
                        icon: IC_UPDATE,
                        label: "Обновить\nдокумент",
                        tip: "Обновить документ: вставки/подписи, транзакция и пересохранение .doc.rde",
                        primary: true,
                    },
                    Slot::Toggle {
                        cmd: Cmd::ToggleAutoUpdate,
                        icon: "",
                        label: "Авто-\nобновление",
                        tip: "Автообновление документа",
                    },
                ],
            },
            Group {
                title: "Откат",
                slots: &[
                    Slot::Button {
                        cmd: Cmd::RollbackDocument,
                        icon: "",
                        label: "Откат\nдокумента",
                        tip: "Отменить обновление документа",
                        primary: false,
                    },
                    Slot::Button {
                        cmd: Cmd::RollbackArtifacts,
                        icon: "",
                        label: "Откат\nартефактов",
                        tip: "Отменить обновление артефактов",
                        primary: false,
                    },
                ],
            },
        ],
        // P4 (ADR-0020): вкладка «Формула» — панель редактора рисуется вместо групп
        // (`ribbon_ui` → `formula_ribbon_editor`); слот-маркер держит инвариант «у вкладки есть
        // группа и слот» и не несёт команды.
        RibbonTab::Formula => &[Group { title: "Редактор формулы", slots: &[Slot::FormulaEditor] }],
        RibbonTab::Insert => &[
            Group { title: "Вставки", slots: INSERT_SLOTS },
            Group { title: "Позиция", slots: &[Slot::InsertPosition] },
        ],
        RibbonTab::Document => &[
            Group {
                title: "Оформление",
                slots: &[
                    Slot::Button {
                        cmd: Cmd::ApplyGost,
                        icon: IC_DOC,
                        label: "Оформить\nпо ГОСТ",
                        tip: "Оформить по ГОСТ 7.32-2017 и вставить библиографию проекта",
                        primary: true,
                    },
                    Slot::Button {
                        cmd: Cmd::InsertBibliography,
                        icon: IC_SOURCE,
                        label: "Вставить\nбиблиографию",
                        tip: "Вставить библиографию: список источников проекта в документ",
                        primary: false,
                    },
                ],
            },
            Group {
                title: "Формулы",
                // P3: вставка формулы и явная миграция OMML вне реестра (ADR-0005 §7).
                // P4: создание и редактирование source-of-truth формулы в GUI (ADR-0020).
                slots: &[
                    Slot::Button {
                        cmd: Cmd::NewFormula,
                        icon: IC_ADD,
                        label: "Создать\nформулу…",
                        tip: "Создать формулу проекта из GUI: LaTeX + предпросмотр, политика нумерации (P4)",
                        primary: true,
                    },
                    Slot::Button {
                        cmd: Cmd::EditFormula,
                        icon: IC_RENAME,
                        label: "Редактировать\nформулу…",
                        tip: "Редактировать источник формулы (title/LaTeX/display/нумерация) и вставить в документ",
                        primary: false,
                    },
                    Slot::Button {
                        cmd: Cmd::InsertFormula,
                        icon: IC_CONST,
                        label: "Вставить\nформулу…",
                        tip: "Вставить формулу проекта: Word-объект (OMML) или SVG-вектор (P3)",
                        primary: false,
                    },
                    Slot::Button {
                        cmd: Cmd::MigrateFormulas,
                        icon: IC_CODE,
                        label: "Принять формулы\nкак объекты",
                        tip: "Мигрировать OMML вне реестра в project-level формулы и вставки-ссылки (явная команда, идемпотентна)",
                        primary: false,
                    },
                ],
            },
            Group {
                title: "Масштаб",
                // Весь кластер масштаба живёт на ленте (§19-7): в строке вкладок центра его нет.
                slots: &[Slot::Zoom],
            },
        ],
        RibbonTab::Project => &[
            Group {
                title: "Проверка",
                slots: &[
                    Slot::Button { cmd: Cmd::CheckProject, icon: IC_CHECK, label: "Проверить\nи починить…", tip: "Проверить и починить…", primary: false },
                    Slot::Button { cmd: Cmd::UpdateAll, icon: IC_UPDATE, label: "Обновить все\nартефакты", tip: "Обновить все артефакты", primary: false },
                ],
            },
            Group {
                title: "Настройки",
                slots: &[
                    Slot::Button { cmd: Cmd::Settings, icon: "", label: "Проекта…", tip: "Настройки проекта…", primary: false },
                    Slot::Button { cmd: Cmd::AppSettings, icon: "", label: "Приложения…", tip: "Настройки приложения…", primary: false },
                    Slot::Button { cmd: Cmd::ExportDiagnostics, icon: "", label: "Диагностика…", tip: "Экспорт диагностики…", primary: false },
                ],
            },
            Group {
                title: "Git",
                slots: &[
                    Slot::Button { cmd: Cmd::GitInit, icon: IC_REPO, label: "Git: init", tip: "Git: инициализировать репозиторий", primary: false },
                    Slot::Button { cmd: Cmd::GitCommit, icon: IC_REPO, label: "Коммит…", tip: "Git: закоммитить изменения…", primary: false },
                    Slot::Button { cmd: Cmd::GitHistory, icon: IC_REPO, label: "История", tip: "Git: история", primary: false },
                ],
            },
            Group {
                title: "Бэкапы",
                slots: &[
                    Slot::Button { cmd: Cmd::BackupProject, icon: IC_SAVE, label: "Бэкап\n(zip)", tip: "Сделать бэкап проекта (zip)", primary: false },
                    Slot::Button {
                        cmd: Cmd::RestoreBackup,
                        icon: "",
                        label: "Бэкапы:\nвосстановить…",
                        tip: "Бэкапы проекта: список/восстановить…",
                        primary: false,
                    },
                    Slot::Button {
                        cmd: Cmd::RollbackArtifacts,
                        icon: "",
                        label: "Откат\nартефактов",
                        tip: "Откатить последнее обновление артефактов",
                        primary: false,
                    },
                ],
            },
        ],
        RibbonTab::View => &[Group {
            title: "Панели",
            slots: &[
                Slot::Toggle { cmd: Cmd::ToggleTree, icon: IC_TREE, label: "Дерево\nпроекта", tip: "Дерево проекта" },
                Slot::Toggle { cmd: Cmd::ToggleAgent, icon: "", label: "Панель\nагента", tip: "Панель агента" },
                Slot::Toggle { cmd: Cmd::ToggleLogs, icon: "", label: "Журнал и\nзадачи", tip: "Журнал и задачи" },
                Slot::Toggle { cmd: Cmd::ToggleInspector, icon: IC_INSPECT, label: "Инспектор\nобъекта", tip: "Инспектор объекта" },
                Slot::Toggle {
                    cmd: Cmd::ToggleServiceObjects,
                    icon: IC_CONST,
                    label: "Служебные\nобъекты",
                    tip: "Показать/скрыть служебные объекты (формулы проекта) в дереве",
                },
            ],
        }],
        RibbonTab::Agent => &[Group {
            title: "Агент",
            slots: &[
                Slot::Note("MCP/ACP — v0.3"),
                Slot::Button { cmd: Cmd::AgentSuggestions, icon: IC_RENAME, label: "Правки\nагента…", tip: "Правки агента (suggested edits): diff, принять/отклонить", primary: false },
                Slot::Button { cmd: Cmd::Audit, icon: IC_CHECK, label: "Аудит\nдействий…", tip: "Аудит действий…", primary: false },
            ],
        }],
        RibbonTab::Help => &[Group {
            title: "Справка",
            slots: &[Slot::Button { cmd: Cmd::About, icon: "", label: "О\nпрототипе", tip: "О прототипе", primary: false }],
        }],
    }
}

/// Модель бывшего меню «Файл…Помощь»: меню → команды. Эталон для теста полноты ленты
/// (меню больше не строится; модель нужна только тестам).
#[cfg(test)]
pub(crate) const MENU_MODEL: &[(&str, &[Cmd])] = &[
    ("Файл", &[Cmd::NewProject, Cmd::OpenProject, Cmd::ExportDocx, Cmd::ExportProject]),
    ("Правка", &[Cmd::Undo, Cmd::Redo, Cmd::Find]),
    (
        "Документ",
        &[
            Cmd::InsertValue,
            Cmd::InsertFigure,
            Cmd::InsertTable,
            Cmd::InsertConstant,
            Cmd::InsertCrossref,
            Cmd::InsertCitation,
            Cmd::InsertDate,
            Cmd::InsertFormula,
            Cmd::MigrateFormulas,
            Cmd::UpdateDocument,
            Cmd::RollbackDocument,
        ],
    ),
    (
        "Проект",
        &[
            Cmd::CheckProject,
            Cmd::UpdateAll,
            Cmd::Settings,
            Cmd::AppSettings,
            Cmd::ExportDiagnostics,
            Cmd::GitInit,
            Cmd::GitCommit,
            Cmd::GitHistory,
            Cmd::BackupProject,
            Cmd::RestoreBackup,
            Cmd::RollbackArtifacts,
        ],
    ),
    ("Вид", &[Cmd::ToggleTree, Cmd::ToggleAgent, Cmd::ToggleLogs, Cmd::ToggleInspector]),
    ("Агент", &[Cmd::Audit]),
    ("Помощь", &[Cmd::About]),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn ribbon_cmds() -> Vec<Cmd> {
        RibbonTab::ALL
            .iter()
            .flat_map(|tab| ribbon_groups(*tab).iter())
            .flat_map(|group| group.slots.iter())
            .filter_map(|slot| slot.cmd())
            .collect()
    }

    /// (а) Полнота: каждый пункт бывшего меню есть хотя бы на одной вкладке ленты.
    #[test]
    fn every_menu_item_is_on_some_ribbon_tab() {
        let ribbon = ribbon_cmds();
        for (menu, cmds) in MENU_MODEL {
            for cmd in cmds.iter() {
                assert!(ribbon.contains(cmd), "пункт меню «{menu}» → {cmd:?} не найден ни на одной вкладке ленты");
            }
        }
    }

    /// Меню содержит 35 пунктов: 33 исходных (P9) + 2 P3-команды формул.
    #[test]
    fn menu_model_keeps_all_original_items() {
        let total: usize = MENU_MODEL.iter().map(|(_, cmds)| cmds.len()).sum();
        assert_eq!(total, 35);
    }

    /// (а2) Каждая команда диспетчера достижима из GUI: на ленте или в явном исключении
    /// с причиной (новая команда не может молча остаться без кнопки).
    #[test]
    fn every_command_is_reachable_or_documented_exception() {
        let ribbon = ribbon_cmds();
        // Не на ленте по устройству UI (не «забыли»):
        // • UpdateInserts — кнопка панели предпросмотра страниц (лента там не показывается);
        // • Zoom* / PageWidth — кластер масштаба `Slot::Zoom` (отрисовка в lib.rs, не Slot::Button).
        let exceptions = [Cmd::UpdateInserts, Cmd::Zoom100, Cmd::PageWidth, Cmd::ZoomOut, Cmd::ZoomIn];
        for cmd in Cmd::ALL {
            assert!(
                ribbon.contains(cmd) || exceptions.contains(cmd),
                "команда {cmd:?} недостижима из GUI: нет на ленте и нет в документированных исключениях"
            );
        }
        // Каждая команда ленты присутствует в реестре (обратная полнота).
        for cmd in ribbon {
            assert!(Cmd::ALL.contains(&cmd), "команда {cmd:?} есть на ленте, но забыта в Cmd::ALL");
        }
    }

    /// Вкладок десять (P4 добавила «Формула»), у каждой есть хотя бы одна группа со слотом.
    #[test]
    fn every_ribbon_tab_has_groups_and_slots() {
        assert_eq!(RibbonTab::ALL.len(), 10);
        for tab in RibbonTab::ALL {
            let groups = ribbon_groups(tab);
            assert!(!groups.is_empty(), "вкладка {} без групп", tab.title());
            for group in groups {
                assert!(!group.slots.is_empty(), "группа «{}» пуста", group.title);
            }
        }
    }

    /// Вставка: восемь видов на вкладке «Вставка» (с P3 — формула), у каждого иконка и подсказка.
    #[test]
    fn insert_tab_has_seven_kinds_with_icon_and_tip() {
        let slots: Vec<&Slot> = ribbon_groups(RibbonTab::Insert)[0].slots.iter().collect();
        assert_eq!(slots.len(), 8);
        for slot in slots {
            match slot {
                Slot::Icon { icon, tip, .. } => {
                    assert!(!icon.is_empty() && !tip.is_empty());
                }
                other => panic!("ожидалась иконка вставки, получено {:?}", other.cmd()),
            }
        }
    }
}
