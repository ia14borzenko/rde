//! Интеграционные тесты мутаций M2 (`rde-host`): чекпоинты + авто-откат, запись
//! манифеста/скрипта/заметки, `RDE_CONFLICT`, git-коммит, job-модель запуска артефактов.
//!
//! Работают на временной копии `demo-project/` (без `.rde`/`exports`/`out`/`.venv`/`.git`),
//! как и e2e MCP-сервера; запуск артефактов — через stub-исполнитель (без `uv`/python).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rde_host::{ArtifactExecutor, ExecOutcome, ExecRequest, HeadlessHost};
use rde_mcp::model::ArtifactManifestDto;
use rde_mcp::{HostError, RdeErrorCode, RdeHost};

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-host-mut-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_project(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap().flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if matches!(&*name, ".rde" | "exports" | "out" | "__pycache__" | ".venv" | ".git" | "target")
            || name.ends_with(".bak")
        {
            continue;
        }
        let from = entry.path();
        let to = dst.join(&*name);
        if from.is_dir() {
            copy_project(&from, &to);
        } else {
            std::fs::copy(&from, &to).unwrap();
        }
    }
}

fn demo_copy(name: &str) -> PathBuf {
    let root = temp_dir(name);
    copy_project(&rde_core::demo_project_dir(), &root);
    root
}

fn demo_host(name: &str) -> (HeadlessHost, PathBuf) {
    let root = demo_copy(name);
    let host = HeadlessHost::open(&root).expect("демо-проект открывается headless-хостом");
    (host, root)
}

fn expect_code(error: HostError, code: RdeErrorCode) {
    assert_eq!(error.code, code, "ожидался код {code}, получено {error}");
}

fn compute_artifact_id(host: &HeadlessHost) -> String {
    host.artifact_list()
        .unwrap()
        .into_iter()
        .find(|artifact| artifact.artifact_type == "compute")
        .expect("в демо есть compute-артефакт")
        .id
}

fn note_id(host: &HeadlessHost) -> String {
    host.note_list().unwrap().first().expect("в демо есть заметка").id.clone()
}

// --- запись манифеста ------------------------------------------------------

#[test]
fn manifest_write_is_checkpointed_visible_and_restorable() {
    let (host, root) = demo_host("manifest");
    let id = compute_artifact_id(&host);
    let manifest_path = root.join("objects/artifacts").join(&id).join("artifact.toml");
    let original = std::fs::read_to_string(&manifest_path).unwrap();

    let patch = ArtifactManifestDto {
        title: Some("Заголовок, изменённый M2".to_owned()),
        rebuild: None,
        kind: None,
        runtime: None,
        params: Some(serde_json::json!({"m2": true})),
        outputs: None,
        script_files: None,
        timeout_s: None,
        network_allow: None,
        groups: None,
        values: None,
    };
    let outcome = host.artifact_write_manifest(&id, patch, false, None).unwrap();
    assert!(!outcome.dry_run);
    let checkpoint_id = outcome.checkpoint_id.expect("чекпоинт создан");
    assert_eq!(outcome.changed_files.len(), 1);
    assert!(outcome.changed_files[0].ends_with("artifact.toml"));
    // Файл изменён, проект видит новый заголовок, чекпоинт лежит в `.rde/agent/checkpoints`.
    let changed = std::fs::read_to_string(&manifest_path).unwrap();
    assert!(changed.contains("Заголовок, изменённый M2"));
    let info = host.artifact_list().unwrap().into_iter().find(|artifact| artifact.id == id).unwrap();
    assert_eq!(info.title, "Заголовок, изменённый M2");
    assert!(root.join(".rde/agent/checkpoints").join(&checkpoint_id).join("manifest.json").is_file());
    // Откат чекпоинта: файл байт-в-байт как до операции, чекпоинт помечен откаченным.
    let restored = host.checkpoint_restore(&checkpoint_id).unwrap();
    assert_eq!(restored.changed_files.len(), 1);
    assert_eq!(std::fs::read_to_string(&manifest_path).unwrap(), original);
    let manifest_json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join(".rde/agent/checkpoints").join(&checkpoint_id).join("manifest.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest_json["rolled_back"], true);
    // Повторный откат — RDE_CONFLICT.
    expect_code(host.checkpoint_restore(&checkpoint_id).unwrap_err(), RdeErrorCode::Conflict);
}

#[test]
fn manifest_write_works_for_data_and_constants_manifests_without_params() {
    // Регрессия: у data/constants-манифестов `params` = null; TOML не сериализует null —
    // хост канонизирует в `params = {}`, иначе запись манифеста падала бы с RDE_INVALID_ARGUMENT.
    let (host, root) = demo_host("manifest-data");
    let artifacts = host.artifact_list().unwrap();
    for kind in ["data", "constants"] {
        let id = artifacts.iter().find(|artifact| artifact.artifact_type == kind).expect(kind).id.clone();
        let path = root.join("objects/artifacts").join(&id).join("artifact.toml");
        let before = std::fs::read_to_string(&path).unwrap();
        assert!(before.contains(kind));
        let outcome = host
            .artifact_write_manifest(
                &id,
                ArtifactManifestDto { title: Some(format!("M2 {kind}")), ..Default::default() },
                false,
                None,
            )
            .unwrap();
        assert!(outcome.checkpoint_id.is_some());
        let after = std::fs::read_to_string(&path).unwrap();
        assert!(after.contains(&format!("M2 {kind}")), "после записи ({kind}): {after}");
        assert!(
            after.contains("[params]") || after.contains("params = {}"),
            "null params канонизирован ({kind}): {after}"
        );
        // Проект остаётся загружаемым, объект/значения не потеряны.
        let project = rde_core::load(&root).expect("проект загружается");
        assert!(project.object(&id).is_some());
    }
}

#[test]
fn manifest_dry_run_does_not_write() {
    let (host, root) = demo_host("manifest-dry");
    let id = compute_artifact_id(&host);
    let manifest_path = root.join("objects/artifacts").join(&id).join("artifact.toml");
    let original = std::fs::read_to_string(&manifest_path).unwrap();
    let outcome = host
        .artifact_write_manifest(&id, ArtifactManifestDto { title: Some("dry".to_owned()), ..Default::default() }, true, None)
        .unwrap();
    assert!(outcome.dry_run);
    assert!(outcome.checkpoint_id.is_none());
    assert_eq!(std::fs::read_to_string(&manifest_path).unwrap(), original, "dry_run не пишет файлы");
    assert!(!root.join(".rde/agent/checkpoints").exists(), "dry_run не создаёт чекпоинтов");
}

#[test]
fn manifest_validation_error_keeps_file_intact() {
    let (host, root) = demo_host("manifest-invalid");
    let id = compute_artifact_id(&host);
    let manifest_path = root.join("objects/artifacts").join(&id).join("artifact.toml");
    let original = std::fs::read_to_string(&manifest_path).unwrap();
    // `values` для constants-артефакта требует `value: f64` — невалидная полезная нагрузка.
    let patch: ArtifactManifestDto =
        serde_json::from_value(serde_json::json!({"values": {"sigma": {"unit": "м"}}})).expect("DTO парсится");
    let error = host.artifact_write_manifest(&id, patch, false, None).expect_err("невалидный манифест");
    assert_eq!(error.code, RdeErrorCode::InvalidArgument, "{error}");
    assert_eq!(std::fs::read_to_string(&manifest_path).unwrap(), original, "файл не тронут");
    expect_code(
        host.artifact_write_manifest("01NOTANARTIFACT00000000000", ArtifactManifestDto::default(), true, None)
            .unwrap_err(),
        RdeErrorCode::NotFound,
    );
}

#[test]
fn manifest_external_edit_is_detected_by_expected_sha() {
    let (host, root) = demo_host("manifest-conflict");
    let id = compute_artifact_id(&host);
    let manifest_path = root.join("objects/artifacts").join(&id).join("artifact.toml");
    let error = host
        .artifact_write_manifest(
            &id,
            ArtifactManifestDto { title: Some("x".to_owned()), ..Default::default() },
            false,
            Some(&"0".repeat(64)),
        )
        .expect_err("устаревший expected_sha256");
    assert_eq!(error.code, RdeErrorCode::Conflict);
    // Файл не перезаписан.
    assert!(!std::fs::read_to_string(&manifest_path).unwrap().contains("title = \"x\""));
}

// --- запись скрипта --------------------------------------------------------

#[test]
fn script_write_is_sandboxed_to_declared_files() {
    let (host, root) = demo_host("script");
    let id = compute_artifact_id(&host);
    let detail = host.artifact_read(&id).unwrap();
    let runtime = detail.runtime.expect("у compute-артефакта есть runtime");
    let entry = runtime.entry.clone();
    let script_path = root.join("objects/artifacts").join(&id).join(&entry);
    let outcome = host
        .artifact_write_script(&id, &entry, "# M2: скрипт\nprint(1)\n", false, None)
        .unwrap();
    assert_eq!(outcome.changed_files.len(), 1);
    assert!(std::fs::read_to_string(&script_path).unwrap().contains("M2"));
    // Чужой файл (не из runtime.entry/script_files) — отказ, файла нет.
    let error = host
        .artifact_write_script(&id, "evil.sh", "rm -rf /\n", false, None)
        .expect_err("чужой файл запрещён");
    assert_eq!(error.code, RdeErrorCode::InvalidArgument);
    assert!(!root.join("objects/artifacts").join(&id).join("evil.sh").exists());
    // Выход из каталога артефакта — RDE_PATH_ESCAPE.
    let error = host
        .artifact_write_script(&id, "../escape.py", "x", false, None)
        .expect_err("выход из каталога");
    assert_eq!(error.code, RdeErrorCode::PathEscape);
    // Слишком большой файл — RDE_OUT_OF_RANGE.
    let big = "x".repeat(rde_mcp::model::ARTIFACT_SCRIPT_MAX_BYTES + 1);
    expect_code(host.artifact_write_script(&id, &entry, &big, false, None).unwrap_err(), RdeErrorCode::OutOfRange);
    // Внешняя правка (expected_sha256) — RDE_CONFLICT.
    expect_code(
        host.artifact_write_script(&id, &entry, "x", false, Some(&"f".repeat(64))).unwrap_err(),
        RdeErrorCode::Conflict,
    );
}

#[test]
fn script_write_rejects_shell_runtime() {
    let (host, _root) = demo_host("script-shell");
    let id = compute_artifact_id(&host);
    // Меняем runtime на shell через валидный патч манифеста, затем запись скрипта обязана отказать.
    let patch: ArtifactManifestDto = serde_json::from_value(serde_json::json!({
        "runtime": {"kind": "shell", "entry": "run.sh"}
    }))
    .unwrap();
    let _ = host.artifact_write_manifest(&id, patch, false, None);
    // Если схема не пропустила shell — тоже допустимо (тогда манифест не изменён).
    if host.artifact_read(&id).unwrap().runtime.map(|runtime| runtime.kind) == Some("shell".to_owned()) {
        let error = host.artifact_write_script(&id, "run.sh", "echo hi", false, None).unwrap_err();
        assert_eq!(error.code, RdeErrorCode::Unsupported);
    }
}

// --- заметки ---------------------------------------------------------------

#[test]
fn note_write_updates_content_and_title_and_restores() {
    let (host, root) = demo_host("note");
    let id = note_id(&host);
    let note = host.note_list().unwrap().into_iter().find(|note| note.id == id).unwrap();
    let note_path = root.join(&note.file);
    let original_content = std::fs::read_to_string(&note_path).unwrap();
    let outcome = host
        .note_write(&id, Some("# M2: заметка агента\n".to_owned()), Some("Заголовок M2".to_owned()), false, None)
        .unwrap();
    assert_eq!(outcome.changed_files.len(), 2, "note.md + note.toml");
    let checkpoint_id = outcome.checkpoint_id.expect("чекпоинт");
    assert!(std::fs::read_to_string(&note_path).unwrap().contains("M2: заметка агента"));
    let updated = host.note_list().unwrap().into_iter().find(|note| note.id == id).unwrap();
    assert_eq!(updated.title, "Заголовок M2");
    // Откат: содержимое и заголовок как были.
    host.checkpoint_restore(&checkpoint_id).unwrap();
    assert_eq!(std::fs::read_to_string(&note_path).unwrap(), original_content);
    let reverted = host.note_list().unwrap().into_iter().find(|note| note.id == id).unwrap();
    assert_eq!(reverted.title, note.title);
}

#[test]
fn note_write_validates_and_detects_conflicts() {
    let (host, _root) = demo_host("note-invalid");
    let id = note_id(&host);
    expect_code(host.note_write(&id, None, None, false, None).unwrap_err(), RdeErrorCode::InvalidArgument);
    expect_code(host.note_write("01NOSUCHNOTE000000000000", Some("x".to_owned()), None, true, None).unwrap_err(), RdeErrorCode::NotFound);
    expect_code(
        host.note_write(&id, Some("x".to_owned()), None, false, Some(&"a".repeat(64))).unwrap_err(),
        RdeErrorCode::Conflict,
    );
}

// --- git -------------------------------------------------------------------

fn run_git(root: &Path, args: &[&str]) {
    let status = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .expect("git запускается");
    assert!(status.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&status.stderr));
}

#[test]
fn git_commit_commits_project_without_rde_state() {
    if !rde_vcs::git_available() {
        eprintln!("git недоступен — тест пропущен");
        return;
    }
    let (host, root) = demo_host("git-commit");
    run_git(&root, &["init", "-q"]);
    run_git(&root, &["config", "user.name", "RDE test"]);
    run_git(&root, &["config", "user.email", "test@rde.local"]);
    // Изменяем заметку (через мутацию с чекпоинтом), затем коммитим.
    let id = note_id(&host);
    host.note_write(&id, Some("# Коммит M2\n".to_owned()), None, false, None).unwrap();
    let dry = host.git_commit("M2: dry run", true).unwrap();
    assert!(dry.dry_run);
    assert!(dry.commit.is_none());
    assert!(dry.files >= 1, "есть что коммитить: {dry:?}");

    let outcome = host.git_commit("M2: мутации через approvals/checkpoints", false).unwrap();
    let commit = outcome.commit.expect("коммит создан");
    assert!(!commit.id.is_empty());
    assert!(outcome.checkpoint_id.is_some());
    assert!(outcome.files >= 1);
    // В коммит не попал `.rde/**` (store/аудит/чекпоинты — локальное состояние).
    let tracked = std::process::Command::new("git")
        .args(["ls-files"])
        .current_dir(&root)
        .output()
        .expect("git ls-files");
    let tracked = String::from_utf8_lossy(&tracked.stdout);
    assert!(!tracked.lines().any(|line| line.starts_with(".rde/")), "в коммите .rde: {tracked}");
    // Статус после коммита: доступен, есть ветка, изменений нет.
    let status = host.git_status().unwrap();
    assert!(status.available && status.is_repo);
    assert!(status.branch.as_deref().is_some_and(|branch| !branch.is_empty()));
    assert_eq!(status.changed, Vec::<String>::new(), "рабочее дерево чистое");
    // Повторный коммит без изменений — RDE_NOT_READY.
    expect_code(host.git_commit("пусто", false).unwrap_err(), RdeErrorCode::NotReady);
    // Автор — human-автор из rde.toml (O7).
    assert_eq!(commit.author, "Илья");
}

#[test]
fn git_commit_requires_repository() {
    if !rde_vcs::git_available() {
        eprintln!("git недоступен — тест пропущен");
        return;
    }
    let (host, _root) = demo_host("git-norepo");
    let error = host.git_commit("нет репозитория", false).expect_err("не git-репозиторий");
    assert_eq!(error.code, RdeErrorCode::NotReady);
}

// --- запуск артефактов: job-модель на stub-исполнителе ---------------------

/// Stub-исполнитель: «работает» `delay_ms` (или до отмены), итог — успех.
struct StubExecutor {
    delay_ms: u64,
}

impl ArtifactExecutor for StubExecutor {
    fn execute(
        &self,
        _root: &Path,
        _request: &ExecRequest,
        cancel: Arc<AtomicBool>,
    ) -> Result<ExecOutcome, String> {
        let deadline = Instant::now() + Duration::from_millis(self.delay_ms);
        while Instant::now() < deadline {
            if cancel.load(Ordering::SeqCst) {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(ExecOutcome {
            total: 1,
            ran: 1,
            skipped: 0,
            failed: 0,
            errors: Vec::new(),
            run_ids: vec!["run-stub".to_owned()],
            log_paths: Vec::new(),
            message: "обновление (stub): запущено 1".to_owned(),
        })
    }
}

fn wait_finished(host: &HeadlessHost, job_id: &str) -> rde_mcp::model::JobInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let info = host.run_status(job_id).expect("job в реестре");
        if info.state != "running" {
            return info;
        }
        assert!(Instant::now() < deadline, "job не завершился за 10 с");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn artifact_run_job_lifecycle_with_stub_executor() {
    let root = demo_copy("run-stub");
    let host = HeadlessHost::open_with_executor(&root, Arc::new(StubExecutor { delay_ms: 300 }))
        .expect("хост со stub-исполнителем");
    let id = compute_artifact_id(&host);
    let job = host.artifact_run(Some(vec![id.clone()]), false, true).expect("job стартует");
    assert_eq!(job.state, "running", "artifact_run не блокирует вызов");
    assert!(job.checkpoint_id.is_some(), "перед запуском создан чекпоинт");
    // Второй job одновременно — RDE_BUSY.
    if host.run_status(&job.job_id).unwrap().state == "running" {
        expect_code(host.artifact_run(None, false, false).unwrap_err(), RdeErrorCode::Busy);
    }
    let finished = wait_finished(&host, &job.job_id);
    assert_eq!(finished.state, "succeeded");
    assert_eq!(finished.ran, 1);
    assert_eq!(finished.run_id.as_deref(), Some("run-stub"));
    // Лог пока пуст (у stub нет пути) — ответ без ошибки.
    let log = host.run_log(&job.job_id, 0, 1024).unwrap();
    assert_eq!(log.job_id, job.job_id);
    assert!(log.text.is_empty());
    // Неизвестный job — RDE_JOB_NOT_FOUND.
    expect_code(host.run_status("01NOJOB").unwrap_err(), RdeErrorCode::JobNotFound);
    host.shutdown_jobs();
}

#[test]
fn artifact_run_cancel_stops_job() {
    let root = demo_copy("run-cancel");
    let host = HeadlessHost::open_with_executor(&root, Arc::new(StubExecutor { delay_ms: 5_000 }))
        .expect("хост со stub-исполнителем");
    let id = compute_artifact_id(&host);
    let job = host.artifact_run(Some(vec![id]), false, false).expect("job стартует");
    let cancel = host.run_cancel(&job.job_id).expect("отмена запрошена");
    assert!(cancel.message.contains("отмена"));
    let finished = wait_finished(&host, &job.job_id);
    assert_eq!(finished.state, "cancelled");
    host.shutdown_jobs();
}

#[test]
fn artifact_run_requires_trusted_project() {
    let root = demo_copy("run-untrusted");
    let text = std::fs::read_to_string(root.join("rde.toml")).unwrap().replace("trusted = true", "trusted = false");
    std::fs::write(root.join("rde.toml"), text).unwrap();
    let host = HeadlessHost::open_with_executor(&root, Arc::new(StubExecutor { delay_ms: 0 })).unwrap();
    let id = compute_artifact_id(&host);
    let error = host.artifact_run(Some(vec![id]), false, false).expect_err("trusted обязателен");
    assert_eq!(error.code, RdeErrorCode::ApprovalDenied);
    assert!(error.message.contains("доверенным"), "{error}");
    host.shutdown_jobs();
}
