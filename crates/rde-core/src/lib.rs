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
pub mod selector;
pub mod settings;

pub use manifest::{
    ArtifactManifest, AuthorManifest, ConstantValue, DocumentManifest, GroupManifest, InputSpec,
    ManifestKind, NoteManifest, ProjectManifest, ReferenceManifest, RepositoryManifest,
    RuntimeManifest, SourceManifest,
};
pub use ops::{
    CreateRequest, NewObjectKind, clear_artifact_out_hash, create_document_stub, create_object,
    create_project, delete_object, record_artifact_out_hash, rename_object, set_project_authors,
    set_project_trusted, write_artifact_manifest,
};
pub use project::{
    Freshness, Group, Issue, Level, LogLine, Object, ObjectData, ObjectKind, Project, load_project,
};
pub use selector::{SelectorMatch, expand_selector, glob_match, is_glob};
pub use settings::{AppSettings, config_dir, load_from as load_app_settings, save_to as save_app_settings};

/// Новый ULID для объектов проекта (единая точка генерации id).
pub fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

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

    // ── §7.3: ref-рёбра, циклы, константы ────────────────────────────────────

    fn write_project_with_artifacts(name: &str, manifests: &[(&str, &str)]) -> PathBuf {
        let dir = unique_tmp(name);
        std::fs::write(
            dir.join("rde.toml"),
            "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"T\"\n",
        )
        .unwrap();
        for (id, manifest) in manifests {
            let art = dir.join("objects/artifacts").join(id);
            std::fs::create_dir_all(&art).unwrap();
            std::fs::write(art.join("artifact.toml"), manifest).unwrap();
        }
        dir
    }

    #[test]
    fn ref_groups_do_not_create_dag_edges() {
        let a = "01M5VJ1JG0HD99FDTGJ8YEPFZX";
        let b = "01M5VJ1JG0QQJM2S6R76FQCZ25";
        let dir = write_project_with_artifacts(
            "ref-edges",
            &[
                (a, &format!("schema=\"rde/artifact@1\"\nid=\"{a}\"\ntitle=\"A\"\ntype=\"data\"\n")),
                (
                    b,
                    &format!(
                        "schema=\"rde/artifact@1\"\nid=\"{b}\"\ntitle=\"B\"\ntype=\"compute\"\n\
                         outputs=[\"out/b.txt\"]\n[runtime]\nkind=\"python\"\nenv=\"project\"\nentry=\"s.py\"\n\
                         [[groups]]\nname=\"link\"\nmode=\"ref\"\ninputs=[{{ artifact=\"{a}\", selector=\"inputs/x.csv\" }}]\n"
                    ),
                ),
            ],
        );
        let project = load(&dir).unwrap();
        assert!(project.issues.is_empty(), "{:?}", project.issues);
        assert!(project.dag_edges().is_empty(), "ref-группа не должна давать hashed-ребро");
        assert!(project.dag_cycle().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hashed_cycle_is_detected_with_chain() {
        let a = "01M5VJ1JG0HD99FDTGJ8YEPFZX";
        let b = "01M5VJ1JG0QQJM2S6R76FQCZ25";
        let manifest = |id: &str, title: &str, dep: &str| {
            format!(
                "schema=\"rde/artifact@1\"\nid=\"{id}\"\ntitle=\"{title}\"\ntype=\"compute\"\n\
                 outputs=[\"out/x.txt\"]\n[runtime]\nkind=\"python\"\nenv=\"project\"\nentry=\"s.py\"\n\
                 [[groups]]\nname=\"g\"\nmode=\"hashed\"\ninputs=[{{ artifact=\"{dep}\", selector=\"out/x.txt\" }}]\n"
            )
        };
        let dir = write_project_with_artifacts(
            "cycle",
            &[(a, &manifest(a, "A", b)), (b, &manifest(b, "B", a))],
        );
        let project = load(&dir).unwrap();
        assert!(
            project.issues.iter().any(|issue| issue.message.contains("цикл")),
            "цикл должен попасть в issues: {:?}",
            project.issues
        );
        let cycle = project.dag_cycle().expect("dag_cycle должен найти цикл");
        assert!(cycle.len() >= 3, "цепочка цикла: {cycle:?}");
        assert_eq!(cycle.first(), cycle.last(), "цикл замкнут");
        assert!(project.dag_cycle_titles().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn constants_inputs_parse_and_validate() {
        let constants = "01M5VJ1JG0T9NS5SEZX35HGNFY";
        let compute = "01M5VJ1JG039MMV97SZGY6V9EQ";
        let dir = write_project_with_artifacts(
            "constants",
            &[
                (
                    constants,
                    &format!(
                        "schema=\"rde/artifact@1\"\nid=\"{constants}\"\ntitle=\"K\"\ntype=\"constants\"\n\
                         [values.R_n]\nvalue=1.0\nunit=\"Ом\"\n"
                    ),
                ),
                (
                    compute,
                    &format!(
                        "schema=\"rde/artifact@1\"\nid=\"{compute}\"\ntitle=\"C\"\ntype=\"compute\"\n\
                         outputs=[\"out/c.txt\"]\n[runtime]\nkind=\"python\"\nenv=\"project\"\nentry=\"s.py\"\n\
                         [[groups]]\nname=\"consts\"\nmode=\"hashed\"\n\
                         inputs=[{{ constants=\"{constants}\", selector=\"R_n\" }}]\n"
                    ),
                ),
            ],
        );
        let project = load(&dir).unwrap();
        assert!(project.issues.is_empty(), "{:?}", project.issues);
        let object = project.artifact_object(compute).unwrap();
        let ObjectData::Artifact(manifest) = &object.data else { panic!() };
        let inputs: Vec<&InputSpec> = manifest.hashed_inputs().map(|(_, input)| input).collect();
        assert_eq!(inputs.len(), 1);
        assert!(matches!(inputs[0], InputSpec::Constants { selector, .. } if selector == "R_n"));
        assert_eq!(manifest.hashed_inputs().next().unwrap().0.name, "consts");
        // Нет файла-входа — но константы есть, поэтому вход присутствует.
        assert_eq!(object.freshness, Freshness::Stale, "выходов нет — устарел");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
