//! RDE project model (M2.1): типизированные TOML-манифесты, JSON Schema, загрузка проекта.
//!
//! Канон формата — `tz-02` §8.3; реализовано подмножество v0.1. Модель всегда пересоздаётся из
//! файлов проекта (source of truth — файлы, как требует ТЗ §8.4), поэтому здесь нет состояния
//! редактирования: только чтение/валидация/факты и статусы.
//!
//! ```
//! let project = rde_core::demo();
//! assert!(!project.groups.is_empty());
//! ```

use std::path::PathBuf;

pub mod manifest;
pub mod ops;
pub mod project;

pub use manifest::{
    ArtifactManifest, AuthorManifest, ConstantValue, DocumentManifest, GroupManifest, InputSpec,
    ManifestKind, NoteManifest, ProjectManifest, ReferenceManifest, RepositoryManifest,
    RuntimeManifest, SourceManifest,
};
pub use ops::{CreateRequest, NewObjectKind, create_object, delete_object, rename_object};
pub use project::{
    Freshness, Group, Issue, Level, LogLine, Object, ObjectData, ObjectKind, Project, load_project,
};

/// Каталог демо-проекта по умолчанию.
///
/// `RDE_PROJECT` (или `RDE_DEMO_PROJECT`) переопределяет путь; иначе ищется `demo-project/`
/// вверх от текущего каталога, затем — рядом с крейтом (`rde/demo-project`).
pub fn demo_project_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("RDE_PROJECT").or_else(|| std::env::var_os("RDE_DEMO_PROJECT")) {
        return PathBuf::from(dir);
    }
    if let Ok(cwd) = std::env::current_dir() {
        for base in cwd.ancestors() {
            let candidate = base.join("demo-project");
            if candidate.join("rde.toml").is_file() {
                return candidate;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../demo-project")
}

/// Демо-проект (паникует, если он сломан — это ошибка репозитория, а не пользовательского ввода).
pub fn demo() -> Project {
    let dir = demo_project_dir();
    load_project(&dir).unwrap_or_else(|e| panic!("демо-проект {} не загружается: {e}", dir.display()))
}

/// Загрузить проект из каталога (внешние ошибки — `Err`, проблемы объектов — `Project::issues`).
pub fn load(root: impl AsRef<std::path::Path>) -> Result<Project, String> {
    load_project(root.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{self, ManifestKind};

    fn unique_tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-core-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn copy_demo_without_generated_state(target: &std::path::Path) {
        fn copy(src: &std::path::Path, dst: &std::path::Path) {
            std::fs::create_dir_all(dst).unwrap();
            for entry in std::fs::read_dir(src).unwrap().flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if matches!(&*name, ".rde" | "exports" | "__pycache__" | "out") {
                    continue;
                }
                let path = entry.path();
                let to = dst.join(&*name);
                if path.is_dir() {
                    copy(&path, &to);
                } else {
                    std::fs::copy(&path, &to).unwrap();
                }
            }
        }
        copy(&demo_project_dir(), target);
    }

    #[test]
    fn demo_project_loads_and_has_expected_objects() {
        // Копия без .rde/out: тест не зависит от того, запускал ли кто-то артефакты в рабочем demo-project.
        let dir = unique_tmp("demo-copy");
        copy_demo_without_generated_state(&dir);
        let project = load_project(&dir).unwrap();
        assert!(project.issues.is_empty(), "issues: {:?}", project.issues);
        assert_eq!(
            project.object_count(),
            10,
            "демо-проект: 1 документ + 4 артефакта + 3 источника + 1 заметка + 1 репозиторий"
        );
        // compute-артефакты без выходов честно «устарели»; данные/источники/заметки — актуальны/внешние.
        assert_eq!(project.stale_count(), 2);
        assert!(project.document_path().is_some_and(|p| p.is_file()));
        assert_eq!(project.artifacts().count(), 4);
        // Порядок артефактов топологический: realizations → stats → plot.
        let artifacts: Vec<&str> = project.artifacts().map(|a| a.title.as_str()).collect();
        let pos = |title: &str| artifacts.iter().position(|t| *t == title).unwrap();
        assert!(pos("Реализации случайного процесса") < pos("Статистики реализаций"));
        assert!(pos("Статистики реализаций") < pos("График статистик"));
        // DAG: stats зависит от realizations.
        let stats_id = project
            .objects
            .iter()
            .find(|o| o.title == "Статистики реализаций")
            .map(|o| o.id.clone())
            .unwrap();
        let real_id = project
            .objects
            .iter()
            .find(|o| o.title == "Реализации случайного процесса")
            .map(|o| o.id.clone())
            .unwrap();
        let edges = project.dag_edges();
        assert!(edges.contains(&(real_id, stats_id)));
        // Константа из artifact type=constants попадает в группу «Константы».
        assert!(project.groups.iter().any(|g| g.kind == ObjectKind::Constants && !g.objects.is_empty()));
        assert_eq!(project.figures(), 1);
        assert!(!project.log.is_empty());
    }

    #[test]
    fn broken_manifest_is_reported_not_fatal() {
        let dir = unique_tmp("broken");
        std::fs::write(
            dir.join("rde.toml"),
            "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"T\"\n",
        )
        .unwrap();
        let art = dir.join("objects/artifacts/01M5VJ1JG0HD99FDTGJ8YEPFZX");
        std::fs::create_dir_all(&art).unwrap();
        // Нет обязательного title и неверный type.
        std::fs::write(
            art.join("artifact.toml"),
            "schema = \"rde/artifact@1\"\nid = \"01M5VJ1JG0HD99FDTGJ8YEPFZX\"\ntype = \"nonsense\"\n",
        )
        .unwrap();
        let project = load(&dir).expect("rde.toml валиден — загрузка не фатальна");
        assert_eq!(project.object_count(), 0);
        assert!(!project.issues.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manifest_schema_rejects_bad_types() {
        let good: toml::Value = toml::from_str(
            "schema=\"rde/artifact@1\"\nid=\"01M5VJ1JG0HD99FDTGJ8YEPFZX\"\ntitle=\"A\"\ntype=\"compute\"\n",
        )
        .unwrap();
        assert!(manifest::validate(ManifestKind::Artifact, &good).is_empty());
        let bad: toml::Value =
            toml::from_str("schema=\"rde/artifact@1\"\nid=42\ntitle=\"A\"\ntype=\"compute\"\n").unwrap();
        assert!(!manifest::validate(ManifestKind::Artifact, &bad).is_empty());
        // Все схемы генерируются и являются JSON-объектами.
        for kind in [
            ManifestKind::Project,
            ManifestKind::Artifact,
            ManifestKind::Document,
            ManifestKind::Source,
            ManifestKind::Note,
            ManifestKind::Repository,
        ] {
            let schema = kind.schema();
            assert!(schema.is_object(), "схема {} пуста", kind.file_name());
        }
    }

    #[test]
    fn wrong_schema_version_is_an_issue() {
        let dir = unique_tmp("issue-schema");
        std::fs::write(
            dir.join("rde.toml"),
            "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"T\"\n",
        )
        .unwrap();
        let art = dir.join("objects/artifacts/01M5VJ1JG0HD99FDTGJ8YEPFZX");
        std::fs::create_dir_all(&art).unwrap();
        std::fs::write(
            art.join("artifact.toml"),
            "schema = \"rde/artifact@2\"\nid = \"01M5VJ1JG0QQJM2S6R76FQCZ25\"\ntitle = \"A\"\ntype = \"data\"\n",
        )
        .unwrap();
        let project = load(&dir).unwrap();
        assert_eq!(project.object_count(), 0, "несовместимая schema не должна загружаться");
        assert!(project.issues.iter().any(|i| i.message.contains("schema")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
