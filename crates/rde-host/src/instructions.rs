//! Построение MCP-инструкций: карта проекта + политика безопасности + правила чтения.
//!
//! Требования: ≤32 KiB (лимит DSH, S16), **детерминированный** текст при том же проекте
//! (никакого времени/случайности — prompt-cache, R-AGT-08), стабильные ID и группы объектов.
//! Финальное усечение до 32 KiB делает протокольный слой (`rde_mcp::instructions::clamp`);
//! здесь текст тоже укладывается в лимит, чтобы хост не отдавал заведомо непригодное.

use rde_core::{Object, ObjectData, ObjectKind, Project};

/// Сколько объектов перечислять в карте проекта (остальные — счётчиком, чтобы не раздувать
/// контекст; детали доступны через `project_search`/`project_tree`).
const MAX_LISTED_OBJECTS: usize = 60;

/// Собрать текст MCP-инструкций для проекта.
pub fn build(project: &Project) -> String {
    let policy = rde_mcp::McpPolicy::read_only();
    let mut out = String::with_capacity(4096);
    out.push_str("# RDE MCP server (M2.5: read-only + управляемые мутации; транспорт stdio/HTTP)\n\n");
    out.push_str("RDE — среда разработки отчёта. Проект — каталог с TOML-манифестами объектов; ");
    out.push_str("документ — `.doc.rde` (OOXML + `rde:doc`), артефакты — скрипты с хешами/CAS/ledger.\n\n");

    out.push_str("## Проект\n");
    out.push_str(&format!("- имя: {}\n", project.name));
    out.push_str(&format!("- id: {}\n", project.manifest.id));
    out.push_str(&format!("- схема: {}\n", project.manifest.schema));
    out.push_str(&format!(
        "- объектов: {} (документы {}, артефакты {}, источники {}, заметки {}, репозитории {}, формулы {})\n",
        project.object_count(),
        count_kind(project, ObjectKind::Document),
        count_kind(project, ObjectKind::Artifact),
        count_kind(project, ObjectKind::Source),
        count_kind(project, ObjectKind::Note),
        count_kind(project, ObjectKind::Repository),
        count_kind(project, ObjectKind::Formula),
    ));
    out.push_str(&format!("- устаревших/с ошибками: {}\n", project.stale_count()));
    out.push_str(&format!("- доверенный проект: {}\n", if project.manifest.trusted { "да" } else { "нет" }));
    out.push_str(&format!("- шаблон по умолчанию: {}\n\n", project.manifest.default_template));

    out.push_str("## Политика безопасности (fail-closed)\n");
    out.push_str("- read-only инструменты разрешены; мутации — только с грантом политики и approval (см. раздел ниже);\n");
    out.push_str(&format!(
        "- вызовы аудируются (`.rde/agent/audit.log`: решение, причина, `checkpoint_id`; аргументы — SHA-256); rate-limit {} вызовов/мин;\n",
        policy.rate_limit_per_min
    ));
    out.push_str(&format!(
        "- лимиты ответа: ≤{} KiB на вызов, таймаут инструмента {} с;\n",
        policy.max_response_bytes / 1024,
        policy.tool_timeout_ms / 1000
    ));
    out.push_str("- пути — только внутри корня проекта; `.rde/**` закрыт; ошибки — машиночитаемые коды `RDE_*`;\n");
    out.push_str(&format!("- инструкции ≤{} KiB (эти).\n\n", rde_mcp::MAX_INSTRUCTIONS_BYTES / 1024));

    out.push_str("## Мутации (M2/M2.5: grants + approvals + checkpoints)\n");
    out.push_str("- без гранта CLI (`--approval=deny` по умолчанию) мутация возвращает `RDE_APPROVAL_DENIED` и НЕ меняет файлы;\n");
    out.push_str("- перед каждой мутацией создаётся чекпоинт `.rde/agent/checkpoints/<id>/` (копии файлов + корни store); при ошибке — авто-откат;\n");
    out.push_str("- `checkpoint_id` возвращается в ответе и пишется в аудит; ручной откат — `RDE`/UI (MCP `checkpoint_*` в M2/M2.5 не публикуются);\n");
    out.push_str("- `expected_sha256`/`expected_text`/`expected_body_hash` защищают от перезаписи ручной правки: расхождение → `RDE_CONFLICT`;\n");
    out.push_str("- `dry_run: true` — только проверка, файлы не пишутся; `artifact_run` требует `trusted`-проект (R-RUN-06), один job за раз (`RDE_BUSY`);\n");
    out.push_str("- jobs (`artifact_run` → `run_status`/`run_log`/`run_cancel`) живут в процессе сервера и не переживают рестарт;\n");
    out.push_str("- общего `file_write` и произвольного shell нет.\n\n");

    out.push_str("## Документы (M2.5/M2.6: suggested edits, правки блоков, ревизии, R-DOC-32/R-DOC-41)\n");
    out.push_str("- правка текста документа — только по блокам (`block_id` из `doc_outline`) и только инструментами:\n");
    out.push_str("  `doc_suggest_edit` создаёт предложение (документ НЕ меняется) → `suggestion_list` → пользователь или\n");
    out.push_str("  `suggestion_accept`/`suggestion_reject`; `doc_replace_range` — прямая правка с approval (destructive), без ревью;\n");
    out.push_str("- `expected_text` обязателен и должен совпадать с текущим текстом блока (`doc_read`), иначе `RDE_CONFLICT`;\n");
    out.push_str("  `expected_body_hash` (из `doc_list`) — дополнительная защита от внешних правок (R-DOC-13: не перезаписываем молча);\n");
    out.push_str("- каждая правка: чекпоинт `.doc.rde` + версия документа в store (ревизия); `revision_list` — история версий,\n");
    out.push_str("  откат ревизии — операция RDE (API хоста/UI), MCP-инструмент `revision_restore` в M2.5 не публикуется;\n");
    out.push_str("- правятся только простые абзацы: без вставок RDE (`w:sdt`), полей, рисунков, формул, track changes;\n");
    out.push_str("  один абзац без переводов строк;\n");
    out.push_str("- `doc_insert` (M2.6) — вставить новый абзац (якорь/конец, style); `doc_delete` (M2.6) — удалить\n");
    out.push_str("  абзац по `block_id`+`expected_text`; оба — approval+чекпоинт+ревизия; id блоков `bN` после места\n");
    out.push_str("  изменения сдвигаются; последний блок удалить нельзя; вставки/поля/рисунки/формулы не правятся;\n");
    out.push_str("- документ, изменённый вне RDE (`body_hash` не совпадает с `rde:doc`), не правится до перечитывания/восстановления.\n\n");

    out.push_str("## Объекты проекта\n");
    let listed: Vec<&Object> = project.objects.iter().take(MAX_LISTED_OBJECTS).collect();
    for object in &listed {
        out.push_str(&format!("- {}: `{}` — {}{}\n", kind_str(object.kind), object.id, object.title, object_hint(object)));
    }
    if project.objects.len() > listed.len() {
        out.push_str(&format!(
            "- … ещё {} объектов (используйте project_tree/project_search)\n",
            project.objects.len() - listed.len()
        ));
    }
    out.push('\n');

    out.push_str("## Инструменты (29: 17 read-only + 12 мутаций)\n");
    let names: Vec<&str> = rde_mcp::tool_registry().iter().map(|tool| tool.name).collect();
    out.push_str(&format!("{}\n\n", names.join(", ")));

    out.push_str("## Ресурсы\n");
    out.push_str("- `rde://project/tree` — дерево проекта;\n");
    out.push_str("- `rde://doc/<doc_id>/outline` — оглавление документа;\n");
    out.push_str("- `rde://artifact/<artifact_id>/meta` — манифест + ledger-метаданные артефакта;\n");
    out.push_str("- `rde://templates` — каталог шаблонов.\n\n");

    out.push_str("## Правила чтения (экономия контекста)\n");
    out.push_str("- тела документов читайте `doc_outline` → `doc_read` по `block_ids`/диапазону;\n");
    out.push_str("- файлы — `file_read` с `range` (байты или строки), большие тела — частями;\n");
    out.push_str("- ответы помечают `truncated` при упоре в лимит — запрашивайте следующий диапазон;\n");
    out.push_str("- используйте стабильные id из ответов (`doc_list`/`artifact_list`/…), не придумывайте пути;\n");
    out.push_str("- `doc_*` работают только с `.doc.rde`; на непреобразованном документе — `RDE_DOC_NOT_MIGRATED`;\n");
    out.push_str("- git — `git_status` (чтение) и `git_commit` (мутация с approval); ledger-статусы артефактов не означают запуск.\n");

    rde_mcp::instructions::clamp(&out)
}

fn count_kind(project: &Project, kind: ObjectKind) -> usize {
    project.objects.iter().filter(|object| object.kind == kind).count()
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

/// Короткая подсказка формата объекта для карты.
fn object_hint(object: &Object) -> String {
    match &object.data {
        ObjectData::Document(document) => format!(" (`{}`)", document.file),
        ObjectData::Artifact(artifact) => {
            format!(" ({} · выходов: {})", artifact.kind, artifact.outputs.len())
        }
        ObjectData::Formula(formula) => {
            let display = if formula.display { "display" } else { "inline" };
            format!(" (LaTeX, {display})")
        }
        ObjectData::Note(note) => format!(" (`{}`)", note.file),
        ObjectData::Source(source) => {
            if let Some(file) = &source.file {
                format!(" ({}, `{}`)", source.kind, file)
            } else if let Some(url) = &source.url {
                format!(" ({}, {})", source.kind, url)
            } else {
                format!(" ({})", source.kind)
            }
        }
        ObjectData::Repository(repository) => format!(" ({}, `{}`)", repository.kind, repository.path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instructions_fit_limit_and_contain_explicit_contract() {
        let project = rde_core::demo();
        let text = build(&project);
        assert!(text.len() <= rde_mcp::MAX_INSTRUCTIONS_BYTES, "{} байт", text.len());
        assert!(text.contains(&project.name));
        assert!(text.contains(&project.manifest.id));
        assert!(text.contains("read-only"));
        assert!(text.contains("rde://project/tree"));
        for tool in rde_mcp::tool_registry() {
            assert!(text.contains(tool.name), "в инструкциях нет инструмента {}", tool.name);
        }
    }

    #[test]
    fn instructions_are_deterministic() {
        let project = rde_core::demo();
        assert_eq!(build(&project), build(&project));
    }
}
