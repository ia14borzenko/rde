//! Хуки независимого слоя: approvals, аудит, события.
//!
//! Объявлены здесь (в протокольном слое), а реализации подключаются **снаружи** — GUI,
//! `rde-host`, `rde-agent`/DSH — без правок инструментов и сервера (A4).
//!
//! M1/M2 используют:
//! - [`ApprovalHook`] — только для мутирующих инструментов (M2): `DenyAllHook` — дефолт
//!   (fail-closed), headless [`AutoApproveHook`]/[`AllowlistHook`] — явные CLI-режимы, GUI/ACP
//!   в M4 добавят диалог с diff без правок протокольного слоя;
//! - [`AuditSink`] — на **каждый** вызов инструмента (включая отказанные/неизвестные):
//!   JSONL-строка в `.rde/agent/audit.log` (R-AGT-06) с `decision`/`reason`/`checkpoint_id`;
//! - [`EventSink`] — интерфейс уведомлений (M1/M2 не отправляет notifications; интерфейс
//!   объявлен, чтобы GUI/ACP подключились позже без изменения протокольного слоя).

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::RdeErrorCode;
use crate::sha256_hex;

/// Запрос разрешения на действие (для мутирующих/опасных инструментов).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalRequest {
    /// Имя инструмента.
    pub tool: String,
    /// Краткая выжимка аргументов (без тел/секретов; обрезана).
    pub args_summary: String,
    /// SHA-256 канонизированного JSON аргументов (ключ сессионного кэша разрешений).
    pub args_sha256: String,
    /// Действие деструктивно (перезапись/удаление/commit/run).
    pub destructive: bool,
    /// Кто просит (`mcp`/`agent`/…).
    pub requested_by: String,
}

/// Решение по запросу разрешения.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ApprovalDecision {
    /// Разрешить один вызов.
    AllowOnce,
    /// Разрешить до конца сессии.
    AllowSession,
    /// Отказать (fail-closed).
    Deny,
}

/// Хук разрешений (async: реализация может ждать ответа UI).
#[async_trait]
pub trait ApprovalHook: Send + Sync {
    /// Запросить разрешение. Политика обязана трактовать любые ошибки как `Deny`.
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision;
}

/// Реализация по умолчанию: запрещает всё (fail-closed, R-AGT-06).
#[derive(Clone, Copy, Debug, Default)]
pub struct DenyAllHook;

#[async_trait]
impl ApprovalHook for DenyAllHook {
    async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::Deny
    }
}

/// Headless-хук `--approval=auto`: разрешает всё, что уже допущено грантами политики.
///
/// Используется только при явном флаге CLI (`auto`); без флагов сервер собирает
/// [`DenyAllHook`] и не включает гранты — мутации запрещены (fail-closed, A4). Для
/// `artifact_run` дополнительно требуется `trusted`-проект (проверка в хосте, R-RUN-06).
#[derive(Clone, Copy, Debug, Default)]
pub struct AutoApproveHook;

#[async_trait]
impl ApprovalHook for AutoApproveHook {
    async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
        ApprovalDecision::AllowOnce
    }
}

/// Headless-хук `--approval=allowlist`: разрешает только перечисленные инструменты.
///
/// Гранты политики при этом тоже включаются только для них; всё остальное — `Deny`
/// (fail-closed). GUI/ACP в M4 заменят хук диалогом с diff, ничего в сервере не меняя.
#[derive(Clone, Debug, Default)]
pub struct AllowlistHook {
    tools: std::collections::BTreeSet<String>,
}

impl AllowlistHook {
    /// Хук по списку разрешённых инструментов (имена уже провалидированы политикой).
    pub fn new(tools: impl IntoIterator<Item = String>) -> Self {
        Self { tools: tools.into_iter().collect() }
    }

    /// Разрешён ли инструмент хуком.
    pub fn allows(&self, tool: &str) -> bool {
        self.tools.contains(tool)
    }
}

#[async_trait]
impl ApprovalHook for AllowlistHook {
    async fn request(&self, request: ApprovalRequest) -> ApprovalDecision {
        if self.tools.contains(&request.tool) {
            ApprovalDecision::AllowOnce
        } else {
            ApprovalDecision::Deny
        }
    }
}

/// Запись аудита вызова инструмента (R-AGT-06).
///
/// Поля стабильны; тела аргументов/секреты не пишутся — только SHA-256 канонизированного
/// JSON аргументов (`params_sha256`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unix-время записи, секунды.
    #[serde(rename = "ts")]
    pub ts_unix: u64,
    /// Имя инструмента (или `tools/call` для неизвестного).
    pub tool: String,
    /// SHA-256 JSON-аргументов вызова (hex).
    pub params_sha256: String,
    /// Решение политики: `allow|deny`.
    pub decision: String,
    /// Вызов завершился успешно.
    pub ok: bool,
    /// Код ошибки (`RDE_*`), если `ok == false`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    /// Длительность вызова, мс.
    pub duration_ms: u64,
    /// Причина решения политики/approval (`policy`/`approval`/`session`/`allow_once`/…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Чекпоинт, созданный мутирующим инструментом (R-AGT-05, для отката/аудита).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub checkpoint_id: Option<String>,
}

impl AuditEntry {
    /// Создать запись аудита.
    pub fn new(tool: impl Into<String>, params_sha256: impl Into<String>, decision: &str, ok: bool) -> Self {
        Self {
            ts_unix: now_unix(),
            tool: tool.into(),
            params_sha256: params_sha256.into(),
            decision: decision.to_owned(),
            ok,
            error_code: None,
            duration_ms: 0,
            reason: None,
            checkpoint_id: None,
        }
    }

    /// Проставить код ошибки.
    #[must_use]
    pub fn with_error(mut self, code: RdeErrorCode) -> Self {
        self.error_code = Some(code.as_str().to_owned());
        self
    }

    /// Проставить длительность.
    #[must_use]
    pub fn with_duration_ms(mut self, duration_ms: u64) -> Self {
        self.duration_ms = duration_ms;
        self
    }

    /// Проставить причину решения (`policy`/`approval`/`session`/…).
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// Проставить id чекпоинта, созданного мутацией.
    #[must_use]
    pub fn with_checkpoint_id(mut self, checkpoint_id: impl Into<String>) -> Self {
        self.checkpoint_id = Some(checkpoint_id.into());
        self
    }
}

/// Приёмник аудита: синхронный (быстрый append; не блокирует async-рантайм надолго).
pub trait AuditSink: Send + Sync {
    /// Записать событие. Ошибки записи не должны ронять сервер (fail-open для аудита,
    /// fail-closed для действий: вызов уже разрешён/отказан политикой).
    fn record(&self, entry: &AuditEntry);
}

/// Никуда не пишущий приёмник (unit-тесты).
#[derive(Clone, Copy, Debug, Default)]
pub struct NullAuditSink;

impl AuditSink for NullAuditSink {
    fn record(&self, _entry: &AuditEntry) {}
}

/// JSONL-аудит в `<project>/.rde/agent/audit.log` (по строке на вызов).
///
/// Каталог `.rde/agent/` создаётся при первой записи; `.rde/` не попадает в git (R-AGT-06).
pub struct JsonlAuditSink {
    path: PathBuf,
}

impl JsonlAuditSink {
    /// Открыть приёмник для проекта (файл создаётся лениво, при первой записи).
    pub fn new(project_root: &Path) -> Self {
        Self { path: project_root.join(".rde").join("agent").join("audit.log") }
    }

    /// Путь к файлу аудита.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AuditSink for JsonlAuditSink {
    fn record(&self, entry: &AuditEntry) {
        let Some(parent) = self.path.parent() else { return };
        if let Err(error) = std::fs::create_dir_all(parent) {
            crate::log_stderr(crate::LogLevel::Error, format!("аудит: не создан каталог {}: {error}", parent.display()));
            return;
        }
        let Ok(line) = serde_json::to_string(entry) else {
            crate::log_stderr(crate::LogLevel::Error, "аудит: не сериализована запись");
            return;
        };
        match OpenOptions::new().create(true).append(true).open(&self.path) {
            Ok(mut file) => {
                if let Err(error) = writeln!(file, "{line}") {
                    crate::log_stderr(crate::LogLevel::Error, format!("аудит: ошибка записи: {error}"));
                }
            }
            Err(error) => {
                crate::log_stderr(
                    crate::LogLevel::Error,
                    format!("аудит: не открыт {}: {error}", self.path.display()),
                );
            }
        }
    }
}

/// Событие хоста для внешних подписчиков (GUI/ACP). M1 их не отправляет по протоколу.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum HostEvent {
    /// Вызов инструмента завершён.
    ToolCallFinished {
        /// Имя инструмента.
        tool: String,
        /// Успешно.
        ok: bool,
        /// Длительность, мс.
        duration_ms: u64,
    },
    /// Информационное сообщение (диагностика).
    Note {
        /// Текст.
        message: String,
    },
}

/// Приёмник событий хоста (интерфейс расширения; в M1 — [`NullEventSink`]).
pub trait EventSink: Send + Sync {
    /// Отправить событие подписчикам.
    fn emit(&self, event: HostEvent);
}

/// Приёмник событий по умолчанию: ничего не делает.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullEventSink;

impl EventSink for NullEventSink {
    fn emit(&self, _event: HostEvent) {}
}

/// Набор хуков сервера: approvals + аудит + события.
#[derive(Clone)]
pub struct HostHooks {
    /// Разрешения для мутирующих/опасных инструментов.
    pub approvals: Arc<dyn ApprovalHook>,
    /// Аудит каждого вызова.
    pub audit: Arc<dyn AuditSink>,
    /// События для GUI/ACP (M1 — не используются протоколом).
    pub events: Arc<dyn EventSink>,
}

impl HostHooks {
    /// Хуки по умолчанию: всё запрещено, аудит никуда, событий нет (для тестов/CLI без проекта).
    pub fn new(approvals: Arc<dyn ApprovalHook>, audit: Arc<dyn AuditSink>, events: Arc<dyn EventSink>) -> Self {
        Self { approvals, audit, events }
    }

    /// Хуки для реального сервера: [`DenyAllHook`] + JSONL-аудит проекта.
    pub fn jsonl(project_root: &Path) -> Self {
        Self::new(
            Arc::new(DenyAllHook),
            Arc::new(JsonlAuditSink::new(project_root)),
            Arc::new(NullEventSink),
        )
    }

    /// Fail-closed без записи (unit-тесты).
    pub fn silent() -> Self {
        Self::new(Arc::new(DenyAllHook), Arc::new(NullAuditSink), Arc::new(NullEventSink))
    }

    /// Заменить приёмник аудита.
    #[must_use]
    pub fn with_audit(mut self, audit: Arc<dyn AuditSink>) -> Self {
        self.audit = audit;
        self
    }

    /// Заменить хук разрешений.
    #[must_use]
    pub fn with_approvals(mut self, approvals: Arc<dyn ApprovalHook>) -> Self {
        self.approvals = approvals;
        self
    }
}

impl Default for HostHooks {
    fn default() -> Self {
        Self::silent()
    }
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Хеш аргументов вызова для аудита (канонизированный JSON; тела не пишем).
pub fn params_sha256(args: &serde_json::Value) -> String {
    let text = serde_json::to_string(args).unwrap_or_default();
    sha256_hex(text.as_bytes())
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Записывающий приёмник аудита для тестов.
    #[derive(Default)]
    pub(crate) struct RecordingAuditSink {
        entries: Mutex<Vec<AuditEntry>>,
    }

    impl RecordingAuditSink {
        pub(crate) fn entries(&self) -> Vec<AuditEntry> {
            self.entries.lock().unwrap().clone()
        }
    }

    impl AuditSink for RecordingAuditSink {
        fn record(&self, entry: &AuditEntry) {
            self.entries.lock().unwrap().push(entry.clone());
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-mcp-hooks-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn jsonl_audit_sink_writes_one_valid_line_per_call() {
        let dir = temp_dir("jsonl");
        let sink = JsonlAuditSink::new(&dir);
        assert_eq!(sink.path(), dir.join(".rde/agent/audit.log"));
        sink.record(&AuditEntry::new("project_tree", "a".repeat(64), "allow", true).with_duration_ms(3));
        sink.record(&AuditEntry::new("file_read", "b".repeat(64), "deny", false).with_error(RdeErrorCode::PathEscape));
        let text = std::fs::read_to_string(sink.path()).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "по строке на вызов: {text}");
        let first: AuditEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first.tool, "project_tree");
        assert_eq!(first.decision, "allow");
        assert!(first.ok);
        assert_eq!(first.duration_ms, 3);
        let value: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(value["error_code"], "RDE_PATH_ESCAPE");
        // «ts» — именно так называется поле в файле аудита (R-AGT-06).
        assert!(value.get("ts").is_some());
        assert!(value.get("body").is_none());
    }

    #[test]
    fn audit_entry_shape_is_stable() {
        let entry = AuditEntry::new("doc_read", "c".repeat(64), "allow", true);
        let value = serde_json::to_value(&entry).unwrap();
        for key in ["ts", "tool", "params_sha256", "decision", "ok", "duration_ms"] {
            assert!(value.get(key).is_some(), "в аудите нет поля {key}");
        }
    }

    #[test]
    fn params_sha256_is_deterministic_and_body_free() {
        let args = serde_json::json!({"path": "rde.toml", "range": {"lines": {"start": 1, "count": 2}}});
        assert_eq!(params_sha256(&args), params_sha256(&args.clone()));
        assert_eq!(params_sha256(&args).len(), 64);
    }

    #[tokio::test]
    async fn deny_all_hook_denies() {
        let hook = DenyAllHook;
        let decision = hook
            .request(ApprovalRequest {
                tool: "artifact_run".to_owned(),
                args_summary: "{}".to_owned(),
                args_sha256: "0".repeat(64),
                destructive: true,
                requested_by: "mcp".to_owned(),
            })
            .await;
        assert_eq!(decision, ApprovalDecision::Deny);
    }

    #[tokio::test]
    async fn headless_approval_hooks_are_fail_closed_by_default() {
        let request = |tool: &str| ApprovalRequest {
            tool: tool.to_owned(),
            args_summary: "{}".to_owned(),
            args_sha256: "0".repeat(64),
            destructive: true,
            requested_by: "mcp".to_owned(),
        };
        // --approval=auto: разрешает всё, что допустила политика грантов.
        assert_eq!(AutoApproveHook.request(request("artifact_run")).await, ApprovalDecision::AllowOnce);
        // --approval=allowlist: только перечисленные инструменты, остальное — Deny.
        let hook = AllowlistHook::new(vec!["note_write".to_owned()]);
        assert!(hook.allows("note_write"));
        assert!(!hook.allows("artifact_run"));
        assert_eq!(hook.request(request("note_write")).await, ApprovalDecision::AllowOnce);
        assert_eq!(hook.request(request("artifact_run")).await, ApprovalDecision::Deny);
    }

    #[test]
    fn audit_entry_extended_fields_are_optional() {
        let entry = AuditEntry::new("artifact_write_manifest", "a".repeat(64), "allow", true)
            .with_reason("allow_once")
            .with_checkpoint_id("01CHECKPOINT");
        let value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["reason"], "allow_once");
        assert_eq!(value["checkpoint_id"], "01CHECKPOINT");
        // Старые поля аудита не изменились.
        for key in ["ts", "tool", "params_sha256", "decision", "ok", "duration_ms"] {
            assert!(value.get(key).is_some(), "в аудите нет поля {key}");
        }
        let plain = AuditEntry::new("project_tree", "b".repeat(64), "allow", true);
        let value = serde_json::to_value(&plain).unwrap();
        assert!(value.get("reason").is_none(), "reason опционален");
        assert!(value.get("checkpoint_id").is_none(), "checkpoint_id опционален");
    }
}
