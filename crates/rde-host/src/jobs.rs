//! Асинхронные job'ы запуска артефактов (M2, R-AGT-09): реестр состояний + исполнитель.
//!
//! `artifact_run` не блокирует `tools/call`: job запускается в отдельном потоке, а клиент
//! опрашивает `run_status` / читает `run_log` / отменяет `run_cancel` (флаг отмены уходит в
//! `rde-run`, где SIGTERM → grace → SIGKILL). Итог — компактная сводка (`ran/skipped/failed`,
//! ошибки, `run_id`, `log_path`), а не полный отчёт.
//!
//! Jobs живут **в процессе** сервера и не переживают рестарт (M2, документировано). Одновременно
//! выполняется один job (`RDE_BUSY`): так проще гарантии отмены и предсказуемая нагрузка.
//!
//! Исполнитель вынесен в трейт [`ArtifactExecutor`]: реальный — [`DagExecutor`] (`rde-dag::update`),
//! тестовый — stub (unit-тесты state machine/cancel без `uv`/python, O5).

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rde_mcp::model::JobInfo;
use rde_mcp::HostError;

/// Сколько завершённых job'ов хранить в реестре (история для `run_status`).
const JOB_HISTORY: usize = 20;

/// Максимум ошибок в сводке job'а (не раздуваем ответ/контекст).
const JOB_MAX_ERRORS: usize = 20;

/// Запрос на обновление артефактов.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecRequest {
    /// Цели (`None` — все устаревшие).
    pub targets: Option<Vec<String>>,
    /// Включить транзитивно зависимые.
    pub include_dependents: bool,
    /// Принудительный пересчёт целей.
    pub force: bool,
}

/// Компактный итог выполнения job'а.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecOutcome {
    /// Всего артефактов в отчёте.
    pub total: usize,
    /// Запущено.
    pub ran: usize,
    /// Пропущено.
    pub skipped: usize,
    /// Неудач.
    pub failed: usize,
    /// Ошибки (санитизируются при записи в job).
    pub errors: Vec<String>,
    /// Id запусков (`rde-run`).
    pub run_ids: Vec<String>,
    /// Абсолютные пути логов запусков.
    pub log_paths: Vec<PathBuf>,
    /// Итоговое сообщение.
    pub message: String,
}

/// Исполнитель обновления артефактов (реальный — `rde-dag::update`; в тестах — stub).
pub trait ArtifactExecutor: Send + Sync {
    /// Выполнить обновление; `cancel` — флаг отмены (пробрасывается в `rde-run`).
    fn execute(
        &self,
        root: &Path,
        request: &ExecRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<ExecOutcome, String>;
}

/// Реальный исполнитель: `rde-dag::update` в отдельном `Store` (lock store реентерабелен в процессе).
#[derive(Clone, Copy, Debug, Default)]
pub struct DagExecutor;

impl ArtifactExecutor for DagExecutor {
    fn execute(
        &self,
        root: &Path,
        request: &ExecRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<ExecOutcome, String> {
        let project = rde_core::load_project(root)?;
        let mut store = rde_store::Store::open(root).map_err(|error| error.to_string())?;
        let options = rde_dag::UpdateOptions {
            targets: request.targets.clone(),
            include_dependents: request.include_dependents,
            force: request.force,
            // Гранулярный откат: пачка — не цель MCP-запуска; continue_on_error — job не падает
            // целиком из-за одного артефакта, итог содержит failed/errors.
            batch: false,
            continue_on_error: true,
            cancel: Some(cancel),
        };
        let report = rde_dag::update(&project, &mut store, &options)?;
        let mut run_ids = Vec::new();
        let mut log_paths = Vec::new();
        for artifact in &report.artifacts {
            if let Some(run_id) = &artifact.run_id {
                run_ids.push(run_id.clone());
            }
            if let Some(path) = &artifact.log_path {
                log_paths.push(path.clone());
            }
        }
        let message = format!(
            "обновление: запущено {}, пропущено {}, ошибок {}",
            report.ran(),
            report.skipped(),
            report.failed()
        );
        Ok(ExecOutcome {
            total: report.artifacts.len(),
            ran: report.ran(),
            skipped: report.skipped(),
            failed: report.failed(),
            errors: report.errors,
            run_ids,
            log_paths,
            message,
        })
    }
}

/// Состояние job'а (под мьютексом; читается `run_status`).
#[derive(Clone, Debug)]
struct JobState {
    state: String,
    finished_unix: Option<u64>,
    total: usize,
    ran: usize,
    skipped: usize,
    failed: usize,
    errors: Vec<String>,
    run_id: Option<String>,
    log_path: Option<String>,
    log_path_abs: Option<PathBuf>,
    message: String,
}

/// Один job запуска артефактов.
pub struct Job {
    /// Идентификатор (ULID).
    pub id: String,
    /// Unix-время создания.
    pub created_unix: u64,
    /// Запрошенные цели.
    pub targets: Vec<String>,
    /// Включены зависимые.
    pub include_dependents: bool,
    /// Принудительный пересчёт.
    pub force: bool,
    /// Чекпоинт, созданный перед запуском.
    pub checkpoint_id: Option<String>,
    cancel: Arc<AtomicBool>,
    state: Mutex<JobState>,
    handle: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl Job {
    /// Снимок job'а для контракта хоста.
    pub fn info(&self) -> JobInfo {
        let state = match self.state.lock() {
            Ok(state) => state.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        JobInfo {
            job_id: self.id.clone(),
            state: state.state,
            created_unix: self.created_unix,
            finished_unix: state.finished_unix,
            targets: self.targets.clone(),
            include_dependents: self.include_dependents,
            force: self.force,
            total: state.total,
            ran: state.ran,
            skipped: state.skipped,
            failed: state.failed,
            errors: state.errors,
            run_id: state.run_id,
            log_path: state.log_path,
            checkpoint_id: self.checkpoint_id.clone(),
            message: state.message,
        }
    }

    /// Запрошена ли отмена.
    pub fn is_cancel_requested(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Абсолютный путь лога (для `run_log`).
    pub fn log_path_abs(&self) -> Option<PathBuf> {
        match self.state.lock() {
            Ok(state) => state.log_path_abs.clone(),
            Err(poisoned) => poisoned.into_inner().log_path_abs.clone(),
        }
    }

    /// Сводка job'а: итог исполнителя (ошибки/пути санитизируются в `finish`).
    fn finish(&self, state: &'static str, outcome: ExecOutcome, root: &Path) {
        let root_text = root.to_string_lossy();
        let sanitize = |text: String| text.replace(root_text.as_ref(), "<project>");
        let rel_log_path = outcome
            .log_paths
            .last()
            .and_then(|path| path.strip_prefix(root).ok())
            .map(rel_posix);
        let log_path_abs = outcome.log_paths.last().cloned();
        let errors: Vec<String> =
            outcome.errors.into_iter().take(JOB_MAX_ERRORS).map(sanitize).collect();
        let message = sanitize(outcome.message);
        let mut state_data = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state_data.state = state.to_owned();
        state_data.finished_unix = Some(now_unix());
        state_data.total = outcome.total;
        state_data.ran = outcome.ran;
        state_data.skipped = outcome.skipped;
        state_data.failed = outcome.failed;
        state_data.errors = errors;
        state_data.run_id = outcome.run_ids.last().cloned();
        state_data.log_path = rel_log_path;
        state_data.log_path_abs = log_path_abs;
        state_data.message = message;
    }

    fn mark_cancel_requested(&self) {
        self.cancel.store(true, Ordering::SeqCst);
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        if state.state == "running" && !state.message.contains("отмена") {
            state.message = format!("{} (запрошена отмена)", state.message);
        }
    }
}

/// Реестр job'ов: история + активный job (не больше одного, `RDE_BUSY`).
#[derive(Default)]
pub struct JobRegistry {
    jobs: Mutex<VecDeque<Arc<Job>>>,
    active: Mutex<Option<Arc<Job>>>,
}

impl JobRegistry {
    /// Новый пустой реестр.
    pub fn new() -> Self {
        Self::default()
    }

    /// Активный (выполняющийся) job, если есть.
    pub fn active(&self) -> Option<Arc<Job>> {
        match self.active.lock() {
            Ok(active) => active.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Запустить job в отдельном потоке; возвращает снимок «running» сразу (не блокирует вызов).
    ///
    /// # Errors
    ///
    /// `RDE_BUSY` — уже выполняется другой job (M2: один job за раз).
    pub fn start(
        &self,
        registry: Arc<JobRegistry>,
        root: PathBuf,
        executor: Arc<dyn ArtifactExecutor>,
        request: ExecRequest,
        checkpoint_id: Option<String>,
    ) -> Result<JobInfo, HostError> {
        if let Some(active) = self.active() {
            if active.info().state == "running" {
                return Err(HostError::busy(format!(
                    "уже выполняется job `{}` — дождитесь завершения или отмените его",
                    active.id
                ))
                .with_hint("run_status/run_cancel по активному job_id"));
            }
        }
        let id = ulid::Ulid::generate().to_string();
        let targets = request.targets.clone().unwrap_or_default();
        let job = Arc::new(Job {
            id,
            created_unix: now_unix(),
            targets,
            include_dependents: request.include_dependents,
            force: request.force,
            checkpoint_id,
            cancel: Arc::new(AtomicBool::new(false)),
            state: Mutex::new(JobState {
                state: "running".to_owned(),
                finished_unix: None,
                total: 0,
                ran: 0,
                skipped: 0,
                failed: 0,
                errors: Vec::new(),
                run_id: None,
                log_path: None,
                log_path_abs: None,
                message: "job запущен".to_owned(),
            }),
            handle: Mutex::new(None),
        });
        {
            let mut jobs = match self.jobs.lock() {
                Ok(jobs) => jobs,
                Err(poisoned) => poisoned.into_inner(),
            };
            jobs.push_back(Arc::clone(&job));
            while jobs.len() > JOB_HISTORY {
                jobs.pop_front();
            }
        }
        {
            let mut active = match self.active.lock() {
                Ok(active) => active,
                Err(poisoned) => poisoned.into_inner(),
            };
            *active = Some(Arc::clone(&job));
        }
        let info = job.info();
        let thread_job = Arc::clone(&job);
        let handle = std::thread::spawn(move || {
            let result = executor.execute(&root, &request, Arc::clone(&thread_job.cancel));
            let final_state = match &result {
                _ if thread_job.is_cancel_requested() => "cancelled",
                Ok(outcome) if outcome.failed > 0 => "failed",
                Ok(_) => "succeeded",
                Err(_) => "failed",
            };
            match result {
                Ok(outcome) => thread_job.finish(final_state, outcome, &root),
                Err(error) => thread_job.finish(
                    "failed",
                    ExecOutcome { message: format!("job упал: {error}"), ..Default::default() },
                    &root,
                ),
            }
            let mut active = match registry.active.lock() {
                Ok(active) => active,
                Err(poisoned) => poisoned.into_inner(),
            };
            if active.as_ref().is_some_and(|active| active.id == thread_job.id) {
                *active = None;
            }
        });
        if let Ok(mut slot) = job.handle.lock() {
            *slot = Some(handle);
        }
        Ok(info)
    }

    /// Найти job по id (история + активный).
    pub fn get(&self, job_id: &str) -> Option<Arc<Job>> {
        let jobs = match self.jobs.lock() {
            Ok(jobs) => jobs,
            Err(poisoned) => poisoned.into_inner(),
        };
        jobs.iter().find(|job| job.id == job_id).cloned()
    }

    /// Запросить отмену job'а (флаг отмены увидит `rde-run`).
    pub fn cancel(&self, job_id: &str) -> Result<JobInfo, HostError> {
        let job = self
            .get(job_id)
            .ok_or_else(|| HostError::job_not_found(format!("job `{job_id}` не найден")).with_hint("jobs живут в процессе сервера (M2)"))?;
        job.mark_cancel_requested();
        Ok(job.info())
    }

    /// Отменить активные jobs и дождаться завершения потоков (bounded; graceful shutdown).
    pub fn shutdown(&self) {
        let jobs: Vec<Arc<Job>> = match self.jobs.lock() {
            Ok(jobs) => jobs.iter().cloned().collect(),
            Err(poisoned) => poisoned.into_inner().iter().cloned().collect(),
        };
        for job in &jobs {
            if job.info().state == "running" {
                job.cancel.store(true, Ordering::SeqCst);
            }
        }
        for job in &jobs {
            let handle = match job.handle.lock() {
                Ok(mut slot) => slot.take(),
                Err(poisoned) => poisoned.into_inner().take(),
            };
            let Some(handle) = handle else { continue };
            let (sender, receiver) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let _ = handle.join();
                let _ = sender.send(());
            });
            let _ = receiver.recv_timeout(Duration::from_secs(5));
        }
    }
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|duration| duration.as_secs()).unwrap_or(0)
}

fn rel_posix(path: &Path) -> String {
    let mut out = String::new();
    for component in path.components() {
        if !out.is_empty() {
            out.push('/');
        }
        out.push_str(&component.as_os_str().to_string_lossy());
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicBool;

    use super::*;

    /// Stub-исполнитель: ждёт до отмены/таймаута и отдаёт заданный результат.
    struct StubExecutor {
        delay_ms: u64,
        outcome: Result<ExecOutcome, String>,
    }

    impl ArtifactExecutor for StubExecutor {
        fn execute(
            &self,
            _root: &Path,
            _request: &ExecRequest,
            cancel: Arc<AtomicBool>,
        ) -> Result<ExecOutcome, String> {
            let deadline = std::time::Instant::now() + Duration::from_millis(self.delay_ms);
            while std::time::Instant::now() < deadline {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            self.outcome.clone()
        }
    }

    fn wait_finished(registry: &JobRegistry, job_id: &str) -> JobInfo {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let info = registry.get(job_id).expect("job в реестре").info();
            if info.state != "running" {
                return info;
            }
            assert!(std::time::Instant::now() < deadline, "job не завершился за 5 с");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn start(
        registry: &Arc<JobRegistry>,
        executor: Arc<dyn ArtifactExecutor>,
        request: ExecRequest,
    ) -> JobInfo {
        registry
            .start(
                Arc::clone(registry),
                std::env::temp_dir(),
                executor,
                request,
                Some("01CHECKPOINT".to_owned()),
            )
            .expect("job стартует")
    }

    fn success() -> ExecOutcome {
        ExecOutcome {
            total: 2,
            ran: 1,
            skipped: 1,
            failed: 0,
            errors: Vec::new(),
            run_ids: vec!["run-1".to_owned()],
            log_paths: Vec::new(),
            message: "обновление: запущено 1, пропущено 1, ошибок 0".to_owned(),
        }
    }

    #[test]
    fn job_state_machine_success() {
        let registry = Arc::new(JobRegistry::new());
        let job = start(
            &registry,
            Arc::new(StubExecutor { delay_ms: 10, outcome: Ok(success()) }),
            ExecRequest { targets: Some(vec!["art-1".to_owned()]), ..Default::default() },
        );
        assert_eq!(job.state, "running");
        assert_eq!(job.checkpoint_id.as_deref(), Some("01CHECKPOINT"));
        let finished = wait_finished(&registry, &job.job_id);
        assert_eq!(finished.state, "succeeded");
        assert_eq!(finished.ran, 1);
        assert_eq!(finished.skipped, 1);
        assert_eq!(finished.run_id.as_deref(), Some("run-1"));
        assert!(finished.finished_unix.is_some());
        assert!(registry.active().is_none(), "активный job снят после завершения");
    }

    #[test]
    fn job_failure_reports_errors() {
        let registry = Arc::new(JobRegistry::new());
        let outcome = ExecOutcome { failed: 1, errors: vec!["скрипт упал".to_owned()], ..Default::default() };
        let job = start(&registry, Arc::new(StubExecutor { delay_ms: 0, outcome: Ok(outcome) }), ExecRequest::default());
        let finished = wait_finished(&registry, &job.job_id);
        assert_eq!(finished.state, "failed");
        assert_eq!(finished.failed, 1);
        assert!(finished.errors[0].contains("скрипт упал"));
    }

    #[test]
    fn job_cancel_marks_cancelled() {
        let registry = Arc::new(JobRegistry::new());
        let job = start(
            &registry,
            Arc::new(StubExecutor { delay_ms: 2_000, outcome: Ok(success()) }),
            ExecRequest::default(),
        );
        let cancelled = registry.cancel(&job.job_id).expect("отмена принимается");
        assert_eq!(cancelled.state, "running", "job ещё не остановлен");
        assert!(cancelled.message.contains("отмена"));
        let finished = wait_finished(&registry, &job.job_id);
        assert_eq!(finished.state, "cancelled");
    }

    #[test]
    fn one_job_at_a_time_returns_busy() {
        let registry = Arc::new(JobRegistry::new());
        let job = start(
            &registry,
            Arc::new(StubExecutor { delay_ms: 500, outcome: Ok(success()) }),
            ExecRequest::default(),
        );
        let error = registry
            .start(
                Arc::clone(&registry),
                std::env::temp_dir(),
                Arc::new(StubExecutor { delay_ms: 0, outcome: Ok(success()) }),
                ExecRequest::default(),
                None,
            )
            .expect_err("второй job не стартует");
        assert_eq!(error.code, rde_mcp::RdeErrorCode::Busy);
        let _ = registry.cancel(&job.job_id);
        let _ = wait_finished(&registry, &job.job_id);
    }

    #[test]
    fn unknown_job_is_job_not_found() {
        let registry = JobRegistry::new();
        assert!(registry.get("01NOSUCH").is_none());
        let error = registry.cancel("01NOSUCH").expect_err("нет job'а");
        assert_eq!(error.code, rde_mcp::RdeErrorCode::JobNotFound);
    }

    #[test]
    fn job_info_does_not_leak_project_root() {
        let registry = Arc::new(JobRegistry::new());
        let root = std::env::temp_dir().join("rde-host-job-root-leak");
        let job = registry
            .start(
                Arc::clone(&registry),
                root.clone(),
                Arc::new(StubExecutor {
                    delay_ms: 0,
                    outcome: Err(format!("ошибка в {}", root.display())),
                }),
                ExecRequest::default(),
                None,
            )
            .expect("job стартует");
        let finished = wait_finished(&registry, &job.job_id);
        assert_eq!(finished.state, "failed");
        assert!(!finished.message.contains(&*root.to_string_lossy()), "путь проекта не утекает: {}", finished.message);
        assert!(finished.message.contains("<project>"));
    }

    #[test]
    fn shutdown_cancels_active_jobs() {
        let registry = Arc::new(JobRegistry::new());
        let job = start(
            &registry,
            Arc::new(StubExecutor { delay_ms: 2_000, outcome: Ok(success()) }),
            ExecRequest::default(),
        );
        registry.shutdown();
        let finished = wait_finished(&registry, &job.job_id);
        assert_eq!(finished.state, "cancelled");
    }
}
