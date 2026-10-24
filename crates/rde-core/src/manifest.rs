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
}

/// Окружение исполнения скрипта артефакта.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct RuntimeManifest {
    /// `python` | `shell` | `node` | `r`.
    pub kind: String,
    #[serde(default = "default_env")]
    pub env: String,
    /// Относительный путь к исполняемому файлу артефакта.
    pub entry: String,
}

/// Вход артефакта: ссылка на выход другого артефакта, файл, заметка или источник.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
#[serde(untagged)]
pub enum InputSpec {
    Artifact {
        artifact: String,
        #[serde(default)]
        selector: String,
    },
    File {
        file: String,
    },
    Note {
        note: String,
    },
    Source {
        source: String,
    },
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

/// Значение константы проекта (тип `constants`).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug)]
pub struct ConstantValue {
    pub value: f64,
    #[serde(default)]
    pub unit: String,
    #[serde(default)]
    pub display: String,
    /// Подсказка форматирования (например, `italic-subscript` для `R_н`).
    #[serde(default)]
    pub style: String,
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
    /// Произвольные параметры запуска (входят в `params_hash` в M2.3).
    #[serde(default)]
    pub params: serde_json::Value,
    #[serde(default)]
    pub outputs: Vec<String>,
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
}

impl ManifestKind {
    pub const ALL_OBJECT_KINDS: [ManifestKind; 5] = [
        ManifestKind::Document,
        ManifestKind::Artifact,
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
        }
    }
}

fn schema_json<T: JsonSchema>() -> serde_json::Value {
    let schema = schemars::schema_for!(T);
    serde_json::to_value(schema).unwrap_or(serde_json::Value::Null)
}

/// Проверить TOML-значение против JSON Schema манифеста. Возвращает список ошибок.
pub fn validate(kind: ManifestKind, value: &toml::Value) -> Vec<String> {
    let schema = kind.schema();
    if schema.is_null() {
        return vec!["не удалось сгенерировать JSON Schema".to_owned()];
    }
    let Ok(json) = serde_json::to_value(value) else {
        return vec!["манифест не конвертируется в JSON для валидации".to_owned()];
    };
    let Ok(validator) = jsonschema::validator_for(&schema) else {
        return vec!["JSON Schema манифеста не компилируется".to_owned()];
    };
    validator.iter_errors(&json).map(|e| e.to_string()).collect()
}
