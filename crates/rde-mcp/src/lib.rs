//! RDE MCP — независимый протокольный слой MCP-сервера RDE (v0.3, вехи M1–M2).
//!
//! # Зачем отдельный крейт
//!
//! MCP — внешний контракт: его видят сторонние агенты, а не только код RDE. Поэтому он
//! реализован в отдельном крейте, который **не зависит** от библиотечных крейтов RDE
//! (`rde-core`/`rde-dag`/`rde-doc`/…) и от GUI (`rde-ui`/`egui`). Всё, что нужно серверу от
//! проекта, описано синхронным трейтом [`RdeHost`] и DTO из [`model`]. Реализация контракта
//! для реального проекта живёт в отдельном крейте `rde-host` (headless, поверх библиотек RDE),
//! поэтому:
//!
//! - протокольный слой тестируется на mock-хосте без ФС и без GUI;
//! - хост переиспользуем в любом headless-режиме (CLI/ACP), не затаскивая UI;
//! - стрелка зависимостей: `apps/rde → rde-host → rde-mcp`, `rde-mcp` ничего не знает про RDE.
//!
//! Архитектурные решения и границы — `docs/adr/0006-mcp-independent-layer.md` (M1) и
//! `docs/adr/0007-mutations-approvals-checkpoints.md` (M2: мутации через те же хуки).
//!
//! # Что внутри
//!
//! - [`host`] — контракт [`RdeHost`] (синхронный, dyn-совместимый) и [`HostError`]; M2 добавляет
//!   мутирующие методы аддитивно (default-реализации `RDE_UNSUPPORTED` — O1-a);
//! - [`model`] — DTO/схемы (только они пересекают границу слоя; доменные типы RDE не протекают),
//!   включая `MutationOutcome`/`JobInfo`/`JobLog`/`ArtifactManifestDto`;
//! - [`tools`] — статический детерминированный реестр 21 инструмента (13 read-only M1 + 8 мутаций
//!   M2), их JSON-схемы, аннотации, гранты политики и обработчики;
//! - [`resources`] — ресурсы `rde://…` и resource-templates;
//! - [`policy`] — [`McpPolicy`] (fail-closed: гранты `project_write`/`run_script`/`git_commit` по
//!   умолчанию выключены), сессионный кэш approvals, [`authorize_tool_call`] и headless-режимы
//!   approvals (`--approval=deny|auto|allowlist`);
//! - [`hooks`] — [`ApprovalHook`] (+`AutoApproveHook`/`AllowlistHook`/`DenyAllHook`), [`AuditSink`]
//!   (JSONL в `.rde/agent/audit.log`, поля `reason`/`checkpoint_id`), [`EventSink`];
//! - [`server`] — [`McpServer`] на `rmcp` (spec MCP 2026-07-28: `server/discover`, stateless,
//!   `ttlMs`/`cacheScope`), расширенный аудит решений/чекпоинтов и [`serve_stdio`];
//! - [`instructions`] — карта проекта/политика для клиента (≤32 KiB, стабильный текст).
//!
//! # Пример запуска
//!
//! Реальный запуск — `rde mcp serve --project <dir>` (см. `apps/rde`): бинарь собирает
//! `HeadlessHost` из крейта `rde-host` (он единственный знает библиотеки RDE), политику/хук
//! approvals из CLI-флагов и [`HostHooks::jsonl`], затем вызывает [`serve_stdio_blocking`].
//! Протокольный слой при этом остаётся независимым: в тестах он работает на mock-хосте.
//!
//! Транспорт M1/M2 — только stdio (в R-MCP-02 HTTP — позже, loopback + токен). `stdout` занят
//! JSON-RPC: все логи пишутся только в `stderr` (уровень — переменная `RDE_MCP_LOG`).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod error;
pub mod hooks;
pub mod host;
pub mod instructions;
pub mod model;
pub mod policy;
pub mod resources;
pub mod server;
#[cfg(test)]
mod test_support;
pub mod tools;

pub use error::{HostError, RdeErrorCode};
pub use hooks::{
    AllowlistHook, ApprovalDecision, ApprovalHook, ApprovalRequest, AuditEntry, AuditSink, AutoApproveHook,
    DenyAllHook, EventSink, HostEvent, HostHooks, JsonlAuditSink, NullAuditSink, NullEventSink,
};
pub use host::{RdeHost, SharedHost};
pub use policy::{
    authorize_tool_call, build_policy_and_hook, ApprovalCache, ApprovalMode, ApprovalOptions, AuthDenial,
    AuthOutcome, McpPolicy, RateLimiter, ToolGrant,
};
pub use resources::{parse_resource_uri, ResourceTarget};
pub use server::{serve_stdio, serve_stdio_blocking, McpServer};
pub use tools::{find_tool, tool_registry, ToolSpec};

use rmcp::model::ProtocolVersion;
use sha2::{Digest, Sha256};

/// Имя сервера в `serverInfo` (короткое, стабильное).
pub const SERVER_NAME: &str = "rde";

/// Версия сервера в `serverInfo` (версия крейта RDE).
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Единственная версия протокола MCP, которую реализует M1 (spec 2026-07-28, stateless core).
pub const MCP_PROTOCOL_VERSION: ProtocolVersion = ProtocolVersion::V_2026_07_28;

/// Список поддерживаемых версий для `server/discover` и валидации `_meta` каждого запроса.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[ProtocolVersion] = &[MCP_PROTOCOL_VERSION];

/// Ограничение MCP-инструкций (лимит DSH, S16): 32 KiB.
pub const MAX_INSTRUCTIONS_BYTES: usize = 32 * 1024;

/// `tools/list` кэшируется долго: каталог M1 статичен и детерминирован (R-AGT-08).
pub const TOOLS_LIST_TTL_MS: u64 = 300_000;

/// `resources/list` зависит от проекта: короткий TTL, только приватное кэширование.
pub const RESOURCES_LIST_TTL_MS: u64 = 5_000;

/// `resources/templates/list` статичен: длинный TTL.
pub const RESOURCE_TEMPLATES_LIST_TTL_MS: u64 = 300_000;

/// `server/discover` — короткий TTL: инструкции/карта проекта могут измениться.
pub const DISCOVER_TTL_MS: u64 = 5_000;

/// TTL чтения изменяемых ресурсов (дерево проекта, оглавление документа, метаданные артефакта).
pub const MUTABLE_RESOURCE_TTL_MS: u64 = 2_000;

/// TTL чтения статических ресурсов (каталог шаблонов).
pub const STATIC_RESOURCE_TTL_MS: u64 = 300_000;

/// SHA-256 в hex: используется для `params_sha256` в аудите (полные аргументы не пишутся).
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// Уровень stderr-лога MCP-сервера (`RDE_MCP_LOG`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    /// Ошибки (печатаются всегда).
    Error,
    /// Жизненный цикл/аудит (по умолчанию).
    Info,
    /// Отладка (по запросу).
    Debug,
}

impl LogLevel {
    fn from_env() -> Self {
        match std::env::var("RDE_MCP_LOG").ok().as_deref() {
            Some("debug") => LogLevel::Debug,
            Some("info") => LogLevel::Info,
            _ => LogLevel::Error,
        }
    }

    fn label(self) -> &'static str {
        match self {
            LogLevel::Error => "ERROR",
            LogLevel::Info => "INFO",
            LogLevel::Debug => "DEBUG",
        }
    }
}

/// Записать сообщение в `stderr` (stdout занят JSON-RPC), если включён уровень `level`.
pub fn log_stderr(level: LogLevel, message: impl AsRef<str>) {
    let configured = LogLevel::from_env();
    if level <= configured {
        eprintln!("RDE MCP [{}] {}", level.label(), message.as_ref());
    }
}

/// Короткий info-лог: жизненный цикл сервера и решения политики.
pub fn log_info(message: impl AsRef<str>) {
    log_stderr(LogLevel::Info, message);
}

/// Debug-лог: детали вызовов (только при `RDE_MCP_LOG=debug`).
pub fn log_debug(message: impl AsRef<str>) {
    log_stderr(LogLevel::Debug, message);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_known_vector() {
        assert_eq!(sha256_hex(b""), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }
}
