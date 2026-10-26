//! Типизированные манифесты проекта RDE (M2.1).
//!
//! Манифесты — обычные TOML-файлы; схема каждого (`rde/<kind>@1`) генерируется из Rust-типов
//! через `schemars` и используется для валидации (`jsonschema`) при загрузке проекта.
//! Нормативные поля — из ТЗ `tz-02` §8.3; здесь реализовано подмножество v0.1.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const SCHEMA_PROJECT: &str = "rde/project@1";
pub const SCHEMA_ARTIFACT: &str = "rde/artifact@1";
pub const SCHEMA_DOCUMENT: &str = "rde/document@1";
pub const SCHEMA_SOURCE: &str = "rde/source@1";
pub const SCHEMA_NOTE: &str = "rde/note@1";
pub const SCHEMA_REPOSITORY: &str = "rde/repo@1";
pub const SCHEMA_FORMULA: &str = "rde/formula@1";

fn default_kind() -> String {
    "human".to_owned()
}

fn default_rebuild() -> String {
    "manual".to_owned()
}

fn default_mode() -> String {
    "hashed".to_owned()
}

fn default_env() -> String {
    "project".to_owned()
}

/// Автор проекта (профиль; см. tz-02 §3.2).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
pub struct AuthorManifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub initials: String,
    #[serde(default)]
    pub email: String,
    #[serde(default = "default_kind")]
    pub kind: String,
}

/// `rde.toml` — манифест проекта.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct ProjectManifest {
    pub schema: String,
    pub id: String,
    pub name: String,
    #[serde(default = "default_rebuild")]
    pub default_rebuild: String,
    #[serde(default)]
    pub default_template: String,
    #[serde(default)]
    pub authors: Vec<AuthorManifest>,
    /// Проект доверенный: пользователь дал согласие на запуск скриптов (R-RUN-06).
    #[serde(default)]
    pub trusted: bool,
    /// Сеть разрешена артефактам проекта по умолчанию (R-RUN-06; артефакт может переопределить).
    #[serde(default)]
    pub network_allow: bool,
    /// Автообновление документа при обновлении артефактов (R-DOC-12: off/manual/auto; здесь auto=on).
    #[serde(default)]
    pub document_auto_update: bool,
}

/// Окружение исполнения скрипта артефакта.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct RuntimeManifest {
    /// `python` | `shell` | `node` | `r`.
    pub kind: String,
    /// Режим окружения: `project` (uv/pnpm/pixi), `system` или `artifact`.
    #[serde(default = "default_env")]
    pub env: String,
    /// Относительный путь к исполняемому файлу артефакта.
    pub entry: String,
    /// Объявленные переменные окружения (входят в `env_hash`, tz-02 §7.2).
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
}

/// Вход артефакта: ссылка на выход другого артефакта, файл, заметка, источник или константы.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[serde(untagged)]
pub enum InputSpec {
    /// Выход другого артефакта; `selector` — относительный путь или glob (`out/*.csv`).
    Artifact {
        artifact: String,
        #[serde(default)]
        selector: String,
    },
    /// Файл данных внутри каталога артефакта (`inputs/data.csv`).
    File {
        file: String,
    },
    Note {
        note: String,
    },
    Source {
        source: String,
    },
    /// Типизированные константы проекта (артефакт `type = "constants"`).
    /// `selector` — имя конкретной величины; пусто — весь набор значений.
    Constants {
        constants: String,
        #[serde(default)]
        selector: String,
    },
}

impl InputSpec {
    /// Короткая подпись входа для UI/логов.
    pub fn label_ru(&self) -> String {
        match self {
            InputSpec::Artifact { artifact, selector } => {
                if selector.is_empty() {
                    format!("артефакт {artifact}")
                } else {
                    format!("{artifact}/{selector}")
                }
            }
            InputSpec::File { file } => format!("файл {file}"),
            InputSpec::Note { note } => format!("заметка {note}"),
            InputSpec::Source { source } => format!("источник {source}"),
            InputSpec::Constants { constants, selector } => {
                if selector.is_empty() {
                    format!("константы {constants}")
                } else {
                    format!("константа {constants}/{selector}")
                }
            }
        }
    }
}

/// Группа зависимостей артефакта (`hashed` — влияет на хеш/каскад, `ref` — только provenance).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct GroupManifest {
    pub name: String,
    #[serde(default = "default_mode")]
    pub mode: String,
    #[serde(default)]
    pub inputs: Vec<InputSpec>,
}

impl GroupManifest {
    /// Входит ли группа в `in_hash`/топосортировку (tz-02 §7.3 п.1/п.4).
    pub fn is_hashed(&self) -> bool {
        self.mode == "hashed"
    }
}

/// Значение константы проекта (тип `constants`, R-ART-09).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct ConstantValue {
    /// Литерал значения ИЛИ кэш последнего разрешённого значения при заданном `source`.
    pub value: f64,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub display: String,
    /// Подсказка форматирования (например, `italic-subscript` для `R_н`).
    #[serde(default)]
    pub style: String,
    /// Обозначение величины (R-DOC-23): базовая буква; курсив обеспечивается `style`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub symbol: String,
    /// Подстрочный индекс обозначения (R-DOC-23), например `н` в `R_н`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub subscript: String,
    /// Формат отображения значения: `auto|raw|int|dec:N|sci:N|percent:N` (R-DOC-11).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub format: String,
    /// Источник значения (R-ART-09, ADR-0005): URI выхода артефакта
    /// `rde://artifact/<id>[@<out_hash>]/<selector>`; при заданном `source` значение разрешается
    /// из артефакта, а `value` — кэш последнего разрешённого значения.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Разобранный источник значения константы (`ConstantValue::source`, ADR-0005 P1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConstantSource {
    /// Артефакт-источник (id).
    pub artifact_id: String,
    /// Закреплённый `out_hash` цели (если указан в URI через `@`).
    pub out_hash: Option<String>,
    /// Путь файла внутри каталога артефакта (до `#`), например `out/stats.json`.
    pub file: String,
    /// JSON pointer после `#` (`/stats/x1/mean`), если задан.
    pub pointer: Option<String>,
}

impl ConstantValue {
    /// Разобрать `source` (если задан). Формат URI — как у вставок (R-DOC-11):
    /// `rde://artifact/<id>[@<out_hash>]/<selector>`, где selector — путь внутри каталога
    /// артефакта; JSON pointer — после `#`.
    pub fn source_parsed(&self) -> Result<Option<ConstantSource>, String> {
        let Some(uri) = self.source.as_deref().map(str::trim).filter(|uri| !uri.is_empty()) else {
            return Ok(None);
        };
        let rest = uri
            .strip_prefix("rde://artifact/")
            .ok_or_else(|| format!("source константы должен начинаться с rde://artifact/: {uri}"))?;
        let (head, selector) = match rest.split_once('/') {
            Some((head, selector)) => (head, selector.trim().to_owned()),
            None => return Err(format!("source константы неполный (нужен artifact/<id>/<selector>): {uri}")),
        };
        let (artifact_id, out_hash) = match head.split_once('@') {
            Some((id, hash)) => (id.to_owned(), Some(hash.to_owned())),
            None => (head.to_owned(), None),
        };
        if artifact_id.is_empty() || selector.is_empty() {
            return Err(format!("source константы неполный (нужен artifact/<id>/<selector>): {uri}"));
        }
        let (file, pointer) = match selector.split_once('#') {
            Some((file, pointer)) => (file.to_owned(), Some(pointer.to_owned())),
            None => (selector, None),
        };
        Ok(Some(ConstantSource { artifact_id, out_hash, file, pointer }))
    }

    /// Текст обозначения/значения для документа (R-DOC-23, §9.7) — см. `value_format`.
    pub fn document_text(&self) -> String {
        crate::value_format::constant_text(self.value, &self.unit, &self.display, &self.symbol, &self.subscript, &self.format)
    }

    /// Rich-стиль текста обозначения (`italic`/`italic-subscript`), R-ART-09.
    pub fn document_run_style(&self) -> String {
        crate::value_format::constant_run_style(&self.style, &self.symbol, &self.subscript)
    }
}

/// `artifact.toml` — манифест артефакта (`compute`, `data` или `constants`).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct ArtifactManifest {
    pub schema: String,
    pub id: String,
    pub title: String,
    /// `compute` | `data` | `constants`.
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub rebuild: String,
    #[serde(default)]
    pub runtime: Option<RuntimeManifest>,
    /// Произвольные параметры запуска (входят в `params_hash`).
    #[serde(default)]
    pub params: serde_json::Value,
    /// Объявленные выходы (относительные пути/glob внутри каталога артефакта).
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Дополнительные файлы скрипта, входящие в `script_hash` (tz-02 §7.2, `extra_script_files`).
    /// Пути относительно каталога артефакта; читаются вместе с `runtime.entry`.
    #[serde(default)]
    pub script_files: Vec<String>,
    /// `out_hash` последнего успешного обновления (идентичность выходов, tz-02 §7.3 п.10).
    /// Пишется RDE после успешного обновления; отсутствует до первого запуска.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_hash: Option<String>,
    #[serde(default)]
    pub timeout_s: Option<u64>,
    #[serde(default)]
    pub network_allow: Option<bool>,
    #[serde(default)]
    pub groups: Vec<GroupManifest>,
    /// Для `type = "constants"`: типизированные значения проекта.
    #[serde(default)]
    pub values: BTreeMap<String, ConstantValue>,
}

impl ArtifactManifest {
    /// Входы всех `hashed`-групп (только они влияют на `in_hash` и каскад, §7.3 п.1).
    pub fn hashed_inputs(&self) -> impl Iterator<Item = (&GroupManifest, &InputSpec)> {
        self.groups
            .iter()
            .filter(|group| group.is_hashed())
            .flat_map(|group| group.inputs.iter().map(move |input| (group, input)))
    }

    /// Входы `ref`-групп: связаны, но не пересчитываются (§7.3 п.1).
    pub fn ref_inputs(&self) -> impl Iterator<Item = (&GroupManifest, &InputSpec)> {
        self.groups
            .iter()
            .filter(|group| !group.is_hashed())
            .flat_map(|group| group.inputs.iter().map(move |input| (group, input)))
    }
}

/// `document.toml` — манифест документа.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct DocumentManifest {
    pub schema: String,
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub template: String,
    /// Рабочий файл документа (в M3 — `.doc.rde`, пока `.docx`).
    pub file: String,
}

fn default_formula_numbering() -> String {
    "auto".to_owned()
}

fn default_true() -> bool {
    true
}

fn default_input_as() -> String {
    "value".to_owned()
}

/// Политика нумерации формулы (ADR-0005): фактический номер — документно-локальный.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
pub struct FormulaNumbering {
    /// `auto` — нумеровать при вставке/обновлении документа; `none` — без номера.
    #[serde(default = "default_formula_numbering")]
    pub mode: String,
    /// Метка/группа для будущих правил нумерации (по разделам/приложениям), пусто — нет.
    #[serde(default)]
    pub label: String,
}

impl Default for FormulaNumbering {
    fn default() -> Self {
        Self { mode: default_formula_numbering(), label: String::new() }
    }
}

/// Вход формулы (ADR-0005 §2.2): `alias` + source на константу/выход артефакта.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
pub struct FormulaInput {
    pub alias: String,
    /// `hashed` — влияет на `act_key`/каскад; `ref` — только provenance.
    #[serde(default = "default_mode")]
    pub mode: String,
    /// `rde://constants/<id>/<key>` или `rde://artifact/<id>[@<out_hash>]/<selector>`.
    pub source: String,
    /// Как подставлять: `value` — число по `format`; `symbol` — обозначение величины; `raw` — текст.
    #[serde(default = "default_input_as", rename = "as")]
    pub as_kind: String,
    /// Формат значения (R-DOC-11), пусто — из константы/авто.
    #[serde(default)]
    pub format: String,
    /// Переопределение единицы (пусто — из источника).
    #[serde(default)]
    pub unit: String,
}

/// Фрагмент формулы (ADR-0005 §2.3): другая формула проекта, подставляемая в LaTeX.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Eq)]
pub struct FormulaFragment {
    pub alias: String,
    /// `rde://formula/<id>[@<act_key>]`.
    pub source: String,
}

/// Кэш/зеркало ledger состояния формулы в `formula.toml` (не источник истины, пишет RDE).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default, PartialEq, Eq)]
pub struct FormulaState {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub act_key: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub resolved_latex_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub omml_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub svg_hash: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub png_hash: String,
    #[serde(default)]
    pub computed_at: u64,
}

/// `formula.toml` — project-level объект формулы (ADR-0005 §2): LaTeX-источник истины,
/// входы/фрагменты, политика нумерации, кэш-хеши.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct FormulaManifest {
    pub schema: String,
    pub id: String,
    pub title: String,
    /// LaTeX-источник истины (с плейсхолдерами `\rdeinput{...}`/`\rdefragment{...}`).
    pub latex: String,
    /// `true` — display (отдельная строка), `false` — inline.
    #[serde(default = "default_true")]
    pub display: bool,
    #[serde(default)]
    pub numbering: FormulaNumbering,
    #[serde(default)]
    pub inputs: Vec<FormulaInput>,
    #[serde(default)]
    pub fragments: Vec<FormulaFragment>,
    #[serde(default)]
    pub state: FormulaState,
}

/// Источник значения входа формулы (ADR-0005): константа проекта или выход артефакта.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormulaInputSource {
    Constants { constants_id: String, key: String },
    Artifact { artifact_id: String, out_hash: Option<String>, selector: String },
}

/// Ссылка на формулу-фрагмент (`rde://formula/<id>[@<act_key>]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormulaRef {
    pub formula_id: String,
    pub act_key: Option<String>,
}

/// Разобрать `source` входа формулы: `rde://constants/<id>/<key>` или
/// `rde://artifact/<id>[@<out_hash>]/<selector>`.
pub fn parse_formula_input_source(uri: &str) -> Result<FormulaInputSource, String> {
    let uri = uri.trim();
    if let Some(rest) = uri.strip_prefix("rde://constants/") {
        let (constants_id, key) = rest
            .split_once('/')
            .ok_or_else(|| format!("source входа формулы неполный (нужен rde://constants/<id>/<key>): {uri}"))?;
        if constants_id.is_empty() || key.is_empty() {
            return Err(format!("source входа формулы неполный: {uri}"));
        }
        return Ok(FormulaInputSource::Constants { constants_id: constants_id.to_owned(), key: key.to_owned() });
    }
    let rest = uri
        .strip_prefix("rde://artifact/")
        .ok_or_else(|| format!("source входа формулы должен быть rde://constants/… или rde://artifact/…: {uri}"))?;
    let (head, selector) = match rest.split_once('/') {
        Some((head, selector)) if !selector.trim().is_empty() => (head, selector.trim().to_owned()),
        _ => return Err(format!("source входа формулы неполный (нужен artifact/<id>/<selector>): {uri}")),
    };
    let (artifact_id, out_hash) = match head.split_once('@') {
        Some((id, hash)) => (id.to_owned(), Some(hash.to_owned())),
        None => (head.to_owned(), None),
    };
    if artifact_id.is_empty() {
        return Err(format!("source входа формулы: пустой id артефакта: {uri}"));
    }
    Ok(FormulaInputSource::Artifact { artifact_id, out_hash, selector })
}

/// Разобрать ссылку на формулу-фрагмент `rde://formula/<id>[@<act_key>]`.
pub fn parse_formula_ref(uri: &str) -> Result<FormulaRef, String> {
    let rest = uri
        .trim()
        .strip_prefix("rde://formula/")
        .ok_or_else(|| format!("ссылка на формулу должна начинаться с rde://formula/: {uri}"))?;
    let (id, act_key) = match rest.split_once('@') {
        Some((id, key)) => (id.to_owned(), Some(key.to_owned())),
        None => (rest.to_owned(), None),
    };
    let id = id.trim_end_matches('/').to_owned();
    if id.is_empty() {
        return Err(format!("ссылка на формулу без id: {uri}"));
    }
    Ok(FormulaRef { formula_id: id, act_key })
}

impl FormulaManifest {
    /// Проверить разметку формулы (без обращения к другим объектам): LaTeX, нумерация, входы,
    /// фрагменты. Ошибки — строки для `issues` загрузчика.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        if self.latex.trim().is_empty() {
            errors.push("latex не может быть пустым".to_owned());
        }
        if !matches!(self.numbering.mode.as_str(), "auto" | "none") {
            errors.push(format!("numbering.mode должен быть auto|none, а не `{}`", self.numbering.mode));
        }
        let mut aliases = std::collections::BTreeSet::new();
        for input in &self.inputs {
            if input.alias.trim().is_empty() {
                errors.push("вход формулы без alias".to_owned());
            } else if !aliases.insert(input.alias.clone()) {
                errors.push(format!("повторяющийся alias входа `{}`", input.alias));
            }
            if !matches!(input.mode.as_str(), "hashed" | "ref") {
                errors.push(format!("вход `{}`: mode должен быть hashed|ref", input.alias));
            }
            if !matches!(input.as_kind.as_str(), "value" | "symbol" | "raw") {
                errors.push(format!("вход `{}`: as должен быть value|symbol|raw", input.alias));
            }
            if let Err(error) = parse_formula_input_source(&input.source) {
                errors.push(format!("вход `{}`: {error}", input.alias));
            }
        }
        let mut fragment_aliases = std::collections::BTreeSet::new();
        for fragment in &self.fragments {
            if fragment.alias.trim().is_empty() {
                errors.push("фрагмент формулы без alias".to_owned());
            } else if !fragment_aliases.insert(fragment.alias.clone()) {
                errors.push(format!("повторяющийся alias фрагмента `{}`", fragment.alias));
            }
            if let Err(error) = parse_formula_ref(&fragment.source) {
                errors.push(format!("фрагмент `{}`: {error}", fragment.alias));
            }
        }
        errors
    }
}

/// Библиографическая запись источника (подмножество ГОСТ 7.0.100-2018).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, Default)]
pub struct ReferenceManifest {
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub year: Option<u16>,
    #[serde(default)]
    pub designation: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub publisher: String,
}

/// `source.toml` — манифест источника.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct SourceManifest {
    pub schema: String,
    pub id: String,
    pub title: String,
    /// `file` | `url` | `repo`.
    pub kind: String,
    #[serde(default)]
    pub file: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub retrieved_at: Option<String>,
    #[serde(default)]
    pub reference: Option<ReferenceManifest>,
}

/// `note.toml` — манифест заметки.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct NoteManifest {
    pub schema: String,
    pub id: String,
    pub title: String,
    pub file: String,
}

/// `repo.toml` — манифест репозитория.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct RepositoryManifest {
    pub schema: String,
    pub id: String,
    pub title: String,
    /// `internal` | `external` | `git` (git-клоны — фаза 2).
    pub kind: String,
    /// Путь: относительный внутри проекта или абсолютный (внешний).
    pub path: String,
}

/// Вид манифеста — для выбора схемы и имени файла.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ManifestKind {
    Project,
    Artifact,
    Document,
    Source,
    Note,
    Repository,
    Formula,
}

impl ManifestKind {
    /// Виды объектов проекта, которые грузятся из `objects/<dir>/` (ADR-0005: формула —
    /// project-level служебный объект).
    pub const ALL_OBJECT_KINDS: [ManifestKind; 6] = [
        ManifestKind::Document,
        ManifestKind::Artifact,
        ManifestKind::Source,
        ManifestKind::Note,
        ManifestKind::Repository,
        ManifestKind::Formula,
    ];

    /// Загружаемые «простые» виды (документы/источники/заметки/репозитории), кроме артефактов и
    /// формул — у них свои порядки загрузки.
    pub const SIMPLE_OBJECT_KINDS: [ManifestKind; 4] = [
        ManifestKind::Document,
        ManifestKind::Source,
        ManifestKind::Note,
        ManifestKind::Repository,
    ];

    /// Каталог объектов внутри `objects/`.
    pub fn objects_dir(self) -> &'static str {
        match self {
            ManifestKind::Project => "",
            ManifestKind::Artifact => "artifacts",
            ManifestKind::Document => "documents",
            ManifestKind::Source => "sources",
            ManifestKind::Note => "notes",
            ManifestKind::Repository => "repos",
            ManifestKind::Formula => "formulas",
        }
    }

    pub fn file_name(self) -> &'static str {
        match self {
            ManifestKind::Project => "rde.toml",
            ManifestKind::Artifact => "artifact.toml",
            ManifestKind::Document => "document.toml",
            ManifestKind::Source => "source.toml",
            ManifestKind::Note => "note.toml",
            ManifestKind::Repository => "repo.toml",
            ManifestKind::Formula => "formula.toml",
        }
    }

    pub fn schema_id(self) -> &'static str {
        match self {
            ManifestKind::Project => SCHEMA_PROJECT,
            ManifestKind::Artifact => SCHEMA_ARTIFACT,
            ManifestKind::Document => SCHEMA_DOCUMENT,
            ManifestKind::Source => SCHEMA_SOURCE,
            ManifestKind::Note => SCHEMA_NOTE,
            ManifestKind::Repository => SCHEMA_REPOSITORY,
            ManifestKind::Formula => SCHEMA_FORMULA,
        }
    }

    /// JSON Schema манифеста (генерируется из Rust-типа).
    pub fn schema(self) -> serde_json::Value {
        match self {
            ManifestKind::Project => schema_json::<ProjectManifest>(),
            ManifestKind::Artifact => schema_json::<ArtifactManifest>(),
            ManifestKind::Document => schema_json::<DocumentManifest>(),
            ManifestKind::Source => schema_json::<SourceManifest>(),
            ManifestKind::Note => schema_json::<NoteManifest>(),
            ManifestKind::Repository => schema_json::<RepositoryManifest>(),
            ManifestKind::Formula => schema_json::<FormulaManifest>(),
        }
    }
}

fn schema_json<T: JsonSchema>() -> serde_json::Value {
    let schema = schemars::schema_for!(T);
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

/// Скомпилированные JSON Schema (кэш на процесс): компиляция схемы на каждый манифест —
/// доминирующая стоимость загрузки больших проектов (NFR-PERF-01: 10k объектов < 2 с).
fn validator_cache() -> &'static std::sync::Mutex<std::collections::BTreeMap<&'static str, jsonschema::Validator>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<std::collections::BTreeMap<&'static str, jsonschema::Validator>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::BTreeMap::new()))
}

/// Проверить TOML-значение против JSON Schema манифеста. Возвращает список ошибок.
pub fn validate(kind: ManifestKind, value: &toml::Value) -> Vec<String> {
    let Ok(json) = serde_json::to_value(value) else {
        return vec!["манифест не конвертируется в JSON для валидации".to_owned()];
    };
    let mut cache = match validator_cache().lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    let schema_id = kind.schema_id();
    if !cache.contains_key(schema_id) {
        let schema = kind.schema();
        if schema.is_null() {
            return vec!["не удалось сгенерировать JSON Schema".to_owned()];
        }
        let Ok(validator) = jsonschema::validator_for(&schema) else {
            return vec!["JSON Schema манифеста не компилируется".to_owned()];
        };
        cache.insert(schema_id, validator);
    }
    let validator = cache.get(schema_id).expect("валидатор добавлен выше");
    validator.iter_errors(&json).map(|e| e.to_string()).collect()
}
