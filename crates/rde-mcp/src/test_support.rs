//! Mock-хост для unit-тестов протокольного слоя (без ФС, без доменных крейтов RDE).
//!
//! Доказывает независимость слоя: `rde-mcp` тестируется на синтетическом хосте (A2/A8).

use std::sync::{Arc, Mutex};

use crate::error::HostError;
use crate::host::RdeHost;
use crate::model::{
    ArtifactDetail, ArtifactInfo, ArtifactManifestDto, ArtifactRuntimeInfo, DocInfo, DocMutationOutcome,
    DocOutline, DocReadRequest, DocRevisionInfo, DocText, DocTextBlock, FileChunk, GitCommitInfo,
    GitCommitOutcome, GitStatusInfo, JobInfo, JobLog, MutationOutcome, NoteInfo, ObjectNode, OutlineBlock,
    SearchHit, SourceInfo, SuggestedEditInfo, TemplateInfo, WorkspaceInfo,
};

/// Id «job'а» mock-хоста (детерминированный; тесты tools/сервера).
pub(crate) const MOCK_JOB_ID: &str = "01MOCKJOB000000000000000000";
/// Id «чекпоинта» mock-хоста.
pub(crate) const MOCK_CHECKPOINT_ID: &str = "01MOCKCHECKPOINT00000000000";
/// Id «предложения правки» mock-хоста.
pub(crate) const MOCK_SUGGESTION_ID: &str = "01MOCKSUGGESTION0000000000";
/// Id «ревизии документа» mock-хоста.
pub(crate) const MOCK_REVISION_ID: &str = "01MOCKREVISION000000000000";

/// Синтетический хост: стабильные данные о «проекте» из трёх объектов.
#[derive(Clone, Debug)]
pub(crate) struct MockHost {
    /// Текст MCP-инструкций (тесты усечения/стабильности).
    pub instructions: String,
    /// Сколько узлов вернуть в `project_tree`.
    pub tree_len: usize,
    /// Журнал мутаций (имя инструмента + id) — проверка «deny ничего не пишет».
    pub mutations: Arc<Mutex<Vec<String>>>,
}

impl Default for MockHost {
    fn default() -> Self {
        Self {
            instructions: "RDE: mock-проект, политика read-only (M1)".to_owned(),
            tree_len: 3,
            mutations: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl MockHost {
    /// Хост с длинными (усекаемыми) инструкциями.
    pub(crate) fn with_long_instructions(bytes: usize) -> Self {
        let mut instructions = String::with_capacity(bytes + 16);
        while instructions.len() < bytes {
            instructions.push_str("строка карты проекта\n");
        }
        Self { instructions, tree_len: 3, mutations: Arc::new(Mutex::new(Vec::new())) }
    }

    /// Записать мутацию в журнал.
    fn record(&self, what: impl Into<String>) {
        match self.mutations.lock() {
            Ok(mut log) => log.push(what.into()),
            Err(poisoned) => poisoned.into_inner().push(what.into()),
        }
    }

    /// Сколько мутаций было выполнено (deny обязан оставить 0).
    pub(crate) fn mutation_count(&self) -> usize {
        match self.mutations.lock() {
            Ok(log) => log.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    /// Журнал мутаций.
    pub(crate) fn mutation_log(&self) -> Vec<String> {
        match self.mutations.lock() {
            Ok(log) => log.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }
}

impl MockHost {
    fn mutation_outcome(&self, what: &str, id: &str, dry_run: bool) -> MutationOutcome {
        if !dry_run {
            self.record(format!("{what}:{id}"));
        }
        MutationOutcome {
            dry_run,
            checkpoint_id: if dry_run { None } else { Some(MOCK_CHECKPOINT_ID.to_owned()) },
            changed_files: if dry_run { Vec::new() } else { vec![format!("objects/{what}/{id}")] },
            detail: None,
        }
    }

    fn job_info(&self, state: &str) -> JobInfo {
        JobInfo {
            job_id: MOCK_JOB_ID.to_owned(),
            state: state.to_owned(),
            created_unix: 1_700_000_000,
            finished_unix: if state == "running" { None } else { Some(1_700_000_010) },
            targets: vec!["art-1".to_owned()],
            include_dependents: false,
            force: false,
            total: 1,
            ran: usize::from(state == "succeeded"),
            skipped: 0,
            failed: usize::from(state == "failed"),
            errors: Vec::new(),
            run_id: Some("run-1".to_owned()),
            log_path: Some(".rde/runs/run-1/run.log".to_owned()),
            checkpoint_id: Some(MOCK_CHECKPOINT_ID.to_owned()),
            message: format!("mock job: {state}"),
        }
    }

    fn suggestion_info(&self, doc_id: &str, block_id: &str, status: &str) -> SuggestedEditInfo {
        SuggestedEditInfo {
            id: MOCK_SUGGESTION_ID.to_owned(),
            doc_id: doc_id.to_owned(),
            block_id: block_id.to_owned(),
            status: status.to_owned(),
            before: "Старый текст блока".to_owned(),
            after: "Новый текст блока".to_owned(),
            author: "MCP agent".to_owned(),
            author_kind: "agent".to_owned(),
            created_unix: 1_700_000_000,
            base_body_hash: "c".repeat(64),
            para_id: Some("12345678".to_owned()),
            note: Some("mock note".to_owned()),
            resolved_unix: if status == "pending" { None } else { Some(1_700_000_020) },
            resolution: if status == "pending" { None } else { Some(status.to_owned()) },
        }
    }

    fn doc_outcome(&self, doc_id: &str, block_id: Option<&str>, dry_run: bool) -> DocMutationOutcome {
        DocMutationOutcome {
            dry_run,
            checkpoint_id: if dry_run { None } else { Some(MOCK_CHECKPOINT_ID.to_owned()) },
            changed_files: if dry_run { Vec::new() } else { vec!["objects/documents/01DOC/report.doc.rde".to_owned()] },
            doc_id: doc_id.to_owned(),
            block_id: block_id.map(str::to_owned),
            before: block_id.map(|_| "Старый текст блока".to_owned()),
            after: block_id.map(|_| "Новый текст блока".to_owned()),
            body_hash: Some("d".repeat(64)),
            revision_id: if dry_run { None } else { Some(MOCK_REVISION_ID.to_owned()) },
            detail: None,
        }
    }
}

fn node(index: usize) -> ObjectNode {
    ObjectNode {
        group: "Артефакты".to_owned(),
        kind: "artifact".to_owned(),
        id: format!("01MOCK{index:020}"),
        title: format!("Мок-артефакт {index}"),
        detail: "compute · 1 выход".to_owned(),
        freshness: "up_to_date".to_owned(),
        service: false,
    }
}

impl RdeHost for MockHost {
    fn workspace_snapshot(&self) -> Result<WorkspaceInfo, HostError> {
        let mut counts = std::collections::BTreeMap::new();
        counts.insert("artifact".to_owned(), self.tree_len);
        Ok(WorkspaceInfo {
            id: "01MOCKPROJECT0000000000000".to_owned(),
            name: "Mock project".to_owned(),
            version: "rde/project@1".to_owned(),
            trusted: true,
            object_count: self.tree_len,
            object_counts_by_kind: counts,
            stale_count: 0,
        })
    }

    fn project_tree(&self) -> Result<Vec<ObjectNode>, HostError> {
        Ok((0..self.tree_len).map(node).collect())
    }

    fn search_project(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, HostError> {
        let query = query.to_lowercase();
        let hits: Vec<SearchHit> = (0..self.tree_len)
            .map(node)
            .filter(|node| node.title.to_lowercase().contains(&query) || node.id.to_lowercase().contains(&query))
            .take(limit)
            .map(|node| SearchHit {
                kind: node.kind,
                id: node.id,
                title: node.title,
                field: "title".to_owned(),
                snippet: query.clone(),
            })
            .collect();
        Ok(hits)
    }

    fn read_project_file(&self, rel_path: &str, range: Option<crate::model::FileRange>) -> Result<FileChunk, HostError> {
        if rel_path.contains("..") || rel_path.starts_with('/') {
            return Err(HostError::path_escape("путь вне проекта"));
        }
        if rel_path.starts_with(".rde/") {
            return Err(HostError::approval_denied("каталог .rde закрыт"));
        }
        let (mode, start, end) = match range {
            Some(crate::model::FileRange::Bytes { offset, limit }) => ("bytes".to_owned(), offset, offset + limit),
            Some(crate::model::FileRange::Lines { start, count }) => ("lines".to_owned(), start, start + count),
            None => ("bytes".to_owned(), 0, 4),
        };
        Ok(FileChunk {
            path: rel_path.to_owned(),
            mode,
            start,
            end,
            total_bytes: 4,
            truncated: false,
            text: "mock".to_owned(),
        })
    }

    fn doc_list(&self) -> Result<Vec<DocInfo>, HostError> {
        Ok(vec![DocInfo {
            id: "doc-1".to_owned(),
            title: "Мок-документ".to_owned(),
            file: "report.doc.rde".to_owned(),
            migrated: true,
            schema_version: Some("2".to_owned()),
            blocks: Some(2),
            inserts: Some(1),
            body_hash: Some("b".repeat(64)),
        }])
    }

    fn doc_outline(&self, doc_id: &str) -> Result<DocOutline, HostError> {
        if doc_id != "doc-1" {
            return Err(HostError::not_found("документ не найден"));
        }
        Ok(DocOutline {
            doc_id: doc_id.to_owned(),
            title: "Мок-документ".to_owned(),
            template: "free".to_owned(),
            schema_version: "2".to_owned(),
            total_blocks: 2,
            total_inserts: 1,
            truncated: false,
            blocks: vec![
                OutlineBlock { id: "b1".to_owned(), kind: "heading1".to_owned(), title: "Введение".to_owned(), chars: 9, index: 0 },
                OutlineBlock { id: "b2".to_owned(), kind: "paragraph".to_owned(), title: "Текст".to_owned(), chars: 4, index: 1 },
            ],
        })
    }

    fn doc_read(&self, doc_id: &str, request: DocReadRequest) -> Result<DocText, HostError> {
        if doc_id != "doc-1" {
            return Err(HostError::not_found("документ не найден"));
        }
        if let Some(ids) = &request.block_ids {
            if let Some(missing) = ids.iter().find(|id| id.as_str() != "b1" && id.as_str() != "b2") {
                return Err(HostError::not_found(format!("блок `{missing}` не найден")));
            }
        }
        Ok(DocText {
            doc_id: doc_id.to_owned(),
            total_blocks: 2,
            returned_blocks: 2,
            truncated: false,
            text: "Введение\n\nТекст".to_owned(),
            blocks: vec![
                DocTextBlock { id: "b1".to_owned(), kind: "heading1".to_owned(), text: "Введение".to_owned() },
                DocTextBlock { id: "b2".to_owned(), kind: "paragraph".to_owned(), text: "Текст".to_owned() },
            ],
        })
    }

    fn artifact_list(&self) -> Result<Vec<ArtifactInfo>, HostError> {
        Ok(vec![ArtifactInfo {
            id: "art-1".to_owned(),
            title: "Мок-артефакт".to_owned(),
            artifact_type: "compute".to_owned(),
            status: "fresh".to_owned(),
            status_reason: None,
            out_hash: Some("a".repeat(64)),
            act_key: Some("b".repeat(64)),
            updated_unix: Some(1_700_000_000),
            run_id: Some("run-1".to_owned()),
            outputs: 1,
        }])
    }

    fn artifact_read(&self, id: &str) -> Result<ArtifactDetail, HostError> {
        if id == "missing" {
            return Err(HostError::not_found("артефакт не найден"));
        }
        let list = self.artifact_list()?;
        let info = list.into_iter().find(|artifact| artifact.id == id).unwrap_or(ArtifactInfo {
            id: id.to_owned(),
            title: "Мок-артефакт".to_owned(),
            artifact_type: "compute".to_owned(),
            status: "never_run".to_owned(),
            status_reason: None,
            out_hash: None,
            act_key: None,
            updated_unix: None,
            run_id: None,
            outputs: 0,
        });
        Ok(ArtifactDetail {
            info,
            rebuild: "manual".to_owned(),
            runtime: Some(ArtifactRuntimeInfo {
                kind: "python".to_owned(),
                env: "project".to_owned(),
                entry: "compute.py".to_owned(),
            }),
            params: serde_json::json!({"alpha": 1}),
            outputs_declared: vec!["out/result.json".to_owned()],
            outputs: vec![crate::model::ArtifactOutputInfo {
                rel: "out/result.json".to_owned(),
                size: 12,
                blob: "c".repeat(64),
            }],
            script_files: Vec::new(),
            timeout_s: None,
            network_allow: None,
            groups: serde_json::json!([]),
        })
    }

    fn source_list(&self) -> Result<Vec<SourceInfo>, HostError> {
        Ok(vec![SourceInfo {
            id: "src-1".to_owned(),
            title: "Мок-источник".to_owned(),
            kind: "file".to_owned(),
            file: Some("sources/doc.pdf".to_owned()),
            url: None,
            retrieved_at: None,
            bibliography: Some("Мок-источник // 2026.".to_owned()),
        }])
    }

    fn note_list(&self) -> Result<Vec<NoteInfo>, HostError> {
        Ok(vec![NoteInfo { id: "note-1".to_owned(), title: "Мок-заметка".to_owned(), file: "notes/idea.md".to_owned(), bytes: Some(5) }])
    }

    fn template_list(&self) -> Result<Vec<TemplateInfo>, HostError> {
        Ok(vec![TemplateInfo { id: "free".to_owned(), title: "Свободный".to_owned(), description: "без ГОСТ".to_owned() }])
    }

    fn git_status(&self) -> Result<GitStatusInfo, HostError> {
        Ok(GitStatusInfo {
            available: true,
            is_repo: true,
            branch: Some("main".to_owned()),
            changed: Vec::new(),
            untracked: Vec::new(),
            last_commit: None,
            ahead: Some(0),
        })
    }

    // --- M2: мутации/запуск (тесты слоя без ФС) ---------------------------------

    fn artifact_write_manifest(
        &self,
        id: &str,
        manifest: ArtifactManifestDto,
        dry_run: bool,
        _expected_sha256: Option<&str>,
    ) -> Result<MutationOutcome, HostError> {
        if id == "missing" {
            return Err(HostError::not_found("артефакт не найден"));
        }
        let _ = manifest;
        Ok(self.mutation_outcome("artifact_manifest", id, dry_run))
    }

    fn artifact_write_script(
        &self,
        id: &str,
        file: &str,
        _content: &str,
        dry_run: bool,
        _expected_sha256: Option<&str>,
    ) -> Result<MutationOutcome, HostError> {
        if file.contains("..") {
            return Err(HostError::path_escape("путь выходит из каталога артефакта"));
        }
        Ok(self.mutation_outcome("artifact_script", id, dry_run))
    }

    fn note_write(
        &self,
        id: &str,
        _content: Option<String>,
        _title: Option<String>,
        dry_run: bool,
        _expected_sha256: Option<&str>,
    ) -> Result<MutationOutcome, HostError> {
        if id == "missing" {
            return Err(HostError::not_found("заметка не найдена"));
        }
        Ok(self.mutation_outcome("note", id, dry_run))
    }

    fn artifact_run(
        &self,
        _targets: Option<Vec<String>>,
        _include_dependents: bool,
        _force: bool,
    ) -> Result<JobInfo, HostError> {
        self.record("artifact_run:mock");
        Ok(self.job_info("succeeded"))
    }

    fn run_status(&self, job_id: &str) -> Result<JobInfo, HostError> {
        if job_id != MOCK_JOB_ID {
            return Err(HostError::job_not_found(format!("job `{job_id}` не найден")).with_hint("jobs живут в процессе сервера"));
        }
        Ok(self.job_info("succeeded"))
    }

    fn run_cancel(&self, job_id: &str) -> Result<JobInfo, HostError> {
        if job_id != MOCK_JOB_ID {
            return Err(HostError::job_not_found(format!("job `{job_id}` не найден")));
        }
        self.record("run_cancel:mock");
        Ok(self.job_info("cancelled"))
    }

    fn run_log(&self, job_id: &str, offset: u64, max_bytes: usize) -> Result<JobLog, HostError> {
        if job_id != MOCK_JOB_ID {
            return Err(HostError::job_not_found(format!("job `{job_id}` не найден")));
        }
        let text = format!("mock log offset={offset} max={max_bytes}");
        Ok(JobLog {
            job_id: job_id.to_owned(),
            offset,
            next_offset: offset + text.len() as u64,
            eof: true,
            truncated: false,
            text,
        })
    }

    fn git_commit(&self, message: &str, dry_run: bool) -> Result<GitCommitOutcome, HostError> {
        if message.trim().is_empty() {
            return Err(HostError::invalid_argument("сообщение коммита пустое"));
        }
        if !dry_run {
            self.record("git_commit:mock");
        }
        Ok(GitCommitOutcome {
            dry_run,
            checkpoint_id: if dry_run { None } else { Some(MOCK_CHECKPOINT_ID.to_owned()) },
            commit: if dry_run {
                None
            } else {
                Some(GitCommitInfo {
                    id: "abc1234".to_owned(),
                    summary: message.to_owned(),
                    author: "Mock".to_owned(),
                    time: "2026-10-27".to_owned(),
                })
            },
            files: 1,
            message: if dry_run { "будет закоммичен 1 файл".to_owned() } else { "коммит создан".to_owned() },
        })
    }

    fn checkpoint_restore(&self, checkpoint_id: &str) -> Result<MutationOutcome, HostError> {
        self.record(format!("checkpoint_restore:{checkpoint_id}"));
        Ok(MutationOutcome {
            dry_run: false,
            checkpoint_id: Some(checkpoint_id.to_owned()),
            changed_files: vec!["objects/artifacts/art-1/artifact.toml".to_owned()],
            detail: Some("чекпоинт откачен".to_owned()),
        })
    }

    // --- M2.5: suggested edits документа / правки блоков / ревизии -------------

    fn doc_suggest_edit(
        &self,
        doc_id: &str,
        block_id: &str,
        _expected_text: &str,
        _new_text: &str,
        _note: Option<&str>,
    ) -> Result<SuggestedEditInfo, HostError> {
        if doc_id != "doc-1" {
            return Err(HostError::not_found("документ не найден"));
        }
        if block_id == "b999" {
            return Err(HostError::not_found("блок не найден"));
        }
        self.record(format!("doc_suggest_edit:{doc_id}:{block_id}"));
        Ok(self.suggestion_info(doc_id, block_id, "pending"))
    }

    fn suggestion_list(
        &self,
        doc_id: Option<&str>,
        status: Option<&str>,
        _limit: usize,
    ) -> Result<Vec<SuggestedEditInfo>, HostError> {
        if doc_id.is_some_and(|doc_id| doc_id != "doc-1") {
            return Ok(Vec::new());
        }
        Ok(vec![self.suggestion_info("doc-1", "b2", status.unwrap_or("pending"))])
    }

    fn suggestion_accept(&self, suggestion_id: &str) -> Result<DocMutationOutcome, HostError> {
        if suggestion_id == "01NOSUCH" {
            return Err(HostError::not_found("предложение не найдено"));
        }
        self.record(format!("suggestion_accept:{suggestion_id}"));
        Ok(self.doc_outcome("doc-1", Some("b2"), false))
    }

    fn suggestion_reject(
        &self,
        suggestion_id: &str,
        _reason: Option<&str>,
    ) -> Result<SuggestedEditInfo, HostError> {
        if suggestion_id == "01NOSUCH" {
            return Err(HostError::not_found("предложение не найдено"));
        }
        self.record(format!("suggestion_reject:{suggestion_id}"));
        Ok(self.suggestion_info("doc-1", "b2", "rejected"))
    }

    fn doc_replace_range(
        &self,
        doc_id: &str,
        block_id: &str,
        _expected_text: &str,
        _new_text: &str,
        _expected_body_hash: Option<&str>,
        dry_run: bool,
    ) -> Result<DocMutationOutcome, HostError> {
        if doc_id != "doc-1" {
            return Err(HostError::not_found("документ не найден"));
        }
        if !dry_run {
            self.record(format!("doc_replace_range:{doc_id}:{block_id}"));
        }
        Ok(self.doc_outcome(doc_id, Some(block_id), dry_run))
    }

    fn revision_list(&self, doc_id: &str, _limit: usize) -> Result<Vec<DocRevisionInfo>, HostError> {
        if doc_id != "doc-1" {
            return Err(HostError::not_found("документ не найден"));
        }
        Ok(vec![DocRevisionInfo {
            revision_id: MOCK_REVISION_ID.to_owned(),
            doc_id: doc_id.to_owned(),
            created_unix: 1_700_000_000,
            note: "mock revision".to_owned(),
            root_hash: Some("e".repeat(64)),
            current: true,
        }])
    }

    fn revision_restore(&self, doc_id: &str, revision_id: &str) -> Result<DocMutationOutcome, HostError> {
        self.record(format!("revision_restore:{doc_id}:{revision_id}"));
        Ok(self.doc_outcome(doc_id, None, false))
    }

    fn instructions(&self) -> Result<String, HostError> {
        Ok(self.instructions.clone())
    }
}
