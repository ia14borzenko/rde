//! Контракт хоста RDE: программный интерфейс интеграции MCP-слоя с проектом.
//!
//! Трейт [`RdeHost`] — **синхронный** и dyn-совместимый (`Arc<dyn RdeHost + Send + Sync>`):
//! реализации простые (см. `rde-host::HeadlessHost`), mock для тестов — тривиальный, а тяжёлые
//! вызовы async-слой сервера сам оборачивает в `tokio::task::spawn_blocking` (O2-a).
//!
//! M1 — только чтение; M2 добавляет мутации (ADR-0007) **аддитивно**: новые методы объявлены с
//! default-реализацией (`Err(RDE_UNSUPPORTED)`), поэтому read-only/mock-реализации компилируются
//! без изменений, а реальные реализации в `rde-host` переопределяют нужные методы (O1-a).
//! Мутирующие методы вызываются только после `authorize_tool_call` (грант политики + approval);
//! сами методы создают чекпоинт (R-AGT-05) и пишут через библиотечные API RDE.
//!
//! Методы возвращают DTO из [`crate::model`] — доменные типы RDE здесь запрещены (условие
//! независимости слоя, ADR-0006/A2). Jobs запуска не переживают перезапуск процесса (M2).

use std::sync::Arc;

use crate::error::HostError;
use crate::model::{
    ArtifactDetail, ArtifactInfo, ArtifactManifestDto, DocInfo, DocOutline, DocReadRequest, DocText,
    FileChunk, FileRange, GitCommitOutcome, GitStatusInfo, JobInfo, JobLog, MutationOutcome, NoteInfo,
    ObjectNode, SearchHit, SourceInfo, TemplateInfo, WorkspaceInfo,
};

/// Хост проекта для MCP-слоя: чтение модели проекта/документов/артефактов/источников,
/// управляемые мутации (M2) и асинхронный запуск артефактов.
///
/// Реализация владеет корнем проекта и сама отвечает за:
/// - канонизацию путей и песочницу внутри корня (A5), коды `RDE_PATH_ESCAPE`;
/// - стабильные тексты ошибок без путей вне проекта/секретов;
/// - лимиты размеров и `RDE_OUT_OF_RANGE` при превышении;
/// - чекпоинты перед мутациями и авто-откат при ошибке (M2, R-AGT-05).
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
    ///
    /// Файл целиком в память не читается: без `range` — первые 32 KiB (файлы > 8 MiB →
    /// `RDE_OUT_OF_RANGE`), с `range` — только окно (`seek`/строки).
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
    /// он участвует в prompt-cache и кэшируется клиентом (R-AGT-08, S16). Текст фиксируется на
    /// старте сервера (`server/discover` имеет короткий TTL 5 с, но состав инструкций за сессию
    /// не меняется — документировано в ADR-0006/0007).
    fn instructions(&self) -> Result<String, HostError>;

    // -----------------------------------------------------------------------
    // M2: мутации (ADR-0007). Default — `RDE_UNSUPPORTED` (read-only совместимость, O1-a).
    // -----------------------------------------------------------------------

    /// Записать (патч) манифест артефакта `objects/artifacts/<id>/artifact.toml`.
    ///
    /// `manifest` — патч поверх текущего манифеста; `id`/`schema` подставляет хост.
    /// `expected_sha256` (если задан) — ожидаемый хеш текущего файла: расхождение → `RDE_CONFLICT`
    /// (внешняя правка не теряется, R-DOC-13-аналог). `dry_run` — только валидация.
    fn artifact_write_manifest(
        &self,
        _id: &str,
        _manifest: ArtifactManifestDto,
        _dry_run: bool,
        _expected_sha256: Option<&str>,
    ) -> Result<MutationOutcome, HostError> {
        Err(unsupported("запись манифеста"))
    }

    /// Записать файл скрипта артефакта (`runtime.entry`/`script_files`, только python/data).
    fn artifact_write_script(
        &self,
        _id: &str,
        _file: &str,
        _content: &str,
        _dry_run: bool,
        _expected_sha256: Option<&str>,
    ) -> Result<MutationOutcome, HostError> {
        Err(unsupported("запись скрипта артефакта"))
    }

    /// Записать содержимое (`note.md`) и/или заголовок (`note.toml`) заметки.
    fn note_write(
        &self,
        _id: &str,
        _content: Option<String>,
        _title: Option<String>,
        _dry_run: bool,
        _expected_sha256: Option<&str>,
    ) -> Result<MutationOutcome, HostError> {
        Err(unsupported("запись заметки"))
    }

    /// Запустить асинхронный job обновления артефактов (rde-dag update).
    ///
    /// `targets = None` — все устаревшие; `targets = []` трактуется как `None`. Возвращается
    /// сразу `JobInfo` (state `running`); прогресс — `run_status`/`run_log`. Требует `trusted`.
    fn artifact_run(
        &self,
        _targets: Option<Vec<String>>,
        _include_dependents: bool,
        _force: bool,
    ) -> Result<JobInfo, HostError> {
        Err(unsupported("запуск артефактов"))
    }

    /// Статус job'а запуска артефактов.
    fn run_status(&self, _job_id: &str) -> Result<JobInfo, HostError> {
        Err(unsupported("статус job'а"))
    }

    /// Запросить отмену job'а (флаг отмены внутри rde-run).
    fn run_cancel(&self, _job_id: &str) -> Result<JobInfo, HostError> {
        Err(unsupported("отмена job'а"))
    }

    /// Прочитать лог job'а с байтового смещения (лимит ответа проверен вызывающим).
    fn run_log(&self, _job_id: &str, _offset: u64, _max_bytes: usize) -> Result<JobLog, HostError> {
        Err(unsupported("лог job'а"))
    }

    /// Закоммитить проект в git (автор — по O7; сообщение/лимиты проверены вызывающим).
    fn git_commit(&self, _message: &str, _dry_run: bool) -> Result<GitCommitOutcome, HostError> {
        Err(unsupported("git-коммит"))
    }

    /// Откатить мутацию по чекпоинту (интерфейс хоста; MCP-инструменты `checkpoint_*` в M2 не
    /// публикуются — O6-a). Восстанавливает файлы и корни store; чекпоинт помечается откаченным.
    fn checkpoint_restore(&self, _checkpoint_id: &str) -> Result<MutationOutcome, HostError> {
        Err(unsupported("откат чекпоинта"))
    }

    /// Отменить активные jobs при shutdown сервера (graceful, A6). По умолчанию — ничего.
    fn shutdown_jobs(&self) {}
}

fn unsupported(what: &str) -> HostError {
    HostError::unsupported(format!("{what} не поддерживается этой реализацией хоста"))
        .with_hint("M2-мутации доступны в headless-хосте `rde mcp serve` (rde-host)")
}

/// Разделяемый хост (сервер держит его в `Arc`; вызовы идут через `spawn_blocking`).
pub type SharedHost = Arc<dyn RdeHost + Send + Sync>;
