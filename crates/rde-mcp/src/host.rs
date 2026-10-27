//! Контракт хоста RDE: программный интерфейс интеграции MCP-слоя с проектом.
//!
//! Трейт [`RdeHost`] — **синхронный** и dyn-совместимый (`Arc<dyn RdeHost + Send + Sync>`):
//! реализации простые (см. `rde-host::HeadlessHost`), mock для тестов — тривиальный, а тяжёлые
//! вызовы async-слой сервера сам оборачивает в `tokio::task::spawn_blocking` (O2-a).
//!
//! В M1 контракт **только чтение** (мутирующие методы появятся в M2 через те же хуки
//! approvals/audit). Методы возвращают DTO из [`crate::model`] — доменные типы RDE здесь
//! запрещены (условие независимости слоя, ADR-0006/A2).

use std::sync::Arc;

use crate::error::HostError;
use crate::model::{
    ArtifactDetail, ArtifactInfo, DocInfo, DocOutline, DocReadRequest, DocText, FileChunk, FileRange,
    GitStatusInfo, NoteInfo, ObjectNode, SearchHit, SourceInfo, TemplateInfo, WorkspaceInfo,
};

/// Хост проекта для MCP-слоя: чтение модели проекта/документов/артефактов/источников.
///
/// Реализация владеет корнем проекта и сама отвечает за:
/// - канонизацию путей и песочницу внутри корня (A5), коды `RDE_PATH_ESCAPE`;
/// - стабильные тексты ошибок без путей вне проекта/секретов;
/// - лимиты размеров и `RDE_OUT_OF_RANGE` при превышении;
/// - «не запускает» ничего: M1 read-only, ledger читается, но артефакты не исполняются.
pub trait RdeHost: Send + Sync {
    /// Снимок рабочего пространства: id/имя/счётчики/trusted/версия (без корневого пути).
    fn workspace_snapshot(&self) -> Result<WorkspaceInfo, HostError>;

    /// Плоский список узлов дерева проекта (группа/вид/id/заголовок/подпись/свежесть).
    ///
    /// Больше [`crate::model::MAX_TREE_NODES`] узлов — `RDE_OUT_OF_RANGE` (лимит контекста).
    fn project_tree(&self) -> Result<Vec<ObjectNode>, HostError>;

    /// Поиск подстроки по id/заголовкам/полям манифестов; `limit` уже провалидирован вызывающим.
    fn search_project(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, HostError>;

    /// Прочитать файл проекта по относительному пути с диапазоном (пути — только внутри корня).
    fn read_project_file(&self, rel_path: &str, range: Option<FileRange>) -> Result<FileChunk, HostError>;

    /// Документы проекта (метаданные; без тел).
    fn doc_list(&self) -> Result<Vec<DocInfo>, HostError>;

    /// Оглавление документа по стабильному id (только `.doc.rde`; иначе `RDE_DOC_NOT_MIGRATED`).
    fn doc_outline(&self, doc_id: &str) -> Result<DocOutline, HostError>;

    /// Тело документа по диапазону блоков/списку block_id (реестр блоков, лимиты размера).
    fn doc_read(&self, doc_id: &str, request: DocReadRequest) -> Result<DocText, HostError>;

    /// Артефакты проекта со статусом из ledger (без запуска).
    fn artifact_list(&self) -> Result<Vec<ArtifactInfo>, HostError>;

    /// Детальная информация об артефакте: манифест + outputs/hash/статус из ledger.
    fn artifact_read(&self, id: &str) -> Result<ArtifactDetail, HostError>;

    /// Источники проекта.
    fn source_list(&self) -> Result<Vec<SourceInfo>, HostError>;

    /// Заметки проекта.
    fn note_list(&self) -> Result<Vec<NoteInfo>, HostError>;

    /// Каталог шаблонов оформления.
    fn template_list(&self) -> Result<Vec<TemplateInfo>, HostError>;

    /// Состояние git проекта (только чтение, `rde-vcs::status`).
    fn git_status(&self) -> Result<GitStatusInfo, HostError>;

    /// MCP-инструкции: карта проекта + политика безопасности + правила чтения (≤32 KiB).
    ///
    /// Текст обязан быть детерминированным при одинаковом проекте (без времени/случайности):
    /// он участвует в prompt-cache и кэшируется клиентом (R-AGT-08, S16).
    fn instructions(&self) -> Result<String, HostError>;
}

/// Разделяемый хост (сервер держит его в `Arc`; вызовы идут через `spawn_blocking`).
pub type SharedHost = Arc<dyn RdeHost + Send + Sync>;
