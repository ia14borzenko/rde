//! RDE project model — prototype scope.
//!
//! Сейчас это статическая модель демонстрационного проекта: дерево объектов,
//! статусы свежести и журнал. Настоящие `rde.toml`/`artifact.toml`, CAS, DAG и
//! транзакции появятся в следующих итерациях (см. `.agent/plan-v01.md`, В2–В4);
//! интерфейс этого крейта намеренно беден, чтобы UI не зависел от деталей.

use std::path::PathBuf;

/// Тип объекта дерева проекта (фиксированный набор v0.x, см. tz-02 R-TREE-01).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ObjectKind {
    Document,
    Artifact,
    Constants,
    Source,
    Note,
    Repository,
}

impl ObjectKind {
    pub fn label_ru(self) -> &'static str {
        match self {
            ObjectKind::Document => "Документы",
            ObjectKind::Artifact => "Артефакты",
            ObjectKind::Constants => "Константы",
            ObjectKind::Source => "Источники",
            ObjectKind::Note => "Заметки",
            ObjectKind::Repository => "Репозитории",
        }
    }

    /// Символ-иконка: без внешних ассетов, чтобы не тащить icon-библиотеки в прототип.
    /// Короткая буквенная метка типа (глифы-иконки не во всех шрифтах).
    pub fn icon(self) -> &'static str {
        match self {
            ObjectKind::Document => "Д",
            ObjectKind::Artifact => "А",
            ObjectKind::Constants => "π",
            ObjectKind::Source => "И",
            ObjectKind::Note => "З",
            ObjectKind::Repository => "R",
        }
    }
}

/// Свежесть/целостность объекта (упрощение `stale` из tz-02 §7.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Freshness {
    UpToDate,
    Stale,
    Failed,
    External,
}

impl Freshness {
    pub fn label_ru(self) -> &'static str {
        match self {
            Freshness::UpToDate => "актуален",
            Freshness::Stale => "устарел",
            Freshness::Failed => "ошибка",
            Freshness::External => "внешний",
        }
    }
}

/// Объект дерева проекта.
#[derive(Clone, Debug)]
pub struct Object {
    pub id: String,
    pub title: String,
    pub kind: ObjectKind,
    pub freshness: Freshness,
    pub detail: String,
}

impl Object {
    fn new(id: &str, title: &str, kind: ObjectKind, freshness: Freshness, detail: &str) -> Self {
        Self {
            id: id.to_owned(),
            title: title.to_owned(),
            kind,
            freshness,
            detail: detail.to_owned(),
        }
    }
}

/// Группа объектов одного типа.
#[derive(Clone, Debug)]
pub struct Group {
    pub kind: ObjectKind,
    pub title: &'static str,
    pub objects: Vec<Object>,
}

/// Уровень строки журнала.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Ok,
    Warn,
}

/// Строка журнала проекта/обновлений.
#[derive(Clone, Debug)]
pub struct LogLine {
    pub time: &'static str,
    pub level: Level,
    pub text: String,
}

/// Демонстрационный проект RDE.
#[derive(Clone, Debug)]
pub struct Project {
    pub name: String,
    pub root: String,
    pub branch: String,
    pub updated: String,
    pub groups: Vec<Group>,
    pub stale: usize,
    pub figures: usize,
    /// Путь к демонстрационному `.docx` (в реальном проекте — объект-документ).
    pub docx: PathBuf,
    pub document_title: String,
    pub document_pages: usize,
    pub document_paragraphs: usize,
    pub log: Vec<LogLine>,
}

impl Project {
    /// Путь к демонстрационному `.docx` (фикстура репозитория).
    pub fn demo_docx_path(&self) -> PathBuf {
        self.docx.clone()
    }

    /// Всего объектов в дереве.
    pub fn object_count(&self) -> usize {
        self.groups.iter().map(|g| g.objects.len()).sum()
    }
}

/// Путь к фикстуре демо-документа. `RDE_DEMO_DOC` переопределяет (удобно для тестов и скриншотов).
pub fn demo_docx_path() -> PathBuf {
    if let Some(p) = std::env::var_os("RDE_DEMO_DOC") {
        return PathBuf::from(p);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/demo-report.docx")
}

/// Демонстрационный проект: отражает целевой UX RDE (дерево, DAG-статусы, вставки, агент).
pub fn demo() -> Project {
    let groups = vec![
        Group {
            kind: ObjectKind::Document,
            title: "Документы",
            objects: vec![Object::new(
                "doc-report",
                "Отчёт по НИР (демо)",
                ObjectKind::Document,
                Freshness::UpToDate,
                "2 страницы · 1 рисунок",
            )],
        },
        Group {
            kind: ObjectKind::Artifact,
            title: "Артефакты",
            objects: vec![
                Object::new("art-realizations", "realizations.csv", ObjectKind::Artifact, Freshness::UpToDate, "вход · 4 файла"),
                Object::new("art-stats", "stats (compute)", ObjectKind::Artifact, Freshness::Stale, "изменён script.py"),
                Object::new("art-plot", "plot.svg / plot.png", ObjectKind::Artifact, Freshness::Stale, "выход · 2 файла"),
            ],
        },
        Group {
            kind: ObjectKind::Constants,
            title: "Константы",
            objects: vec![Object::new(
                "const-rn",
                "R_н — сопротивление нагрузки",
                ObjectKind::Constants,
                Freshness::UpToDate,
                "12.4 Ом · вставок: 1",
            )],
        },
        Group {
            kind: ObjectKind::Source,
            title: "Источники",
            objects: vec![
                Object::new("src-gost", "ГОСТ 7.32-2017", ObjectKind::Source, Freshness::UpToDate, "PDF · 41 стр."),
                Object::new("src-cw", "Cumming & Wong, гл. 7", ObjectKind::Source, Freshness::UpToDate, "PDF · стр. 312–361"),
                Object::new("note-chapter2", "Заметка к главе 2", ObjectKind::Note, Freshness::UpToDate, "md · 18 строк"),
            ],
        },
        Group {
            kind: ObjectKind::Repository,
            title: "Репозитории",
            objects: vec![Object::new(
                "repo-simdata",
                "sim-data",
                ObjectKind::Repository,
                Freshness::External,
                "~/proj/rde-demo/sim-data",
            )],
        },
    ];

    let log = vec![
        LogLine { time: "21:12:03", level: Level::Warn, text: "stats: изменён script.py → устарел (in_hash)".into() },
        LogLine { time: "21:12:03", level: Level::Warn, text: "plot.svg: устарел — изменился входной артефакт".into() },
        LogLine { time: "21:12:04", level: Level::Info, text: "Документ «Отчёт по НИР»: устаревших вставок — 1 (рисунок 1)".into() },
        LogLine { time: "21:12:05", level: Level::Ok, text: "Проект открыт: 2 страницы, 13 абзацев, 3 источника".into() },
        LogLine { time: "21:12:06", level: Level::Info, text: "Агент: сессия ACP готова — dsh --profile acp · workspace-write · ask".into() },
    ];

    Project {
        name: "Отчёт по НИР".to_owned(),
        docx: demo_docx_path(),
        root: "~/proj/rde-demo".to_owned(),
        branch: "main".to_owned(),
        updated: "2 мин назад".to_owned(),
        groups,
        stale: 2,
        figures: 1,
        document_title: "Отчёт по НИР (демо)".to_owned(),
        document_pages: 2,
        document_paragraphs: 13,
        log,
    }
}
