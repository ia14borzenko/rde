//! RDE host — headless-реализация контракта MCP-слоя (`rde-mcp::RdeHost`) поверх библиотечных
//! крейтов RDE (v0.3, веха M1).
//!
//! Слой `rde-host` — единственное место, которое знает и RDE-библиотеки
//! (`rde-core`/`rde-dag`/`rde-doc`/`rde-store`/`rde-vcs`/`rde-templates`), и протокольный
//! контракт `rde-mcp`. GUI (`rde-ui`/`egui`/`eframe`) здесь не участвует: стрелка зависимостей
//! `apps/rde → rde-host → rde-mcp`, `rde-mcp → { }`. Поэтому один и тот же хост можно
//! использовать из CLI/ACP/будущих headless-режимов, а протокольный слой тестировать на mock.
//!
//! M1 — **только чтение**: хост читает проект/документы/артефакты/источники, ledger и git,
//! но ничего не запускает и не меняет. Мутации появятся в M2 через те же хуки approvals/audit.
//!
//! Безопасность (A5):
//! - все пути — через [`PathSandbox`]: канонизация, `starts_with(корень)`, отказ `..`/абсолютным/
//!   симлинкам наружу, `.rde/**` закрыт (`RDE_PATH_ESCAPE`/`RDE_APPROVAL_DENIED`);
//! - тексты ошибок санитизируются (никаких путей вне проекта/секретов);
//! - лимиты размеров ответов и `RDE_DOC_NOT_MIGRATED` для непреобразованных документов.

pub mod instructions;
pub mod sandbox;

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rde_core::{Freshness, Object, ObjectData, ObjectKind, Project};
use rde_dag::ArtifactState;
use rde_doc::{RdeBlock, RdeDoc};
use rde_mcp::model::{
    ArtifactDetail, ArtifactInfo, ArtifactOutputInfo, ArtifactRuntimeInfo, DocInfo, DocOutline, DocReadRequest,
    DocText, DocTextBlock, FileChunk, FileRange, GitCommitInfo, GitStatusInfo, NoteInfo, ObjectNode,
    OutlineBlock, SearchHit, SourceInfo, TemplateInfo, WorkspaceInfo, DOC_READ_DEFAULT_BLOCKS,
    DOC_READ_MAX_BLOCKS, DOC_READ_MAX_BYTES, FILE_READ_MAX_BYTES, FILE_READ_MAX_LINES, MAX_OUTLINE_BLOCKS,
    MAX_TREE_NODES, SEARCH_MAX_LIMIT,
};
use rde_mcp::{HostError, RdeHost};
use rde_store::Store;

pub use sandbox::PathSandbox;

/// Лимит `file_read` по умолчанию (байт) — не заставляем модель просить «первые N» вручную.
const FILE_READ_DEFAULT_BYTES: u64 = 32 * 1024;

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
}

impl HeadlessHost {
    /// Открыть проект: канонизировать корень, проверить, что проект загружается.
    ///
    /// # Errors
    ///
    /// `RDE_NOT_FOUND` — каталог не найден/не каталог; `RDE_NOT_READY` — проект не грузится
    /// (нет `rde.toml`, битые манифесты). Корень наружу не раскрывается.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, HostError> {
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
        Ok(Self { sandbox: PathSandbox::new(canonical.clone()), root: canonical, store: Mutex::new(None) })
    }

    /// Канонический корень проекта (внутреннее; наружу — только id).
    pub fn root(&self) -> &Path {
        &self.root
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
        let store = guard.as_ref().expect("store открыт выше");
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
        let bytes = fs::read(&path)
            .map_err(|error| HostError::not_ready(self.sanitize(format!("файл не читается: {error}"))))?;
        read_chunk(&relative, &bytes, range)
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
        Ok(GitStatusInfo {
            available: status.available,
            is_repo: status.is_repo,
            branch: status.branch,
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

/// Прочитать кусок файла по диапазону: байты или строки, с UTF-8-выравниванием и лимитами.
fn read_chunk(relative: &str, bytes: &[u8], range: Option<FileRange>) -> Result<FileChunk, HostError> {
    // Бинарный файл (NUL-байты / не UTF-8) инструментом file_read не читается (A5: без «сырых» тел).
    if bytes.contains(&0) {
        return Err(binary_error());
    }
    let total_bytes = bytes.len() as u64;
    match range {
        None => {
            let max = FILE_READ_DEFAULT_BYTES.min(total_bytes);
            let end = (max as usize).min(bytes.len());
            let text = utf8_prefix(bytes, 0, end).ok_or_else(binary_error)?;
            Ok(FileChunk {
                path: relative.to_owned(),
                mode: "bytes".to_owned(),
                start: 0,
                end: text_consumed_bytes(text) as u64,
                total_bytes,
                truncated: text.len() < bytes.len(),
                text: text.to_owned(),
            })
        }
        Some(FileRange::Bytes { offset, limit }) => {
            if limit == 0 {
                return Err(HostError::invalid_argument("limit диапазона байт должен быть больше 0"));
            }
            if limit > FILE_READ_MAX_BYTES as u64 {
                return Err(HostError::out_of_range(format!("limit больше максимума {FILE_READ_MAX_BYTES} байт")));
            }
            let start = (offset as usize).min(bytes.len());
            let end = (start + limit as usize).min(bytes.len());
            let text = utf8_prefix(bytes, start, end).ok_or_else(binary_error)?;
            let consumed = text_consumed_bytes(text);
            Ok(FileChunk {
                path: relative.to_owned(),
                mode: "bytes".to_owned(),
                start: offset,
                end: offset + consumed as u64,
                total_bytes,
                truncated: start + consumed < bytes.len(),
                text: text.to_owned(),
            })
        }
        Some(FileRange::Lines { start, count }) => {
            if start == 0 {
                return Err(HostError::invalid_argument("start строк нумеруется с 1"));
            }
            if count == 0 {
                return Err(HostError::invalid_argument("count строк должен быть больше 0"));
            }
            if count > FILE_READ_MAX_LINES as u64 {
                return Err(HostError::out_of_range(format!("count больше максимума {FILE_READ_MAX_LINES} строк")));
            }
            let text = std::str::from_utf8(bytes).map_err(|_| binary_error())?;
            let lines: Vec<&str> = text.lines().collect();
            let from = (start - 1) as usize;
            if from >= lines.len() {
                return Ok(FileChunk {
                    path: relative.to_owned(),
                    mode: "lines".to_owned(),
                    start,
                    end: start,
                    total_bytes,
                    truncated: false,
                    text: String::new(),
                });
            }
            let to = (from + count as usize).min(lines.len());
            let chunk = lines[from..to].join("\n");
            Ok(FileChunk {
                path: relative.to_owned(),
                mode: "lines".to_owned(),
                start,
                end: start + (to - from) as u64,
                total_bytes,
                truncated: to < lines.len(),
                text: chunk,
            })
        }
    }
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

fn text_consumed_bytes(text: &str) -> usize {
    text.len()
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
}
