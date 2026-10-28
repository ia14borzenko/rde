//! Контекст-ссылки RDE (Cursor-like, §8.7.2 ТЗ; R-AGT-03): URI-схема `rde://…`.
//!
//! Стабильные ID + revision-scoped ссылки. Ссылка — только **идентичность** контекста: тело
//! (текст блока/страницы/выход артефакта) подгружается лениво при разрешении в UI/агенте
//! (`resource_link` + диапазонные чтения), а в чат вставляется **чип**.
//!
//! Поддерживаемые формы (канон §8.7.2):
//!
//! ```text
//! rde://project/<project_id>
//! rde://doc/<doc_id>@<rev>#block/<block_id>
//! rde://doc/<doc_id>@<rev>#para/<w14:paraId>
//! rde://doc/<doc_id>@<rev>#comment/<comment_id>
//! rde://source/<source_id>/<content_hash>?page=<n>&bbox=<x0,y0,x1,y1>&pageSize=<w,h>
//! rde://artifact/<artifact_id>@<out_hash>/<relpath>
//! rde://note/<note_id>
//! rde://template/<pack_id>@<version>
//! ```
//!
//! Необязательные части (rev, content_hash, out_hash, version, page/bbox/pageSize) допускаются
//! опущенными — тогда ссылка указывает на «текущее» состояние (revision-scoped ссылки
//! фиксируют ревизию, если она известна). Разбор/форматирование — **отдельный модуль без
//! зависимостей на документы/PDF**: разрешение ссылки в тело — задача слоя UI/агента.
//!
//! Комментарии Word: `#comment/<comment_id>` — id комментария из пакета (Word `w:id`, RDE
//! ULID при импорте ревью); разрешение списка комментариев — `rde_doc::comments`.

use std::fmt;

/// Схема контекст-ссылок.
pub const CONTEXT_URI_SCHEME: &str = "rde";

/// Прямоугольник региона PDF в точках (PDF coordinate space, начало — левый верхний угол).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BBox {
    /// Левый край.
    pub x0: f32,
    /// Верхний край.
    pub y0: f32,
    /// Правый край.
    pub x1: f32,
    /// Нижний край.
    pub y1: f32,
}

impl BBox {
    /// Создать прямоугольник (порядок углов не важен — нормализуется).
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0, y0, x1, y1 }.normalized()
    }

    /// Нормализованный прямоугольник (`x0 ≤ x1`, `y0 ≤ y1`).
    pub fn normalized(self) -> Self {
        Self {
            x0: self.x0.min(self.x1),
            y0: self.y0.min(self.y1),
            x1: self.x0.max(self.x1),
            y1: self.y0.max(self.y1),
        }
    }

    /// Площадь региона.
    pub fn area(self) -> f32 {
        let normalized = self.normalized();
        (normalized.x1 - normalized.x0).max(0.0) * (normalized.y1 - normalized.y0).max(0.0)
    }

    /// Формат для query: `x0,y0,x1,y1` (2 знака после запятой).
    pub fn to_query(self) -> String {
        let normalized = self.normalized();
        format!(
            "{:.2},{:.2},{:.2},{:.2}",
            normalized.x0, normalized.y0, normalized.x1, normalized.y1
        )
    }

    /// Разобрать `x0,y0,x1,y1`.
    pub fn parse(text: &str) -> Result<Self, ContextRefError> {
        let parts: Vec<&str> = text.split(',').collect();
        if parts.len() != 4 {
            return Err(ContextRefError(format!(
                "bbox должен быть `x0,y0,x1,y1`, получено `{text}`"
            )));
        }
        let mut values = [0.0f32; 4];
        for (index, part) in parts.iter().enumerate() {
            values[index] = part
                .trim()
                .parse::<f32>()
                .map_err(|error| ContextRefError(format!("bbox[{index}]: {error}")))?;
            if !values[index].is_finite() {
                return Err(ContextRefError(format!("bbox[{index}] не число")));
            }
        }
        let bbox = Self { x0: values[0], y0: values[1], x1: values[2], y1: values[3] };
        if bbox.area() <= 0.0 {
            return Err(ContextRefError("bbox вырожден (нулевая площадь)".to_owned()));
        }
        Ok(bbox.normalized())
    }
}

/// Ошибка разбора/проверки контекст-ссылки.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextRefError(pub String);

impl fmt::Display for ContextRefError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for ContextRefError {}

/// Контекст-ссылка §8.7.2.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextRef {
    /// Весь проект.
    Project {
        /// Идентификатор проекта (из `rde.toml` id).
        project_id: String,
    },
    /// Блок документа (`#block/<block_id>`; `block_id` — стабильный `bN`).
    DocBlock {
        /// Идентификатор документа.
        doc_id: String,
        /// Ревизия (`document.xml` body hash / версия store), если зафиксирована.
        rev: Option<String>,
        /// Идентификатор блока (`b1`, `b2`, …).
        block_id: String,
    },
    /// Абзац документа по стабильному `w14:paraId`.
    DocPara {
        /// Идентификатор документа.
        doc_id: String,
        /// Ревизия, если зафиксирована.
        rev: Option<String>,
        /// `w14:paraId` (8 hex-цифр).
        para_id: String,
    },
    /// Комментарий Word из документа.
    DocComment {
        /// Идентификатор документа.
        doc_id: String,
        /// Ревизия, если зафиксирована.
        rev: Option<String>,
        /// Идентификатор комментария (Word `w:id` / ULID импорта ревью).
        comment_id: String,
    },
    /// Источник (файл/URL/PDF): страница и регион — для PDF.
    Source {
        /// Идентификатор источника.
        source_id: String,
        /// Хэш содержимого источника (если известен: `content_hash`/sha256 файла).
        content_hash: Option<String>,
        /// Номер страницы PDF (с 1).
        page: Option<u32>,
        /// Регион страницы (точки).
        bbox: Option<BBox>,
        /// Размер страницы (точки, `w,h`) для пересчёта региона.
        page_size: Option<(f32, f32)>,
    },
    /// Выход артефакта (`relpath` внутри каталога артефакта).
    Artifact {
        /// Идентификатор артефакта.
        artifact_id: String,
        /// `out_hash` последнего успешного запуска (идентичность выхода), если зафиксирован.
        out_hash: Option<String>,
        /// Относительный путь выхода внутри каталога артефакта.
        relpath: String,
    },
    /// Заметка проекта.
    Note {
        /// Идентификатор заметки.
        note_id: String,
    },
    /// Шаблон документа (пакет ГОСТ-правил).
    Template {
        /// Идентификатор пакета/файла шаблона.
        pack_id: String,
        /// Версия шаблона, если зафиксирована.
        version: Option<String>,
    },
}

impl ContextRef {
    /// Разобрать `rde://…` (проверяет схему, структуру и обязательные части).
    pub fn parse(uri: &str) -> Result<Self, ContextRefError> {
        let trimmed = uri.trim();
        let Some(rest) = trimmed.strip_prefix("rde://") else {
            return Err(ContextRefError(format!("ссылка должна начинаться с `rde://`: `{uri}`")));
        };
        if rest.is_empty() {
            return Err(ContextRefError("пустая ссылка `rde://`".to_owned()));
        }
        // fragment (#block/…, #para/…, #comment/…) отдельно от path?query.
        let (path_query, fragment) = match rest.split_once('#') {
            Some((head, fragment)) => (head, Some(fragment)),
            None => (rest, None),
        };
        let (path, query) = match path_query.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (path_query, None),
        };
        let mut segments = path.split('/');
        let kind = segments.next().unwrap_or_default();
        match kind {
            "project" => {
                let project_id = decode_id(segments.next(), "project")?;
                ensure_no_extra(segments, "project")?;
                ensure_no_query_fragment(query, fragment, "project")?;
                Ok(ContextRef::Project { project_id })
            }
            "doc" => {
                let (doc_id, rev) = decode_id_rev(segments.next(), "doc")?;
                ensure_no_extra(segments, "doc")?;
                if query.filter(|query| !query.is_empty()).is_some() {
                    return Err(ContextRefError("doc: query-параметры не поддерживаются".to_owned()));
                }
                match fragment {
                    Some(fragment) if fragment.starts_with("block/") => {
                        let block_id = decode_raw_id(&fragment["block/".len()..], "block_id")?;
                        Ok(ContextRef::DocBlock { doc_id, rev, block_id })
                    }
                    Some(fragment) if fragment.starts_with("para/") => {
                        let para_id = decode_raw_id(&fragment["para/".len()..], "para_id")?;
                        Ok(ContextRef::DocPara { doc_id, rev, para_id })
                    }
                    Some(fragment) if fragment.starts_with("comment/") => {
                        let comment_id = decode_raw_id(&fragment["comment/".len()..], "comment_id")?;
                        Ok(ContextRef::DocComment { doc_id, rev, comment_id })
                    }
                    Some(other) => Err(ContextRefError(format!(
                        "doc: неизвестный fragment `#{other}` (ожидается block/…, para/…, comment/…)"
                    ))),
                    None => Err(ContextRefError(
                        "doc: укажите блок `#block/…`, абзац `#para/…` или комментарий `#comment/…`"
                            .to_owned(),
                    )),
                }
            }
            "source" => {
                let source_id = decode_id(segments.next(), "source")?;
                let content_hash = segments
                    .next()
                    .map(percent_decode)
                    .filter(|value| !value.is_empty());
                ensure_no_extra(segments, "source")?;
                ensure_no_fragment(fragment, "source")?;
                let mut page = None;
                let mut bbox = None;
                let mut page_size = None;
                if let Some(query) = query {
                    for pair in query.split('&').filter(|pair| !pair.is_empty()) {
                        let (key, value) = pair
                            .split_once('=')
                            .ok_or_else(|| ContextRefError(format!("query `{pair}` без `=`")))?;
                        match key {
                            "page" => {
                                page = Some(
                                    value
                                        .parse::<u32>()
                                        .map_err(|error| ContextRefError(format!("page: {error}")))?,
                                );
                            }
                            "bbox" => bbox = Some(BBox::parse(value)?),
                            "pageSize" | "page_size" => {
                                let parts: Vec<&str> = value.split(',').collect();
                                if parts.len() != 2 {
                                    return Err(ContextRefError(
                                        "pageSize должен быть `w,h`".to_owned(),
                                    ));
                                }
                                let width = parts[0]
                                    .parse::<f32>()
                                    .map_err(|error| ContextRefError(format!("pageSize: {error}")))?;
                                let height = parts[1]
                                    .parse::<f32>()
                                    .map_err(|error| ContextRefError(format!("pageSize: {error}")))?;
                                page_size = Some((width, height));
                            }
                            other => {
                                return Err(ContextRefError(format!(
                                    "source: неизвестный параметр `{other}`"
                                )));
                            }
                        }
                    }
                }
                if page == Some(0) {
                    return Err(ContextRefError("page нумеруется с 1".to_owned()));
                }
                if bbox.is_some() && page.is_none() {
                    return Err(ContextRefError(
                        "bbox допустим только вместе с page (регион страницы PDF)".to_owned(),
                    ));
                }
                Ok(ContextRef::Source { source_id, content_hash, page, bbox, page_size })
            }
            "artifact" => {
                let (artifact_id, out_hash) = decode_id_rev(segments.next(), "artifact")?;
                if out_hash.as_deref().is_some_and(|hash| hash.is_empty()) {
                    return Err(ContextRefError("artifact: пустой out_hash".to_owned()));
                }
                let relpath = percent_decode(&segments.collect::<Vec<_>>().join("/"));
                if relpath.is_empty() {
                    return Err(ContextRefError(
                        "artifact: укажите relpath выхода (`/path/to/output`)".to_owned(),
                    ));
                }
                ensure_no_query_fragment(query, fragment, "artifact")?;
                Ok(ContextRef::Artifact { artifact_id, out_hash, relpath })
            }
            "note" => {
                let note_id = decode_id(segments.next(), "note")?;
                ensure_no_extra(segments, "note")?;
                ensure_no_query_fragment(query, fragment, "note")?;
                Ok(ContextRef::Note { note_id })
            }
            "template" => {
                let (pack_id, version) = decode_id_rev(segments.next(), "template")?;
                ensure_no_extra(segments, "template")?;
                ensure_no_query_fragment(query, fragment, "template")?;
                Ok(ContextRef::Template { pack_id, version })
            }
            other => Err(ContextRefError(format!(
                "неизвестный тип ссылки `{other}` (project|doc|source|artifact|note|template)"
            ))),
        }
    }

    /// Канонический URI (все известные части; отсутствующие — опущены).
    pub fn to_uri(&self) -> String {
        match self {
            ContextRef::Project { project_id } => {
                format!("rde://project/{}", percent_encode(project_id))
            }
            ContextRef::DocBlock { doc_id, rev, block_id } => format!(
                "rde://doc/{}{}#block/{}",
                percent_encode(doc_id),
                rev_suffix(rev),
                percent_encode(block_id)
            ),
            ContextRef::DocPara { doc_id, rev, para_id } => format!(
                "rde://doc/{}{}#para/{}",
                percent_encode(doc_id),
                rev_suffix(rev),
                percent_encode(para_id)
            ),
            ContextRef::DocComment { doc_id, rev, comment_id } => format!(
                "rde://doc/{}{}#comment/{}",
                percent_encode(doc_id),
                rev_suffix(rev),
                percent_encode(comment_id)
            ),
            ContextRef::Source { source_id, content_hash, page, bbox, page_size } => {
                let mut uri = format!("rde://source/{}", percent_encode(source_id));
                if let Some(hash) = content_hash {
                    uri.push('/');
                    uri.push_str(&percent_encode(hash));
                }
                let mut query: Vec<String> = Vec::new();
                if let Some(page) = page {
                    query.push(format!("page={page}"));
                }
                if let Some(bbox) = bbox {
                    query.push(format!("bbox={}", bbox.to_query()));
                }
                if let Some((width, height)) = page_size {
                    query.push(format!("pageSize={width:.2},{height:.2}"));
                }
                if !query.is_empty() {
                    uri.push('?');
                    uri.push_str(&query.join("&"));
                }
                uri
            }
            ContextRef::Artifact { artifact_id, out_hash, relpath } => format!(
                "rde://artifact/{}{}/{}",
                percent_encode(artifact_id),
                rev_suffix(out_hash),
                relpath.split('/').map(percent_encode).collect::<Vec<_>>().join("/")
            ),
            ContextRef::Note { note_id } => format!("rde://note/{}", percent_encode(note_id)),
            ContextRef::Template { pack_id, version } => format!(
                "rde://template/{}{}",
                percent_encode(pack_id),
                rev_suffix(version)
            ),
        }
    }

    /// Тип ссылки по-русски (для чипа/списка вложений).
    pub fn kind_label(&self) -> &'static str {
        match self {
            ContextRef::Project { .. } => "проект",
            ContextRef::DocBlock { .. } => "блок документа",
            ContextRef::DocPara { .. } => "абзац документа",
            ContextRef::DocComment { .. } => "комментарий",
            ContextRef::Source { .. } => "источник",
            ContextRef::Artifact { .. } => "выход артефакта",
            ContextRef::Note { .. } => "заметка",
            ContextRef::Template { .. } => "шаблон",
        }
    }

    /// Идентификатор целевого объекта проекта (если ссылка на объект).
    pub fn target_object_id(&self) -> Option<&str> {
        match self {
            ContextRef::Project { .. } => None,
            ContextRef::DocBlock { doc_id, .. }
            | ContextRef::DocPara { doc_id, .. }
            | ContextRef::DocComment { doc_id, .. } => Some(doc_id),
            ContextRef::Source { source_id, .. } => Some(source_id),
            ContextRef::Artifact { artifact_id, .. } => Some(artifact_id),
            ContextRef::Note { note_id } => Some(note_id),
            ContextRef::Template { pack_id, .. } => Some(pack_id),
        }
    }

    /// Короткая подпись для чипа (`блок b3`, `PDF с. 37`, `выход report.csv`, …).
    pub fn short_label(&self) -> String {
        match self {
            ContextRef::Project { project_id } => format!("проект {project_id}"),
            ContextRef::DocBlock { doc_id, block_id, .. } => {
                format!("doc {doc_id}: блок {block_id}")
            }
            ContextRef::DocPara { doc_id, para_id, .. } => {
                format!("doc {doc_id}: абзац {para_id}")
            }
            ContextRef::DocComment { doc_id, comment_id, .. } => {
                format!("doc {doc_id}: комментарий {comment_id}")
            }
            ContextRef::Source { source_id, page, bbox, .. } => match (page, bbox) {
                (Some(page), Some(bbox)) => {
                    format!("источник {source_id}: с. {page}, регион {:.0}×{:.0} пт", bbox.x1 - bbox.x0, bbox.y1 - bbox.y0)
                }
                (Some(page), None) => format!("источник {source_id}: с. {page}"),
                _ => format!("источник {source_id}"),
            },
            ContextRef::Artifact { artifact_id, relpath, .. } => {
                let name = relpath.rsplit('/').next().unwrap_or(relpath);
                format!("выход {artifact_id}: {name}")
            }
            ContextRef::Note { note_id } => format!("заметка {note_id}"),
            ContextRef::Template { pack_id, version } => match version {
                Some(version) => format!("шаблон {pack_id}@{version}"),
                None => format!("шаблон {pack_id}"),
            },
        }
    }

    /// Ссылки указывают на один и тот же контекст (для дедупликации вложений в чате).
    ///
    /// Сравнивается канонический URI: `rev`/`out_hash`/`content_hash` входят в идентичность.
    pub fn same_target(&self, other: &Self) -> bool {
        self.to_uri() == other.to_uri()
    }
}

impl fmt::Display for ContextRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_uri())
    }
}

fn rev_suffix(rev: &Option<String>) -> String {
    match rev {
        Some(rev) if !rev.is_empty() => format!("@{rev}"),
        _ => String::new(),
    }
}

/// Декодировать сегмент-id: проверяем **сырой** (percent-encoded) вид, затем декодируем.
///
/// Канонический URI не содержит сырых пробелов/разделителей — они обязаны быть `%XX`;
/// декодированное значение (например, заметка с пробелом) допустимо.
fn decode_raw_id(raw: &str, kind: &str) -> Result<String, ContextRefError> {
    if raw.is_empty() {
        return Err(ContextRefError(format!("{kind}: пустой идентификатор")));
    }
    if raw.chars().any(|c| c.is_whitespace() || matches!(c, '/' | '?' | '#')) {
        return Err(ContextRefError(format!(
            "{kind}: идентификатор содержит недопустимый для URI символ: `{raw}`"
        )));
    }
    let decoded = percent_decode(raw);
    if decoded.trim().is_empty() {
        return Err(ContextRefError(format!("{kind}: пустой идентификатор")));
    }
    Ok(decoded)
}

fn decode_id<'a>(
    segment: Option<&'a str>,
    kind: &str,
) -> Result<String, ContextRefError> {
    match segment {
        Some(raw) if !raw.is_empty() => decode_raw_id(raw, kind),
        _ => Err(ContextRefError(format!("{kind}: ожидается `<id>`"))),
    }
}

fn decode_id_rev<'a>(
    segment: Option<&'a str>,
    kind: &str,
) -> Result<(String, Option<String>), ContextRefError> {
    let raw = match segment {
        Some(raw) if !raw.is_empty() => raw,
        _ => return Err(ContextRefError(format!("{kind}: ожидается `<id>[@<rev>]`"))),
    };
    match raw.split_once('@') {
        Some((raw_id, raw_rev)) => {
            let id = decode_raw_id(raw_id, kind)?;
            if raw_rev.is_empty() {
                return Err(ContextRefError(format!("{kind}: пустой rev после `@`")));
            }
            let rev = percent_decode(raw_rev);
            Ok((id, Some(rev)))
        }
        None => Ok((decode_raw_id(raw, kind)?, None)),
    }
}

fn ensure_no_extra<'a>(
    mut segments: impl Iterator<Item = &'a str>,
    kind: &str,
) -> Result<(), ContextRefError> {
    if let Some(extra) = segments.find(|segment| !segment.is_empty()) {
        return Err(ContextRefError(format!("{kind}: лишний сегмент `{extra}`")));
    }
    Ok(())
}

fn ensure_no_query_fragment(
    query: Option<&str>,
    fragment: Option<&str>,
    kind: &str,
) -> Result<(), ContextRefError> {
    if query.filter(|query| !query.is_empty()).is_some() {
        return Err(ContextRefError(format!("{kind}: query-параметры не поддерживаются")));
    }
    if fragment.filter(|fragment| !fragment.is_empty()).is_some() {
        return Err(ContextRefError(format!("{kind}: fragment не поддерживается")));
    }
    Ok(())
}

fn ensure_no_fragment(fragment: Option<&str>, kind: &str) -> Result<(), ContextRefError> {
    if fragment.filter(|fragment| !fragment.is_empty()).is_some() {
        return Err(ContextRefError(format!("{kind}: fragment не поддерживается")));
    }
    Ok(())
}

/// Процентное кодирование минимального набора символов (пробел, `%`, `#`, `?`, `&`, `=`, `+`, `,`).
pub fn percent_encode(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' | b'@' | b':' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }
    encoded
}

/// Обратное к [`percent_encode`] (неизвестные/битые последовательности остаются как есть).
pub fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = &value[index + 1..index + 3];
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_examples_parse_and_format_roundtrip() {
        let examples = [
            "rde://project/01JPROJECT",
            "rde://doc/01JDOC@abc123#block/b3",
            "rde://doc/01JDOC#block/b12",
            "rde://doc/01JDOC@abc123#para/1A2B3C4D",
            "rde://doc/01JDOC@abc123#comment/42",
            "rde://source/01JSRC/abcdef?page=37&bbox=10.00,20.00,210.00,120.00&pageSize=595.28,841.89",
            "rde://source/01JSRC?page=2",
            "rde://artifact/01JART@deadbeef/report.csv",
            "rde://artifact/01JART/report.csv",
            "rde://note/01JNOTE",
            "rde://template/gost-7.32-2017@1.0",
        ];
        for uri in examples {
            let parsed = ContextRef::parse(uri).unwrap_or_else(|error| panic!("{uri}: {error}"));
            assert_eq!(parsed.to_uri(), uri, "канонический URI должен быть стабильным");
        }
    }

    #[test]
    fn doc_block_and_source_match_tz_scheme_semantics() {
        match ContextRef::parse("rde://doc/01JDOC@abc#block/b3").expect("doc block") {
            ContextRef::DocBlock { doc_id, rev, block_id } => {
                assert_eq!(doc_id, "01JDOC");
                assert_eq!(rev.as_deref(), Some("abc"));
                assert_eq!(block_id, "b3");
                assert_eq!(rev.as_deref(), Some("abc"));
            }
            other => panic!("ожидался DocBlock: {other:?}"),
        }
        match ContextRef::parse(
            "rde://source/01JSRC/hash1?page=37&bbox=100,50,10,200&pageSize=595,842",
        )
        .expect("source region")
        {
            ContextRef::Source { source_id, content_hash, page, bbox, page_size } => {
                assert_eq!(source_id, "01JSRC");
                assert_eq!(content_hash.as_deref(), Some("hash1"));
                assert_eq!(page, Some(37));
                // bbox нормализован (10,50 → 100,200).
                let bbox = bbox.expect("bbox");
                assert_eq!((bbox.x0, bbox.y0, bbox.x1, bbox.y1), (10.0, 50.0, 100.0, 200.0));
                assert_eq!(page_size, Some((595.0, 842.0)));
            }
            other => panic!("ожидался Source: {other:?}"),
        }
    }

    #[test]
    fn labels_and_targets_are_human_readable() {
        let doc = ContextRef::parse("rde://doc/D#block/b3").expect("doc");
        assert_eq!(doc.kind_label(), "блок документа");
        assert_eq!(doc.target_object_id(), Some("D"));
        assert_eq!(doc.short_label(), "doc D: блок b3");
        let source = ContextRef::parse("rde://source/S?page=37&bbox=0,0,100,50").expect("source");
        assert_eq!(source.kind_label(), "источник");
        assert!(source.short_label().contains("с. 37"));
        let artifact = ContextRef::parse("rde://artifact/A@h/out/report.csv").expect("artifact");
        assert_eq!(artifact.kind_label(), "выход артефакта");
        assert!(artifact.short_label().contains("report.csv"));
        assert_eq!(artifact.target_object_id(), Some("A"));
    }

    #[test]
    fn dedupe_compares_canonical_uri_and_revision() {
        let first = ContextRef::parse("rde://doc/D@rev1#block/b3").expect("first");
        let same = ContextRef::parse("rde://doc/D@rev1#block/b3").expect("same");
        let newer = ContextRef::parse("rde://doc/D@rev2#block/b3").expect("newer");
        assert!(first.same_target(&same));
        assert!(!first.same_target(&newer), "revision-scoped ссылки различаются");
    }

    #[test]
    fn invalid_uris_are_rejected_with_reasons() {
        let invalid = [
            "http://example.com/doc/D#block/b1",
            "rde://",
            "rde://doc/D",
            "rde://doc/D#block/",
            "rde://doc/D#unknown/x",
            "rde://source/S?page=0",
            "rde://source/S?bbox=1,2,3,4",
            "rde://source/S?bbox=1,2,3",
            "rde://source/S?page=2&bbox=0,0,10,20&pageSize=100",
            "rde://artifact/A@h/",
            "rde://artifact/A",
            "rde://note/",
            "rde://template/",
            "rde://doc/D@rev#block/b1#frag",
            "rde://doc/D@rev#block/b 1",
        ];
        for uri in invalid {
            assert!(ContextRef::parse(uri).is_err(), "должна быть ошибка: {uri}");
        }
    }

    #[test]
    fn percent_encoding_survives_spaces_and_delimiters() {
        let note = ContextRef::Note { note_id: "заметка 1".to_owned() };
        let uri = note.to_uri();
        assert!(!uri.contains(' '), "пробел должен кодироваться: {uri}");
        assert_eq!(ContextRef::parse(&uri).expect("roundtrip"), note);
        let artifact = ContextRef::Artifact {
            artifact_id: "A".to_owned(),
            out_hash: Some("h".to_owned()),
            relpath: "out dir/report 1.csv".to_owned(),
        };
        let uri = artifact.to_uri();
        assert_eq!(ContextRef::parse(&uri).expect("artifact roundtrip"), artifact);
        assert_eq!(percent_decode(&percent_encode("a b#c?d&e=f,g+h")), "a b#c?d&e=f,g+h");
    }

    #[test]
    fn bbox_validates_and_normalizes() {
        let bbox = BBox::parse("100.50,200,10,20").expect("bbox");
        assert_eq!(bbox.to_query(), "10.00,20.00,100.50,200.00");
        assert!(bbox.area() > 0.0);
        assert!(BBox::parse("1,1,1,1").is_err(), "вырожденный регион");
        assert!(BBox::parse("1,2,3").is_err());
        assert!(BBox::parse("a,b,c,d").is_err());
    }
}
