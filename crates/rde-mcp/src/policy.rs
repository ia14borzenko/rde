//! Политика безопасности MCP-сервера и авторизация вызовов инструментов (M1 → M2).
//!
//! Дефолт — **fail-closed**: `allow_read_only = true`, все гранты мутаций (`allow_project_write`,
//! `allow_run_script`, `allow_git_commit`) — `false`. Без явных CLI-флагов любая мутация
//! отклоняется с `RDE_APPROVAL_DENIED`, ничего не пишется (R-MCP-05, R-AGT-06, A4).
//!
//! Авторизация вынесена в свободную async-функцию [`authorize_tool_call`], чтобы:
//! - её можно было тестировать синтетическим мутирующим [`ToolSpec`](crate::tools::ToolSpec);
//! - мутирующие инструменты подключались через тот же код и те же хуки, без правок сервера/реестра.
//!
//! Порядок проверки (fail-closed): белый список → read-only/грант политики → кэш сессионных
//! разрешений (`AllowSession`) → [`ApprovalHook`]. Каждое решение аудируется сервером
//! (`decision`, `reason`); перед мутацией хост создаёт чекпоинт (`checkpoint_id` в аудите).
//!
//! Headless-режимы approvals (`--approval=deny|auto|allowlist`) собираются в
//! [`build_policy_and_hook`]; GUI/ACP в M4 подключат свой [`ApprovalHook`] (диалог с diff)
//! без изменения сервера/реестра.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::error::HostError;
use crate::hooks::{AllowlistHook, ApprovalDecision, ApprovalRequest, AutoApproveHook, DenyAllHook, HostHooks};
use crate::tools::{self, ToolSpec};

/// Грант политики, необходимый мутирующему инструменту (A4: grants, а не один `allow_mutation`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolGrant {
    /// Запись в проект: манифест артефакта, скрипты, заметки (перезапись файлов проекта).
    ProjectWrite,
    /// Запуск скриптов/артефактов (R-AGT-06: отдельный грант; плюс `trusted`-проект).
    RunScript,
    /// git-коммит (R-VCS: destructive, отдельный грант).
    GitCommit,
}

impl ToolGrant {
    /// Строковое имя гранта (для инструкций/аннотаций).
    pub fn as_str(self) -> &'static str {
        match self {
            ToolGrant::ProjectWrite => "project_write",
            ToolGrant::RunScript => "run_script",
            ToolGrant::GitCommit => "git_commit",
        }
    }

    /// Все гранты (для диагностики/тестов).
    pub const ALL: [ToolGrant; 3] = [ToolGrant::ProjectWrite, ToolGrant::RunScript, ToolGrant::GitCommit];
}

/// Политика MCP-сервера (синхронная, `Clone`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct McpPolicy {
    /// Разрешать read-only инструменты.
    pub allow_read_only: bool,
    /// Грант «запись в проект» (манифест/скрипт/заметка); по умолчанию `false`.
    pub allow_project_write: bool,
    /// Грант «запуск скриптов/артефактов» (R-AGT-06); по умолчанию `false`.
    pub allow_run_script: bool,
    /// Грант «git-коммит»; по умолчанию `false`.
    pub allow_git_commit: bool,
    /// Режим approvals (`deny|auto|allowlist`) — только для инструкций/диагностики.
    pub approval_mode: String,
    /// Лимит вызовов инструментов в минуту (0 — без лимита).
    pub rate_limit_per_min: u32,
    /// Максимальный размер JSON-ответа инструмента, байт.
    pub max_response_bytes: usize,
    /// Таймаут одного инструмента, мс.
    pub tool_timeout_ms: u64,
    /// Белый список инструментов (`None` — все из реестра).
    pub allowed_tools: Option<Vec<String>>,
}

impl Default for McpPolicy {
    fn default() -> Self {
        Self {
            allow_read_only: true,
            allow_project_write: false,
            allow_run_script: false,
            allow_git_commit: false,
            approval_mode: "deny".to_owned(),
            rate_limit_per_min: 120,
            max_response_bytes: 256 * 1024,
            tool_timeout_ms: 10_000,
            allowed_tools: None,
        }
    }
}

impl McpPolicy {
    /// Политика read-only (дефолт и режим `--approval=deny`): мутации запрещены.
    pub fn read_only() -> Self {
        Self::default()
    }

    /// Разрешён ли инструмент по имени (белый список).
    pub fn tool_allowed(&self, name: &str) -> bool {
        match &self.allowed_tools {
            None => true,
            Some(list) => list.iter().any(|allowed| allowed == name),
        }
    }

    /// Включён ли грант политики.
    pub fn grant_allowed(&self, grant: ToolGrant) -> bool {
        match grant {
            ToolGrant::ProjectWrite => self.allow_project_write,
            ToolGrant::RunScript => self.allow_run_script,
            ToolGrant::GitCommit => self.allow_git_commit,
        }
    }

    /// Раздел MCP-инструкций с фактической политикой сессии (детерминирован по флагам).
    pub fn instructions_section(&self) -> String {
        let yes_no = |value: bool| if value { "разрешено" } else { "запрещено" };
        format!(
            "\n## Политика мутаций этой сессии (M2, grants + approvals)\n\
             - режим approvals: `{}`; без гранта мутация → `RDE_APPROVAL_DENIED` (файлы не меняются);\n\
             - запись в проект (`artifact_write_manifest`/`artifact_write_script`/`note_write`): {};\n\
             - запуск артефактов (`artifact_run`/`run_cancel`, нужен `trusted`-проект): {};\n\
             - git-коммит (`git_commit`): {};\n\
             - перед мутацией создаётся чекпоинт `.rde/agent/checkpoints/<id>/` (файлы + корни store), \
             при ошибке — авто-откат; `checkpoint_id` пишется в аудит;\n\
             - `run_status`/`run_log` — чтение; jobs живут в процессе сервера (рестарт их теряет).\n",
            self.approval_mode,
            yes_no(self.allow_project_write),
            yes_no(self.allow_run_script),
            yes_no(self.allow_git_commit),
        )
    }
}

/// Скользящее окно rate-limit (вызовов в минуту); атомарно для `&self`.
pub struct RateLimiter {
    limit_per_min: u32,
    window: Duration,
    calls: Mutex<VecDeque<Instant>>,
}

impl RateLimiter {
    /// Новый лимитер на `limit_per_min` вызовов в минуту (0 — без ограничения).
    pub fn new(limit_per_min: u32) -> Self {
        Self { limit_per_min, window: Duration::from_secs(60), calls: Mutex::new(VecDeque::new()) }
    }

    /// Зарегистрировать вызов; `true` — можно, `false` — лимит исчерпан.
    pub fn check(&self) -> bool {
        if self.limit_per_min == 0 {
            return true;
        }
        let now = Instant::now();
        let mut calls = match self.calls.lock() {
            Ok(calls) => calls,
            // Отравленный мьютекс не должен открывать «шлюз»: fail-closed.
            Err(poisoned) => poisoned.into_inner(),
        };
        while let Some(front) = calls.front() {
            if now.duration_since(*front) > self.window {
                calls.pop_front();
            } else {
                break;
            }
        }
        if calls.len() >= self.limit_per_min as usize {
            return false;
        }
        calls.push_back(now);
        true
    }

    /// Сколько вызовов ещё доступно в текущем окне (для диагностики/тестов).
    pub fn remaining(&self) -> u32 {
        if self.limit_per_min == 0 {
            return u32::MAX;
        }
        let calls = match self.calls.lock() {
            Ok(calls) => calls,
            Err(poisoned) => poisoned.into_inner(),
        };
        let now = Instant::now();
        let used = calls.iter().filter(|at| now.duration_since(**at) <= self.window).count();
        (self.limit_per_min as usize).saturating_sub(used) as u32
    }
}

/// Сессионный кэш разрешений `AllowSession` (ключ — инструмент + SHA-256 аргументов).
///
/// `AllowOnce` в кэш не попадает: каждый следующий вызов спрашивает approval-хук снова.
/// Кэш живёт в процессе сервера (как и jobs M2) и не переживает перезапуск.
#[derive(Debug, Default)]
pub struct ApprovalCache {
    allowed: Mutex<std::collections::HashSet<String>>,
}

impl ApprovalCache {
    /// Новый пустой кэш.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ключ разрешения: инструмент + хеш аргументов (одинаковые аргументы → одно разрешение).
    pub fn key(tool: &str, args_sha256: &str) -> String {
        format!("{tool}:{args_sha256}")
    }

    /// Было ли разрешение `AllowSession` для этих аргументов.
    pub fn is_allowed(&self, key: &str) -> bool {
        match self.allowed.lock() {
            Ok(allowed) => allowed.contains(key),
            Err(poisoned) => poisoned.into_inner().contains(key),
        }
    }

    /// Запомнить сессионное разрешение.
    pub fn remember(&self, key: String) {
        match self.allowed.lock() {
            Ok(mut allowed) => {
                allowed.insert(key);
            }
            Err(poisoned) => {
                poisoned.into_inner().insert(key);
            }
        }
    }

    /// Сколько разрешений закэшировано (тесты/диагностика).
    pub fn len(&self) -> usize {
        match self.allowed.lock() {
            Ok(allowed) => allowed.len(),
            Err(poisoned) => poisoned.into_inner().len(),
        }
    }

    /// Кэш пуст.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Успешная авторизация вызова: `reason` идёт в аудит (`allow`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthOutcome {
    /// Причина разрешения (`read_only`, `policy`, `allow_once`, `allow_session`, `session`).
    pub reason: &'static str,
}

/// Отказ авторизации: ошибка клиенту + причина для аудита (`deny`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthDenial {
    /// Машиночитаемая ошибка (`RDE_APPROVAL_DENIED`/`RDE_UNSUPPORTED`).
    pub error: HostError,
    /// Причина отказа (`allowlist`, `policy`, `grant`, `approval`).
    pub reason: &'static str,
}

impl AuthDenial {
    fn new(error: HostError, reason: &'static str) -> Self {
        Self { error, reason }
    }
}

/// Проверить право на вызов инструмента (без rate-limit — он у сервера, т.к. состояние).
///
/// Порядок (fail-closed):
/// 1. белый список `allowed_tools`;
/// 2. read-only: `allow_read_only`; мутирующие: грант политики [`ToolGrant`], затем кэш
///    `AllowSession`, затем [`ApprovalHook`];
/// 3. `AllowSession` запоминается в [`ApprovalCache`] (ключ — инструмент + хеш аргументов).
///
/// Ошибки всегда `RDE_APPROVAL_DENIED`/`RDE_UNSUPPORTED` — никогда panic.
pub async fn authorize_tool_call(
    policy: &McpPolicy,
    hooks: &HostHooks,
    cache: &ApprovalCache,
    tool: &ToolSpec,
    args: &serde_json::Value,
) -> Result<AuthOutcome, AuthDenial> {
    if !policy.tool_allowed(tool.name) {
        return Err(AuthDenial::new(
            HostError::approval_denied(format!("инструмент `{}` не разрешён политикой", tool.name))
                .with_hint("allowed_tools политики не содержит этот инструмент"),
            "allowlist",
        ));
    }
    if tool.annotations.read_only {
        if !policy.allow_read_only {
            return Err(AuthDenial::new(
                HostError::approval_denied("read-only инструменты запрещены политикой"),
                "policy",
            ));
        }
        return Ok(AuthOutcome { reason: "read_only" });
    }
    // Мутирующий инструмент обязан объявить грант: без гранта — ошибка слоя (fail-closed).
    let Some(grant) = tool.grant else {
        return Err(AuthDenial::new(
            HostError::unsupported(format!("мутирующий инструмент `{}` без гранта политики", tool.name)),
            "grant",
        ));
    };
    if !policy.grant_allowed(grant) {
        return Err(AuthDenial::new(
            HostError::approval_denied(format!(
                "мутации запрещены политикой: инструмент `{}` не выполнен",
                tool.name
            ))
            .with_hint(format!(
                "грант `{}` выключен; approvals по умолчанию deny (CLI: --approval=auto|allowlist)",
                grant.as_str()
            )),
            "policy",
        ));
    }
    let args_sha256 = crate::hooks::params_sha256(args);
    let cache_key = ApprovalCache::key(tool.name, &args_sha256);
    if cache.is_allowed(&cache_key) {
        return Ok(AuthOutcome { reason: "session" });
    }
    let request = ApprovalRequest {
        tool: tool.name.to_owned(),
        args_summary: summarize_args(args),
        args_sha256: args_sha256.clone(),
        destructive: tool.annotations.destructive,
        requested_by: "mcp".to_owned(),
    };
    match hooks.approvals.request(request).await {
        ApprovalDecision::AllowOnce => Ok(AuthOutcome { reason: "allow_once" }),
        ApprovalDecision::AllowSession => {
            cache.remember(cache_key);
            Ok(AuthOutcome { reason: "allow_session" })
        }
        ApprovalDecision::Deny => Err(AuthDenial::new(
            HostError::approval_denied(format!("пользователь отклонил вызов `{}`", tool.name)),
            "approval",
        )),
    }
}

/// Короткая выжимка аргументов для approval-запроса (без тел; обрезана 200 символами).
pub fn summarize_args(args: &serde_json::Value) -> String {
    let text = serde_json::to_string(args).unwrap_or_else(|_| "{}".to_owned());
    if text.chars().count() <= 200 {
        text
    } else {
        let truncated: String = text.chars().take(200).collect();
        format!("{truncated}…")
    }
}

/// Headless-режим approvals (`--approval=…`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalMode {
    /// Всё, что не read-only, отклоняется (дефолт, fail-closed).
    Deny,
    /// Мутации, допущенные грантами, автоматически разрешаются (явный флаг CLI).
    Auto,
    /// Разрешаются только перечисленные инструменты (`--allow-tools`), fail-closed.
    Allowlist,
}

impl ApprovalMode {
    /// Разбор значения флага `--approval`.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim() {
            "deny" => Ok(ApprovalMode::Deny),
            "auto" => Ok(ApprovalMode::Auto),
            "allowlist" => Ok(ApprovalMode::Allowlist),
            other => Err(format!("неизвестный режим approvals `{other}` (deny|auto|allowlist)")),
        }
    }

    /// Строковое имя режима.
    pub fn as_str(self) -> &'static str {
        match self {
            ApprovalMode::Deny => "deny",
            ApprovalMode::Auto => "auto",
            ApprovalMode::Allowlist => "allowlist",
        }
    }
}

/// Параметры CLI-approvals (`rde mcp serve`): режим + флаги грантов.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ApprovalOptions {
    /// Режим approvals.
    pub mode: Option<ApprovalMode>,
    /// Разрешённые инструменты для режима `allowlist`.
    pub allow_tools: Vec<String>,
    /// `--allow-run`: отдельный грант запуска скриптов (R-AGT-06).
    pub allow_run: bool,
    /// `--allow-destructive`: грант деструктивных инструментов вне run (git-коммит).
    pub allow_destructive: bool,
}

impl ApprovalOptions {
    /// Дефолт: `--approval=deny`, никаких грантов (fail-closed).
    pub fn deny() -> Self {
        Self { mode: Some(ApprovalMode::Deny), ..Default::default() }
    }

    /// Режим (по умолчанию `deny`).
    pub fn mode(&self) -> ApprovalMode {
        self.mode.unwrap_or(ApprovalMode::Deny)
    }

    /// Проверить согласованность флагов (понятные ошибки вместо молчаливого игнорирования).
    pub fn validate(&self) -> Result<(), String> {
        let mode = self.mode();
        if mode == ApprovalMode::Deny {
            if !self.allow_tools.is_empty() {
                return Err("--allow-tools требует --approval=allowlist".to_owned());
            }
            if self.allow_run || self.allow_destructive {
                return Err(
                    "--allow-run/--allow-destructive требуют --approval=auto|allowlist (с --approval=deny мутации запрещены)"
                        .to_owned(),
                );
            }
            return Ok(());
        }
        if mode == ApprovalMode::Auto && !self.allow_tools.is_empty() {
            return Err("--allow-tools используется только с --approval=allowlist".to_owned());
        }
        if mode == ApprovalMode::Allowlist {
            if self.allow_tools.is_empty() {
                return Err("--approval=allowlist требует --allow-tools=<инструмент,...>".to_owned());
            }
            if self.allow_run {
                return Err("с --approval=allowlist грант запуска задаётся через --allow-tools".to_owned());
            }
            if self.allow_destructive {
                return Err("с --approval=allowlist грант git-коммита задаётся через --allow-tools".to_owned());
            }
            for name in &self.allow_tools {
                let Some(tool) = tools::find_tool(name) else {
                    return Err(format!("--allow-tools: неизвестный инструмент `{name}`"));
                };
                if tool.annotations.read_only {
                    return Err(format!(
                        "--allow-tools: `{name}` — read-only инструмент (разрешён всегда, грант не нужен)"
                    ));
                }
            }
        }
        Ok(())
    }

    /// Собрать политику прав по параметрам (без approvals-хука).
    pub fn policy(&self) -> McpPolicy {
        let mut policy = McpPolicy::read_only();
        policy.approval_mode = self.mode().as_str().to_owned();
        match self.mode() {
            ApprovalMode::Deny => {}
            ApprovalMode::Auto => {
                policy.allow_project_write = true;
                policy.allow_run_script = self.allow_run;
                policy.allow_git_commit = self.allow_destructive;
            }
            ApprovalMode::Allowlist => {
                for name in &self.allow_tools {
                    let Some(tool) = tools::find_tool(name) else { continue };
                    match tool.grant {
                        Some(ToolGrant::ProjectWrite) => policy.allow_project_write = true,
                        Some(ToolGrant::RunScript) => policy.allow_run_script = true,
                        Some(ToolGrant::GitCommit) => policy.allow_git_commit = true,
                        None => {}
                    }
                }
            }
        }
        policy
    }

    /// Собрать approvals-хук по параметрам (всегда fail-closed по умолчанию).
    pub fn hook(&self) -> Arc<dyn crate::hooks::ApprovalHook> {
        match self.mode() {
            ApprovalMode::Deny => Arc::new(DenyAllHook),
            ApprovalMode::Auto => Arc::new(AutoApproveHook),
            ApprovalMode::Allowlist => Arc::new(AllowlistHook::new(self.allow_tools.clone())),
        }
    }
}

/// Собрать политику и хук approvals по CLI-параметрам (валидирует флаги).
///
/// # Errors
///
/// Строка с понятной причиной: неизвестный режим/инструмент, несогласованные флаги.
pub fn build_policy_and_hook(
    options: &ApprovalOptions,
) -> Result<(McpPolicy, Arc<dyn crate::hooks::ApprovalHook>), String> {
    options.validate()?;
    Ok((options.policy(), options.hook()))
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::error::RdeErrorCode;
    use crate::hooks::{
        ApprovalHook, ApprovalRequest, HostHooks, NullAuditSink, NullEventSink,
    };
    use crate::tools::ToolAnnotationsSpec;

    fn read_only_spec() -> ToolSpec {
        ToolSpec {
            name: "project_tree",
            title: "tree",
            description: "read",
            input_schema: || serde_json::json!({"type": "object"}),
            output_schema: || serde_json::json!({"type": "object"}),
            annotations: ToolAnnotationsSpec::read_only(),
            grant: None,
            handler: |host, _args| host.project_tree().map(|tree| serde_json::json!(tree)),
        }
    }

    fn mutating_spec() -> ToolSpec {
        ToolSpec {
            name: "artifact_run",
            title: "run",
            description: "mutate",
            input_schema: || serde_json::json!({"type": "object"}),
            output_schema: || serde_json::json!({"type": "object"}),
            annotations: ToolAnnotationsSpec::mutating(true),
            grant: Some(ToolGrant::RunScript),
            handler: |_host, _args| Err(HostError::unsupported("нет в M1")),
        }
    }

    struct AllowHook;

    #[async_trait]
    impl ApprovalHook for AllowHook {
        async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
            ApprovalDecision::AllowOnce
        }
    }

    struct SessionHook;

    #[async_trait]
    impl ApprovalHook for SessionHook {
        async fn request(&self, _request: ApprovalRequest) -> ApprovalDecision {
            ApprovalDecision::AllowSession
        }
    }

    fn hooks_with(approvals: Arc<dyn ApprovalHook>) -> HostHooks {
        HostHooks::new(approvals, Arc::new(NullAuditSink), Arc::new(NullEventSink))
    }

    fn policy_with_grant(grant: ToolGrant) -> McpPolicy {
        let mut policy = McpPolicy::read_only();
        match grant {
            ToolGrant::ProjectWrite => policy.allow_project_write = true,
            ToolGrant::RunScript => policy.allow_run_script = true,
            ToolGrant::GitCommit => policy.allow_git_commit = true,
        }
        policy
    }

    #[tokio::test]
    async fn read_only_allowed_by_default() {
        let policy = McpPolicy::read_only();
        let hooks = HostHooks::silent();
        let cache = ApprovalCache::new();
        let outcome = authorize_tool_call(&policy, &hooks, &cache, &read_only_spec(), &serde_json::json!({}))
            .await
            .expect("read-only инструмент разрешён по умолчанию");
        assert_eq!(outcome.reason, "read_only");
    }

    #[tokio::test]
    async fn mutating_tool_is_denied_by_default_fail_closed() {
        let policy = McpPolicy::read_only();
        let hooks = HostHooks::silent();
        let cache = ApprovalCache::new();
        let denial = authorize_tool_call(&policy, &hooks, &cache, &mutating_spec(), &serde_json::json!({}))
            .await
            .expect_err("мутирующий инструмент должен быть запрещён по умолчанию");
        assert_eq!(denial.error.code, RdeErrorCode::ApprovalDenied);
        assert_eq!(denial.reason, "policy");
    }

    #[tokio::test]
    async fn mutating_tool_with_grant_uses_deny_all_hook() {
        let policy = policy_with_grant(ToolGrant::RunScript);
        // DenyAllHook по умолчанию: fail-closed даже если политика разрешила грант.
        let hooks = HostHooks::silent();
        let cache = ApprovalCache::new();
        let denial = authorize_tool_call(&policy, &hooks, &cache, &mutating_spec(), &serde_json::json!({"id": "x"}))
            .await
            .expect_err("DenyAllHook обязан отклонить");
        assert_eq!(denial.error.code, RdeErrorCode::ApprovalDenied);
        assert_eq!(denial.reason, "approval");
    }

    #[tokio::test]
    async fn mutating_tool_allowed_when_hook_allows() {
        let policy = policy_with_grant(ToolGrant::RunScript);
        let hooks = hooks_with(Arc::new(AllowHook));
        let cache = ApprovalCache::new();
        let outcome = authorize_tool_call(&policy, &hooks, &cache, &mutating_spec(), &serde_json::json!({}))
            .await
            .expect("хук разрешил — вызов авторизован");
        assert_eq!(outcome.reason, "allow_once");
    }

    #[tokio::test]
    async fn wrong_grant_does_not_authorize_tool() {
        let policy = policy_with_grant(ToolGrant::ProjectWrite);
        let hooks = hooks_with(Arc::new(AllowHook));
        let cache = ApprovalCache::new();
        let denial = authorize_tool_call(&policy, &hooks, &cache, &mutating_spec(), &serde_json::json!({}))
            .await
            .expect_err("грант project_write не покрывает run_script");
        assert_eq!(denial.reason, "policy");
    }

    #[tokio::test]
    async fn allow_session_is_cached_by_tool_and_args() {
        let policy = policy_with_grant(ToolGrant::RunScript);
        let hooks = hooks_with(Arc::new(SessionHook));
        let cache = ApprovalCache::new();
        let first = authorize_tool_call(&policy, &hooks, &cache, &mutating_spec(), &serde_json::json!({"id": "x"}))
            .await
            .expect("сессионное разрешение");
        assert_eq!(first.reason, "allow_session");
        assert_eq!(cache.len(), 1);
        let second = authorize_tool_call(&policy, &hooks, &cache, &mutating_spec(), &serde_json::json!({"id": "x"}))
            .await
            .expect("кэшированное разрешение");
        assert_eq!(second.reason, "session");
        // Другие аргументы — снова approval (ключ = (tool, args_sha256)).
        let third = authorize_tool_call(&policy, &hooks, &cache, &mutating_spec(), &serde_json::json!({"id": "y"}))
            .await
            .expect("другие аргументы — новый approval");
        assert_eq!(third.reason, "allow_session");
        assert_eq!(cache.len(), 2);
    }

    #[tokio::test]
    async fn allowlist_restricts_tools() {
        let mut policy = McpPolicy::read_only();
        policy.allowed_tools = Some(vec!["doc_list".to_owned()]);
        let hooks = HostHooks::silent();
        let cache = ApprovalCache::new();
        let denial = authorize_tool_call(&policy, &hooks, &cache, &read_only_spec(), &serde_json::json!({}))
            .await
            .expect_err("инструмент вне белого списка запрещён");
        assert_eq!(denial.error.code, RdeErrorCode::ApprovalDenied);
        assert_eq!(denial.reason, "allowlist");
        assert!(policy.tool_allowed("doc_list"));
        assert!(!policy.tool_allowed("project_tree"));
    }

    #[test]
    fn approval_modes_build_policy_and_hook_fail_closed() {
        let deny = ApprovalOptions::deny();
        deny.validate().unwrap();
        let (policy, _hook) = build_policy_and_hook(&deny).unwrap();
        assert!(!policy.allow_project_write && !policy.allow_run_script && !policy.allow_git_commit);
        assert_eq!(policy.approval_mode, "deny");

        let auto = ApprovalOptions {
            mode: Some(ApprovalMode::Auto),
            allow_run: true,
            ..Default::default()
        };
        let (policy, _hook) = build_policy_and_hook(&auto).unwrap();
        assert!(policy.allow_project_write);
        assert!(policy.allow_run_script);
        assert!(!policy.allow_git_commit, "git-коммит требует --allow-destructive");
        assert_eq!(policy.approval_mode, "auto");

        let allowlist = ApprovalOptions {
            mode: Some(ApprovalMode::Allowlist),
            allow_tools: vec!["artifact_run".to_owned(), "note_write".to_owned()],
            ..Default::default()
        };
        let (policy, _hook) = build_policy_and_hook(&allowlist).unwrap();
        assert!(policy.allow_project_write, "note_write — project_write");
        assert!(policy.allow_run_script, "artifact_run — run_script");
        assert!(!policy.allow_git_commit);

        // Несогласованные флаги — понятные ошибки, а не молчаливое игнорирование.
        let bad = ApprovalOptions { mode: Some(ApprovalMode::Deny), allow_run: true, ..Default::default() };
        assert!(bad.validate().is_err());
        let bad = ApprovalOptions { mode: Some(ApprovalMode::Allowlist), ..Default::default() };
        assert!(bad.validate().is_err());
        let bad = ApprovalOptions {
            mode: Some(ApprovalMode::Allowlist),
            allow_tools: vec!["unknown_tool".to_owned()],
            ..Default::default()
        };
        assert!(bad.validate().is_err());
        let bad = ApprovalOptions {
            mode: Some(ApprovalMode::Allowlist),
            allow_tools: vec!["project_tree".to_owned()],
            ..Default::default()
        };
        assert!(bad.validate().is_err(), "read-only инструмент в --allow-tools — ошибка");
    }

    #[test]
    fn instructions_section_reflects_grants() {
        let deny = McpPolicy::read_only();
        let text = deny.instructions_section();
        assert!(text.contains("`deny`"));
        assert!(text.contains("запись в проект (`artifact_write_manifest`/`artifact_write_script`/`note_write`): запрещено"));
        let (auto, _hook) = build_policy_and_hook(&ApprovalOptions {
            mode: Some(ApprovalMode::Auto),
            allow_run: true,
            ..Default::default()
        })
        .unwrap();
        let text = auto.instructions_section();
        assert!(text.contains("`auto`"));
        assert!(text.contains("запуск артефактов (`artifact_run`/`run_cancel`, нужен `trusted`-проект): разрешено"));
    }

    #[test]
    fn rate_limiter_window_and_limit() {
        let limiter = RateLimiter::new(2);
        assert!(limiter.check());
        assert!(limiter.check());
        assert!(!limiter.check(), "третий вызов за минуту должен быть отклонён");
        assert_eq!(limiter.remaining(), 0);
        let unlimited = RateLimiter::new(0);
        for _ in 0..100 {
            assert!(unlimited.check());
        }
    }

    #[test]
    fn args_summary_is_bounded() {
        let args = serde_json::json!({"text": "x".repeat(1000)});
        let summary = summarize_args(&args);
        assert!(summary.chars().count() <= 201, "выжимка обрезана: {}", summary.chars().count());
    }

    #[test]
    fn grant_names_are_stable() {
        assert_eq!(ToolGrant::ProjectWrite.as_str(), "project_write");
        assert_eq!(ToolGrant::RunScript.as_str(), "run_script");
        assert_eq!(ToolGrant::GitCommit.as_str(), "git_commit");
        assert_eq!(ToolGrant::ALL.len(), 3);
    }
}
