//! Ресурсы MCP (`rde://…`) и resource-templates (M1: 4 ресурса, R-MCP-04/A3).
//!
//! Ресурс — стабильный URI, который клиент может читать напрямую (без вызова инструмента):
//! - `rde://project/tree` — дерево проекта;
//! - `rde://doc/{doc_id}/outline` — оглавление документа;
//! - `rde://artifact/{artifact_id}/meta` — манифест + ledger-метаданные артефакта;
//! - `rde://templates` — каталог шаблонов.
//!
//! `ttlMs`/`cacheScope` (spec MCP 2026-07-28, SEP-2549) — в [`ResourceTarget::ttl_ms`]:
//! изменяемые ресурсы живут 2 с и кэшируются только приватно, статический каталог шаблонов —
//! 5 минут и публично. Значения зафиксированы в ADR-0006.

use crate::error::HostError;

/// URI дерева проекта.
pub const PROJECT_TREE_URI: &str = "rde://project/tree";

/// URI каталога шаблонов.
pub const TEMPLATES_URI: &str = "rde://templates";

/// Шаблон URI оглавления документа.
pub const DOC_OUTLINE_URI_TEMPLATE: &str = "rde://doc/{doc_id}/outline";

/// Шаблон URI метаданных артефакта.
pub const ARTIFACT_META_URI_TEMPLATE: &str = "rde://artifact/{artifact_id}/meta";

/// Префикс URI документа.
const DOC_PREFIX: &str = "rde://doc/";

/// Префикс URI артефакта.
const ARTIFACT_PREFIX: &str = "rde://artifact/";

/// Суффикс URI оглавления документа.
const OUTLINE_SUFFIX: &str = "/outline";

/// Суффикс URI метаданных артефакта.
const META_SUFFIX: &str = "/meta";

/// Разобранная цель ресурса.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResourceTarget {
    /// `rde://project/tree`.
    ProjectTree,
    /// `rde://templates`.
    Templates,
    /// `rde://doc/{id}/outline`.
    DocOutline(String),
    /// `rde://artifact/{id}/meta`.
    ArtifactMeta(String),
}

impl ResourceTarget {
    /// Канонический URI этой цели.
    pub fn uri(&self) -> String {
        match self {
            ResourceTarget::ProjectTree => PROJECT_TREE_URI.to_owned(),
            ResourceTarget::Templates => TEMPLATES_URI.to_owned(),
            ResourceTarget::DocOutline(id) => doc_outline_uri(id),
            ResourceTarget::ArtifactMeta(id) => artifact_meta_uri(id),
        }
    }

    /// `ttlMs` результата чтения (spec 2026-07-28, A3).
    pub fn ttl_ms(&self) -> u64 {
        match self {
            ResourceTarget::Templates => crate::STATIC_RESOURCE_TTL_MS,
            ResourceTarget::ProjectTree | ResourceTarget::DocOutline(_) | ResourceTarget::ArtifactMeta(_) => {
                crate::MUTABLE_RESOURCE_TTL_MS
            }
        }
    }

    /// Публичное ли кэширование (только статический каталог шаблонов).
    pub fn cache_scope_public(&self) -> bool {
        matches!(self, ResourceTarget::Templates)
    }
}

/// URI оглавления документа.
pub fn doc_outline_uri(doc_id: &str) -> String {
    format!("{DOC_PREFIX}{doc_id}{OUTLINE_SUFFIX}")
}

/// URI метаданных артефакта.
pub fn artifact_meta_uri(artifact_id: &str) -> String {
    format!("{ARTIFACT_PREFIX}{artifact_id}{META_SUFFIX}")
}

/// Разобрать URI ресурса; `None` — URI неизвестен этому серверу.
///
/// Идентификаторы — стабильные ULID без `/`; лишние сегменты/пустые id отвергаются.
pub fn parse_resource_uri(uri: &str) -> Option<ResourceTarget> {
    let uri = uri.trim();
    if uri == PROJECT_TREE_URI {
        return Some(ResourceTarget::ProjectTree);
    }
    if uri == TEMPLATES_URI {
        return Some(ResourceTarget::Templates);
    }
    if let Some(rest) = uri.strip_prefix(DOC_PREFIX) {
        let id = rest.strip_suffix(OUTLINE_SUFFIX)?;
        if is_valid_id(id) {
            return Some(ResourceTarget::DocOutline(id.to_owned()));
        }
        return None;
    }
    if let Some(rest) = uri.strip_prefix(ARTIFACT_PREFIX) {
        let id = rest.strip_suffix(META_SUFFIX)?;
        if is_valid_id(id) {
            return Some(ResourceTarget::ArtifactMeta(id.to_owned()));
        }
        return None;
    }
    None
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty() && !id.contains('/') && !id.contains("..") && id.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// Проверить, что URI ресурса читается сервером (ошибка `RDE_NOT_FOUND` с подсказкой).
pub fn require_resource(uri: &str) -> Result<ResourceTarget, HostError> {
    parse_resource_uri(uri).ok_or_else(|| {
        HostError::not_found(format!("ресурс `{uri}` не найден")).with_hint(
            "доступны: rde://project/tree, rde://templates, rde://doc/<id>/outline, rde://artifact/<id>/meta",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_uris() {
        assert_eq!(parse_resource_uri(PROJECT_TREE_URI), Some(ResourceTarget::ProjectTree));
        assert_eq!(parse_resource_uri(TEMPLATES_URI), Some(ResourceTarget::Templates));
        assert_eq!(
            parse_resource_uri("rde://doc/01M5VJ1JG04TFF59TDWH9EDD1R/outline"),
            Some(ResourceTarget::DocOutline("01M5VJ1JG04TFF59TDWH9EDD1R".to_owned()))
        );
        assert_eq!(
            parse_resource_uri("rde://artifact/01M5VJ1JG0ABCDEFGHJKMNPQRS/meta"),
            Some(ResourceTarget::ArtifactMeta("01M5VJ1JG0ABCDEFGHJKMNPQRS".to_owned()))
        );
    }

    #[test]
    fn rejects_unknown_or_malformed_uris() {
        for uri in [
            "rde://project/other",
            "rde://doc//outline",
            "rde://doc/../outline",
            "rde://artifact/x/meta/extra",
            "file:///etc/passwd",
            "",
        ] {
            assert!(parse_resource_uri(uri).is_none(), "URI должен быть отвергнут: {uri:?}");
        }
    }

    #[test]
    fn uris_roundtrip() {
        for target in [
            ResourceTarget::ProjectTree,
            ResourceTarget::Templates,
            ResourceTarget::DocOutline("01ABC".to_owned()),
            ResourceTarget::ArtifactMeta("01XYZ".to_owned()),
        ] {
            assert_eq!(parse_resource_uri(&target.uri()), Some(target));
        }
    }

    #[test]
    fn ttl_policy_matches_adr() {
        assert_eq!(ResourceTarget::ProjectTree.ttl_ms(), crate::MUTABLE_RESOURCE_TTL_MS);
        assert_eq!(ResourceTarget::DocOutline("x".to_owned()).ttl_ms(), crate::MUTABLE_RESOURCE_TTL_MS);
        assert_eq!(ResourceTarget::ArtifactMeta("x".to_owned()).ttl_ms(), crate::MUTABLE_RESOURCE_TTL_MS);
        assert_eq!(ResourceTarget::Templates.ttl_ms(), crate::STATIC_RESOURCE_TTL_MS);
        assert!(ResourceTarget::Templates.cache_scope_public());
        assert!(!ResourceTarget::ProjectTree.cache_scope_public());
    }

    #[test]
    fn unknown_resource_is_not_found_error() {
        let error = require_resource("rde://nope").expect_err("неизвестный ресурс");
        assert_eq!(error.code, crate::error::RdeErrorCode::NotFound);
    }
}
