//! Домен suggested edits (M2.5, R-AGT-05 / R-DOC-32): предложения правок документа.
//!
//! Агент не переписывает пользовательский текст молча: он создаёт **changeset** — предложение
//! заменить текст блока (`block_id` + ожидаемый исходный текст + предложенный текст + автор +
//! базовый `body_hash` документа). Пользователь (UI) или явный MCP-инструмент
//! `suggestion_accept` применяет предложение; `suggestion_reject` — отклоняет. Применение идёт
//! через [`crate::replace_block_text`] с проверками «документ не менялся вне RDE» и
//! «текст блока не устарел» (R-DOC-13).
//!
//! Где хранится: `.rde/agent/suggestions/<ULID>.json` — локальное состояние проекта (как аудит и
//! чекпоинты), `.rde` в `.gitignore`; в `.doc.rde` предложения не пишутся (пакет остаётся чистым
//! OOXML, R-DOC-02/03). Статусы: `pending` → `accepted` | `rejected` (терминальные, повтор —
//! `Conflict`).

use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::text_edit::{replace_block_text, BlockTextEdit, BlockTextEditError};

/// Каталог предложений относительно корня проекта.
pub const SUGGESTIONS_DIR: &str = ".rde/agent/suggestions";

/// Статус «ожидает решения».
pub const STATUS_PENDING: &str = "pending";
/// Статус «принято» (правка применена к документу).
pub const STATUS_ACCEPTED: &str = "accepted";
/// Статус «отклонено».
pub const STATUS_REJECTED: &str = "rejected";

/// Предложение правки документа (changeset).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuggestedEdit {
    /// Идентификатор предложения (время+счётчик, лексикографически сортируемый).
    pub id: String,
    /// Стабильный id документа.
    pub doc_id: String,
    /// Стабильный id блока (`b1`, `b2`, …).
    pub block_id: String,
    /// `w14:paraId` блока на момент создания (диагностика).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub para_id: Option<String>,
    /// Исходный текст блока (обязателен для применения — защита от устаревания, R-DOC-13).
    pub before: String,
    /// Предложенный текст блока.
    pub after: String,
    /// Автор предложения (имя/идентификатор; без секретов).
    pub author: String,
    /// `agent` | `human` (R-DOC-31).
    pub author_kind: String,
    /// Unix-время создания.
    pub created_unix: u64,
    /// `pending` | `accepted` | `rejected`.
    pub status: String,
    /// `body_hash` документа на момент создания: если документ изменился — применение отклонится
    /// (`Conflict`, предложение устарело).
    pub base_body_hash: String,
    /// Комментарий автора предложения (необязательный).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Unix-время решения (accept/reject).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_unix: Option<u64>,
    /// Причина решения (`accepted`/`rejected` + текст причины для reject).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolution: Option<String>,
}

/// Параметры создания предложения.
#[derive(Clone, Debug)]
pub struct NewSuggestion {
    /// Id документа.
    pub doc_id: String,
    /// Id блока.
    pub block_id: String,
    /// `w14:paraId` блока (если известен).
    pub para_id: Option<String>,
    /// Ожидаемый текущий текст блока.
    pub before: String,
    /// Предложенный текст блока.
    pub after: String,
    /// Автор.
    pub author: String,
    /// `agent` | `human`.
    pub author_kind: String,
    /// Базовый `body_hash` документа.
    pub base_body_hash: String,
    /// Комментарий.
    pub note: Option<String>,
}

/// Ошибка домена предложений.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SuggestionError {
    /// Предложение не найдено.
    NotFound(String),
    /// Недопустимое состояние: уже обработано, предложение устарело, документ изменился.
    Conflict(String),
    /// Некорректные данные предложения.
    Invalid(String),
    /// Ошибка ФС/сериализации.
    Io(String),
}

impl fmt::Display for SuggestionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SuggestionError::NotFound(message) => write!(f, "{message}"),
            SuggestionError::Conflict(message) => write!(f, "{message}"),
            SuggestionError::Invalid(message) => write!(f, "{message}"),
            SuggestionError::Io(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for SuggestionError {}

impl From<BlockTextEditError> for SuggestionError {
    fn from(error: BlockTextEditError) -> Self {
        match error {
            BlockTextEditError::NotFound(message) => SuggestionError::NotFound(message),
            BlockTextEditError::Unsupported(message) => SuggestionError::Invalid(message),
            BlockTextEditError::Stale { .. } => SuggestionError::Conflict(error.to_string()),
            BlockTextEditError::Conflict(message) => SuggestionError::Conflict(message),
            BlockTextEditError::Internal(message) => SuggestionError::Io(message),
        }
    }
}

/// Хранилище предложений проекта (`.rde/agent/suggestions`).
#[derive(Clone, Debug)]
pub struct Suggestions {
    dir: PathBuf,
    root: PathBuf,
}

impl Suggestions {
    /// Хранилище для корня проекта.
    pub fn new(project_root: &Path) -> Self {
        Self {
            dir: project_root.join(SUGGESTIONS_DIR),
            root: project_root.to_path_buf(),
        }
    }

    /// Корень проекта.
    pub fn project_root(&self) -> &Path {
        &self.root
    }

    /// Каталог предложений (может ещё не существовать).
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Создать предложение (status = `pending`).
    ///
    /// # Errors
    ///
    /// [`SuggestionError::Invalid`] — пустые/некорректные поля; [`SuggestionError::Io`] — ФС.
    pub fn create(&self, params: NewSuggestion) -> Result<SuggestedEdit, SuggestionError> {
        if params.doc_id.trim().is_empty() || params.block_id.trim().is_empty() {
            return Err(SuggestionError::Invalid("нужны doc_id и block_id".to_owned()));
        }
        if params.before.trim().is_empty() {
            return Err(SuggestionError::Invalid(
                "before (исходный текст блока) не может быть пустым".to_owned(),
            ));
        }
        if params.after.trim().is_empty() {
            return Err(SuggestionError::Invalid("after не может быть пустым".to_owned()));
        }
        if params.after == params.before {
            return Err(SuggestionError::Invalid(
                "предложение не меняет текст блока (after == before)".to_owned(),
            ));
        }
        if params.base_body_hash.trim().is_empty() {
            return Err(SuggestionError::Invalid(
                "base_body_hash обязателен (защита от устаревания)".to_owned(),
            ));
        }
        fs::create_dir_all(&self.dir)
            .map_err(|error| SuggestionError::Io(format!("создать каталог предложений: {error}")))?;
        let edit = SuggestedEdit {
            id: self.next_id(),
            doc_id: params.doc_id,
            block_id: params.block_id,
            para_id: params.para_id,
            before: params.before,
            after: params.after,
            author: if params.author.trim().is_empty() { "agent".to_owned() } else { params.author },
            author_kind: if params.author_kind.trim().is_empty() { "agent".to_owned() } else { params.author_kind },
            created_unix: now_unix(),
            status: STATUS_PENDING.to_owned(),
            base_body_hash: params.base_body_hash,
            note: params.note,
            resolved_unix: None,
            resolution: None,
        };
        self.save(&edit)?;
        Ok(edit)
    }

    /// Загрузить предложение по id.
    pub fn load(&self, id: &str) -> Result<SuggestedEdit, SuggestionError> {
        if id.is_empty() || id.contains('/') || id.contains("..") {
            return Err(SuggestionError::Invalid("некорректный suggestion_id".to_owned()));
        }
        let path = self.dir.join(format!("{id}.json"));
        let text = fs::read_to_string(&path)
            .map_err(|error| SuggestionError::NotFound(format!("предложение `{id}` не найдено: {error}")))?;
        serde_json::from_str(&text)
            .map_err(|error| SuggestionError::Io(format!("предложение `{id}`: {error}")))
    }

    /// Список предложений (новые первыми), фильтры `doc_id`/`status` — необязательные.
    pub fn list(
        &self,
        doc_id: Option<&str>,
        status: Option<&str>,
    ) -> Result<Vec<SuggestedEdit>, SuggestionError> {
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(SuggestionError::Io(format!("каталог предложений: {error}"))),
        };
        let mut edits = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = fs::read_to_string(&path) else { continue };
            let Ok(edit) = serde_json::from_str::<SuggestedEdit>(&text) else { continue };
            if doc_id.is_some_and(|doc_id| edit.doc_id != doc_id) {
                continue;
            }
            if status.is_some_and(|status| edit.status != status) {
                continue;
            }
            edits.push(edit);
        }
        edits.sort_by(|left, right| {
            right.created_unix.cmp(&left.created_unix).then_with(|| right.id.cmp(&left.id))
        });
        Ok(edits)
    }

    /// Отклонить предложение (`pending` → `rejected`).
    pub fn reject(&self, id: &str, reason: Option<&str>) -> Result<SuggestedEdit, SuggestionError> {
        let mut edit = self.load(id)?;
        self.ensure_pending(&edit)?;
        edit.status = STATUS_REJECTED.to_owned();
        edit.resolved_unix = Some(now_unix());
        edit.resolution = Some(match reason {
            Some(reason) if !reason.trim().is_empty() => format!("rejected: {}", reason.trim()),
            _ => "rejected".to_owned(),
        });
        self.save(&edit)?;
        Ok(edit)
    }

    /// Применить предложение к документу (`pending` → `accepted`).
    ///
    /// Проверки: документ существует и не менялся после создания предложения (`base_body_hash`),
    /// текст блока совпадает с `before`. Правка — [`replace_block_text`].
    ///
    /// # Errors
    ///
    /// [`SuggestionError::Conflict`] — предложение уже обработано или устарело;
    /// [`SuggestionError::Invalid`] — блок нельзя править; [`SuggestionError::Io`] — ФС/пакет.
    pub fn apply(&self, id: &str, package: &Path) -> Result<SuggestedEdit, SuggestionError> {
        let mut edit = self.load(id)?;
        self.ensure_pending(&edit)?;
        if !package.is_file() {
            return Err(SuggestionError::NotFound(format!(
                "пакет документа не найден: {}",
                package.display()
            )));
        }
        let check = crate::check_external_edit(package)
            .map_err(|error| SuggestionError::Io(format!("проверить документ: {error}")))?;
        if !check.current_body_hash.is_empty() && check.current_body_hash != edit.base_body_hash {
            return Err(SuggestionError::Conflict(
                "документ изменён после создания предложения (body_hash не совпадает) — предложение устарело, создайте новое"
                    .to_owned(),
            ));
        }
        let report = replace_block_text(
            package,
            &BlockTextEdit {
                block_id: edit.block_id.clone(),
                expected_text: edit.before.clone(),
                new_text: edit.after.clone(),
            },
        )?;
        // Фиксируем фактический текст (WordCraft может нормализовать пробелы).
        edit.after = report.after.clone();
        edit.status = STATUS_ACCEPTED.to_owned();
        edit.resolved_unix = Some(now_unix());
        edit.resolution = Some("accepted".to_owned());
        self.save(&edit)?;
        Ok(edit)
    }

    /// Массово отклонить все `pending` предложения документа (UI «отклонить всё»); возвращает число.
    pub fn reject_all(&self, doc_id: Option<&str>, reason: Option<&str>) -> Result<usize, SuggestionError> {
        let pending = self.list(doc_id, Some(STATUS_PENDING))?;
        let mut count = 0;
        for edit in pending {
            self.reject(&edit.id, reason)?;
            count += 1;
        }
        Ok(count)
    }

    fn ensure_pending(&self, edit: &SuggestedEdit) -> Result<(), SuggestionError> {
        if edit.status != STATUS_PENDING {
            return Err(SuggestionError::Conflict(format!(
                "предложение `{}` уже обработано (status={})",
                edit.id, edit.status
            )));
        }
        Ok(())
    }

    fn save(&self, edit: &SuggestedEdit) -> Result<(), SuggestionError> {
        fs::create_dir_all(&self.dir)
            .map_err(|error| SuggestionError::Io(format!("создать каталог предложений: {error}")))?;
        let path = self.dir.join(format!("{}.json", edit.id));
        let bytes = serde_json::to_vec_pretty(edit)
            .map_err(|error| SuggestionError::Io(format!("JSON предложения: {error}")))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, &bytes)
            .map_err(|error| SuggestionError::Io(format!("записать предложение: {error}")))?;
        fs::rename(&tmp, &path)
            .map_err(|error| SuggestionError::Io(format!("переименовать предложение: {error}")))
    }

    /// Идентификатор: время в наносекундах (hex), при совпадении — счётчик (детерминированный порядок).
    fn next_id(&self) -> String {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let mut candidate = format!("{nanos:032x}");
        let mut counter = 0u32;
        while self.dir.join(format!("{candidate}.json")).exists() {
            counter += 1;
            candidate = format!("{:032x}", nanos.wrapping_add(counter as u128));
        }
        candidate
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}
