//! M4.4: diff-UI suggested edits (R-AGT-05, §8.7.3 ТЗ; ADR-0008/ADR-0015).
//!
//! Агент (встроенный ACP или внешний по MCP) не переписывает документ молча: он создаёт
//! **предложения** в `.rde/agent/suggestions/*.json` (`rde_doc::Suggestions`). Это окно —
//! UI-ревью правок: pending-список, **word-diff** «было/станет», accept/reject по одной,
//! «отклонить все», история решений.
//!
//! Принятие идёт тем же безопасным путём, что у MCP-инструмента `suggestion_accept` (M2.5):
//! чекпоинт файлов + корней store (`.rde/agent/checkpoints`), `backup_package`, применение
//! changeset (проверки `base_body_hash`/`expected_text` — R-DOC-13), версия документа в store,
//! аудит `.rde/agent/audit.log` (`tool = suggestion_accept|suggestion_reject`). Отклонение —
//! решение пользователя (`deny`) с аудитом. Это Human-in-the-loop: агент не может принять
//! правку сам.

use std::collections::BTreeMap;
use std::path::Path;

use rde_core::Level;
use rde_doc::{STATUS_PENDING, SuggestedEdit, Suggestions};
use rde_host::CheckpointStore;
use rde_mcp::{AuditEntry, AuditSink, JsonlAuditSink};

use crate::RdeApp;

/// Сегмент word-diff (`before` → `after`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffSegment {
    /// Слово не изменилось.
    Same(String),
    /// Слово удалено.
    Removed(String),
    /// Слово добавлено.
    Added(String),
}

impl DiffSegment {
    /// Текст сегмента.
    pub fn text(&self) -> &str {
        match self {
            DiffSegment::Same(text) | DiffSegment::Removed(text) | DiffSegment::Added(text) => text,
        }
    }
}

/// Word-level diff «было/станет» (LCS по словам; пробелы не сравниваются).
///
/// Для очень длинных блоков (> 600 слов с каждой стороны) — консервативный fallback:
/// общий префикс/суффикс, середина — целиком removed+added (без O(n²)).
pub fn word_diff(before: &str, after: &str) -> Vec<DiffSegment> {
    let left: Vec<&str> = before.split_whitespace().collect();
    let right: Vec<&str> = after.split_whitespace().collect();
    if left == right {
        return if left.is_empty() { Vec::new() } else { vec![DiffSegment::Same(left.join(" "))] };
    }
    if left.len() > 600 || right.len() > 600 {
        return prefix_suffix_diff(&left, &right);
    }
    // LCS DP (n*m; для блоков документа это сотни слов — приемлемо).
    let n = left.len();
    let m = right.len();
    let mut dp = vec![vec![0usize; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if left[i] == right[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut segments: Vec<DiffSegment> = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < n && j < m {
        if left[i] == right[j] {
            push_segment(&mut segments, DiffSegment::Same(left[i].to_owned()));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            push_segment(&mut segments, DiffSegment::Removed(left[i].to_owned()));
            i += 1;
        } else {
            push_segment(&mut segments, DiffSegment::Added(right[j].to_owned()));
            j += 1;
        }
    }
    while i < n {
        push_segment(&mut segments, DiffSegment::Removed(left[i].to_owned()));
        i += 1;
    }
    while j < m {
        push_segment(&mut segments, DiffSegment::Added(right[j].to_owned()));
        j += 1;
    }
    segments
}

fn prefix_suffix_diff(left: &[&str], right: &[&str]) -> Vec<DiffSegment> {
    let mut prefix = 0;
    while prefix < left.len() && prefix < right.len() && left[prefix] == right[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < left.len() - prefix
        && suffix < right.len() - prefix
        && left[left.len() - 1 - suffix] == right[right.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let mut segments = Vec::new();
    if prefix > 0 {
        push_segment(&mut segments, DiffSegment::Same(left[..prefix].join(" ")));
    }
    if prefix < left.len() - suffix {
        push_segment(
            &mut segments,
            DiffSegment::Removed(left[prefix..left.len() - suffix].join(" ")),
        );
    }
    if prefix < right.len() - suffix {
        push_segment(
            &mut segments,
            DiffSegment::Added(right[prefix..right.len() - suffix].join(" ")),
        );
    }
    if suffix > 0 {
        push_segment(&mut segments, DiffSegment::Same(left[left.len() - suffix..].join(" ")));
    }
    segments
}

fn push_segment(segments: &mut Vec<DiffSegment>, segment: DiffSegment) {
    match (segments.last_mut(), segment) {
        (Some(DiffSegment::Same(previous)), DiffSegment::Same(text))
        | (Some(DiffSegment::Removed(previous)), DiffSegment::Removed(text))
        | (Some(DiffSegment::Added(previous)), DiffSegment::Added(text)) => {
            previous.push(' ');
            previous.push_str(&text);
        }
        (_, segment) => segments.push(segment),
    }
}

/// Статистика diff: (добавлено слов, удалено слов).
pub fn diff_stats(segments: &[DiffSegment]) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for segment in segments {
        match segment {
            DiffSegment::Added(text) => added += text.split_whitespace().count(),
            DiffSegment::Removed(text) => removed += text.split_whitespace().count(),
            DiffSegment::Same(_) => {}
        }
    }
    (added, removed)
}

/// SHA-256 канонизированных аргументов решения (тела документа не пишутся — как в аудите M2).
fn suggestion_args_sha(edit: &SuggestedEdit) -> String {
    let value = serde_json::json!({
        "suggestion_id": edit.id,
        "doc_id": edit.doc_id,
        "block_id": edit.block_id,
        "author": edit.author,
        "author_kind": edit.author_kind,
        "base_body_hash": edit.base_body_hash,
        "before_len": edit.before.chars().count(),
        "after_len": edit.after.chars().count(),
    });
    let canonical = rde_hash::canonical_json(&value).unwrap_or_else(|_| value.to_string());
    rde_hash::sha256(canonical.as_bytes()).hex()
}

/// Запись решения по правке в аудит проекта.
fn audit_suggestion(
    project_root: &Path,
    tool: &str,
    decision: &str,
    reason: &str,
    checkpoint_id: Option<&str>,
    args_sha256: &str,
) {
    let mut entry = AuditEntry::new(tool, args_sha256.to_owned(), decision, true).with_reason(reason);
    if let Some(checkpoint_id) = checkpoint_id {
        entry = entry.with_checkpoint_id(checkpoint_id);
    }
    JsonlAuditSink::new(project_root).record(&entry);
}

fn relative_posix(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| path.to_string_lossy().replace('\\', "/"))
}

impl RdeApp {
    /// Pending-правки для окна ревью (кэш; `refresh` — перечитать `.rde/agent/suggestions`).
    ///
    /// Демо/тест-режим (`suggestions_demo`) возвращает подготовленные записи без ФС.
    pub(crate) fn load_suggestions(&mut self, refresh: bool) -> Vec<SuggestedEdit> {
        if let Some(demo) = &self.suggestions_demo {
            return demo
                .iter()
                .filter(|edit| edit.status == STATUS_PENDING)
                .cloned()
                .collect();
        }
        if refresh || self.suggestions_cache.is_none() {
            self.suggestions_cache = Some(
                Suggestions::new(&self.project.root)
                    .list(None, Some(STATUS_PENDING))
                    .unwrap_or_else(|_| Vec::new()),
            );
        }
        self.suggestions_cache.clone().unwrap_or_default()
    }

    /// Перечитать pending-правки; возвращает их число (для бейджа «Правки…»).
    pub(crate) fn refresh_suggestions(&mut self) -> usize {
        self.load_suggestions(true).len()
    }

    /// Число pending-правок из кэша (0, если ещё не читали).
    pub(crate) fn pending_suggestions(&self) -> usize {
        self.suggestions_demo
            .as_ref()
            .map(|demo| demo.iter().filter(|edit| edit.status == STATUS_PENDING).count())
            .or_else(|| self.suggestions_cache.as_ref().map(Vec::len))
            .unwrap_or(0)
    }

    /// Открыть окно ревью правок (лента «Агент» → «Правки…», панель чата).
    pub(crate) fn open_suggestions(&mut self) {
        self.show_suggestions = true;
        let count = self.refresh_suggestions();
        self.push_log(Level::Info, format!("Правки агента: {count} ожидают решения"));
    }

    /// Принять предложение правки: чекпоинт → backup → changeset → версия в store → аудит.
    pub(crate) fn suggestion_accept(&mut self, suggestion_id: &str) -> Result<String, String> {
        if self.suggestions_demo.is_some() {
            if let Some(demo) = self.suggestions_demo.as_mut() {
                demo.retain(|edit| edit.id != suggestion_id);
            }
            self.suggestions_cache = None;
            return Ok(format!("демо-правка {suggestion_id} принята"));
        }
        let root = self.project.root.clone();
        let suggestions = Suggestions::new(&root);
        let edit = suggestions.load(suggestion_id).map_err(|error| error.to_string())?;
        if edit.status != STATUS_PENDING {
            return Err(format!(
                "правка `{suggestion_id}` уже обработана (status={})",
                edit.status
            ));
        }
        let package = self
            .project
            .document_path()
            .ok_or_else(|| "в проекте нет документа".to_owned())?;
        let relative = relative_posix(&root, &package);
        let args_sha = suggestion_args_sha(&edit);
        // 1) Чекпоинт: байт-в-байт копия пакета + корни store (откат M2-схемой).
        let roots = self.store.as_ref().and_then(|store| store.current_roots().ok());
        let checkpoints = CheckpointStore::new(&root);
        let checkpoint = checkpoints
            .create(
                "suggestion_accept",
                &args_sha,
                roots,
                &[(relative.clone(), package.clone())],
                Some(format!("suggestion {suggestion_id}: {}/{}", edit.doc_id, edit.block_id)),
            )
            .map_err(|error| error.to_string())?;
        // 2) Backup пакета + применение changeset (проверки base_body_hash/expected_text).
        let _ = rde_doc::backup_package(&root, &package);
        let applied = match suggestions.apply(suggestion_id, &package) {
            Ok(applied) => applied,
            Err(error) => {
                let _ = checkpoints.rollback_files(&checkpoint);
                let _ = checkpoints.mark_rolled_back(&checkpoint);
                audit_suggestion(&root, "suggestion_accept", "deny", "apply_error", Some(&checkpoint.id), &args_sha);
                return Err(format!("правка не применена (откат чекпоинта): {error}"));
            }
        };
        // 3) Версия документа в store (история/откат).
        let mut revision_id = String::new();
        if let Some(store) = self.store.as_mut() {
            let payload = serde_json::json!({
                "kind": "document-mutation",
                "tool": "suggestion_accept",
                "suggestion_id": suggestion_id,
                "doc_id": applied.doc_id,
            });
            match rde_doc::commit_document_with(
                &root,
                &package,
                &applied.doc_id,
                store,
                &format!("suggestion accept {suggestion_id}"),
                &BTreeMap::new(),
                Some(payload),
            ) {
                Ok(revision) => revision_id = revision,
                Err(error) => self.push_log(Level::Warn, format!("Версия документа: {error}")),
            }
        }
        // 4) Аудит: allow + чекпоинт.
        audit_suggestion(
            &root,
            "suggestion_accept",
            "allow",
            &format!("approval:{}", applied.block_id),
            Some(&checkpoint.id),
            &args_sha,
        );
        // 5) Обновить модель проекта/структуру/кэш правок.
        self.reload_project();
        self.suggestions_cache = None;
        let mut message = format!(
            "Правка принята: {} ({}/{})",
            suggestion_id, applied.doc_id, applied.block_id
        );
        if !revision_id.is_empty() {
            message.push_str(&format!(" · версия {}…", revision_id.chars().take(8).collect::<String>()));
        }
        self.push_log(Level::Ok, message.clone());
        Ok(message)
    }

    /// Отклонить предложение (решение пользователя, аудит `deny`).
    pub(crate) fn suggestion_reject(
        &mut self,
        suggestion_id: &str,
        reason: &str,
    ) -> Result<String, String> {
        if self.suggestions_demo.is_some() {
            if let Some(demo) = self.suggestions_demo.as_mut() {
                demo.retain(|edit| edit.id != suggestion_id);
            }
            self.suggestions_cache = None;
            return Ok(format!("демо-правка {suggestion_id} отклонена"));
        }
        let root = self.project.root.clone();
        let suggestions = Suggestions::new(&root);
        let edit = suggestions.load(suggestion_id).map_err(|error| error.to_string())?;
        let args_sha = suggestion_args_sha(&edit);
        let reason = reason.trim();
        let reason = if reason.is_empty() { "пользователь отклонил" } else { reason };
        suggestions
            .reject(suggestion_id, Some(reason))
            .map_err(|error| error.to_string())?;
        audit_suggestion(&root, "suggestion_reject", "deny", reason, None, &args_sha);
        self.suggestions_cache = None;
        let message = format!("Правка отклонена: {suggestion_id} ({reason})");
        self.push_log(Level::Info, message.clone());
        Ok(message)
    }

    /// Отклонить все pending-правки; возвращает число.
    pub(crate) fn suggestion_reject_all(&mut self, reason: &str) -> Result<usize, String> {
        if self.suggestions_demo.is_some() {
            let count = self
                .suggestions_demo
                .as_ref()
                .map(|demo| demo.iter().filter(|edit| edit.status == STATUS_PENDING).count())
                .unwrap_or(0);
            if let Some(demo) = self.suggestions_demo.as_mut() {
                demo.retain(|edit| edit.status != STATUS_PENDING);
            }
            self.suggestions_cache = None;
            return Ok(count);
        }
        let root = self.project.root.clone();
        let suggestions = Suggestions::new(&root);
        let pending = suggestions
            .list(None, Some(STATUS_PENDING))
            .map_err(|error| error.to_string())?;
        let reason = reason.trim();
        let reason = if reason.is_empty() { "пользователь отклонил все" } else { reason };
        let mut count = 0;
        for edit in pending {
            let args_sha = suggestion_args_sha(&edit);
            if suggestions.reject(&edit.id, Some(reason)).is_ok() {
                audit_suggestion(&root, "suggestion_reject", "deny", reason, None, &args_sha);
                count += 1;
            }
        }
        self.suggestions_cache = None;
        self.push_log(Level::Info, format!("Правки отклонены: {count}"));
        Ok(count)
    }

    /// Окно «Правки агента»: pending-список с word-diff, accept/reject, история решений.
    pub(crate) fn suggestions_window(&mut self, ctx: &egui::Context) {
        if !self.show_suggestions {
            return;
        }
        let pal = crate::theme_colors(ctx.theme());
        let pending = self.load_suggestions(false);
        let mut accept: Option<String> = None;
        let mut reject: Option<String> = None;
        let mut reject_all = false;
        let mut refresh = false;
        let mut close = false;
        egui::Window::new("Правки агента (suggested edits)")
            .id(egui::Id::new("rde_suggestions_window"))
            .collapsible(false)
            .resizable(true)
            .default_width(760.0)
            .default_height(560.0)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, -20.0))
            .show(ctx, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        egui::RichText::new(format!("Ожидают решения: {}", pending.len()))
                            .strong()
                            .color(pal.text),
                    );
                    ui.label(
                        egui::RichText::new(
                            "Агент не меняет документ молча: правка применяется только после «Принять» \
                             (чекпоинт + версия + аудит); «Отклонить» — решение с аудитом.",
                        )
                        .small()
                        .color(pal.muted),
                    );
                });
                ui.horizontal(|ui| {
                    if ui.button("Обновить").clicked() {
                        refresh = true;
                    }
                    if ui
                        .add_enabled(!pending.is_empty(), egui::Button::new("Отклонить все"))
                        .clicked()
                    {
                        reject_all = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Закрыть").clicked() {
                            close = true;
                        }
                    });
                });
                ui.separator();
                if pending.is_empty() {
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(
                            "Pending-правок нет. Агент создаёт их инструментом `doc_suggest_edit` \
                             (встроенный ACP-агент или внешний по MCP) — они появятся здесь.",
                        )
                        .small()
                        .color(pal.muted),
                    );
                }
                egui::ScrollArea::vertical()
                    .id_salt("rde_suggestions_list")
                    .auto_shrink([false, false])
                    .max_height(430.0)
                    .show(ui, |ui| {
                        for edit in &pending {
                            let segments = word_diff(&edit.before, &edit.after);
                            let (added, removed) = diff_stats(&segments);
                            egui::Frame::new()
                                .fill(pal.panel_hi)
                                .inner_margin(egui::Margin::symmetric(8, 6))
                                .show(ui, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} · блок {}",
                                                edit.doc_id, edit.block_id
                                            ))
                                            .strong()
                                            .size(12.0)
                                            .color(pal.text),
                                        );
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} · {}{}",
                                                edit.author,
                                                edit.author_kind,
                                                if edit.note.as_deref().unwrap_or("").is_empty() {
                                                    String::new()
                                                } else {
                                                    format!(" · {}", edit.note.as_deref().unwrap_or(""))
                                                }
                                            ))
                                            .small()
                                            .color(pal.muted),
                                        );
                                        ui.label(
                                            egui::RichText::new(format!("+{added} / −{removed} слов"))
                                                .small()
                                                .color(pal.muted),
                                        );
                                    });
                                    ui.add_space(2.0);
                                    ui.horizontal_wrapped(|ui| {
                                        for segment in &segments {
                                            let text = segment.text();
                                            match segment {
                                                DiffSegment::Same(_) => {
                                                    ui.label(
                                                        egui::RichText::new(text)
                                                            .size(11.5)
                                                            .color(pal.muted),
                                                    );
                                                }
                                                DiffSegment::Removed(_) => {
                                                    ui.label(
                                                        egui::RichText::new(text)
                                                            .size(11.5)
                                                            .strikethrough()
                                                            .color(pal.red),
                                                    );
                                                }
                                                DiffSegment::Added(_) => {
                                                    ui.label(
                                                        egui::RichText::new(text)
                                                            .size(11.5)
                                                            .strong()
                                                            .color(pal.green),
                                                    );
                                                }
                                            }
                                        }
                                    });
                                    ui.add_space(4.0);
                                    ui.horizontal(|ui| {
                                        if ui.button("Принять").clicked() {
                                            accept = Some(edit.id.clone());
                                        }
                                        if ui.button("Отклонить").clicked() {
                                            reject = Some(edit.id.clone());
                                        }
                                    });
                                });
                            ui.add_space(4.0);
                        }
                    });
                // История последних решений (accepted/rejected) — кратко.
                let history = if self.suggestions_demo.is_some() {
                    Vec::new()
                } else {
                    let mut history = Suggestions::new(&self.project.root)
                        .list(None, None)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|edit| edit.status != STATUS_PENDING)
                        .collect::<Vec<_>>();
                    history.truncate(5);
                    history
                };
                if !history.is_empty() {
                    ui.separator();
                    egui::CollapsingHeader::new(
                        egui::RichText::new(format!("История решений ({})", history.len()))
                            .small()
                            .color(pal.muted),
                    )
                    .id_salt("rde_suggestions_history")
                    .show(ui, |ui| {
                        for edit in &history {
                            let mark = if edit.status == rde_doc::STATUS_ACCEPTED { "✔" } else { "✖" };
                            ui.label(
                                egui::RichText::new(format!(
                                    "{mark} {}/{} · {} · {}",
                                    edit.doc_id,
                                    edit.block_id,
                                    edit.resolution.as_deref().unwrap_or(&edit.status),
                                    edit.author
                                ))
                                .small()
                                .color(pal.muted),
                            );
                        }
                    });
                }
            });
        if let Some(suggestion_id) = accept {
            match self.suggestion_accept(&suggestion_id) {
                Ok(message) => self.push_log(Level::Ok, message),
                Err(error) => self.push_log(Level::Warn, format!("Принять правку: {error}")),
            }
        }
        if let Some(suggestion_id) = reject {
            match self.suggestion_reject(&suggestion_id, "пользователь отклонил") {
                Ok(message) => self.push_log(Level::Info, message),
                Err(error) => self.push_log(Level::Warn, format!("Отклонить правку: {error}")),
            }
        }
        if reject_all && self.suggestion_reject_all("пользователь отклонил все").is_err() {
            self.push_log(Level::Warn, "Отклонить все правки не удалось".to_owned());
        }
        if refresh {
            let count = self.refresh_suggestions();
            self.push_log(Level::Info, format!("Правки агента обновлены: {count}"));
        }
        if close {
            self.show_suggestions = false;
        }
    }

    /// Демо/скриншоты: окно правок с фейковыми предложениями (без записи в проект).
    pub fn demo_open_suggestions(&mut self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);
        let demo = vec![
            SuggestedEdit {
                id: "01JDEMOSUG1".to_owned(),
                doc_id: "01JDOCDEMO".to_owned(),
                block_id: "b7".to_owned(),
                para_id: Some("1A2B3C4D".to_owned()),
                before: "В разделе 2.3 описаны основные подходы к автоматизации отчётности.".to_owned(),
                after: "В разделе 2.3 описаны основные подходы к автоматизации отчётности и их \
                        применимость к задачам НИР [1]."
                    .to_owned(),
                author: "deepseek-v4-flash".to_owned(),
                author_kind: "agent".to_owned(),
                created_unix: now,
                status: STATUS_PENDING.to_owned(),
                base_body_hash: "demo-body-hash".to_owned(),
                note: Some("Добавлена ссылка на источник [1]".to_owned()),
                resolved_unix: None,
                resolution: None,
            },
            SuggestedEdit {
                id: "01JDEMOSUG2".to_owned(),
                doc_id: "01JDOCDEMO".to_owned(),
                block_id: "b12".to_owned(),
                para_id: Some("5E6F7A8B".to_owned()),
                before: "Экспериментальная часть содержит результаты расчётов.".to_owned(),
                after: "Экспериментальная часть содержит результаты расчётов и их интерпретацию.".to_owned(),
                author: "deepseek-v4-flash".to_owned(),
                author_kind: "agent".to_owned(),
                created_unix: now,
                status: STATUS_PENDING.to_owned(),
                base_body_hash: "demo-body-hash".to_owned(),
                note: None,
                resolved_unix: None,
                resolution: None,
            },
        ];
        self.suggestions_demo = Some(demo);
        self.show_suggestions = true;
        self.suggestions_cache = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_diff_marks_insertions_and_deletions() {
        let segments = word_diff("перепиши абзац", "перепиши этот абзац");
        let (added, removed) = diff_stats(&segments);
        assert_eq!((added, removed), (1, 0));
        assert!(segments.iter().any(|segment| matches!(segment, DiffSegment::Added(text) if text == "этот")));
        let segments = word_diff("старый текст", "новый текст");
        let (added, removed) = diff_stats(&segments);
        assert_eq!((added, removed), (1, 1));
        assert!(segments.iter().any(|segment| matches!(segment, DiffSegment::Removed(text) if text == "старый")));
        assert!(segments.iter().any(|segment| matches!(segment, DiffSegment::Added(text) if text == "новый")));
        assert_eq!(word_diff("тот же текст", "тот же текст"), vec![DiffSegment::Same("тот же текст".to_owned())]);
        assert!(word_diff("", "").is_empty());
    }

    #[test]
    fn word_diff_handles_long_text_and_empty_side() {
        let long_before = (0..800).map(|index| format!("слово{index}")).collect::<Vec<_>>().join(" ");
        let long_after = format!("{long_before} добавка");
        let segments = word_diff(&long_before, &long_after);
        assert!(!segments.is_empty());
        assert!(matches!(segments.last(), Some(DiffSegment::Added(text)) if text == "добавка"));
        let segments = word_diff("", "новый");
        assert_eq!(segments, vec![DiffSegment::Added("новый".to_owned())]);
        let segments = word_diff("старый", "");
        assert_eq!(segments, vec![DiffSegment::Removed("старый".to_owned())]);
    }

    use std::path::PathBuf;

    /// Копия демо-проекта во временный каталог (тесты не пишут в репозиторий).
    fn temp_project(name: &str) -> PathBuf {
        fn copy(src: &std::path::Path, dst: &std::path::Path) {
            std::fs::create_dir_all(dst).unwrap();
            for entry in std::fs::read_dir(src).unwrap().flatten() {
                let from = entry.path();
                let to = dst.join(entry.file_name());
                if from.is_dir() {
                    copy(&from, &to);
                } else {
                    std::fs::copy(&from, &to).unwrap();
                }
            }
        }
        let root = std::env::temp_dir().join(format!("rde-ui-suggestions-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        copy(&rde_core::demo_project_dir(), &root);
        // Скопированная блокировка store принадлежит другому процессу/запуску — снимаем.
        let _ = std::fs::remove_file(root.join(".rde/lock"));
        root
    }

    fn app_with_project(name: &str) -> (RdeApp, PathBuf) {
        let root = temp_project(name);
        let project = rde_core::load_project(&root).expect("проект");
        let mut app = RdeApp::new(project);
        app.app_settings.allow_external_mcp = false;
        (app, root)
    }

    /// Создать реальную pending-правку над первым текстовым блоком демо-документа.
    fn create_pending(
        app: &RdeApp,
        root: &std::path::Path,
        suffix: &str,
        before_override: Option<String>,
    ) -> (String, String, String) {
        let package = app.project.document_path().expect("документ проекта");
        let blocks = rde_doc::document_blocks(&package).expect("блоки");
        let block = blocks
            .iter()
            .find(|block| !block.text.trim().is_empty())
            .expect("текстовый блок")
            .clone();
        let info = rde_doc::package_info(&package).expect("пакет");
        let before = before_override.unwrap_or_else(|| block.text.clone());
        let edit = rde_doc::Suggestions::new(root)
            .create(rde_doc::NewSuggestion {
                doc_id: app.project.document_object().expect("документ").id.clone(),
                block_id: block.id.clone(),
                para_id: block.para_id.clone(),
                before,
                after: format!("{} [правка {suffix}]", block.text.trim_end()),
                author: "deepseek-v4-flash".to_owned(),
                author_kind: "agent".to_owned(),
                base_body_hash: info.body_hash,
                note: Some("M4.4 diff-UI".to_owned()),
            })
            .expect("создать правку");
        (edit.id, block.id, block.text)
    }

    #[test]
    fn accept_applies_edit_with_checkpoint_revision_and_audit() {
        let (mut app, root) = app_with_project("accept");
        let (id, block_id, before) = create_pending(&app, &root, "accept", None);
        let message = app.suggestion_accept(&id).expect("принять правку");
        assert!(message.contains("Правка принята"), "{message}");
        // 1) Changeset применён: текст блока обновлён, правка — accepted.
        let package = app.project.document_path().expect("документ");
        let blocks = rde_doc::document_blocks(&package).expect("блоки");
        let updated = blocks.iter().find(|block| block.id == block_id).expect("блок");
        assert!(updated.text.contains("[правка accept]"), "новый текст: {}", updated.text);
        assert_ne!(updated.text, before);
        let loaded = rde_doc::Suggestions::new(&root).load(&id).expect("правка");
        assert_eq!(loaded.status, rde_doc::STATUS_ACCEPTED);
        // 2) Чекпоинт M2-схемы с tool=suggestion_accept (байт-в-байт пакет).
        let manifests = rde_host::CheckpointStore::new(&root).list().expect("чекпоинты");
        assert!(
            manifests.iter().any(|manifest| manifest.tool == "suggestion_accept"),
            "чекпоинт suggestion_accept: {:?}",
            manifests.iter().map(|manifest| &manifest.tool).collect::<Vec<_>>()
        );
        // 3) Версия документа в store (история/откат).
        let store = app.store.as_ref().unwrap_or_else(|| {
            panic!(
                "store: {:?}",
                app.project.log.iter().map(|line| format!("{:?}: {}", line.level, line.text)).collect::<Vec<_>>()
            )
        });
        let txs = store.list_txs().expect("транзакции");
        assert!(
            txs.iter().any(|tx| tx.note.contains(&format!("suggestion accept {id}"))),
            "транзакция версии документа"
        );
        // 4) Аудит: allow + решение привязано к чекпоинту.
        let audit = std::fs::read_to_string(root.join(".rde/agent/audit.log")).expect("аудит");
        let allow = audit
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .find(|entry| entry["tool"] == "suggestion_accept" && entry["decision"] == "allow")
            .expect("запись аудита allow");
        assert!(allow["checkpoint_id"].as_str().is_some(), "audit.checkpoint_id: {allow}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stale_accept_rolls_back_and_keeps_suggestion_pending() {
        let (mut app, root) = app_with_project("stale");
        // before не совпадает с текущим текстом блока → changeset отклоняется проверкой
        // R-DOC-13; принимающий обязан откатить чекпоинт и не оставить документ изменённым.
        let (id, block_id, before) = create_pending(&app, &root, "stale", Some("неверный текст блока".to_owned()));
        let error = app.suggestion_accept(&id).expect_err("устаревшая правка не применяется");
        assert!(error.contains("не применена") || error.contains("откат"), "{error}");
        let package = app.project.document_path().expect("документ");
        let blocks = rde_doc::document_blocks(&package).expect("блоки");
        let current = blocks.iter().find(|block| block.id == block_id).expect("блок");
        assert_eq!(current.text, before, "документ не изменён после отката");
        let loaded = rde_doc::Suggestions::new(&root).load(&id).expect("правка");
        assert_eq!(loaded.status, rde_doc::STATUS_PENDING, "правка остаётся на ревью");
        let manifests = rde_host::CheckpointStore::new(&root).list().expect("чекпоинты");
        let checkpoint = manifests.iter().find(|manifest| manifest.tool == "suggestion_accept").expect("чекпоинт");
        assert!(checkpoint.rolled_back, "чекпоинт помечен rolled_back");
        // Аудит фиксирует отказ применения (deny/apply_error), а не allow.
        let audit = std::fs::read_to_string(root.join(".rde/agent/audit.log")).expect("аудит");
        assert!(
            audit.lines().any(|line| line.contains("suggestion_accept") && line.contains("apply_error")),
            "аудит отказа применения: {audit}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reject_and_reject_all_are_audited() {
        let (mut app, root) = app_with_project("reject");
        let (first, _, _) = create_pending(&app, &root, "reject-1", None);
        let (second, _, _) = create_pending(&app, &root, "reject-2", None);
        app.suggestion_reject(&first, "не подходит по смыслу").expect("отклонить");
        let loaded = rde_doc::Suggestions::new(&root).load(&first).expect("правка");
        assert_eq!(loaded.status, rde_doc::STATUS_REJECTED);
        assert!(loaded.resolution.as_deref().unwrap_or_default().contains("не подходит"));
        let count = app.suggestion_reject_all("пользователь отклонил все").expect("отклонить все");
        assert_eq!(count, 1, "вторая правка отклонена оптом");
        let pending = rde_doc::Suggestions::new(&root)
            .list(None, Some(rde_doc::STATUS_PENDING))
            .expect("pending");
        assert!(pending.is_empty(), "pending-правок не осталось: {pending:?}");
        let loaded = rde_doc::Suggestions::new(&root).load(&second).expect("правка");
        assert_eq!(loaded.status, rde_doc::STATUS_REJECTED);
        let audit = std::fs::read_to_string(root.join(".rde/agent/audit.log")).expect("аудит");
        let denials = audit
            .lines()
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter(|entry| entry["tool"] == "suggestion_reject" && entry["decision"] == "deny")
            .count();
        assert_eq!(denials, 2, "оба отклонения — в аудите deny");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn suggestions_window_renders_diff_and_accepts() {
        let (mut app, root) = app_with_project("window");
        app.demo_open_suggestions();
        let mut harness = egui_kittest::Harness::builder()
            .with_size(egui::vec2(1600.0, 1000.0))
            .with_max_steps(200_000)
            .build_ui_state(|ui, app: &mut RdeApp| app.ui(ui), app);
        for _ in 0..4 {
            harness.step();
        }
        use egui_kittest::kittest::Queryable;
        assert!(
            harness.query_all_by_label_contains("Ожидают решения: 2").next().is_some(),
            "окно правок с pending-списком"
        );
        assert!(
            harness.query_all_by_label_contains("новый текст").next().is_some()
                || harness.query_all_by_label_contains("Добавлена ссылка").next().is_some(),
            "подсказка/заметка правки видна"
        );
        assert!(
            harness.query_all_by_label_contains("слов").next().is_some(),
            "статистика word-diff (+N/−M слов)"
        );
        // В списке две pending-правки — у каждой своя кнопка «Принять»; берём первую.
        let accept = harness.get_all_by_label("Принять").next().expect("кнопка «Принять»");
        accept.click();
        harness.run_steps(2);
        assert_eq!(
            harness.state().suggestions_demo.as_ref().map(Vec::len),
            Some(1),
            "после «Принять» остаётся одна pending-правка"
        );
        assert!(
            harness
                .query_all_by_label_contains("Правки агента: ожидают решения — 1")
                .next()
                .is_some()
                || harness.query_all_by_label_contains("Ожидают решения: 1").next().is_some(),
            "счётчик обновился"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
