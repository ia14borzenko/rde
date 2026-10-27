//! DTO контракта [`RdeHost`](crate::host::RdeHost) и протокольного слоя.
//!
//! Правило независимости слоя: через границу `rde-mcp` ходят **только эти типы** —
//! `Project`, `Store`, `RdeDoc`, WordCraft-`Document` и прочие доменные типы RDE сюда не
//! протекают. Все DTO реализуют `serde` + [`schemars::JsonSchema`], поэтому JSON-схемы
//! инструментов генерируются из них (`schemars`), а не пишутся руками (O3-a).
//!
//! Соглашения:
//! - идентификаторы — только стабильные (`id`, ULID, `out_hash`/`act_key` из ledger);
//! - пути в ответах — относительные, POSIX-разделители, канонизированы внутри корня проекта;
//! - тела документов/файлов — только по запрошенному диапазону с лимитом и флагом `truncated`;
//! - «сырые» доменные значения/infrastructure-пути не раскрываются (A2/A5).

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Лимит инструмента `project_tree`: больше узлов — `RDE_OUT_OF_RANGE` (A3).
pub const MAX_TREE_NODES: usize = 500;

/// Лимит блоков в `doc_outline` (ровно 500 — предел A3 для деревьев; для оглавления тот же).
pub const MAX_OUTLINE_BLOCKS: usize = 500;

/// Лимит инструмента `doc_read`: блоков за вызов по умолчанию.
pub const DOC_READ_DEFAULT_BLOCKS: usize = 32;

/// Лимит инструмента `doc_read`: блоков за вызов максимум.
pub const DOC_READ_MAX_BLOCKS: usize = 256;

/// Лимит текста, возвращаемого `doc_read` за вызов, в байтах UTF-8.
pub const DOC_READ_MAX_BYTES: usize = 256 * 1024;

/// Лимит `file_read` в байтах за вызов.
pub const FILE_READ_MAX_BYTES: usize = 256 * 1024;

/// Лимит `file_read` в строках за вызов.
pub const FILE_READ_MAX_LINES: usize = 2_000;

/// Лимит `project_search`: результатов на вызов по умолчанию.
pub const SEARCH_DEFAULT_LIMIT: usize = 20;

/// Лимит `project_search`: результатов на вызов максимум.
pub const SEARCH_MAX_LIMIT: usize = 100;

/// Снимок рабочего пространства (без корневого пути — A2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WorkspaceInfo {
    /// Идентификатор проекта (ULID).
    pub id: String,
    /// Отображаемое имя проекта.
    pub name: String,
    /// Версия схемы проекта (`rde/project@1`).
    pub version: String,
    /// Проект доверенный (пользователь разрешил запуск скриптов в проекте).
    pub trusted: bool,
    /// Всего объектов проекта.
    pub object_count: usize,
    /// Объектов по видам (`document`, `artifact`, `source`, `note`, `repository`, `formula`).
    pub object_counts_by_kind: BTreeMap<String, usize>,
    /// Объектов, устаревших/с ошибками.
    pub stale_count: usize,
}

/// Узел дерева проекта (плоский список; группа задаёт секцию).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ObjectNode {
    /// Группа/секция дерева (`Документы`, `Артефакты`, …).
    pub group: String,
    /// Машинный вид объекта: `document|artifact|source|note|repository|formula`.
    pub kind: String,
    /// Стабильный id объекта.
    pub id: String,
    /// Заголовок объекта.
    pub title: String,
    /// Короткая фактическая подпись (что это/сколько выходов).
    pub detail: String,
    /// Свежесть: `up_to_date|stale|failed|external`.
    pub freshness: String,
    /// Служебный объект (формула/константы) — в UI скрыт по умолчанию.
    pub service: bool,
}

/// Совпадение поиска по проекту.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SearchHit {
    /// Машинный вид объекта.
    pub kind: String,
    /// Id объекта.
    pub id: String,
    /// Заголовок объекта.
    pub title: String,
    /// Где найдено: `title|id|manifest`.
    pub field: String,
    /// Фрагмент вокруг совпадения (обрезан, без управляющих символов).
    pub snippet: String,
}

/// Диапазон чтения файла: байты или строки (ровно один режим за вызов).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FileRange {
    /// Байтовый диапазон: `{"bytes": {"offset": 0, "limit": 4096}}`.
    Bytes {
        /// Смещение от начала файла.
        offset: u64,
        /// Сколько байт вернуть.
        limit: u64,
    },
    /// Строковый диапазон: `{"lines": {"start": 1, "count": 40}}` (нумерация с 1).
    Lines {
        /// Первая строка (с 1).
        start: u64,
        /// Сколько строк вернуть.
        count: u64,
    },
}

/// Кусок файла проекта (ответ `file_read`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileChunk {
    /// Относительный путь внутри проекта (POSIX-разделители).
    pub path: String,
    /// Режим диапазона: `bytes|lines`.
    pub mode: String,
    /// Начало диапазона (байт или строка).
    pub start: u64,
    /// Конец диапазона (исключающая граница: байт или строка).
    pub end: u64,
    /// Полный размер файла в байтах.
    pub total_bytes: u64,
    /// Ответ обрезан лимитом (нужен следующий диапазон).
    pub truncated: bool,
    /// Текст (UTF-8; бинарные файлы не читаются).
    pub text: String,
}

/// Описание документа проекта.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocInfo {
    /// Стабильный id документа.
    pub id: String,
    /// Заголовок.
    pub title: String,
    /// Относительный путь файла документа.
    pub file: String,
    /// Документ — валидный `.doc.rde`-пакет (есть `rde:doc`).
    pub migrated: bool,
    /// Версия `rde:doc` (если мигрирован).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    /// Блоков в реестре (если мигрирован).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocks: Option<usize>,
    /// Живых вставок (если мигрирован).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inserts: Option<usize>,
    /// `body_hash` документа (sha256 `word/document.xml`; M2.5) — для optimistic-конкурентности
    /// (`doc_replace_range.expected_body_hash`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_hash: Option<String>,
}

/// Блок в оглавлении документа.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutlineBlock {
    /// Стабильный id блока (`b1`, `b2`, …).
    pub id: String,
    /// Вид: `heading1|heading2|heading|paragraph|figure|caption|table`.
    pub kind: String,
    /// Начало текста блока (обрезано).
    pub title: String,
    /// Длина текста блока в символах.
    pub chars: usize,
    /// Индекс блока в документе (с 0).
    pub index: usize,
}

/// Оглавление документа.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocOutline {
    /// Id документа.
    pub doc_id: String,
    /// Заголовок документа.
    pub title: String,
    /// Шаблон оформления (`gost-7.32-2017`, `free`, …).
    pub template: String,
    /// Версия `rde:doc`.
    pub schema_version: String,
    /// Всего блоков в документе.
    pub total_blocks: usize,
    /// Всего вставок в документе.
    pub total_inserts: usize,
    /// Оглавление обрезано лимитом [`MAX_OUTLINE_BLOCKS`].
    pub truncated: bool,
    /// Блоки в порядке документа.
    pub blocks: Vec<OutlineBlock>,
}

/// Запрос чтения тела документа.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocReadRequest {
    /// Читать только эти блоки (порядок — документный).
    #[serde(default)]
    pub block_ids: Option<Vec<String>>,
    /// Начальный индекс блока (с 0) при чтении диапазоном.
    #[serde(default)]
    pub start: Option<usize>,
    /// Сколько блоков вернуть (по умолчанию [`DOC_READ_DEFAULT_BLOCKS`]).
    #[serde(default)]
    pub limit: Option<usize>,
}

/// Блок с текстом (ответ `doc_read`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocTextBlock {
    /// Id блока.
    pub id: String,
    /// Вид блока.
    pub kind: String,
    /// Текст блока (полный или обрезанный лимитом).
    pub text: String,
}

/// Тело документа по диапазону.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocText {
    /// Id документа.
    pub doc_id: String,
    /// Всего блоков в документе.
    pub total_blocks: usize,
    /// Сколько блоков вернулось.
    pub returned_blocks: usize,
    /// Ответ обрезан лимитом блоков или байтов.
    pub truncated: bool,
    /// Текст документом (блоки через пустую строку).
    pub text: String,
    /// Блоки с текстом.
    pub blocks: Vec<DocTextBlock>,
}

/// Краткая информация об артефакте.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactInfo {
    /// Id артефакта.
    pub id: String,
    /// Заголовок.
    pub title: String,
    /// Тип из манифеста: `compute|data|constants`.
    pub artifact_type: String,
    /// Статус из ledger: `fresh` (out_hash манифеста = ledger), `stale` (расходится),
    /// `never_run` (записи нет), `unknown` (хранилище `.rde` недоступно).
    pub status: String,
    /// Причина статуса (человекочитаемая, без путей вне проекта).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_reason: Option<String>,
    /// `out_hash` последнего успешного обновления.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out_hash: Option<String>,
    /// `act_key` последней активации.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub act_key: Option<String>,
    /// Unix-время последнего успешного обновления.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_unix: Option<u64>,
    /// Идентификатор запуска последнего успешного обновления.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Число выходов в ledger.
    pub outputs: usize,
}

/// Выход артефакта из ledger.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactOutputInfo {
    /// Относительный путь внутри каталога артефакта.
    pub rel: String,
    /// Размер в байтах.
    pub size: u64,
    /// SHA-256 блоба (`out_hash`-составляющая).
    pub blob: String,
}

/// Runtime артефакта.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactRuntimeInfo {
    /// `python|shell|node|r`.
    pub kind: String,
    /// `project|system|artifact`.
    pub env: String,
    /// Относительный путь к исполняемому файлу.
    pub entry: String,
}

/// Детальная информация об артефакте (манифест + ledger; без запуска).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactDetail {
    /// Краткая информация/статус.
    pub info: ArtifactInfo,
    /// Политика пересборки: `manual|auto`.
    pub rebuild: String,
    /// Runtime (если задан).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ArtifactRuntimeInfo>,
    /// Параметры запуска (произвольный JSON из манифеста).
    #[serde(default)]
    pub params: serde_json::Value,
    /// Объявленные в манифесте выходы.
    #[serde(default)]
    pub outputs_declared: Vec<String>,
    /// Выходы из ledger (пусто, если не запускался).
    #[serde(default)]
    pub outputs: Vec<ArtifactOutputInfo>,
    /// Дополнительные файлы скрипта (входят в `script_hash`).
    #[serde(default)]
    pub script_files: Vec<String>,
    /// Таймаут запуска, секунды.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
    /// Разрешена ли сеть артефакту.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_allow: Option<bool>,
    /// Группы hashed-входов (произвольный JSON из манифеста).
    #[serde(default)]
    pub groups: serde_json::Value,
}

/// Источник проекта.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceInfo {
    /// Id источника.
    pub id: String,
    /// Заголовок/название.
    pub title: String,
    /// `file|url|repo`.
    pub kind: String,
    /// Относительный путь файла (для `file`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// URL (для `url`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Когда получен (ISO-8601 из манифеста, если есть).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieved_at: Option<String>,
    /// Библиографическая запись по ГОСТ (если есть reference).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bibliography: Option<String>,
}

/// Заметка проекта.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NoteInfo {
    /// Id заметки.
    pub id: String,
    /// Заголовок.
    pub title: String,
    /// Относительный путь файла заметки.
    pub file: String,
    /// Размер файла в байтах (если читается).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
}

/// Шаблон оформления.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemplateInfo {
    /// Id шаблона (`gost-7.32-2017`, `lab-report`, `free`).
    pub id: String,
    /// Название.
    pub title: String,
    /// Описание.
    pub description: String,
}

/// Коммит git.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitCommitInfo {
    /// Короткий хеш.
    pub id: String,
    /// Тема коммита.
    pub summary: String,
    /// Автор.
    pub author: String,
    /// Дата (как отдаёт git, `YYYY-MM-DD`).
    pub time: String,
}

/// Состояние git-репозитория проекта (только чтение).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitStatusInfo {
    /// git доступен.
    pub available: bool,
    /// Каталог проекта внутри git-репозитория.
    pub is_repo: bool,
    /// Ветка (`HEAD` в detached-состоянии); `null`, если проект не является git-репозиторием.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// Изменённые/удалённые файлы проекта.
    #[serde(default)]
    pub changed: Vec<String>,
    /// Неотслеживаемые файлы проекта.
    #[serde(default)]
    pub untracked: Vec<String>,
    /// Последний коммит.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_commit: Option<GitCommitInfo>,
    /// Сколько коммитов локально впереди upstream.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ahead: Option<u32>,
}

// ---------------------------------------------------------------------------
// M2: DTO мутаций, запуска артефактов и git-коммита (пересекают границу слоя)
// ---------------------------------------------------------------------------

/// Лимит `artifact_write_script`: размер содержимого файла скрипта (256 KiB).
pub const ARTIFACT_SCRIPT_MAX_BYTES: usize = 256 * 1024;

/// Лимит `note_write`: размер содержимого заметки (1 MiB).
pub const NOTE_WRITE_MAX_BYTES: usize = 1024 * 1024;

/// Лимит `run_log`: сколько байт лога отдавать за вызов по умолчанию.
pub const JOB_LOG_DEFAULT_BYTES: usize = 16 * 1024;

/// Лимит `run_log`: максимум байт лога за вызов.
pub const JOB_LOG_MAX_BYTES: usize = 64 * 1024;

/// Лимит `git_commit`: длина сообщения коммита в символах.
pub const GIT_COMMIT_MESSAGE_MAX_CHARS: usize = 2_000;

/// Runtime артефакта в DTO (патч манифеста).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRuntimeDto {
    /// `python` | `data` (shell/node/r политикой M2 запрещены: запуск — только python/данные).
    pub kind: String,
    /// Режим окружения: `project` (uv), `system`, `artifact`; по умолчанию `project`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<String>,
    /// Относительный путь к исполняемому файлу внутри каталога артефакта.
    pub entry: String,
    /// Объявленные переменные окружения.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vars: Option<BTreeMap<String, String>>,
}

/// Вход артефакта в DTO (совпадает по JSON-форме с `rde-core::InputSpec`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum ArtifactInputDto {
    /// Выход другого артефакта (`selector` — путь/glob).
    Artifact {
        /// Id артефакта-источника.
        artifact: String,
        /// Селектор выхода внутри каталога артефакта.
        #[serde(default)]
        selector: String,
    },
    /// Файл внутри каталога артефакта.
    File {
        /// Относительный путь файла.
        file: String,
    },
    /// Заметка проекта.
    Note {
        /// Id заметки.
        note: String,
    },
    /// Источник проекта.
    Source {
        /// Id источника.
        source: String,
    },
    /// Типизированные константы проекта.
    Constants {
        /// Id артефакта типа `constants`.
        constants: String,
        /// Имя конкретной величины (пусто — весь набор).
        #[serde(default)]
        selector: String,
    },
}

/// Группа зависимостей артефакта в DTO.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactGroupDto {
    /// Имя группы.
    pub name: String,
    /// `hashed` (влияет на хеш/каскад) или `ref`; по умолчанию `hashed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Входы группы.
    #[serde(default)]
    pub inputs: Vec<ArtifactInputDto>,
}

/// DTO манифеста артефакта: **патч** поверх текущего `artifact.toml` (поле `None` — не менять).
///
/// Агент присылает только изменяемые поля; `id`/`schema` подставляет хост, `out_hash` не
/// пишется вручную (его владелец — RDE после успешного обновления).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifestDto {
    /// Заголовок артефакта.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Тип: `compute` | `data` | `constants`.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Политика пересборки: `manual` | `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rebuild: Option<String>,
    /// Runtime артефакта (заменяется целиком).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<ArtifactRuntimeDto>,
    /// Произвольные параметры запуска.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// Объявленные выходы (относительные пути/glob внутри каталога артефакта).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<String>>,
    /// Дополнительные файлы скрипта (входят в `script_hash`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script_files: Option<Vec<String>>,
    /// Таймаут запуска, секунды.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_s: Option<u64>,
    /// Разрешена ли артефакту сеть (политику задаёт проект; по умолчанию запрещена).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network_allow: Option<bool>,
    /// Группы hashed/ref-входов (заменяются целиком).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<ArtifactGroupDto>>,
    /// Для `type = "constants"`: значения (`{имя: {value, unit, …}}`, форматы — `artifact_read`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<serde_json::Value>,
}

/// Результат мутирующего инструмента (запись манифеста/скрипта/заметки, откат чекпоинта).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MutationOutcome {
    /// Операция только проверена (`true`) — файлы не изменены.
    pub dry_run: bool,
    /// Id чекпоинта, созданного перед мутацией (`null` у `dry_run`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// Изменённые относительные пути внутри проекта (отсортированы).
    #[serde(default)]
    pub changed_files: Vec<String>,
    /// Человекочитаемая деталь (например, «заголовок заметки обновлён»).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Информация о job'е запуска артефактов (R-AGT-09: асинхронная модель).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobInfo {
    /// Идентификатор job'а (ULID; не переживает перезапуск процесса/M2).
    pub job_id: String,
    /// Состояние: `running` | `succeeded` | `failed` | `cancelled`.
    pub state: String,
    /// Unix-время создания.
    pub created_unix: u64,
    /// Unix-время завершения (для `running` — `null`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_unix: Option<u64>,
    /// Цели запуска (пусто — все устаревшие артефакты).
    #[serde(default)]
    pub targets: Vec<String>,
    /// Включены транзитивно зависимые артефакты.
    pub include_dependents: bool,
    /// Принудительный пересчёт целей.
    pub force: bool,
    /// Всего артефактов в отчёте.
    pub total: usize,
    /// Запущено.
    pub ran: usize,
    /// Пропущено (актуальны).
    pub skipped: usize,
    /// Ошибок/неудач.
    pub failed: usize,
    /// Ошибки (без путей вне проекта, кратко).
    #[serde(default)]
    pub errors: Vec<String>,
    /// Id последнего запуска (`rde-run`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    /// Относительный путь лога последнего артефакта.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_path: Option<String>,
    /// Чекпоинт, созданный перед запуском (для отката ledger-корней).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// Короткое сообщение/итог.
    pub message: String,
}

/// Кусок лога job'а (`run_log`: диапазонное чтение по байтовому смещению).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct JobLog {
    /// Идентификатор job'а.
    pub job_id: String,
    /// Запрошенное байтовое смещение.
    pub offset: u64,
    /// Смещение для следующего чтения.
    pub next_offset: u64,
    /// Достигнут конец файла лога.
    pub eof: bool,
    /// Ответ обрезан лимитом `max_bytes`.
    pub truncated: bool,
    /// Текст лога (UTF-8; невалидные байты заменяются).
    #[serde(default)]
    pub text: String,
}

/// Результат `git_commit`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GitCommitOutcome {
    /// Только проверка (`true`) — коммит не создавался.
    pub dry_run: bool,
    /// Чекпоинт перед коммитом (`null` у `dry_run`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// Созданный коммит (`null` у `dry_run`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub commit: Option<GitCommitInfo>,
    /// Файлов в коммите.
    #[serde(default)]
    pub files: usize,
    /// Короткое сообщение (при `dry_run` — план, что будет закоммичено).
    pub message: String,
}

// ---------------------------------------------------------------------------
// M2.5: suggested edits документа, правки блоков и ревизии (R-DOC-32/R-AGT-05)
// ---------------------------------------------------------------------------

/// Лимит текста блока в `doc_suggest_edit`/`doc_replace_range` (символы).
pub const DOC_EDIT_MAX_TEXT_CHARS: usize = 4_000;

/// Лимит `suggestion_list`/`revision_list`: записей по умолчанию.
pub const DOC_LIST_DEFAULT_LIMIT: usize = 20;

/// Лимит `suggestion_list`/`revision_list`: записей максимум.
pub const DOC_LIST_MAX_LIMIT: usize = 100;

/// Предложение правки документа (changeset, R-AGT-05 / R-DOC-32).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SuggestedEditInfo {
    /// Идентификатор предложения.
    pub id: String,
    /// Id документа.
    pub doc_id: String,
    /// Id блока документа (`b1`, …).
    pub block_id: String,
    /// `pending` | `accepted` | `rejected`.
    pub status: String,
    /// Исходный текст блока (на момент создания предложения).
    pub before: String,
    /// Предложенный текст блока.
    pub after: String,
    /// Автор предложения.
    pub author: String,
    /// `agent` | `human`.
    pub author_kind: String,
    /// Unix-время создания.
    pub created_unix: u64,
    /// `body_hash` документа на момент создания (устаревание предложения).
    pub base_body_hash: String,
    /// `w14:paraId` блока (диагностика).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub para_id: Option<String>,
    /// Комментарий автора.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Unix-время решения.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_unix: Option<u64>,
    /// Решение/причина (`accepted`/`rejected: …`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

/// Результат мутирующего инструмента документа (правка блока, применение предложения, откат ревизии).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocMutationOutcome {
    /// Только проверка (`true`) — документ не изменён.
    pub dry_run: bool,
    /// Чекпоинт перед правкой (`null` у `dry_run`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
    /// Изменённые относительные пути внутри проекта.
    #[serde(default)]
    pub changed_files: Vec<String>,
    /// Id документа.
    pub doc_id: String,
    /// Id блока (у откатов ревизии может отсутствовать).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub block_id: Option<String>,
    /// Текст блока до правки.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<String>,
    /// Текст блока после правки (фактический).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<String>,
    /// `body_hash` документа после правки без форматирования.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_hash: Option<String>,
    /// Транзакция-версия документа в store (для `revision_list`/`revision_restore`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision_id: Option<String>,
    /// Человекочитаемая деталь.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Версия документа в истории store (R-DOC-41): каждая мутация документа фиксирует корень
/// `document/<doc_id>` транзакцией.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DocRevisionInfo {
    /// Идентификатор ревизии (id транзакции store).
    pub revision_id: String,
    /// Id документа.
    pub doc_id: String,
    /// Unix-время фиксации.
    pub created_unix: u64,
    /// Пометка транзакции (кто/что меняло).
    pub note: String,
    /// Корневой tree-hash версии документа (sha256, hex).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root_hash: Option<String>,
    /// Совпадает с текущими корнями store (последняя версия).
    pub current: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_of<T: JsonSchema>() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(T)).unwrap()
    }

    #[test]
    fn dtos_roundtrip_through_json() {
        let chunk = FileChunk {
            path: "rde.toml".to_owned(),
            mode: "bytes".to_owned(),
            start: 0,
            end: 4,
            total_bytes: 10,
            truncated: true,
            text: "test".to_owned(),
        };
        let json = serde_json::to_value(&chunk).unwrap();
        let back: FileChunk = serde_json::from_value(json).unwrap();
        assert_eq!(back, chunk);
    }

    #[test]
    fn file_range_modes_have_distinct_json_shapes() {
        let bytes = serde_json::to_value(FileRange::Bytes { offset: 0, limit: 8 }).unwrap();
        assert_eq!(bytes, serde_json::json!({"bytes": {"offset": 0, "limit": 8}}));
        let lines = serde_json::to_value(FileRange::Lines { start: 1, count: 3 }).unwrap();
        assert_eq!(lines, serde_json::json!({"lines": {"start": 1, "count": 3}}));
    }

    #[test]
    fn dto_schemas_are_objects() {
        for value in [
            schema_of::<WorkspaceInfo>(),
            schema_of::<FileChunk>(),
            schema_of::<DocOutline>(),
            schema_of::<ArtifactDetail>(),
            schema_of::<GitStatusInfo>(),
        ] {
            assert!(value.is_object(), "схема DTO должна быть JSON-объектом: {value}");
        }
    }
}
