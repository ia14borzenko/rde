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
pub mod value_format;
/// M4.4: контекст-ссылки Cursor-like (§8.7.2 ТЗ, R-AGT-03) — URI-схема `rde://…`.
pub mod context_ref;

pub use manifest::{
    ArtifactManifest, AuthorManifest, ConstantSource, ConstantValue, DocumentManifest, FormulaFragment,
    FormulaInput, FormulaInputSource, FormulaManifest, FormulaNumbering, FormulaRef, FormulaState,
    GroupManifest, InputSpec, ManifestKind, NoteManifest, ProjectManifest, ReferenceManifest,
    RepositoryManifest, RuntimeManifest, SourceManifest, parse_formula_input_source, parse_formula_ref,
};
pub use ops::{
    CreateRequest, NewObjectKind, clear_artifact_out_hash, create_document_stub, create_formula,
    create_formula_with_display,
    create_object, create_project, delete_object, record_artifact_out_hash, record_formula_state,
    rename_object, set_project_authors, backup_project, list_project_backups, restore_project,
    set_project_trusted, write_artifact_manifest, set_document_formula_numbering,
};
pub use project::{
    Freshness, Group, Issue, Level, LogLine, Object, ObjectData, ObjectKind, Project, load_project,
};
pub use selector::{SelectorMatch, expand_selector, glob_match, is_glob};
pub use settings::{
    AppSettings, RecentProject, config_dir, last_project_path, load_from as load_app_settings,
    load_recent_projects, load_recent_projects_from, record_recent_project,
    record_recent_project_in, recent_projects_path, save_to as save_app_settings,
};

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

    pub(crate) fn unique_tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-core-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    pub(crate) fn copy_demo_without_generated_state(target: &std::path::Path) {
        fn copy(src: &std::path::Path, dst: &std::path::Path) {
            std::fs::create_dir_all(dst).unwrap();
            for entry in std::fs::read_dir(src).unwrap().flatten() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if matches!(&*name, ".rde" | "exports" | "__pycache__" | "out" | ".venv") {
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
            12,
            "демо-проект: 1 документ + 5 артефактов + 4 источника + 1 заметка + 1 репозиторий + 1 формула (P3)"
        );
        // P3: формула σ из документа — project-level объект (ADR-0005), а не содержимое документа.
        assert_eq!(project.formulas().count(), 1);
        assert!(project.formulas().next().is_some_and(|formula| formula.latex.contains("sigma")));
        // compute-артефакты без выходов честно «устарели»; данные/источники/заметки — актуальны/внешние.
        assert_eq!(project.stale_count(), 3);
        assert!(project.document_path().is_some_and(|p| p.is_file()));
        assert_eq!(project.artifacts().count(), 5);
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
        // В демо теперь несколько рисунков (миграция + гистограмма + линия).
        assert!(project.figures() >= 2, "рисунков: {}", project.figures());
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

#[cfg(test)]
mod backup_tests {
    use super::*;
    use crate::tests::{copy_demo_without_generated_state, unique_tmp};

    #[test]
    fn project_backup_zip_roundtrip() {
        let dir = unique_tmp("backup");
        copy_demo_without_generated_state(&dir);
        let archive = backup_project(&dir).expect("бэкап проекта");
        assert!(archive.is_file(), "архив создан: {}", archive.display());
        assert!(list_project_backups(&dir).unwrap().contains(&archive));
        // Меняем файл, затем восстанавливаем из бэкапа.
        let manifest = dir.join("rde.toml");
        let original = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(&manifest, "schema = \"broken\"\n").unwrap();
        let restored = restore_project(&dir, &archive).expect("восстановление");
        assert!(restored > 0);
        assert_eq!(std::fs::read_to_string(&manifest).unwrap(), original, "rde.toml восстановлен");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod constant_value_tests {
    use super::*;
    use crate::tests::unique_tmp;

    /// P1 (ADR-0005 §1.3): сериализация литеральной константы не изменилась — хеши `act_key`
    /// зависимых артефактов остаются прежними (новые поля не пишутся, пока пусты).
    #[test]
    fn legacy_constant_serialization_unchanged() {
        let value: ConstantValue = toml::from_str(
            "value = 12.4\nunit = \"Ом\"\ndisplay = \"R_н\"\nstyle = \"italic-subscript\"\n",
        )
        .expect("литеральная константа");
        let json = serde_json::to_value(&value).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"value": 12.4, "unit": "Ом", "display": "R_н", "style": "italic-subscript"}),
            "новые поля не должны попадать в JSON/HJCS литеральной константы"
        );
        // Обратная совместимость: непустые новые поля появляются, пустые — нет.
        let extended: ConstantValue = toml::from_str(
            "value = 1\nsymbol = \"R\"\nsubscript = \"н\"\nformat = \"dec:2\"\nsource = \"rde://artifact/01M5VJ1JG0HD99FDTGJ8YEPFZX/out/x.json#/v\"\n",
        )
        .unwrap();
        let json = serde_json::to_value(&extended).unwrap();
        assert_eq!(json["symbol"], "R");
        assert_eq!(json["subscript"], "н");
        assert_eq!(json["format"], "dec:2");
        assert_eq!(json["source"], "rde://artifact/01M5VJ1JG0HD99FDTGJ8YEPFZX/out/x.json#/v");
        assert!(json.get("value").is_some());
    }

    #[test]
    fn constant_source_uri_parsing() {
        let value: ConstantValue = toml::from_str(
            "value = 1\nsource = \"rde://artifact/01M5VJ1JG039MMV97SZGY6V9EQ@dd7e3c10/out/stats.json#/stats/x1/mean\"\n",
        )
        .unwrap();
        let source = value.source_parsed().unwrap().expect("source разобран");
        assert_eq!(source.artifact_id, "01M5VJ1JG039MMV97SZGY6V9EQ");
        assert_eq!(source.out_hash.as_deref(), Some("dd7e3c10"));
        assert_eq!(source.file, "out/stats.json");
        assert_eq!(source.pointer.as_deref(), Some("/stats/x1/mean"));
        // Без @out_hash и без pointer.
        let plain: ConstantValue =
            toml::from_str("value = 1\nsource = \"rde://artifact/01M5VJ1JG039MMV97SZGY6V9EQ/out/x.txt\"\n").unwrap();
        let source = plain.source_parsed().unwrap().unwrap();
        assert!(source.out_hash.is_none());
        assert!(source.pointer.is_none());
        // Ошибки: чужой scheme / неполный URI.
        for bad in ["constants://x", "rde://constants/01M5/R_n", "rde://artifact/", "rde://artifact/01M5/"] {
            let value: ConstantValue = toml::from_str(&format!("value = 1\nsource = \"{bad}\"\n")).unwrap();
            assert!(value.source_parsed().is_err(), "должен быть отказ: {bad}");
        }
    }

    /// P1: проблемы расширенных констант — `issues` загрузки (не фатальны), а хорошая константа
    /// с `source` на существующий артефакт грузится без issues.
    #[test]
    fn load_reports_bad_constant_sources() {
        let dir = unique_tmp("constants-p1");
        std::fs::write(
            dir.join("rde.toml"),
            "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"T\"\n",
        )
        .unwrap();
        let constants = "01M5VJ1JG0T9NS5SEZX35HGNFY";
        let source_art = "01M5VJ1JG039MMV97SZGY6V9EQ";
        let dirs = [
            (format!("objects/artifacts/{constants}"), "constants"),
            (format!("objects/artifacts/{source_art}"), "data"),
        ];
        for (path, kind) in dirs {
            std::fs::create_dir_all(dir.join(&path)).unwrap();
            let manifest = if kind == "constants" {
                format!(
                    "schema=\"rde/artifact@1\"\nid=\"{constants}\"\ntitle=\"К\"\ntype=\"constants\"\n\
                     [values.R_n]\nvalue=12.4\nunit=\"Ом\"\ndisplay=\"R_н\"\nstyle=\"italic-subscript\"\n\
                     symbol=\"R\"\nsubscript=\"н\"\nformat=\"dec:1\"\n\
                     source=\"rde://artifact/{source_art}/out/stats.json#/v\"\n"
                )
            } else {
                format!(
                    "schema=\"rde/artifact@1\"\nid=\"{source_art}\"\ntitle=\"Д\"\ntype=\"data\"\n\
                     [[groups]]\nname=\"data\"\nmode=\"hashed\"\ninputs=[{{ file=\"inputs/data.csv\" }}]\n"
                )
            };
            std::fs::write(dir.join(&path).join("artifact.toml"), manifest).unwrap();
        }
        std::fs::create_dir_all(dir.join(format!("objects/artifacts/{source_art}/inputs"))).unwrap();
        std::fs::write(dir.join(format!("objects/artifacts/{source_art}/inputs/data.csv")), "x\n1\n").unwrap();
        let project = load(&dir).unwrap();
        assert!(project.issues.is_empty(), "issues: {:?}", project.issues);
        let object = project.artifact_object(constants).unwrap();
        let ObjectData::Artifact(manifest) = &object.data else { panic!() };
        let value = manifest.values.get("R_n").unwrap();
        assert_eq!(value.symbol, "R");
        assert_eq!(value.document_run_style(), "italic-subscript");
        assert_eq!(value.document_text(), "R_н");

        // Плохие источники/поля → issues (загрузка продолжается).
        std::fs::write(
            dir.join(format!("objects/artifacts/{constants}/artifact.toml")),
            format!(
                "schema=\"rde/artifact@1\"\nid=\"{constants}\"\ntitle=\"К\"\ntype=\"constants\"\n\
                 [values.A]\nvalue=1\nsymbol=\"R н\"\nformat=\"nonsense\"\n\
                 source=\"rde://artifact/01M5VJ1JG0ZZZZZZZZZZZZZZZZ/out/x.json\"\n\
                 [values.B]\nvalue=2\nsource=\"вне-URI\"\n"
            ),
        )
        .unwrap();
        let project = load(&dir).unwrap();
        let messages: Vec<&str> = project.issues.iter().map(|issue| issue.message.as_str()).collect();
        assert!(messages.iter().any(|m| m.contains("symbol содержит пробелы")), "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("неизвестный format")), "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("не найден")), "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("rde://artifact/")), "{messages:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod formula_model_tests {
    use super::*;
    use crate::tests::unique_tmp;

    const FORM_A: &str = "01M5VJ1JG0HD99FDTGJ8YEPFZX";
    const FORM_B: &str = "01M5VJ1JG039MMV97SZGY6V9EQ";

    fn project_with(dir: &std::path::Path, formulas: &[(&str, String)]) {
        std::fs::write(
            dir.join("rde.toml"),
            "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"T\"\n",
        )
        .unwrap();
        for (id, body) in formulas {
            let formula_dir = dir.join("objects/formulas").join(id);
            std::fs::create_dir_all(&formula_dir).unwrap();
            std::fs::write(formula_dir.join("formula.toml"), body).unwrap();
        }
    }

    fn formula_toml(id: &str, title: &str, latex: &str, extra: &str) -> String {
        format!(
            "schema = \"rde/formula@1\"\nid = \"{id}\"\ntitle = \"{title}\"\nlatex = \"{latex}\"\ndisplay = true\n\n[numbering]\nmode = \"auto\"\nlabel = \"\"\n{extra}"
        )
    }

    /// P2 (ADR-0005 §2): project-level формула грузится из `objects/formulas/`, граф фрагментов и
    /// топопорядок корректны; цикл — issue загрузки (не паника).
    #[test]
    fn formulas_load_graph_edges_and_cycle() {
        let dir = unique_tmp("formula-graph");
        let a = formula_toml(FORM_A, "A", "a+b", "");
        // В TOML обратный слэш LaTeX экранируется: в файле `\\rdefragment{a}` = LaTeX `\rdefragment{a}`.
        let b = formula_toml(
            FORM_B,
            "B",
            "\\\\rdefragment{a}",
            &format!("\n[[fragments]]\nalias = \"a\"\nsource = \"rde://formula/{FORM_A}\"\n"),
        );
        project_with(&dir, &[(FORM_A, a.clone()), (FORM_B, b.clone())]);
        let project = load(&dir).unwrap();
        assert!(project.issues.is_empty(), "issues: {:?}", project.issues);
        assert_eq!(project.formulas().count(), 2);
        assert!(project.groups.iter().any(|group| group.kind == ObjectKind::Formula && group.objects.len() == 2));
        assert!(project.formula_object(FORM_A).is_some());
        assert_eq!(project.formula_edges(), vec![(FORM_A.to_owned(), FORM_B.to_owned())]);
        assert!(project.formula_cycle().is_none());
        // Цикл A↔B — issue с цепочкой.
        let a_with_fragment = formula_toml(FORM_A, "A", "a+b", &format!("\n[[fragments]]\nalias = \"b\"\nsource = \"rde://formula/{FORM_B}\"\n"));
        project_with(&dir, &[(FORM_A, a_with_fragment), (FORM_B, b)]);
        let project = load(&dir).unwrap();
        assert!(
            project.issues.iter().any(|issue| issue.message.contains("цикл по фрагментам формул")),
            "issues: {:?}",
            project.issues
        );
        assert!(project.formula_cycle().is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// P2: невалидная разметка/ссылки формул — `issues` (загрузка не фатальна); `create_formula`
    /// пишет валидный `formula.toml` и объект грузится.
    #[test]
    fn formula_validation_and_create() {
        let dir = unique_tmp("formula-validate");
        std::fs::create_dir_all(&dir).unwrap();
        create_project(&dir, "Формулы").unwrap();
        let created = create_formula(&dir, "Закон Ома", "\\sigma=\\frac{F}{A}").unwrap();
        assert!(created.join("formula.toml").is_file());
        let project = load(&dir).unwrap();
        assert!(project.issues.is_empty(), "{:?}", project.issues);
        assert_eq!(project.formulas().count(), 1);

        // Плохая формула: пустой latex, mode, вход с чужим URI, фрагмент на несуществующую формулу.
        let bad_id = FORM_A;
        std::fs::create_dir_all(dir.join("objects/formulas").join(bad_id)).unwrap();
        let bad = formula_toml(bad_id, "B", " ", "\n[[inputs]]\nalias = \"x\"\nsource = \"artifact://bad\"\n\n[[fragments]]\nalias = \"f\"\nsource = \"rde://formula/01M5VJ1JG0ZZZZZZZZZZZZZZZZ\"\n");
        std::fs::write(dir.join("objects/formulas").join(bad_id).join("formula.toml"), bad).unwrap();
        let project = load(&dir).unwrap();
        let messages: Vec<&str> = project.issues.iter().map(|issue| issue.message.as_str()).collect();
        assert!(messages.iter().any(|m| m.contains("latex не может быть пустым")), "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("rde://constants/") || m.contains("rde://artifact/")), "{messages:?}");
        assert!(messages.iter().any(|m| m.contains("не найдена")), "{messages:?}");
        // Цикл-детект не мешает загрузке; formula_cycle — None (нет рёбер на существующие).
        assert!(project.formula_cycle().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
