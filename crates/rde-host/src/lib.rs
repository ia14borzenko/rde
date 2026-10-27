//! RDE host — headless-реализация контракта MCP-слоя (`rde-mcp::RdeHost`) поверх библиотечных
//! крейтов RDE (v0.3, вехи M1–M2).
//!
//! Слой `rde-host` — единственное место, которое знает и RDE-библиотеки
//! (`rde-core`/`rde-dag`/`rde-doc`/`rde-store`/`rde-run`/`rde-vcs`/`rde-templates`), и протокольный
//! контракт `rde-mcp`. GUI (`rde-ui`/`egui`/`eframe`) здесь не участвует: стрелка зависимостей
//! `apps/rde → rde-host → rde-mcp`, `rde-mcp → { }`. Поэтому один и тот же хост можно
//! использовать из CLI/ACP/будущих headless-режимов, а протокольный слой тестировать на mock.
//!
//! M1 — только чтение проекта/документов/артефактов/источников, ledger и git.
//! M2 (ADR-0007) — **управляемые мутации через те же хуки**: запись манифеста/скрипта/заметки,
//! git-коммит (с чекпоинтами [`checkpoints`], авто-откатом при ошибке и `RDE_CONFLICT` при
//! внешней правке) и асинхронная job-модель запуска артефактов ([`jobs`]: `artifact_run` →
//! `run_status`/`run_cancel`/`run_log`). Сами мутации сюда приходят только после
//! `authorize_tool_call` (грант политики + approval) — в `rde-mcp`.
//!
//! Безопасность (A5/A7):
//! - все пути — через [`PathSandbox`]: канонизация, `starts_with(корень)`, отказ `..`/абсолютным/
//!   симлинкам наружу, `.rde/**` закрыт (`RDE_PATH_ESCAPE`/`RDE_APPROVAL_DENIED`);
//! - запись — только [`PathSandbox::resolve_for_write`] и только разрешённые цели инструмента
//!   (манифест/файлы скрипта/note.md/note.toml; общего `file_write` нет);
//! - тексты ошибок санитизируются (никаких путей вне проекта/секретов);
//! - лимиты размеров ответов и `RDE_DOC_NOT_MIGRATED` для непреобразованных документов.

pub mod checkpoints;
pub mod instructions;
pub mod jobs;
pub mod sandbox;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rde_core::{Freshness, Object, ObjectData, ObjectKind, Project};
use rde_dag::ArtifactState;
use rde_doc::{RdeBlock, RdeDoc};
use rde_mcp::model::{
    ArtifactDetail, ArtifactInfo, ArtifactManifestDto, ArtifactOutputInfo, ArtifactRuntimeInfo,
    DocInfo, DocOutline, DocReadRequest, DocText, DocTextBlock, FileChunk, FileRange, GitCommitInfo,
    GitCommitOutcome, GitStatusInfo, JobInfo, JobLog, MutationOutcome, NoteInfo, ObjectNode, OutlineBlock,
    SearchHit, SourceInfo, TemplateInfo, WorkspaceInfo, DOC_READ_DEFAULT_BLOCKS, DOC_READ_MAX_BLOCKS,
    DOC_READ_MAX_BYTES, FILE_READ_MAX_BYTES, FILE_READ_MAX_LINES, MAX_OUTLINE_BLOCKS, MAX_TREE_NODES,
    SEARCH_MAX_LIMIT,
};
use rde_mcp::{HostError, RdeHost};
use rde_store::{RootSet, Store};

pub use checkpoints::{Checkpoint, CheckpointManifest, CheckpointStore, CHECKPOINT_KEEP_LAST, CHECKPOINT_MAX_BYTES};
pub use jobs::{ArtifactExecutor, DagExecutor, ExecOutcome, ExecRequest, JobRegistry};
pub use sandbox::PathSandbox;

/// Лимит `file_read` по умолчанию (байт) — не заставляем модель просить «первые N» вручную.
const FILE_READ_DEFAULT_BYTES: u64 = 32 * 1024;

/// Предел чтения файла без `range` (M1-review: файл целиком в память не читается — OOM на PDF).
const FILE_READ_WHOLE_LIMIT: u64 = 8 * 1024 * 1024;

/// Сколько символов заголовка блока показывать в оглавлении.
const OUTLINE_TITLE_CHARS: usize = 80;

/// Длина фрагмента в результатах поиска.
const SEARCH_SNIPPET_CHARS: usize = 120;

/// Headless-хост проекта RDE: реализация [`RdeHost`] без GUI.
pub struct HeadlessHost {
    root: PathBuf,
    sandbox: PathSandbox,
    /// Лениво открываемое хранилище `.rde` (read-only операции; lock держится на процесс).
    store: Mutex<Option<Store>>,
    /// Чекпоинты мутаций (`.rde/agent/checkpoints`, R-AGT-05).
    checkpoints: CheckpointStore,
    /// Реестр job'ов запуска артефактов (R-AGT-09; один активный job).
    jobs: Arc<JobRegistry>,
    /// Исполнитель обновления артефактов (реальный `rde-dag`; в тестах — stub).
    executor: Arc<dyn ArtifactExecutor>,
}

impl HeadlessHost {
    /// Открыть проект: канонизировать корень, проверить, что проект загружается.
    ///
    /// # Errors
    ///
    /// `RDE_NOT_FOUND` — каталог не найден/не каталог; `RDE_NOT_READY` — проект не грузится
    /// (нет `rde.toml`, битые манифесты). Корень наружу не раскрывается.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, HostError> {
        Self::open_with_executor(root, Arc::new(DagExecutor))
    }

    /// Открыть проект с явным исполнителем job'ов (unit-тесты state machine без `uv`/python).
    pub fn open_with_executor(
        root: impl AsRef<Path>,
        executor: Arc<dyn ArtifactExecutor>,
    ) -> Result<Self, HostError> {
        let requested = root.as_ref();
        let canonical = fs::canonicalize(requested).map_err(|error| {
            HostError::not_found(format!("каталог проекта не найден: {error}")).with_hint("укажите --project <dir> или RDE_PROJECT")
        })?;
        if !canonical.is_dir() {
            return Err(HostError::not_found("путь проекта не является каталогом"));
        }
        rde_core::load_project(&canonical).map_err(|error| {
            HostError::not_ready(format!("проект не загружается: {}", sanitize_path(&canonical, &error)))
                .with_hint("проверьте rde.toml и objects/**")
        })?;
        Ok(Self {
            sandbox: PathSandbox::new(canonical.clone()),
            checkpoints: CheckpointStore::new(&canonical),
            root: canonical,
            store: Mutex::new(None),
            jobs: Arc::new(JobRegistry::new()),
            executor,
        })
    }

    /// Канонический корень проекта (внутреннее; наружу — только id).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Корни store до операции (для чекпоинта). `RDE_NOT_READY`, если `.rde` недоступен —
    /// мутация без возможности откката не выполняется (fail-closed, A5).
    pub fn store_roots(&self) -> Result<RootSet, HostError> {
        self.with_store(|store| {
            store
                .current_roots()
                .map_err(|error| HostError::not_ready(self.sanitize(format!("хранилище .rde недоступно: {error}"))))
        })
    }

    /// Перезагрузить проект с диска (файлы — источник истины; кэша модели нет).
    fn project(&self) -> Result<Project, HostError> {
        rde_core::load_project(&self.root)
            .map_err(|error| HostError::not_ready(self.sanitize(format!("проект не загружается: {error}"))))
    }

    /// Санитизация сообщения: корень проекта → `<project>` (A5: без путей наружу).
    fn sanitize(&self, message: impl Into<String>) -> String {
        sanitize_path(&self.root, &message.into())
    }

    /// Выполнить чтение из store (ленивое открытие `.rde`; ошибка — `RDE_NOT_READY`).
    fn with_store<T>(&self, task: impl FnOnce(&Store) -> Result<T, HostError>) -> Result<T, HostError> {
        let mut guard = self
            .store
            .lock()
            .map_err(|_| HostError::internal("блокировка хранилища отравлена (panic в другом вызове)"))?;
        if guard.is_none() {
            let store = Store::open(&self.root)
                .map_err(|error| HostError::not_ready(self.sanitize(format!("хранилище .rde недоступно: {error}"))))?;
            *guard = Some(store);
        }
        let store = guard
            .as_ref()
            .ok_or_else(|| HostError::internal("хранилище .rde недоступно после открытия (внутренняя ошибка)"))?;
        task(store)
    }

    /// Ledger артефактов из `.rde` (без `evaluate`: M1 не пересчитывает act_key/env и не
    /// запускает окружения — «статус из ledger» R-MCP-03; `None` — `.rde` недоступен).
    fn artifact_ledger(&self) -> Option<HashMap<String, ArtifactState>> {
        self.with_store(|store| {
            rde_dag::ledger(store).map_err(|error| HostError::internal(self.sanitize(format!("ledger: {error}"))))
        })
        .ok()
    }

    /// SHA-256 полезной нагрузки мутации (для манифеста чекпоинта; тела/файлы не пишутся).
    fn payload_sha256<T: serde::Serialize>(payload: &T) -> String {
        let bytes = serde_json::to_vec(payload).unwrap_or_default();
        rde_mcp::sha256_hex(&bytes)
    }

    /// Проверить ожидаемый SHA-256 текущего файла: ручная правка вне RDE не теряется
    /// (`RDE_CONFLICT`; аналог R-DOC-13 для файлов проекта, A5).
    fn check_expected_file_sha(
        &self,
        path: &Path,
        rel_path: &str,
        expected: Option<&str>,
    ) -> Result<(), HostError> {
        let Some(expected) = expected else { return Ok(()) };
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Err(HostError::conflict(format!(
                    "файл `{rel_path}` не существует, а expected_sha256 задан"
                ))
                .with_hint("уберите expected_sha256 или прочитайте файл заново"));
            }
            Err(error) => {
                return Err(HostError::not_ready(self.sanitize(format!("файл не читается: {error}"))));
            }
        };
        let actual = rde_hash::sha256(&bytes).hex();
        if actual != expected {
            return Err(HostError::conflict(format!(
                "файл `{rel_path}` изменён вне RDE (sha256 не совпадает с expected_sha256)"
            ))
            .with_hint("перечитайте файл инструментом чтения и повторите с актуальным expected_sha256"));
        }
        Ok(())
    }

    /// Выполнить мутацию через чекпоинт: зафиксировать корни store + копии файлов, выполнить
    /// операцию; на ошибке — авто-откат файлов и корней (чекпоинт помечается `rolled_back`).
    ///
    /// Возвращает `(результат операции, checkpoint_id)`.
    fn with_checkpoint<T>(
        &self,
        tool: &str,
        args_sha256: &str,
        files: Vec<(String, PathBuf)>,
        note: Option<String>,
        operation: impl FnOnce(&Self) -> Result<T, HostError>,
    ) -> Result<(T, String), HostError> {
        // Без доступного store откатить ledger-корни нельзя → мутация не выполняется (fail-closed).
        let roots = self.store_roots()?;
        let checkpoint = self.checkpoints.create(tool, args_sha256, Some(roots), &files, note)?;
        match operation(self) {
            Ok(value) => {
                self.checkpoints.prune(CHECKPOINT_KEEP_LAST, CHECKPOINT_MAX_BYTES);
                Ok((value, checkpoint.id))
            }
            Err(error) => {
                let rollback = (|| -> Result<(), HostError> {
                    self.checkpoints.rollback_files(&checkpoint)?;
                    self.checkpoints.rollback_roots(&checkpoint)?;
                    Ok(())
                })();
                let _ = self.checkpoints.mark_rolled_back(&checkpoint);
                let hint = match rollback {
                    Ok(()) => "изменения откачены из чекпоинта (авто-rollback, R-AGT-05)".to_owned(),
                    Err(rollback_error) => format!(
                        "авто-откат не удался ({rollback_error}); файлы могли остаться изменёнными"
                    ),
                };
                Err(error.with_hint(hint))
            }
        }
    }

    /// Записать файл проекта атомарно (родительские каталоги создаются).
    fn write_project_file(&self, path: &Path, bytes: &[u8]) -> Result<(), HostError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| HostError::internal(self.sanitize(format!("каталог записи: {error}"))))?;
        }
        write_atomic(path, bytes)
            .map_err(|error| HostError::internal(self.sanitize(format!("запись файла: {error}"))))
    }

    /// Объект заметки по стабильному id.
    fn note_object<'a>(project: &'a Project, note_id: &str) -> Result<&'a Object, HostError> {
        project
            .objects
            .iter()
            .find(|object| matches!(&object.data, ObjectData::Note(note) if note.id == note_id))
            .ok_or_else(|| HostError::not_found(format!("заметка `{note_id}` не найдена")).with_hint("список — note_list"))
    }

    /// Относительный POSIX-путь абсолютного пути объекта (проверка песочницы входа).
    fn object_rel_path(&self, absolute: &Path) -> Result<String, HostError> {
        rel_from_root(&self.root, absolute)
    }

    /// Абсолютный путь документа с проверкой песочницы.
    fn document_path(&self, object: &Object) -> Result<PathBuf, HostError> {
        let ObjectData::Document(document) = &object.data else {
            return Err(HostError::not_found("объект не является документом"));
        };
        let absolute = object.dir.join(&document.file);
        let relative = rel_from_root(&self.root, &absolute)?;
        self.sandbox.resolve(&relative)
    }

    /// Загрузить реестр `rde:doc`; документ обязан быть `.doc.rde`-пакетом (без авто-миграции).
    fn load_registry(&self, path: &Path) -> Result<RdeDoc, HostError> {
        if !rde_doc::is_rde_package(path) {
            return Err(self.doc_not_migrated());
        }
        rde_doc::load_rde_doc(path)
            .map_err(|error| self.doc_not_migrated_with(format!("документ не читается как .doc.rde: {error}")))
    }

    fn doc_not_migrated(&self) -> HostError {
        self.doc_not_migrated_with("документ ещё не в формате .doc.rde")
    }

    fn doc_not_migrated_with(&self, message: impl Into<String>) -> HostError {
        HostError::doc_not_migrated(self.sanitize(message)).with_hint(
            "M1 не мигрирует автоматически: сохраните проект в RDE (документ станет .doc.rde) и повторите",
        )
    }

    /// Блоки документа с полным текстом (реестр `rde:doc` — fallback, если модель не открылась).
    fn document_blocks(&self, path: &Path, registry: &RdeDoc) -> Vec<RdeBlock> {
        match rde_doc::document_blocks(path) {
            Ok(blocks) => blocks,
            Err(error) => {
                rde_mcp::log_debug(self.sanitize(format!("блоки из модели недоступны, реестр: {error}")));
                registry.blocks.clone()
            }
        }
    }

    fn doc_object<'a>(project: &'a Project, doc_id: &str) -> Result<&'a Object, HostError> {
        project
            .objects
            .iter()
            .find(|object| matches!(&object.data, ObjectData::Document(document) if document.id == doc_id))
            .ok_or_else(|| HostError::not_found(format!("документ `{doc_id}` не найден")).with_hint("список — doc_list"))
    }

    fn artifact_object<'a>(project: &'a Project, artifact_id: &str) -> Result<&'a Object, HostError> {
        project
            .objects
            .iter()
            .find(|object| matches!(&object.data, ObjectData::Artifact(artifact) if artifact.id == artifact_id))
            .ok_or_else(|| {
                HostError::not_found(format!("артефакт `{artifact_id}` не найден")).with_hint("список — artifact_list")
            })
    }
}

/// Реализация контракта MCP-слоя (только чтение).
impl RdeHost for HeadlessHost {
    fn workspace_snapshot(&self) -> Result<WorkspaceInfo, HostError> {
        let project = self.project()?;
        let mut counts: BTreeMap<String, usize> = BTreeMap::new();
        for object in &project.objects {
            *counts.entry(kind_str(object.kind).to_owned()).or_insert(0) += 1;
        }
        Ok(WorkspaceInfo {
            id: project.manifest.id.clone(),
            name: project.name.clone(),
            version: project.manifest.schema.clone(),
            trusted: project.manifest.trusted,
            object_count: project.object_count(),
            object_counts_by_kind: counts,
            stale_count: project.stale_count(),
        })
    }

    fn project_tree(&self) -> Result<Vec<ObjectNode>, HostError> {
        let project = self.project()?;
        if project.objects.len() > MAX_TREE_NODES {
            return Err(HostError::out_of_range(format!(
                "в проекте {} объектов — больше лимита дерева {MAX_TREE_NODES}",
                project.objects.len()
            ))
            .with_hint("используйте project_search или группы по видам (документы/артефакты/…)"));
        }
        Ok(project
            .objects
            .iter()
            .map(|object| ObjectNode {
                group: object.kind.label_ru().to_owned(),
                kind: kind_str(object.kind).to_owned(),
                id: object.id.clone(),
                title: object.title.clone(),
                detail: object.detail.clone(),
                freshness: freshness_str(object.freshness).to_owned(),
                service: object.kind.is_service(),
            })
            .collect())
    }

    fn search_project(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, HostError> {
        let project = self.project()?;
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return Err(HostError::invalid_argument("query не может быть пустым"));
        }
        let limit = limit.clamp(1, SEARCH_MAX_LIMIT);
        let mut hits = Vec::new();
        for object in &project.objects {
            if hits.len() >= limit {
                break;
            }
            let manifest = manifest_blob(object);
            let fields = [("title", object.title.clone()), ("id", object.id.clone()), ("manifest", manifest)];
            for (field, haystack) in fields {
                if let Some(position) = haystack.to_lowercase().find(&query) {
                    hits.push(SearchHit {
                        kind: kind_str(object.kind).to_owned(),
                        id: object.id.clone(),
                        title: object.title.clone(),
                        field: field.to_owned(),
                        snippet: snippet(&haystack, position, query.chars().count()),
                    });
                    break;
                }
            }
        }
        Ok(hits)
    }

    fn read_project_file(&self, rel_path: &str, range: Option<FileRange>) -> Result<FileChunk, HostError> {
        let path = self.sandbox.resolve(rel_path)?;
        let relative = self.sandbox.rel_posix(&path);
        let metadata = fs::metadata(&path)
            .map_err(|error| HostError::not_ready(self.sanitize(format!("файл недоступен: {error}"))))?;
        if !metadata.is_file() {
            return Err(HostError::unsupported("file_read читает только файлы (не каталоги)"));
        }
        // Файл ЦЕЛИКОМ в память не читается (риск OOM на больших PDF): либо проверяем размер
        // и читаем только запрошенное окно (seek/take, построчно), либо отказываем.
        let total_bytes = metadata.len();
        match range {
            None => read_file_prefix(&path, &relative, total_bytes),
            Some(range) => read_file_range(&path, &relative, total_bytes, range),
        }
    }

    fn doc_list(&self) -> Result<Vec<DocInfo>, HostError> {
        let project = self.project()?;
        let mut documents = Vec::new();
        for object in project.documents() {
            let ObjectData::Document(document) = &object.data else { continue };
            let path = self.document_path(object)?;
            let relative = self.sandbox.rel_posix(&path);
            if !rde_doc::is_rde_package(&path) {
                documents.push(DocInfo {
                    id: document.id.clone(),
                    title: object.title.clone(),
                    file: relative,
                    migrated: false,
                    schema_version: None,
                    blocks: None,
                    inserts: None,
                });
                continue;
            }
            let registry = self.load_registry(&path)?;
            documents.push(DocInfo {
                id: document.id.clone(),
                title: object.title.clone(),
                file: relative,
                migrated: true,
                schema_version: Some(registry.schema_version.clone()),
                blocks: Some(registry.blocks.len()),
                inserts: Some(registry.inserts.len()),
            });
        }
        Ok(documents)
    }

    fn doc_outline(&self, doc_id: &str) -> Result<DocOutline, HostError> {
        let project = self.project()?;
        let object = Self::doc_object(&project, doc_id)?;
        let path = self.document_path(object)?;
        let registry = self.load_registry(&path)?;
        let blocks = self.document_blocks(&path, &registry);
        let total_blocks = blocks.len();
        let truncated = total_blocks > MAX_OUTLINE_BLOCKS;
        let outline: Vec<OutlineBlock> = blocks
            .iter()
            .take(MAX_OUTLINE_BLOCKS)
            .enumerate()
            .map(|(index, block)| OutlineBlock {
                id: block.id.clone(),
                kind: block.kind.clone(),
                title: truncate_chars(block.text.trim(), OUTLINE_TITLE_CHARS),
                chars: block.text.chars().count(),
                index,
            })
            .collect();
        Ok(DocOutline {
            doc_id: doc_id.to_owned(),
            title: object.title.clone(),
            template: registry.template.clone(),
            schema_version: registry.schema_version.clone(),
            total_blocks,
            total_inserts: registry.inserts.len(),
            truncated,
            blocks: outline,
        })
    }

    fn doc_read(&self, doc_id: &str, request: DocReadRequest) -> Result<DocText, HostError> {
        let project = self.project()?;
        let object = Self::doc_object(&project, doc_id)?;
        let path = self.document_path(object)?;
        let registry = self.load_registry(&path)?;
        let blocks = self.document_blocks(&path, &registry);
        let total_blocks = blocks.len();

        let (selected, range_truncated): (Vec<&RdeBlock>, bool) = if let Some(ids) = &request.block_ids {
            let wanted: std::collections::BTreeSet<&str> = ids.iter().map(String::as_str).collect();
            let selected: Vec<&RdeBlock> = blocks.iter().filter(|block| wanted.contains(block.id.as_str())).collect();
            if selected.is_empty() {
                return Err(HostError::not_found("ни один block_id не найден в документе")
                    .with_hint("получите id блоков через doc_outline"));
            }
            (selected, false)
        } else {
            let start = request.start.unwrap_or(0);
            if start >= total_blocks && total_blocks > 0 {
                return Err(HostError::out_of_range(format!("start={start} за пределами документа ({total_blocks} блоков)")));
            }
            let limit = request.limit.unwrap_or(DOC_READ_DEFAULT_BLOCKS);
            let limit = limit.clamp(1, DOC_READ_MAX_BLOCKS);
            let selected: Vec<&RdeBlock> = blocks.iter().skip(start).take(limit).collect();
            let truncated = start + selected.len() < total_blocks;
            (selected, truncated)
        };

        let mut out_blocks = Vec::new();
        let mut text = String::new();
        let mut truncated = range_truncated;
        for block in selected {
            if !text.is_empty() {
                text.push_str("\n\n");
            }
            let budget = DOC_READ_MAX_BYTES.saturating_sub(text.len());
            if block.text.len() > budget {
                let piece = truncate_bytes(block.text.as_str(), budget);
                text.push_str(piece);
                out_blocks.push(DocTextBlock {
                    id: block.id.clone(),
                    kind: block.kind.clone(),
                    text: piece.to_owned(),
                });
                truncated = true;
                break;
            }
            text.push_str(&block.text);
            out_blocks.push(DocTextBlock { id: block.id.clone(), kind: block.kind.clone(), text: block.text.clone() });
        }
        if text.len() > DOC_READ_MAX_BYTES {
            text = truncate_bytes(&text, DOC_READ_MAX_BYTES).to_owned();
            truncated = true;
        }
        Ok(DocText {
            doc_id: doc_id.to_owned(),
            total_blocks,
            returned_blocks: out_blocks.len(),
            truncated,
            text,
            blocks: out_blocks,
        })
    }

    fn artifact_list(&self) -> Result<Vec<ArtifactInfo>, HostError> {
        let project = self.project()?;
        let ledger = self.artifact_ledger();
        let mut artifacts = Vec::new();
        for object in &project.objects {
            let ObjectData::Artifact(manifest) = &object.data else { continue };
            artifacts.push(artifact_info(object, manifest, ledger.as_ref()));
        }
        Ok(artifacts)
    }

    fn artifact_read(&self, id: &str) -> Result<ArtifactDetail, HostError> {
        let project = self.project()?;
        let object = Self::artifact_object(&project, id)?;
        let ObjectData::Artifact(manifest) = &object.data else {
            return Err(HostError::not_found("объект не является артефактом"));
        };
        let ledger = self.artifact_ledger();
        let info = artifact_info(object, manifest, ledger.as_ref());
        let ledger_state = ledger.as_ref().and_then(|ledger| ledger.get(id));
        let outputs = ledger_state
            .map(|state| {
                state
                    .outputs
                    .iter()
                    .map(|output| ArtifactOutputInfo {
                        rel: output.rel.clone(),
                        size: output.size,
                        blob: output.blob.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let groups = serde_json::to_value(&manifest.groups).unwrap_or_else(|_| serde_json::json!([]));
        Ok(ArtifactDetail {
            info,
            rebuild: manifest.rebuild.clone(),
            runtime: manifest.runtime.as_ref().map(|runtime| ArtifactRuntimeInfo {
                kind: runtime.kind.clone(),
                env: runtime.env.clone(),
                entry: runtime.entry.clone(),
            }),
            params: manifest.params.clone(),
            outputs_declared: manifest.outputs.clone(),
            outputs,
            script_files: manifest.script_files.clone(),
            timeout_s: manifest.timeout_s,
            network_allow: manifest.network_allow,
            groups,
        })
    }

    fn source_list(&self) -> Result<Vec<SourceInfo>, HostError> {
        let project = self.project()?;
        let mut sources = Vec::new();
        for object in project.sources() {
            let ObjectData::Source(manifest) = &object.data else { continue };
            let file = manifest
                .file
                .as_ref()
                .map(|file| self.sandbox.rel_posix(&object.dir.join(file)));
            let bibliography = manifest
                .reference
                .as_ref()
                .map(|reference| rde_templates::format_reference(reference, &manifest.title));
            sources.push(SourceInfo {
                id: manifest.id.clone(),
                title: manifest.title.clone(),
                kind: manifest.kind.clone(),
                file,
                url: manifest.url.clone(),
                retrieved_at: manifest.retrieved_at.clone(),
                bibliography,
            });
        }
        Ok(sources)
    }

    fn note_list(&self) -> Result<Vec<NoteInfo>, HostError> {
        let project = self.project()?;
        let mut notes = Vec::new();
        for object in &project.objects {
            let ObjectData::Note(manifest) = &object.data else { continue };
            let path = object.dir.join(&manifest.file);
            let relative = self.sandbox.rel_posix(&path);
            let bytes = fs::metadata(&path).ok().map(|metadata| metadata.len());
            notes.push(NoteInfo { id: manifest.id.clone(), title: object.title.clone(), file: relative, bytes });
        }
        Ok(notes)
    }

    fn template_list(&self) -> Result<Vec<TemplateInfo>, HostError> {
        Ok(rde_templates::builtin_templates()
            .iter()
            .map(|template| TemplateInfo {
                id: template.id.clone(),
                title: template.title.clone(),
                description: template.description.clone(),
            })
            .collect())
    }

    fn git_status(&self) -> Result<GitStatusInfo, HostError> {
        let status = rde_vcs::status(&self.root)
            .map_err(|error| HostError::internal(self.sanitize(format!("git status: {error}"))))?;
        let branch = if status.is_repo && !status.branch.is_empty() {
            Some(status.branch.clone())
        } else {
            None
        };
        Ok(GitStatusInfo {
            available: status.available,
            is_repo: status.is_repo,
            branch,
            changed: status.changed,
            untracked: status.untracked,
            last_commit: status.last_commit.map(|commit| GitCommitInfo {
                id: commit.id,
                summary: commit.summary,
                author: commit.author,
                time: commit.time,
            }),
            ahead: status.ahead,
        })
    }

    fn instructions(&self) -> Result<String, HostError> {
        let project = self.project()?;
        Ok(instructions::build(&project))
    }

    // -----------------------------------------------------------------------
    // M2: мутации через approvals/checkpoints (ADR-0007). Сюда приходят только
    // авторизованные вызовы (грант политики + approval в rde-mcp); здесь — чекпоинт,
    // библиотечная операция, авто-откат при ошибке и RDE_CONFLICT при внешней правке.
    // -----------------------------------------------------------------------

    fn artifact_write_manifest(
        &self,
        id: &str,
        manifest: ArtifactManifestDto,
        dry_run: bool,
        expected_sha256: Option<&str>,
    ) -> Result<MutationOutcome, HostError> {
        let project = self.project()?;
        let object = Self::artifact_object(&project, id)?;
        let ObjectData::Artifact(current) = &object.data else {
            return Err(HostError::not_found("объект не является артефактом"));
        };
        let merged = merge_artifact_manifest(current, id, &manifest)?;
        let path = object.dir.join("artifact.toml");
        let rel_path = self.object_rel_path(&path)?;
        // Валидация ДО записи: при ошибке даже не касаемся файла (схема — rde-core, не своя логика).
        let toml_text = toml::to_string_pretty(&merged)
            .map_err(|error| HostError::invalid_argument(self.sanitize(format!("манифест: {error}"))))?;
        validate_artifact_manifest_toml(&toml_text)
            .map_err(|message| HostError::invalid_argument(self.sanitize(message)))?;
        if dry_run {
            return Ok(MutationOutcome {
                dry_run: true,
                checkpoint_id: None,
                changed_files: Vec::new(),
                detail: Some(format!("манифест валиден, будет записан: {rel_path}")),
            });
        }
        self.check_expected_file_sha(&path, &rel_path, expected_sha256)?;
        let args_hash = Self::payload_sha256(&(id, &manifest, expected_sha256));
        let ((), checkpoint_id) = self.with_checkpoint(
            "artifact_write_manifest",
            &args_hash,
            vec![(rel_path.clone(), path.clone())],
            Some(format!("artifact {id}")),
            |host| {
                rde_core::ops::write_artifact_manifest(&host.root, &merged).map_err(|error| {
                    HostError::invalid_argument(host.sanitize(format!("artifact.toml: {error}")))
                })
            },
        )?;
        Ok(MutationOutcome {
            dry_run: false,
            checkpoint_id: Some(checkpoint_id),
            changed_files: vec![rel_path],
            detail: None,
        })
    }

    fn artifact_write_script(
        &self,
        id: &str,
        file: &str,
        content: &str,
        dry_run: bool,
        expected_sha256: Option<&str>,
    ) -> Result<MutationOutcome, HostError> {
        if content.len() > rde_mcp::model::ARTIFACT_SCRIPT_MAX_BYTES {
            return Err(HostError::out_of_range(format!(
                "содержимое {} байт больше лимита {} байт",
                content.len(),
                rde_mcp::model::ARTIFACT_SCRIPT_MAX_BYTES
            )));
        }
        let project = self.project()?;
        let object = Self::artifact_object(&project, id)?;
        let ObjectData::Artifact(manifest) = &object.data else {
            return Err(HostError::not_found("объект не является артефактом"));
        };
        // A7: только python/data (без shell/node/r) — произвольные команды запрещены.
        if let Some(runtime) = &manifest.runtime {
            if !runtime.kind.eq_ignore_ascii_case("python") && !runtime.kind.eq_ignore_ascii_case("data") {
                return Err(HostError::unsupported(format!(
                    "запись скриптов для runtime `{}` запрещена",
                    runtime.kind
                ))
                .with_hint("M2 пишет только python-скрипты и data-файлы артефактов"));
            }
        }
        let allowed = allowed_script_files(manifest);
        if allowed.is_empty() {
            return Err(HostError::unsupported("у артефакта нет runtime.entry/script_files — записывать нечего")
                .with_hint("объявите runtime.entry или data-входы в манифесте (artifact_write_manifest)"));
        }
        let rel_file = normalize_rel_path(file)?;
        if !allowed.iter().any(|allowed_file| allowed_file == &rel_file) {
            return Err(HostError::invalid_argument(format!(
                "файл `{rel_file}` не объявлен в манифесте артефакта"
            ))
            .with_hint(format!("разрешены: {}", allowed.join(", "))));
        }
        let target = object.dir.join(&rel_file);
        let rel_target = self.object_rel_path(&target)?;
        // Песочница записи: путь внутри проекта, без симлинков наружу, `.rde/**` закрыт.
        let target = self.sandbox.resolve_for_write(&rel_target)?;
        if dry_run {
            return Ok(MutationOutcome {
                dry_run: true,
                checkpoint_id: None,
                changed_files: vec![rel_target],
                detail: Some(format!("проверка пройдена: будет записан {rel_file} ({} байт)", content.len())),
            });
        }
        self.check_expected_file_sha(&target, &rel_target, expected_sha256)?;
        let args_hash = Self::payload_sha256(&(id, &rel_file, content.len(), expected_sha256));
        let ((), checkpoint_id) = self.with_checkpoint(
            "artifact_write_script",
            &args_hash,
            vec![(rel_target.clone(), target.clone())],
            Some(format!("artifact {id}/{rel_file}")),
            |host| host.write_project_file(&target, content.as_bytes()),
        )?;
        Ok(MutationOutcome {
            dry_run: false,
            checkpoint_id: Some(checkpoint_id),
            changed_files: vec![rel_target],
            detail: None,
        })
    }

    fn note_write(
        &self,
        id: &str,
        content: Option<String>,
        title: Option<String>,
        dry_run: bool,
        expected_sha256: Option<&str>,
    ) -> Result<MutationOutcome, HostError> {
        if content.is_none() && title.is_none() {
            return Err(HostError::invalid_argument("укажите content и/или title"));
        }
        let project = self.project()?;
        let object = Self::note_object(&project, id)?;
        let ObjectData::Note(manifest) = &object.data else {
            return Err(HostError::not_found("объект не является заметкой"));
        };
        let note_path = object.dir.join(&manifest.file);
        let note_rel = self.object_rel_path(&note_path)?;
        let manifest_path = object.dir.join("note.toml");
        let manifest_rel = self.object_rel_path(&manifest_path)?;
        let mut files = Vec::new();
        let mut changed = Vec::new();
        if content.is_some() {
            files.push((note_rel.clone(), note_path.clone()));
            changed.push(note_rel.clone());
        }
        if title.is_some() {
            files.push((manifest_rel.clone(), manifest_path.clone()));
            changed.push(manifest_rel.clone());
        }
        if dry_run {
            return Ok(MutationOutcome {
                dry_run: true,
                checkpoint_id: None,
                changed_files: changed,
                detail: Some("проверка пройдена".to_owned()),
            });
        }
        if content.is_some() {
            self.check_expected_file_sha(&note_path, &note_rel, expected_sha256)?;
        }
        let args_hash =
            Self::payload_sha256(&(id, content.as_deref().map(str::len), title.as_deref(), expected_sha256));
        let ((), checkpoint_id) = self.with_checkpoint(
            "note_write",
            &args_hash,
            files,
            Some(format!("note {id}")),
            |host| {
                if let Some(content) = &content {
                    host.write_project_file(&note_path, content.as_bytes())?;
                }
                if let Some(title) = &title {
                    let text = fs::read_to_string(&manifest_path)
                        .map_err(|error| HostError::not_ready(host.sanitize(format!("note.toml: {error}"))))?;
                    let mut note: rde_core::NoteManifest = toml::from_str(&text)
                        .map_err(|error| HostError::invalid_argument(host.sanitize(format!("note.toml: {error}"))))?;
                    note.title = title.clone();
                    let text = toml::to_string_pretty(&note)
                        .map_err(|error| HostError::internal(format!("note.toml: {error}")))?;
                    host.write_project_file(&manifest_path, text.as_bytes())?;
                }
                Ok(())
            },
        )?;
        Ok(MutationOutcome {
            dry_run: false,
            checkpoint_id: Some(checkpoint_id),
            changed_files: changed,
            detail: None,
        })
    }

    fn artifact_run(
        &self,
        targets: Option<Vec<String>>,
        include_dependents: bool,
        force: bool,
    ) -> Result<JobInfo, HostError> {
        let project = self.project()?;
        // R-RUN-06: запуск скриптов — только в доверенном проекте (явное согласие пользователя).
        if !project.manifest.trusted {
            return Err(HostError::approval_denied(
                "запуск скриптов требует согласия: проект не помечен доверенным (trusted = true в rde.toml)",
            )
            .with_hint("R-RUN-06: отметку доверенного проекта делает пользователь (RDE UI/CLI)"));
        }
        if let Some(ids) = &targets {
            for id in ids {
                Self::artifact_object(&project, id)?;
            }
        }
        let targets = targets.filter(|ids| !ids.is_empty());
        // Один активный job (RDE_BUSY) — проверяем до чекпоинта, чтобы не плодить пустые.
        if let Some(active) = self.jobs.active() {
            if active.info().state == "running" {
                return Err(HostError::busy(format!(
                    "уже выполняется job `{}` — дождитесь завершения или отмените его",
                    active.id
                ))
                .with_hint("run_status/run_cancel по активному job_id"));
            }
        }
        let request = ExecRequest { targets, include_dependents, force };
        let args_hash = Self::payload_sha256(&(&request.targets, include_dependents, force));
        let roots = self.store_roots()?;
        let targets_note = match &request.targets {
            Some(ids) => format!("targets: {}", ids.join(", ")),
            None => "все устаревшие".to_owned(),
        };
        let checkpoint = self.checkpoints.create(
            "artifact_run",
            &args_hash,
            Some(roots),
            &[],
            Some(format!("artifact_run: {targets_note}")),
        )?;
        match self.jobs.start(
            Arc::clone(&self.jobs),
            self.root.clone(),
            Arc::clone(&self.executor),
            request,
            Some(checkpoint.id.clone()),
        ) {
            Ok(info) => Ok(info),
            Err(error) => {
                // Job не стартовал — чекпоинт без изменений помечаем откаченным (не мусорим).
                let _ = self.checkpoints.mark_rolled_back(&checkpoint);
                Err(error)
            }
        }
    }

    fn run_status(&self, job_id: &str) -> Result<JobInfo, HostError> {
        self.jobs
            .get(job_id)
            .map(|job| job.info())
            .ok_or_else(|| job_not_found(job_id))
    }

    fn run_cancel(&self, job_id: &str) -> Result<JobInfo, HostError> {
        self.jobs.cancel(job_id)
    }

    fn run_log(&self, job_id: &str, offset: u64, max_bytes: usize) -> Result<JobLog, HostError> {
        let job = self.jobs.get(job_id).ok_or_else(|| job_not_found(job_id))?;
        let Some(path) = job.log_path_abs() else {
            // Скрипт ещё не стартовал (или запусков не было) — пустой лог, без ошибки.
            let info = job.info();
            return Ok(JobLog {
                job_id: job_id.to_owned(),
                offset,
                next_offset: offset,
                eof: info.state != "running",
                truncated: false,
                text: String::new(),
            });
        };
        let chunk = rde_run::read_log_from(&path, offset, max_bytes)
            .map_err(|error| HostError::not_ready(self.sanitize(format!("лог job'а: {error}"))))?;
        let text = String::from_utf8_lossy(&chunk.bytes).into_owned();
        Ok(JobLog {
            job_id: job_id.to_owned(),
            offset,
            next_offset: chunk.next_offset,
            eof: chunk.eof,
            truncated: !chunk.eof,
            text,
        })
    }

    fn git_commit(&self, message: &str, dry_run: bool) -> Result<GitCommitOutcome, HostError> {
        let project = self.project()?;
        let status = rde_vcs::status(&self.root)
            .map_err(|error| HostError::internal(self.sanitize(format!("git status: {error}"))))?;
        if !status.available {
            return Err(HostError::not_ready("git недоступен (установите git)"));
        }
        if !status.is_repo {
            return Err(HostError::not_ready("каталог проекта не является git-репозиторием")
                .with_hint("выполните git init в каталоге проекта (RDE UI: «Инициализировать git»)"));
        }
        // `.rde/**` (store/аудит/чекпоинты) не должен попасть в коммит: иначе внутреннее
        // состояние RDE уйдёт в репозиторий. Проверяем по статусу ДО коммита.
        let rde_entries: Vec<&String> = status
            .changed
            .iter()
            .chain(status.untracked.iter())
            .filter(|path| path.starts_with(".rde/") || path.as_str() == ".rde")
            .collect();
        if !rde_entries.is_empty() {
            return Err(HostError::conflict("каталог `.rde` не исключён из git: коммит включил бы внутреннее состояние RDE")
                .with_hint("добавьте `/.rde/` в .gitignore проекта и повторите"));
        }
        let changed = status.changed.len() + status.untracked.len();
        if changed == 0 {
            return Err(HostError::not_ready("нечего коммитить: нет изменений")
                .with_hint("git_status показывает пустой список изменённых/новых файлов"));
        }
        let (author_name, author_email) = git_author(&project);
        if dry_run {
            return Ok(GitCommitOutcome {
                dry_run: true,
                checkpoint_id: None,
                commit: None,
                files: changed,
                message: format!("будет закоммичено файлов: {changed}; автор: {author_name} <{author_email}>"),
            });
        }
        let args_hash = Self::payload_sha256(&(message, &author_name, &author_email));
        let (result, checkpoint_id) = self.with_checkpoint(
            "git_commit",
            &args_hash,
            Vec::new(),
            Some(format!("git commit: {} файлов", changed)),
            |host| {
                rde_vcs::commit(&host.root, message, &author_name, &author_email).map_err(|error| {
                    HostError::not_ready(host.sanitize(format!("git commit: {error}")))
                })
            },
        )?;
        // Время/автор коммита — из git после операции (ничего не придумываем).
        let commit = rde_vcs::status(&self.root)
            .ok()
            .and_then(|status| status.last_commit)
            .map(|entry| GitCommitInfo {
                id: result.id.clone(),
                summary: result.summary.clone(),
                author: entry.author,
                time: entry.time,
            })
            .unwrap_or_else(|| GitCommitInfo {
                id: result.id.clone(),
                summary: result.summary.clone(),
                author: author_name.clone(),
                time: String::new(),
            });
        Ok(GitCommitOutcome {
            dry_run: false,
            checkpoint_id: Some(checkpoint_id),
            commit: Some(commit),
            files: result.files,
            message: "коммит создан".to_owned(),
        })
    }

    fn checkpoint_restore(&self, checkpoint_id: &str) -> Result<MutationOutcome, HostError> {
        let (changed_files, roots_restored) = self.checkpoints.restore(checkpoint_id)?;
        Ok(MutationOutcome {
            dry_run: false,
            checkpoint_id: Some(checkpoint_id.to_owned()),
            changed_files,
            detail: Some(if roots_restored {
                "файлы и корни store откачены".to_owned()
            } else {
                "файлы откачены (в чекпоинте нет корней store)".to_owned()
            }),
        })
    }

    fn shutdown_jobs(&self) {
        self.jobs.shutdown();
    }
}

fn job_not_found(job_id: &str) -> HostError {
    HostError::job_not_found(format!("job `{job_id}` не найден")).with_hint(
        "jobs живут в процессе сервера и не переживают рестарт (M2); список — из ответа artifact_run",
    )
}

/// Слить патч [`ArtifactManifestDto`] с текущим манифестом (`id`/`schema` — от хоста).
///
/// `out_hash` агентом не меняется (владелец — RDE после успешного обновления, R-ART-02).
fn merge_artifact_manifest(
    current: &rde_core::ArtifactManifest,
    id: &str,
    patch: &ArtifactManifestDto,
) -> Result<rde_core::ArtifactManifest, HostError> {
    let mut value = serde_json::to_value(current)
        .map_err(|error| HostError::internal(format!("манифест: сериализация: {error}")))?;
    let object = value
        .as_object_mut()
        .ok_or_else(|| HostError::internal("манифест: ожидался JSON-объект"))?;
    if let Some(title) = &patch.title {
        object.insert("title".to_owned(), serde_json::Value::String(title.clone()));
    }
    if let Some(kind) = &patch.kind {
        object.insert("type".to_owned(), serde_json::Value::String(kind.clone()));
    }
    if let Some(rebuild) = &patch.rebuild {
        object.insert("rebuild".to_owned(), serde_json::Value::String(rebuild.clone()));
    }
    if let Some(runtime) = &patch.runtime {
        object.insert(
            "runtime".to_owned(),
            serde_json::to_value(runtime).map_err(|error| HostError::invalid_argument(format!("runtime: {error}")))?,
        );
    }
    if let Some(params) = &patch.params {
        object.insert("params".to_owned(), params.clone());
    }
    if let Some(outputs) = &patch.outputs {
        object.insert(
            "outputs".to_owned(),
            serde_json::to_value(outputs).map_err(|error| HostError::invalid_argument(format!("outputs: {error}")))?,
        );
    }
    if let Some(script_files) = &patch.script_files {
        object.insert(
            "script_files".to_owned(),
            serde_json::to_value(script_files)
                .map_err(|error| HostError::invalid_argument(format!("script_files: {error}")))?,
        );
    }
    if let Some(timeout_s) = patch.timeout_s {
        object.insert("timeout_s".to_owned(), serde_json::json!(timeout_s));
    }
    if let Some(network_allow) = patch.network_allow {
        object.insert("network_allow".to_owned(), serde_json::json!(network_allow));
    }
    if let Some(groups) = &patch.groups {
        object.insert(
            "groups".to_owned(),
            serde_json::to_value(groups).map_err(|error| HostError::invalid_argument(format!("groups: {error}")))?,
        );
    }
    if let Some(values) = &patch.values {
        object.insert("values".to_owned(), values.clone());
    }
    // id/schema — стабильные поля артефакта; out_hash — собственность RDE (не патчим).
    object.insert("id".to_owned(), serde_json::Value::String(id.to_owned()));
    object.insert("schema".to_owned(), serde_json::Value::String(current.schema.clone()));
    let mut merged: rde_core::ArtifactManifest = serde_json::from_value(value)
        .map_err(|error| HostError::invalid_argument(format!("манифест не соответствует схеме артефакта: {error}")))?;
    // `params` по умолчанию (`serde_json::Value::Null`) не сериализуется в TOML
    // (`write_artifact_manifest` пишет файл через `toml`): пустой объект эквивалентен
    // отсутствию параметров и пишется как `params = {}` (иначе data/constants-манифесты
    // не сохранялись бы вовсе).
    if merged.params.is_null() {
        merged.params = serde_json::json!({});
    }
    Ok(merged)
}

/// Проверить TOML манифеста артефакта по JSON Schema `rde-core` (без записи файла).
fn validate_artifact_manifest_toml(toml_text: &str) -> Result<(), String> {
    let value: toml::Value = toml::from_str(toml_text).map_err(|error| format!("манифест: TOML: {error}"))?;
    let errors = rde_core::manifest::validate(rde_core::ManifestKind::Artifact, &value);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("манифест не проходит схему: {}", errors.join("; ")))
    }
}

/// Нормализовать относительный путь внутри проекта (`a/./b` → `a/b`; без `..`/абсолютных).
fn normalize_rel_path(path: &str) -> Result<String, HostError> {
    let mut text = path.trim().replace('\\', "/");
    while text.starts_with("./") {
        text = text.trim_start_matches("./").to_owned();
    }
    if text.is_empty() {
        return Err(HostError::invalid_argument("путь файла не может быть пустым"));
    }
    if text.starts_with('/') || text.split('/').any(|part| part == "..") {
        return Err(HostError::path_escape("путь должен быть относительным путём внутри артефакта"));
    }
    Ok(text)
}

/// Разрешённые к записи файлы артефакта: `runtime.entry`/`script_files` (python) или
/// объявленные `File`-входы групп (data). Никакой общей записи в каталог артефакта (A7).
fn allowed_script_files(manifest: &rde_core::ArtifactManifest) -> Vec<String> {
    let mut files: Vec<String> = Vec::new();
    if let Some(runtime) = &manifest.runtime {
        files.push(runtime.entry.clone());
        files.extend(manifest.script_files.iter().cloned());
    } else {
        for group in &manifest.groups {
            for input in &group.inputs {
                if let rde_core::InputSpec::File { file } = input {
                    files.push(file.clone());
                }
            }
        }
    }
    let mut files: Vec<String> = files.iter().filter_map(|file| normalize_rel_path(file).ok()).collect();
    files.sort();
    files.dedup();
    files
}

/// Автор git-коммита (O7): первый human-автор из `rde.toml`, затем kind=agent, затем «MCP client» —
/// так же, как UI; секреты/ключи не используются.
fn git_author(project: &Project) -> (String, String) {
    let pick = |kind: &str| {
        project.manifest.authors.iter().find(|author| author.kind == kind).map(|author| {
            let email = if author.email.trim().is_empty() {
                "rde@localhost".to_owned()
            } else {
                author.email.trim().to_owned()
            };
            (author.name.trim().to_owned(), email)
        })
    };
    pick("human")
        .or_else(|| pick("agent"))
        .unwrap_or_else(|| ("MCP client".to_owned(), "mcp@rde.local".to_owned()))
}

/// Атомарная запись файла (temp в том же каталоге + rename).
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "rde.tmp".to_owned());
    let tmp = path.with_file_name(format!(".{file_name}.rde-tmp-{}", ulid::Ulid::generate()));
    fs::write(&tmp, bytes)?;
    match fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(error) => {
            let _ = fs::remove_file(&tmp);
            Err(error)
        }
    }
}

// ---------------------------------------------------------------------------
// Помощники (чистые функции — удобно тестировать)
// ---------------------------------------------------------------------------

fn sanitize_path(root: &Path, message: &str) -> String {
    let root_text = root.to_string_lossy();
    if root_text.is_empty() {
        return message.to_owned();
    }
    message.replace(root_text.as_ref(), "<project>")
}

fn rel_from_root(root: &Path, path: &Path) -> Result<String, HostError> {
    let relative = path.strip_prefix(root).map_err(|_| HostError::path_escape("путь вне корня проекта"))?;
    let mut out = String::new();
    for component in relative.components() {
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&component.as_os_str().to_string_lossy());
    }
    Ok(out)
}

fn kind_str(kind: ObjectKind) -> &'static str {
    match kind {
        ObjectKind::Document => "document",
        ObjectKind::Artifact => "artifact",
        ObjectKind::Constants => "constants",
        ObjectKind::Source => "source",
        ObjectKind::Note => "note",
        ObjectKind::Repository => "repository",
        ObjectKind::Formula => "formula",
    }
}

fn freshness_str(freshness: Freshness) -> &'static str {
    match freshness {
        Freshness::UpToDate => "up_to_date",
        Freshness::Stale => "stale",
        Freshness::Failed => "failed",
        Freshness::External => "external",
    }
}

/// JSON-представление манифеста для поиска (только данные, без запуска).
fn manifest_blob(object: &Object) -> String {
    let value = match &object.data {
        ObjectData::Document(manifest) => serde_json::to_string(manifest),
        ObjectData::Artifact(manifest) => serde_json::to_string(manifest),
        ObjectData::Source(manifest) => serde_json::to_string(manifest),
        ObjectData::Note(manifest) => serde_json::to_string(manifest),
        ObjectData::Repository(manifest) => serde_json::to_string(manifest),
        ObjectData::Formula(manifest) => serde_json::to_string(manifest),
    };
    value.unwrap_or_default()
}

/// Фрагмент вокруг совпадения: без переводов строк, обрезан по символам.
fn snippet(haystack: &str, position: usize, query_len: usize) -> String {
    let chars: Vec<char> = haystack.chars().collect();
    let start_char = haystack[..position.min(haystack.len())].chars().count();
    let left = start_char.saturating_sub(SEARCH_SNIPPET_CHARS / 2);
    let right = (start_char + query_len + SEARCH_SNIPPET_CHARS / 2).min(chars.len());
    let text: String = chars[left.min(chars.len())..right.max(left.min(chars.len()))].iter().collect();
    let text = text.replace(['\n', '\r', '\t'], " ");
    if left > 0 {
        format!("…{}", text.trim())
    } else {
        text.trim().to_owned()
    }
}

fn artifact_info(
    object: &Object,
    manifest: &rde_core::ArtifactManifest,
    ledger: Option<&HashMap<String, ArtifactState>>,
) -> ArtifactInfo {
    let state = ledger.and_then(|ledger| ledger.get(&manifest.id));
    let (status_text, reason) = match (ledger, state) {
        (None, _) => (
            "unknown".to_owned(),
            Some("ledger .rde недоступен (нет хранилища или нет доступа)".to_owned()),
        ),
        (Some(_), None) => ("never_run".to_owned(), None),
        (Some(_), Some(state)) => match (manifest.out_hash.as_deref(), state.out_hash.as_deref()) {
            (Some(manifest_hash), Some(ledger_hash)) if manifest_hash == ledger_hash => ("fresh".to_owned(), None),
            (None, None) => ("never_run".to_owned(), None),
            _ => (
                "stale".to_owned(),
                Some("out_hash манифеста не совпадает с ledger (выходы/манифест изменены вне RDE)".to_owned()),
            ),
        },
    };
    ArtifactInfo {
        id: manifest.id.clone(),
        title: object.title.clone(),
        artifact_type: manifest.kind.clone(),
        status: status_text,
        status_reason: reason,
        out_hash: state.and_then(|state| state.out_hash.clone()).or_else(|| manifest.out_hash.clone()),
        act_key: state.map(|state| state.act_key.clone()),
        updated_unix: state.map(|state| state.updated_unix),
        run_id: state.and_then(|state| state.run_id.clone()),
        outputs: state.map(|state| state.outputs.len()).unwrap_or_else(|| manifest.outputs.len()),
    }
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        text.to_owned()
    } else {
        let mut out: String = text.chars().take(max).collect();
        out.push('…');
        out
    }
}

fn truncate_bytes(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut end = max;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Прочитать начало файла (вызов без `range`): не больше [`FILE_READ_DEFAULT_BYTES`].
///
/// Файл целиком в память не читается: сначала проверяется размер; файлы больше
/// [`FILE_READ_WHOLE_LIMIT`] требуют явного диапазона (иначе `RDE_OUT_OF_RANGE`) — так
/// большой PDF не может уронить сервер по памяти (M1-review).
fn read_file_prefix(path: &Path, relative: &str, total_bytes: u64) -> Result<FileChunk, HostError> {
    if total_bytes > FILE_READ_WHOLE_LIMIT {
        return Err(HostError::out_of_range(format!(
            "файл {total_bytes} байт больше лимита {FILE_READ_WHOLE_LIMIT} байт для чтения без range"
        ))
        .with_hint("укажите range: {\"bytes\":{\"offset\",\"limit\"}} или {\"lines\":{\"start\",\"count\"}}"));
    }
    let to_read = FILE_READ_DEFAULT_BYTES.min(total_bytes) as usize;
    let mut bytes = Vec::with_capacity(to_read);
    open_project_file(path)?
        .take(to_read as u64)
        .read_to_end(&mut bytes)
        .map_err(io_read_error)?;
    if bytes.contains(&0) {
        return Err(binary_error());
    }
    let text = utf8_prefix(&bytes, 0, bytes.len()).ok_or_else(binary_error)?;
    Ok(FileChunk {
        path: relative.to_owned(),
        mode: "bytes".to_owned(),
        start: 0,
        end: text.len() as u64,
        total_bytes,
        truncated: (text.len() as u64) < total_bytes,
        text: text.to_owned(),
    })
}

/// Прочитать диапазон файла: байты (`seek`+`take`) или строки (`BufReader`, без чтения всего файла).
fn read_file_range(
    path: &Path,
    relative: &str,
    total_bytes: u64,
    range: FileRange,
) -> Result<FileChunk, HostError> {
    match range {
        FileRange::Bytes { offset, limit } => {
            if limit == 0 {
                return Err(HostError::invalid_argument("limit диапазона байт должен быть больше 0"));
            }
            if limit > FILE_READ_MAX_BYTES as u64 {
                return Err(HostError::out_of_range(format!("limit больше максимума {FILE_READ_MAX_BYTES} байт")));
            }
            if offset > total_bytes {
                return Err(HostError::out_of_range(format!(
                    "offset={offset} за концом файла ({total_bytes} байт)"
                )));
            }
            let to_read = limit.min(total_bytes - offset) as usize;
            let mut file = open_project_file(path)?;
            file.seek(SeekFrom::Start(offset)).map_err(io_read_error)?;
            let mut bytes = vec![0u8; to_read];
            let read = file.take(to_read as u64).read(&mut bytes).map_err(io_read_error)?;
            bytes.truncate(read);
            // Бинарность проверяется в читаемом окне (файл целиком не читаем).
            if bytes.contains(&0) {
                return Err(binary_error());
            }
            let text = utf8_prefix(&bytes, 0, bytes.len()).ok_or_else(binary_error)?;
            let consumed = text.len() as u64;
            Ok(FileChunk {
                path: relative.to_owned(),
                mode: "bytes".to_owned(),
                start: offset,
                end: offset + consumed,
                total_bytes,
                truncated: offset + consumed < total_bytes,
                text: text.to_owned(),
            })
        }
        FileRange::Lines { start, count } => {
            if start == 0 {
                return Err(HostError::invalid_argument("start строк нумеруется с 1"));
            }
            if count == 0 {
                return Err(HostError::invalid_argument("count строк должен быть больше 0"));
            }
            if count > FILE_READ_MAX_LINES as u64 {
                return Err(HostError::out_of_range(format!("count больше максимума {FILE_READ_MAX_LINES} строк")));
            }
            let mut reader = BufReader::new(open_project_file(path)?);
            let mut lines: Vec<String> = Vec::new();
            let mut line_no = 0u64;
            let mut buffer: Vec<u8> = Vec::new();
            let mut exhausted = false;
            loop {
                buffer.clear();
                // Строка длиннее лимита ответа не поместится в ответ: не читаем её в память целиком.
                let mut limited = reader.by_ref().take(FILE_READ_MAX_BYTES as u64 + 1);
                let read = limited.read_until(b'\n', &mut buffer).map_err(io_read_error)?;
                if read == 0 {
                    exhausted = true;
                    break;
                }
                if buffer.len() > FILE_READ_MAX_BYTES && !buffer.ends_with(b"\n") {
                    return Err(HostError::out_of_range(format!(
                        "строка {line_no} длиннее {FILE_READ_MAX_BYTES} байт"
                    ))
                    .with_hint("используйте байтовый range (bytes/offset+limit)"));
                }
                line_no += 1;
                if line_no < start {
                    continue;
                }
                if buffer.contains(&0) {
                    return Err(binary_error());
                }
                let mut text = std::str::from_utf8(&buffer).map_err(|_| binary_error())?;
                text = text.strip_suffix('\n').unwrap_or(text);
                text = text.strip_suffix('\r').unwrap_or(text);
                lines.push(text.to_owned());
                if lines.len() >= count as usize {
                    break;
                }
            }
            let returned = lines.len() as u64;
            Ok(FileChunk {
                path: relative.to_owned(),
                mode: "lines".to_owned(),
                start,
                end: start + returned,
                total_bytes,
                truncated: !exhausted && returned > 0,
                text: lines.join("\n"),
            })
        }
    }
}

fn open_project_file(path: &Path) -> Result<fs::File, HostError> {
    fs::File::open(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            HostError::not_found("файл не найден в проекте")
        } else {
            HostError::not_ready(format!("файл не читается: {error}"))
        }
    })
}

fn io_read_error(error: std::io::Error) -> HostError {
    HostError::not_ready(format!("ошибка чтения файла: {error}"))
}

fn binary_error() -> HostError {
    HostError::unsupported("файл не является UTF-8-текстом (бинарные файлы не читаются)")
        .with_hint("для PDF/изображений используйте source_list/artifact_read")
}

/// Максимальный валидный UTF-8-префикс отрезка `bytes[start..end]` (границы выравниваются).
///
/// Смещение `start` сдвигается вперёд максимум на 3 байта (середина 4-байтного символа);
/// если в отрезке нет валидного UTF-8 вообще — `None` (бинарный файл).
fn utf8_prefix(bytes: &[u8], start: usize, end: usize) -> Option<&str> {
    let end = end.min(bytes.len());
    let original_start = start.min(end);
    let mut start = original_start;
    let mut skipped = 0usize;
    while start < end {
        match std::str::from_utf8(&bytes[start..end]) {
            Ok(text) => return Some(text),
            Err(error) => {
                let valid = error.valid_up_to();
                if valid > 0 {
                    return std::str::from_utf8(&bytes[start..start + valid]).ok();
                }
                if skipped >= 3 {
                    return None;
                }
                start += 1;
                skipped += 1;
            }
        }
    }
    if skipped > 0 && end > original_start && skipped == end - original_start {
        return None;
    }
    Some("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utf8_prefix_aligns_boundaries() {
        let bytes = "привет".as_bytes();
        // Начинаем с середины символа «п» (2 байта) — текст всё равно валиден.
        let text = utf8_prefix(bytes, 1, bytes.len()).unwrap();
        assert_eq!(text, "ривет");
        assert!(utf8_prefix(&[0xff, 0xfe], 0, 2).is_none());
    }

    #[test]
    fn snippet_is_bounded_and_single_line() {
        let text = "a\nb\nc-" .to_owned() + &"x".repeat(500);
        let value = snippet(&text, 2, 1);
        assert!(value.len() < 400, "фрагмент обрезан: {}", value.len());
        assert!(!value.contains('\n'));
    }

    #[test]
    fn truncate_keeps_utf8_boundary() {
        let text = "абвгд";
        assert_eq!(truncate_bytes(text, 4), "аб");
        assert_eq!(truncate_chars(text, 2), "аб…");
    }

    /// Пустой проект (только `rde.toml`) для тестов чекпоинт-механики без демо-данных.
    fn empty_project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-host-lib-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("objects")).unwrap();
        std::fs::write(
            dir.join("rde.toml"),
            "schema = \"rde/project@1\"\nid = \"01LIBTEST0000000000000000\"\nname = \"Lib test\"\n",
        )
        .unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    #[test]
    fn with_checkpoint_auto_rolls_back_files_and_marks_checkpoint() {
        let root = empty_project("rollback");
        let host = HeadlessHost::open(&root).expect("проект открывается");
        let path = root.join("note.md");
        std::fs::write(&path, "старое содержимое").unwrap();
        let rel = "note.md".to_owned();
        let error = host
            .with_checkpoint(
                "test_tool",
                &"a".repeat(64),
                vec![(rel.clone(), path.clone())],
                None,
                |_host| -> Result<(), HostError> {
                    // Мутация «ломается» уже после записи файла: чекпоинт обязан откатить.
                    std::fs::write(&path, "испорченное содержимое").unwrap();
                    Err(HostError::internal("тестовая ошибка операции"))
                },
            )
            .expect_err("операция должна упасть");
        assert_eq!(error.code, rde_mcp::RdeErrorCode::Internal);
        assert!(error.hint.as_deref().unwrap_or_default().contains("авто-rollback"), "hint: {:?}", error.hint);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "старое содержимое",
            "файл восстановлен байт-в-байт"
        );
        let manifests = host.checkpoints.list().unwrap();
        assert_eq!(manifests.len(), 1);
        assert!(manifests[0].rolled_back, "чекпоинт помечен откаченным");
        assert_eq!(manifests[0].files.len(), 1);
        assert_eq!(manifests[0].files[0].existed, true);
        assert!(manifests[0].files[0].sha256.is_some());
    }

    #[test]
    fn with_checkpoint_requires_store_roots_fail_closed() {
        let root = empty_project("store-down");
        std::fs::write(root.join(".rde"), "не каталог").unwrap();
        let host = HeadlessHost::open(&root).expect("проект открывается без store");
        let path = root.join("note.md");
        let error = host
            .with_checkpoint("test_tool", &"b".repeat(64), vec![], None, |_host| Ok(()))
            .expect_err("без store мутация не выполняется");
        assert_eq!(error.code, rde_mcp::RdeErrorCode::NotReady);
        assert!(!path.exists());
    }

    #[test]
    fn helper_functions_validate_paths_and_authors() {
        assert_eq!(normalize_rel_path("./compute.py").unwrap(), "compute.py");
        assert_eq!(normalize_rel_path("src/lib/x.py").unwrap(), "src/lib/x.py");
        assert!(normalize_rel_path("../x.py").is_err());
        assert!(normalize_rel_path("/etc/passwd").is_err());
        assert!(normalize_rel_path("   ").is_err());

        // Автор коммита: human → agent → «MCP client» (O7).
        let mut project = rde_core::demo();
        assert_eq!(git_author(&project).0, "Илья", "в демо human-автор");
        project.manifest.authors = Vec::new();
        assert_eq!(git_author(&project), ("MCP client".to_owned(), "mcp@rde.local".to_owned()));
    }
}
