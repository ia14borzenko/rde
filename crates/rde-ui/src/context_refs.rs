//! M4.4: контекст-ссылки RDE (§8.7.2 ТЗ, R-AGT-03/08) — разрешение в тело, чипы «В чат»
//! и передача `resource_link`-блоков встроенному ACP-агенту.
//!
//! URI-схема разбирается в `rde_core::context_ref` (без зависимостей); здесь — **разрешение**
//! ссылки в контексте открытого проекта: текст блока/абзаца/комментария, страница или регион
//! PDF-источника (bbox в точках PDF), выход артефакта, заметка, метаданные проекта/шаблона.
//!
//! Принципы (ТЗ §8.7.2):
//! - в чат вставляется **чип** (идентичность + подпись), тело в промпт не копируется;
//! - агенту уходит `resource_link` (URI + имя + MIME + подсказка «как прочитать»), а тело он
//!   читает лениво через RDE-MCP (`doc_read`, `artifact_read`, `file_read`; R-AGT-08);
//! - исключение M4.4 (честно, см. ADR-0015): PDF-источник/заметка — RDE-MCP **пока не умеет**
//!   читать PDF-страницы (`pdf_*` из каталога §8.7.4 не реализованы) и `note_read` отсутствует,
//!   поэтому для них короткий сниппет текста передаётся в `description` ссылки сразу;
//! - revision-scoped ссылки (`@rev`) при устаревании помечаются «ссылка обновлена»
//!   (re-anchor по `paraId`/`block_id`).

use std::path::{Path, PathBuf};

use rde_core::context_ref::{BBox, ContextRef};
use rde_core::{Object, ObjectData, Project};
use rde_core::Level;

use crate::RdeApp;
use crate::chat::ChatRef;

/// Максимум символов сниппета, добавляемого в подсказку ссылки (описание resource_link).
const EXCERPT_CHARS: usize = 1200;
/// Максимум прочитанного текста файла для сниппета (байты; больше — только метаданные).
const MAX_TEXT_BYTES: u64 = 256 * 1024;

/// URI блока документа для кнопки «В чат» из таблицы структуры.
///
/// `number` — номер блока в `rde_doc::document_blocks` (`bN`), т.е. `b{number}` по тем же
/// `para_paths`, что и структура (пустые блоки без изображений пропускаются одинаково).
pub fn block_uri_for(document_id: &str, number: usize) -> String {
    ContextRef::DocBlock {
        doc_id: document_id.to_owned(),
        rev: None,
        block_id: format!("b{number}"),
    }
    .to_uri()
}

/// Разрешённая контекст-ссылка: чип + подсказка для агента.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRef {
    /// Канонический URI.
    pub uri: String,
    /// Подпись чипа/`resource_link.name`.
    pub label: String,
    /// Тип по-русски (`блок документа`, `источник`, …).
    pub kind: String,
    /// MIME-тип тела (если применим).
    pub mime_type: Option<String>,
    /// Подсказка агенту, как читать тело (MCP-инструмент/параметры); для PDF/заметок — сниппет.
    pub description: Option<String>,
    /// Сниппет тела для preview/`description`.
    pub excerpt: String,
    /// Revision-scoped ссылка устарела (документ/выход/файл изменился) → re-anchor.
    pub updated: bool,
}

impl ResolvedRef {
    /// Вложение для промпта ACP-агента (M4.4): `resource_link` без тела.
    pub fn into_prompt_attachment(self) -> rde_agent::PromptAttachment {
        rde_agent::PromptAttachment {
            uri: self.uri,
            name: self.label,
            mime_type: self.mime_type,
            description: self.description,
        }
    }

    /// Чип для сообщения в чате.
    pub fn into_chat_ref(self) -> ChatRef {
        ChatRef { uri: self.uri, label: self.label }
    }
}

/// Разрешить `rde://…` в контексте проекта.
///
/// # Errors
///
/// Ссылка не разбирается/указывает на несуществующий объект/блок — `Err(причина)`.
pub fn resolve_ref(project: &Project, uri: &str) -> Result<ResolvedRef, String> {
    let parsed = ContextRef::parse(uri).map_err(|error| error.to_string())?;
    // Канонический URI считается до разбора на поля (частичное перемещение `parsed` ниже).
    let canonical = parsed.to_uri();
    match parsed {
        ContextRef::Project { project_id } => {
            if project.manifest.id != project_id {
                return Err(format!(
                    "проект `{project_id}` не открыт (открыт `{}`)",
                    project.manifest.id
                ));
            }
            Ok(ResolvedRef {
                uri: canonical.clone(),
                label: format!("проект «{}»", project.manifest.name),
                kind: "проект".to_owned(),
                mime_type: None,
                description: Some(format!(
                    "RDE-контекст: проект `{}`; объекты/файлы — MCP `project_tree`, `file_read`",
                    project.manifest.id
                )),
                excerpt: format!(
                    "{} · {} объектов · корень {}",
                    project.manifest.name,
                    project.objects.len(),
                    project.root.display()
                ),
                updated: false,
            })
        }
        ContextRef::DocBlock { doc_id, rev, block_id } => {
            let (object, package) = document_package(project, &doc_id)?;
            let blocks = rde_doc::document_blocks(&package)
                .map_err(|error| format!("прочитать блоки документа: {error}"))?;
            let block = blocks
                .iter()
                .find(|block| block.id == block_id)
                .ok_or_else(|| format!("блок `{block_id}` не найден в документе `{doc_id}`"))?;
            let body_hash = rde_doc::package_info(&package).ok().map(|info| info.body_hash);
            let updated = rev.as_deref().is_some_and(|rev| body_hash.as_deref() != Some(rev));
            let text = block_text_or_marker(&block.text, &block.kind);
            Ok(ResolvedRef {
                uri: canonical.clone(),
                label: format!("doc {}: блок {} ({})", object.id, block.id, block.kind),
                kind: "блок документа".to_owned(),
                mime_type: Some("text/plain".to_owned()),
                description: Some(format!(
                    "RDE-контекст: документ `{}`, блок `{}`{}; тело — MCP `doc_read` {{doc_id: \"{}\", block_id: \"{}\"}}",
                    object.id,
                    block.id,
                    if updated { " (ссылка обновлена: re-anchor по block_id)" } else { "" },
                    object.id,
                    block.id
                )),
                excerpt: truncate(&text, EXCERPT_CHARS),
                updated,
            })
        }
        ContextRef::DocPara { doc_id, rev, para_id } => {
            let (object, package) = document_package(project, &doc_id)?;
            let blocks = rde_doc::document_blocks(&package)
                .map_err(|error| format!("прочитать блоки документа: {error}"))?;
            let block = blocks
                .iter()
                .find(|block| block.para_id.as_deref() == Some(para_id.as_str()))
                .ok_or_else(|| {
                    format!("абзац paraId=`{para_id}` не найден (re-anchor по paraId не удался)")
                })?;
            let body_hash = rde_doc::package_info(&package).ok().map(|info| info.body_hash);
            let updated = rev.as_deref().is_some_and(|rev| body_hash.as_deref() != Some(rev));
            let text = block_text_or_marker(&block.text, &block.kind);
            Ok(ResolvedRef {
                uri: canonical.clone(),
                label: format!("doc {}: абзац {} → {}", object.id, para_id, block.id),
                kind: "абзац документа".to_owned(),
                mime_type: Some("text/plain".to_owned()),
                description: Some(format!(
                    "RDE-контекст: документ `{}`, абзац paraId=`{}` → блок `{}`{}; тело — MCP `doc_read` {{doc_id: \"{}\", block_id: \"{}\"}}",
                    object.id,
                    para_id,
                    block.id,
                    if updated { " (ссылка обновлена: re-anchor по paraId)" } else { "" },
                    object.id,
                    block.id
                )),
                excerpt: truncate(&text, EXCERPT_CHARS),
                updated,
            })
        }
        ContextRef::DocComment { doc_id, comment_id, .. } => {
            let (object, package) = document_package(project, &doc_id)?;
            let comments = rde_doc::comments(&package)
                .map_err(|error| format!("прочитать комментарии документа: {error}"))?;
            // Word-комментарии адресуются числовым w:id (1-based); ULID импорта ревью пока не
            // сохраняется в CommentInfo — поддержан числовой id и «cn»-префикс.
            let index = comment_id
                .strip_prefix("cn")
                .unwrap_or(&comment_id)
                .parse::<usize>()
                .map_err(|_| {
                    format!(
                        "комментарий `{comment_id}`: Word-комментарии адресуются числовым w:id (1-based)"
                    )
                })?;
            let comment = comments
                .get(index.saturating_sub(1))
                .ok_or_else(|| format!("комментарий `{comment_id}` не найден (всего {})", comments.len()))?;
            Ok(ResolvedRef {
                uri: canonical.clone(),
                label: format!("комментарий {}: {}", comment_id, truncate(&comment.text, 48)),
                kind: "комментарий".to_owned(),
                mime_type: Some("text/plain".to_owned()),
                description: Some(format!(
                    "RDE-контекст: документ `{}`, комментарий w:id={} от «{}»{}; тело — сниппет ниже",
                    object.id,
                    index,
                    comment.author,
                    if comment.resolved { " (решён)" } else { "" }
                )),
                excerpt: truncate(&comment.text, EXCERPT_CHARS),
                updated: false,
            })
        }
        ContextRef::Source { source_id, content_hash, page, bbox, page_size } => {
            resolve_source(project, canonical.clone(), &source_id, content_hash.as_deref(), page, bbox, page_size)
        }
        ContextRef::Artifact { artifact_id, out_hash, relpath } => {
            resolve_artifact(project, canonical.clone(), &artifact_id, out_hash.as_deref(), &relpath)
        }
        ContextRef::Note { note_id } => resolve_note(project, canonical.clone(), &note_id),
        ContextRef::Template { pack_id, version } => {
            let template = rde_templates::builtin_templates()
                .iter()
                .find(|template| template.id == pack_id || template.id.ends_with(&pack_id))
                .ok_or_else(|| format!("шаблон `{pack_id}` не найден в rde-templates"))?;
            // Встроенные шаблоны v1 не имеют отдельного поля версии (schema = `rde/template@1`):
            // явная версия в ссылке не верифицируется — честно помечаем `updated`.
            let unknown_version = version.is_some();
            Ok(ResolvedRef {
                uri: canonical.clone(),
                label: format!("шаблон «{}»", template.title),
                kind: "шаблон".to_owned(),
                mime_type: Some("text/toml".to_owned()),
                description: Some(format!(
                    "RDE-контекст: встроенный шаблон `{}` ({}); MCP: `template_list`{}",
                    template.id,
                    template.title,
                    if let Some(version) = version.as_deref() {
                        format!("; запрошена версия `{version}` — в модели шаблона v1 версия не зафиксирована (ссылка обновлена)")
                    } else {
                        String::new()
                    }
                )),
                excerpt: format!("{} · {}", template.title, template.description),
                updated: unknown_version,
            })
        }
    }
}

fn document_package(project: &Project, doc_id: &str) -> Result<(Object, PathBuf), String> {
    let object = project
        .document_object()
        .ok_or_else(|| "в проекте нет документа".to_owned())?;
    if object.id != doc_id {
        return Err(format!(
            "документ `{doc_id}` не найден (в проекте документ `{}`)",
            object.id
        ));
    }
    let package = project
        .document_path()
        .ok_or_else(|| "в проекте нет файла документа".to_owned())?;
    Ok((object.clone(), package))
}

fn resolve_source(
    project: &Project,
    uri: String,
    source_id: &str,
    content_hash: Option<&str>,
    page: Option<u32>,
    bbox: Option<BBox>,
    page_size: Option<(f32, f32)>,
) -> Result<ResolvedRef, String> {
    let object = project
        .sources()
        .find(|object| object.id == source_id)
        .ok_or_else(|| format!("источник `{source_id}` не найден"))?;
    let ObjectData::Source(source) = &object.data else {
        return Err(format!("объект `{source_id}` — не источник"));
    };
    let relative = source
        .file
        .as_ref()
        .map(|file| object.dir.join(file))
        .ok_or_else(|| format!("источник `{source_id}` — URL без файла (тело не читается локально)"))?;
    let exists = relative.is_file();
    let mut updated = false;
    if let (Some(expected), true) = (content_hash, exists) {
        if let Ok(bytes) = std::fs::read(&relative) {
            let actual = rde_hash::sha256(&bytes).hex();
            updated = actual != expected;
        }
    }
    let mime = mime_for_path(&relative);
    let mut description = format!(
        "RDE-контекст: источник `{}` (`{}`)",
        object.id,
        object
            .dir
            .join(&relative)
            .strip_prefix(&project.root)
            .map(|relative| relative.display().to_string())
            .unwrap_or_else(|_| relative.display().to_string())
    );
    let mut excerpt = String::new();
    let is_pdf = relative
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
    if is_pdf {
        if let Some(page) = page {
            let page_text = rde_sources::pdf_page_text(&relative, page)
                .unwrap_or_else(|error| format!("(страница {page}: {error})"));
            excerpt = truncate(&page_text, EXCERPT_CHARS);
            let region = match (bbox, page_size.or_else(|| rde_sources::pdf_page_size(&relative, page).ok())) {
                (Some(bbox), Some(page_size)) => format!(
                    "; регион bbox=({}) pageSize={:.2}×{:.2} пт",
                    bbox.to_query(),
                    page_size.0,
                    page_size.1
                ),
                (Some(bbox), None) => format!("; регион bbox=({})", bbox.to_query()),
                _ => String::new(),
            };
            description.push_str(&format!(
                ", PDF страница {page}{region}; MCP pdf-инструментов нет — сниппет текста страницы: «{}»",
                truncate(&page_text, 800)
            ));
        } else {
            description.push_str("; PDF без страницы (укажите `?page=N`); MCP pdf-инструментов нет");
            excerpt = rde_sources::pdf_text(&relative)
                .map(|text| truncate(&text, EXCERPT_CHARS))
                .unwrap_or_default();
        }
    } else if exists {
        if let Ok(text) = read_text_limited(&relative) {
            excerpt = truncate(&text, EXCERPT_CHARS);
            description.push_str(&format!(
                "; тело — MCP `file_read` {{path: \"{}\"}}",
                relative
                    .strip_prefix(&project.root)
                    .map(|relative| relative.display().to_string())
                    .unwrap_or_else(|_| relative.display().to_string())
            ));
        } else {
            description.push_str("; файл бинарный/большой (только метаданные)");
        }
    } else if source.url.is_some() {
        description.push_str(&format!("; URL: {}", source.url.as_deref().unwrap_or_default()));
        excerpt = source.url.clone().unwrap_or_default();
    }
    let label = match page {
        Some(page) => match bbox {
            Some(_) => format!("источник {}: с. {page}, регион", object.title),
            None => format!("источник {}: с. {page}", object.title),
        },
        None => format!("источник {}", object.title),
    };
    Ok(ResolvedRef {
        uri,
        label,
        kind: "источник".to_owned(),
        mime_type: mime,
        description: Some(description),
        excerpt,
        updated,
    })
}

fn resolve_artifact(
    project: &Project,
    uri: String,
    artifact_id: &str,
    out_hash: Option<&str>,
    relpath: &str,
) -> Result<ResolvedRef, String> {
    let object = project
        .artifact_object(artifact_id)
        .ok_or_else(|| format!("артефакт `{artifact_id}` не найден"))?;
    let ObjectData::Artifact(artifact) = &object.data else {
        return Err(format!("объект `{artifact_id}` — не артефакт"));
    };
    if relpath.contains("..") || relpath.starts_with('/') {
        return Err("relpath выхода должен быть относительным путём без `..`".to_owned());
    }
    let path = object.dir.join(relpath);
    let exists = path.is_file();
    let mut updated = false;
    if let (Some(expected), true) = (out_hash, exists) {
        if let Ok(bytes) = std::fs::read(&path) {
            // out_hash — Merkle-хэш выходов (не sha256 файла); помечаем устаревание, если
            // манифест уже фиксирует другой out_hash.
            if artifact.out_hash.as_deref().is_some_and(|current| current != expected) || bytes.is_empty() {
                updated = true;
            }
        }
    }
    let mut description = format!(
        "RDE-контекст: выход артефакта `{}` → `{}`; MCP `artifact_read` {{artifact_id: \"{}\", path: \"{}\"}}",
        object.id, relpath, object.id, relpath
    );
    let excerpt;
    if exists {
        match read_text_limited(&path) {
            Ok(text) => {
                excerpt = truncate(&text, EXCERPT_CHARS);
            }
            Err(_) => {
                let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
                excerpt = format!("(бинарный файл, {size} байт)");
                description.push_str("; файл бинарный");
            }
        }
    } else {
        let available = artifact.outputs.join(", ");
        return Err(format!(
            "выход `{relpath}` артефакта `{artifact_id}` ещё не создан (выходы: {available})"
        ));
    }
    if updated {
        description.push_str("; ссылка обновлена (out_hash изменился)");
    }
    Ok(ResolvedRef {
        uri,
        label: format!("выход {}: {relpath}", object.id),
        kind: "выход артефакта".to_owned(),
        mime_type: mime_for_path(&path),
        description: Some(description),
        excerpt,
        updated,
    })
}

fn resolve_note(project: &Project, uri: String, note_id: &str) -> Result<ResolvedRef, String> {
    let object = project
        .objects
        .iter()
        .find(|object| object.id == note_id)
        .ok_or_else(|| format!("заметка `{note_id}` не найдена"))?;
    let ObjectData::Note(note) = &object.data else {
        return Err(format!("объект `{note_id}` — не заметка"));
    };
    let path = object.dir.join(&note.file);
    let text = read_text_limited(&path).map_err(|error| format!("заметка `{note_id}`: {error}"))?;
    Ok(ResolvedRef {
        uri,
        label: format!("заметка «{}»", object.title),
        kind: "заметка".to_owned(),
        mime_type: mime_for_path(&path),
        description: Some(format!(
            "RDE-контекст: заметка `{}` (`{}`); MCP `note_read` отсутствует — сниппет текста: «{}»",
            object.id,
            note.file,
            truncate(&text, 800)
        )),
        excerpt: truncate(&text, EXCERPT_CHARS),
        updated: false,
    })
}

/// Текст блока или маркер для нетекстовых блоков (рисунок/формула/таблица).
fn block_text_or_marker(text: &str, kind: &str) -> String {
    if text.trim().is_empty() {
        match kind {
            "figure" => "(блок-рисунок: изображение/вставка RDE)".to_owned(),
            "caption" => "(блок-подпись)".to_owned(),
            other => format!("(блок без текста: {other})"),
        }
    } else {
        text.to_owned()
    }
}

fn read_text_limited(path: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(path).map_err(|error| format!("файл {}: {error}", path.display()))?;
    if metadata.len() > MAX_TEXT_BYTES {
        return Err(format!("файл больше {MAX_TEXT_BYTES} байт"));
    }
    std::fs::read_to_string(path).map_err(|error| format!("прочитать {}: {error}", path.display()))
}

fn truncate(text: &str, limit: usize) -> String {
    let mut result: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        result.push('…');
    }
    result
}

/// MIME по расширению (для `resource_link.mimeType`).
pub fn mime_for_path(path: &Path) -> Option<String> {
    let extension = path.extension().and_then(|ext| ext.to_str())?.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "json" => "application/json",
        "toml" => "text/toml",
        "yaml" | "yml" => "text/yaml",
        "py" => "text/x-python",
        "r" => "text/x-r",
        "sh" => "text/x-shellscript",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        _ => return None,
    };
    Some(mime.to_owned())
}

impl RdeApp {
    /// Кнопка «В чат»: разрешить ссылку и приложить чип к следующему сообщению.
    ///
    /// Побочные эффекты: открывает панель «Агент», пишет строку в журнал; дубликат по URI
    /// не добавляется. `true` — чип добавлен.
    pub(crate) fn attach_context_uri(&mut self, uri: &str) -> bool {
        match resolve_ref(&self.project, uri) {
            Ok(resolved) => {
                let label = resolved.label.clone();
                let updated = resolved.updated;
                if self.chat.attach_ref(resolved.into_chat_ref()) {
                    self.show_agent = true;
                    self.push_log(Level::Ok, format!("В чат: {label} ({uri})"));
                    if updated {
                        self.push_log(
                            Level::Info,
                            "Ссылка приложена к текущей версии (revision-scoped; re-anchor по block_id/paraId)",
                        );
                    }
                    true
                } else {
                    self.push_log(Level::Info, format!("В чат: ссылка уже приложена ({uri})"));
                    false
                }
            }
            Err(error) => {
                self.push_log(Level::Warn, format!("Контекст-ссылка `{uri}`: {error}"));
                false
            }
        }
    }

    /// URI контекст-ссылки для объекта (инспектор «В чат»).
    ///
    /// Документ адресуется ссылкой на проект (блок/абзац выбирается во вкладке «Структура»);
    /// артефакт — первым объявленным выходом (если выходов нет — манифестом `artifact.toml`).
    pub(crate) fn context_uri_for_object(&self, object: &Object) -> Result<String, String> {
        match &object.data {
            ObjectData::Document(_) => Ok(ContextRef::Project { project_id: self.project.manifest.id.clone() }.to_uri()),
            ObjectData::Artifact(artifact) => {
                let relpath = artifact
                    .outputs
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "artifact.toml".to_owned());
                Ok(ContextRef::Artifact {
                    artifact_id: object.id.clone(),
                    out_hash: artifact.out_hash.clone(),
                    relpath,
                }
                .to_uri())
            }
            ObjectData::Source(_) => Ok(ContextRef::Source {
                source_id: object.id.clone(),
                content_hash: None,
                page: None,
                bbox: None,
                page_size: None,
            }
            .to_uri()),
            ObjectData::Note(_) => Ok(ContextRef::Note { note_id: object.id.clone() }.to_uri()),
            ObjectData::Repository(_) | ObjectData::Formula(_) => Err(
                "для репозиториев/формул контекст-ссылки §8.7.2 не определены (используйте проект/блоки)"
                    .to_owned(),
            ),
        }
    }

    /// Собрать `resource_link`-вложения из приложенных чипов (для отправки промпта агенту).
    ///
    /// Неразрешимые ссылки не блокируют отправку: они уходят с URI/подписью, ошибка — в журнал
    /// (fail-open для контекста: промпт важнее подсказки).
    pub(crate) fn prompt_attachments(&mut self) -> Vec<rde_agent::PromptAttachment> {
        let refs = self.chat.attachments.clone();
        let root = self.project.root.clone();
        let _ = root;
        let project = self.project.clone();
        let mut attachments = Vec::with_capacity(refs.len());
        let mut errors = Vec::new();
        for chat_ref in refs {
            match resolve_ref(&project, &chat_ref.uri) {
                Ok(resolved) => attachments.push(resolved.into_prompt_attachment()),
                Err(error) => {
                    errors.push(format!("{}: {error}", chat_ref.uri));
                    attachments.push(rde_agent::PromptAttachment {
                        uri: chat_ref.uri,
                        name: chat_ref.label,
                        mime_type: None,
                        description: Some(format!("RDE-контекст: разрешить ссылку не удалось ({error})")),
                    });
                }
            }
        }
        for error in errors {
            self.push_log(Level::Warn, format!("Вложение промпта: {error}"));
        }
        attachments
    }
}

/// Состояние просмотра страницы PDF с выбором региона (§8.7.2, M4.4).
///
/// `Debug` не выводится: `egui::TextureHandle` не реализует `Debug` (не нужен).
pub struct PdfRegionView {
    /// Ключ кэша (`path#page`) — при смене перерисовываем.
    pub key: String,
    /// Файл PDF.
    pub path: PathBuf,
    /// Номер страницы (с 1).
    pub page: u32,
    /// Текстура страницы (PNG-растр `pdftoppm`).
    pub texture: Option<egui::TextureHandle>,
    /// Размер растра, px.
    pub image_size: egui::Vec2,
    /// Размер страницы PDF, точки.
    pub page_size: (f32, f32),
    /// Текущий выделенный регион в координатах растра, px.
    pub region: Option<egui::Rect>,
    /// Начало drag-выделения (в координатах растра, px).
    drag_anchor: Option<egui::Pos2>,
    /// Требуется отрисовать страницу при следующем кадре.
    pub render_requested: bool,
}

impl Default for PdfRegionView {
    fn default() -> Self {
        Self {
            key: String::new(),
            path: PathBuf::new(),
            page: 1,
            texture: None,
            image_size: egui::Vec2::ZERO,
            page_size: (595.28, 841.89),
            region: None,
            drag_anchor: None,
            render_requested: false,
        }
    }
}

impl PdfRegionView {
    /// Регион → bbox в точках PDF (`page_size.0/1` на ширину/высоту растра).
    pub fn region_bbox(&self) -> Option<BBox> {
        let region = self.region?.intersect(egui::Rect::from_min_size(egui::Pos2::ZERO, self.image_size));
        if region.width() < 4.0 || region.height() < 4.0 {
            return None;
        }
        let kx = self.page_size.0 / self.image_size.x.max(1.0);
        let ky = self.page_size.1 / self.image_size.y.max(1.0);
        Some(BBox::new(
            region.min.x * kx,
            region.min.y * ky,
            region.max.x * kx,
            region.max.y * ky,
        ))
    }

    /// URI контекст-ссылки на выбранный регион страницы источника.
    pub fn region_uri(&self, source_id: &str) -> Option<String> {
        let bbox = self.region_bbox()?;
        Some(
            ContextRef::Source {
                source_id: source_id.to_owned(),
                content_hash: None,
                page: Some(self.page),
                bbox: Some(bbox),
                page_size: Some(self.page_size),
            }
            .to_uri(),
        )
    }
}

impl RdeApp {
    /// Подготовить просмотр страницы PDF источника (кэш `.rde/cache/pdf-pages`).
    ///
    /// Идемпотентно: повторный вызов с тем же `path#page` не перерисовывает.
    pub(crate) fn ensure_pdf_region(&mut self, file: &Path, page: u32) {
        let key = format!("{}#{page}", file.display());
        let needs_new = self
            .pdf_region
            .as_ref()
            .map(|view| view.key != key)
            .unwrap_or(true);
        if needs_new {
            self.pdf_region = Some(PdfRegionView {
                key,
                path: file.to_path_buf(),
                page,
                render_requested: true,
                ..PdfRegionView::default()
            });
        }
    }

    /// Отрисовать страницу в текстуру (по запросу) — вызывается из UI-кадра.
    fn render_pdf_region_texture(&mut self, ctx: &egui::Context) {
        let Some(mut view) = self.pdf_region.take() else { return };
        if !view.render_requested || view.texture.is_some() {
            self.pdf_region = Some(view);
            return;
        }
        view.render_requested = false;
        let cache_dir = self.project.root.join(".rde/cache/pdf-pages");
        let hash = rde_hash::sha256(view.path.display().to_string().as_bytes()).short(16);
        let png = cache_dir.join(format!("{hash}-p{}.png", view.page));
        if !png.is_file() {
            let _ = std::fs::create_dir_all(&cache_dir);
            if let Err(error) = rde_sources::render_pdf_page(&view.path, view.page, &png) {
                self.push_log(Level::Warn, format!("PDF-страница {}: {error}", view.page));
                self.pdf_region = Some(view);
                return;
            }
        }
        let Ok(image) = image::open(&png) else {
            self.push_log(Level::Warn, format!("PDF-страница: не прочитать {}", png.display()));
            self.pdf_region = Some(view);
            return;
        };
        let rgba = image.to_rgba8();
        let size = [rgba.width() as usize, rgba.height() as usize];
        let color_image = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
        view.image_size = egui::vec2(size[0] as f32, size[1] as f32);
        view.page_size = rde_sources::pdf_page_size(&view.path, view.page)
            .unwrap_or((view.image_size.x * 72.0 / 150.0, view.image_size.y * 72.0 / 150.0));
        view.texture = Some(ctx.load_texture(
            format!("rde-pdf-page-{}", view.key),
            color_image,
            egui::TextureOptions::LINEAR,
        ));
        if std::env::var_os("RDE_DEMO_PDF_REGION").is_some() {
            // Демо/скриншоты: показать выделение без интеракции.
            view.region = Some(egui::Rect::from_min_max(
                egui::pos2(view.image_size.x * 0.12, view.image_size.y * 0.18),
                egui::pos2(view.image_size.x * 0.88, view.image_size.y * 0.42),
            ));
        }
        self.pdf_region = Some(view);
    }

    /// UI выбора региона PDF-страницы: изображение + drag-выделение + кнопка «В чат».
    pub(crate) fn pdf_region_ui(&mut self, ui: &mut egui::Ui, source_id: &str) {
        let pal = crate::theme_colors(ui.ctx().theme());
        self.render_pdf_region_texture(ui.ctx());
        let Some(mut view) = self.pdf_region.take() else { return };
        let Some(texture) = view.texture.clone() else {
            ui.label(
                egui::RichText::new("Страница PDF отрисовывается…")
                    .small()
                    .color(pal.muted),
            );
            self.pdf_region = Some(view);
            return;
        };
        let available = ui.available_width().min(760.0);
        // Компактный просмотр: страница целиком, но не выше 420 px (помещается в кадр вместе
        // с строкой контекста и подсказкой).
        let scale = (available / view.image_size.x.max(1.0))
            .min(420.0 / view.image_size.y.max(1.0))
            .clamp(0.1, 2.0);
        let size = view.image_size * scale;
        let response = ui.add(
            egui::Image::new(egui::load::SizedTexture::new(texture.id(), size))
                .sense(egui::Sense::drag()),
        );
        let image_rect = response.rect;
        let to_image = |pos: egui::Pos2| -> egui::Pos2 {
            egui::pos2(
                ((pos.x - image_rect.min.x) / scale).clamp(0.0, view.image_size.x),
                ((pos.y - image_rect.min.y) / scale).clamp(0.0, view.image_size.y),
            )
        };
        if response.drag_started() {
            if let Some(pos) = response.interact_pointer_pos() {
                let start = to_image(pos);
                view.drag_anchor = Some(start);
                view.region = Some(egui::Rect::from_two_pos(start, start));
            }
        }
        if response.dragged() {
            if let (Some(anchor), Some(pos)) = (view.drag_anchor, response.interact_pointer_pos()) {
                view.region = Some(egui::Rect::from_two_pos(anchor, to_image(pos)));
            }
        }
        if response.drag_stopped() {
            view.drag_anchor = None;
        }
        if let Some(region) = view.region {
            let screen_rect = egui::Rect::from_min_max(
                image_rect.min + region.min.to_vec2() * scale,
                image_rect.min + region.max.to_vec2() * scale,
            );
            ui.painter()
                .rect_filled(screen_rect, 2.0, pal.accent.gamma_multiply(0.16));
            ui.painter()
                .rect_stroke(screen_rect, 2.0, egui::Stroke::new(1.5, pal.accent), egui::StrokeKind::Inside);
        }
        ui.label(
            egui::RichText::new("Протяните мышью по странице, чтобы выбрать регион (§8.7.2).")
                .small()
                .color(pal.muted),
        );
        let page_uri = ContextRef::Source {
            source_id: source_id.to_owned(),
            content_hash: None,
            page: Some(view.page),
            bbox: None,
            page_size: Some(view.page_size),
        }
        .to_uri();
        let region_uri = view.region_uri(source_id);
        let page = view.page;
        let bbox_label = view
            .region_bbox()
            .map(|bbox| format!("bbox: {:.0},{:.0}–{:.0},{:.0} пт", bbox.x0, bbox.y0, bbox.x1, bbox.y1));
        let can_region = region_uri.is_some();
        let mut attach: Option<String> = None;
        ui.horizontal(|ui| {
            if ui.button(format!("В чат: страница {page}")).clicked() {
                attach = Some(page_uri.clone());
            }
            if ui
                .add_enabled(can_region, egui::Button::new("В чат: выделенный регион"))
                .on_disabled_hover_text("выделите регион мышью на странице")
                .clicked()
            {
                attach = region_uri.clone();
            }
            if let Some(bbox_label) = &bbox_label {
                ui.label(egui::RichText::new(bbox_label).small().color(pal.muted));
            }
        });
        self.pdf_region = Some(view);
        if let Some(uri) = attach {
            self.attach_context_uri(&uri);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rde_core::context_ref::{BBox, ContextRef};

    fn project() -> Project {
        rde_core::demo()
    }

    fn document_id(project: &Project) -> String {
        project.document_object().expect("документ").id.clone()
    }

    #[test]
    fn resolves_document_block_and_para_and_marks_reanchor() {
        let project = project();
        let doc_id = document_id(&project);
        let package = project.document_path().expect("пакет");
        let blocks = rde_doc::document_blocks(&package).expect("блоки");
        let block = blocks
            .iter()
            .find(|block| !block.text.trim().is_empty())
            .expect("текстовый блок");
        let uri = ContextRef::DocBlock {
            doc_id: doc_id.clone(),
            rev: None,
            block_id: block.id.clone(),
        }
        .to_uri();
        let resolved = resolve_ref(&project, &uri).expect("resolve block");
        assert!(resolved.label.contains(&block.id));
        assert!(resolved.excerpt.contains(block.text.trim().chars().take(8).collect::<String>().as_str()));
        assert!(resolved.description.as_deref().unwrap_or_default().contains("doc_read"));
        assert!(!resolved.updated, "без rev ссылка не устаревает");
        // revision-scoped устаревшая ссылка помечается «ссылка обновлена» (re-anchor).
        let stale = ContextRef::DocBlock {
            doc_id: doc_id.clone(),
            rev: Some("not-the-current-body-hash".to_owned()),
            block_id: block.id.clone(),
        }
        .to_uri();
        assert!(resolve_ref(&project, &stale).expect("stale resolve").updated);
        // Абзац по paraId → тот же блок (re-anchor), если paraId известен.
        if let Some(para_id) = &block.para_id {
            let para_uri = ContextRef::DocPara {
                doc_id,
                rev: None,
                para_id: para_id.clone(),
            }
            .to_uri();
            let para = resolve_ref(&project, &para_uri).expect("resolve para");
            assert!(para.label.contains(&block.id), "para re-anchor в block_id: {}", para.label);
        }
    }

    #[test]
    fn resolves_artifact_output_and_note() {
        let project = project();
        let note = project
            .objects
            .iter()
            .find(|object| matches!(object.data, ObjectData::Note(_)))
            .expect("заметка в демо");
        let uri = ContextRef::Note { note_id: note.id.clone() }.to_uri();
        let resolved = resolve_ref(&project, &uri).expect("resolve note");
        assert!(resolved.label.contains(&note.title));
        assert!(!resolved.excerpt.trim().is_empty(), "сниппет заметки");
        // Артефакт: первый существующий выход демо-артефакта (график/статистика после запусков).
        let artifact = project
            .objects
            .iter()
            .find_map(|object| match &object.data {
                ObjectData::Artifact(artifact) => artifact
                    .outputs
                    .iter()
                    .map(|relpath| (object.id.clone(), relpath.clone()))
                    .find(|(id, relpath)| {
                        project
                            .artifact_object(id)
                            .map(|object| object.dir.join(relpath).is_file())
                            .unwrap_or(false)
                    })
                    .map(|(id, relpath)| (id, relpath, object.title.clone())),
                _ => None,
            });
        if let Some((artifact_id, relpath, title)) = artifact {
            let uri = ContextRef::Artifact {
                artifact_id: artifact_id.clone(),
                out_hash: None,
                relpath: relpath.clone(),
            }
            .to_uri();
            let resolved = resolve_ref(&project, &uri).expect("resolve artifact output");
            assert!(resolved.label.contains(&relpath) || resolved.label.contains(&title));
            assert!(resolved.description.as_deref().unwrap_or_default().contains("artifact_read"));
        }
        // Зеркально: несуществующий объект/блок — понятная ошибка (fail-closed для чипа).
        assert!(resolve_ref(&project, "rde://note/01JNONEXISTENT").is_err());
        let doc_id = document_id(&project);
        assert!(resolve_ref(
            &project,
            &ContextRef::DocBlock { doc_id, rev: None, block_id: "b999999".to_owned() }.to_uri()
        )
        .is_err());
    }

    #[test]
    fn resolves_project_and_template_references() {
        let project = project();
        let uri = ContextRef::Project { project_id: project.manifest.id.clone() }.to_uri();
        let resolved = resolve_ref(&project, &uri).expect("resolve project");
        assert!(resolved.label.contains(&project.manifest.name));
        assert_eq!(resolved.kind, "проект");
        // Чужой project_id — ошибка (ссылка не от этого проекта).
        assert!(resolve_ref(&project, "rde://project/01JOTHER").is_err());
        let templates = rde_templates::builtin_templates();
        let template = templates.first().expect("встроенные шаблоны");
        let uri = ContextRef::Template {
            pack_id: template.id.clone(),
            version: None,
        }
        .to_uri();
        let resolved = resolve_ref(&project, &uri).expect("resolve template");
        assert_eq!(resolved.kind, "шаблон");
        assert!(resolved.updated == false);
    }

    #[test]
    fn pdf_region_maps_pixels_to_points_and_builds_uri() {
        let view = PdfRegionView {
            image_size: egui::vec2(1000.0, 1414.0),
            page_size: (500.0, 707.0),
            region: Some(egui::Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(600.0, 500.0))),
            ..PdfRegionView::default()
        };
        let bbox = view.region_bbox().expect("bbox");
        assert!((bbox.x0 - 50.0).abs() < 0.01);
        assert!((bbox.y0 - 50.0).abs() < 0.01);
        assert!((bbox.x1 - 300.0).abs() < 0.01);
        assert!((bbox.y1 - 250.0).abs() < 0.01);
        let uri = view.region_uri("SRC").expect("uri");
        assert!(uri.starts_with("rde://source/SRC?page=1&bbox="), "{uri}");
        assert!(uri.contains("pageSize=500.00,707.00"), "{uri}");
        // Вырожденный регион — не bbox (чип региона недоступен).
        let empty = PdfRegionView {
            image_size: view.image_size,
            page_size: view.page_size,
            region: Some(egui::Rect::from_min_max(egui::pos2(1.0, 1.0), egui::pos2(2.0, 2.0))),
            ..PdfRegionView::default()
        };
        assert!(empty.region_bbox().is_none());
    }

    #[test]
    fn mime_and_bbox_helpers_are_stable() {
        assert_eq!(mime_for_path(Path::new("a/report.csv")).as_deref(), Some("text/csv"));
        assert_eq!(mime_for_path(Path::new("page.PNG")).as_deref(), Some("image/png"));
        assert!(mime_for_path(Path::new("file.bin")).is_none());
        let bbox = BBox::new(1.0, 2.0, 3.0, 4.0);
        assert_eq!(bbox.to_query(), "1.00,2.00,3.00,4.00");
    }
}
