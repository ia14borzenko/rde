//! Файловые операции над объектами проекта (M2.4): создание, переименование, удаление.
//!
//! Все операции меняют только файлы проекта (source of truth — файлы): создаётся каталог объекта
//! с манифестом и заготовкой содержимого; удаление — атомарное перемещение в `.rde/trash/`
//! с метаданными (без безвозвратного удаления); переименование меняет `title` в манифесте
//! верхнего уровня, не трогая `id`/каталог (id — стабильная ссылка для зависимостей).
//!
//! Документ (`type=document`) здесь не создаётся: его файл — OOXML/.doc.rde, для этого нужен
//! движок WordCraft (создание документа — M3, когда RDE станет владельцем сохранения).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::manifest::{self, ManifestKind, SCHEMA_ARTIFACT, SCHEMA_NOTE, SCHEMA_REPOSITORY, SCHEMA_SOURCE};

/// Тип нового объекта в диалоге создания.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NewObjectKind {
    ComputeArtifact,
    DataArtifact,
    ConstantsArtifact,
    Source,
    Note,
    Repository,
}

impl NewObjectKind {
    pub const ALL: [NewObjectKind; 6] = [
        NewObjectKind::ComputeArtifact,
        NewObjectKind::DataArtifact,
        NewObjectKind::ConstantsArtifact,
        NewObjectKind::Source,
        NewObjectKind::Note,
        NewObjectKind::Repository,
    ];

    pub fn label_ru(self) -> &'static str {
        match self {
            NewObjectKind::ComputeArtifact => "Артефакт (compute)",
            NewObjectKind::DataArtifact => "Артефакт (data)",
            NewObjectKind::ConstantsArtifact => "Артефакт (константы)",
            NewObjectKind::Source => "Источник",
            NewObjectKind::Note => "Заметка",
            NewObjectKind::Repository => "Репозиторий",
        }
    }

    pub fn dir_name(self) -> &'static str {
        match self {
            NewObjectKind::ComputeArtifact | NewObjectKind::DataArtifact | NewObjectKind::ConstantsArtifact => "artifacts",
            NewObjectKind::Source => "sources",
            NewObjectKind::Note => "notes",
            NewObjectKind::Repository => "repos",
        }
    }

    pub fn manifest_name(self) -> &'static str {
        match self {
            NewObjectKind::ComputeArtifact | NewObjectKind::DataArtifact | NewObjectKind::ConstantsArtifact => "artifact.toml",
            NewObjectKind::Source => "source.toml",
            NewObjectKind::Note => "note.toml",
            NewObjectKind::Repository => "repo.toml",
        }
    }
}

/// Запрос на создание объекта.
#[derive(Clone, Debug)]
pub struct CreateRequest {
    pub kind: NewObjectKind,
    pub title: String,
}

fn toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

fn new_id() -> String {
    ulid::Ulid::generate().to_string()
}

/// Создать объект проекта. Возвращает каталог объекта.
pub fn create_object(root: &Path, request: &CreateRequest) -> Result<PathBuf, String> {
    let title = request.title.trim();
    if title.is_empty() {
        return Err("название объекта не может быть пустым".to_owned());
    }
    let id = new_id();
    let dir = root.join("objects").join(request.kind.dir_name()).join(&id);
    if dir.exists() {
        return Err(format!("каталог {} уже существует", dir.display()));
    }
    fs::create_dir_all(&dir).map_err(|e| format!("создать каталог объекта: {e}"))?;
    let title_toml = toml_string(title);
    let result = match request.kind {
        NewObjectKind::ComputeArtifact => {
            write_artifact(&dir, &id, title, "compute", &format!(
                "schema = \"{SCHEMA_ARTIFACT}\"\n\
                 id = \"{id}\"\n\
                 title = {title_toml}\n\
                 type = \"compute\"\n\
                 rebuild = \"manual\"\n\
                 timeout_s = 120\n\
                 network_allow = false\n\
                 \n\
                 outputs = [\"out/result.txt\"]\n\
                 \n\
                 [runtime]\n\
                 kind = \"python\"\n\
                 env = \"project\"\n\
                 entry = \"script.py\"\n\
                 \n\
                 [[groups]]\n\
                 name = \"inputs\"\n\
                 mode = \"hashed\"\n\
                 inputs = []\n"
            ));
            fs::write(
                dir.join("script.py"),
                format!(
                    "\"\"\"Скрипт артефакта «{title}» (создан RDE).\n\n\
                     Входы из hashed-групп материализуются в ../inputs/<имя>.\n\
                     Выходы: out/result.txt — объявлены в artifact.toml.\"\"\"\n\
                     from pathlib import Path\n\n\
                     Path(\"out\").mkdir(exist_ok=True)\n\
                     Path(\"out/result.txt\").write_text(\"result: {title}\\n\", encoding=\"utf-8\")\n\
                     print(\"wrote out/result.txt\")\n"
                ),
            )
            .map_err(|e| format!("создать script.py: {e}"))?;
            Ok(())
        }
        NewObjectKind::DataArtifact => {
            write_artifact(&dir, &id, title, "data", &format!(
                "schema = \"{SCHEMA_ARTIFACT}\"\n\
                 id = \"{id}\"\n\
                 title = {title_toml}\n\
                 type = \"data\"\n\
                 \n\
                 [[groups]]\n\
                 name = \"data\"\n\
                 mode = \"hashed\"\n\
                 inputs = [\n  {{ file = \"inputs/data.csv\" }},\n]\n"
            ));
            fs::create_dir_all(dir.join("inputs")).map_err(|e| format!("создать inputs: {e}"))?;
            fs::write(dir.join("inputs/data.csv"), "x,y\n1,2\n2,4\n").map_err(|e| format!("создать data.csv: {e}"))?;
            Ok(())
        }
        NewObjectKind::ConstantsArtifact => write_artifact(&dir, &id, title, "constants", &format!(
            "schema = \"{SCHEMA_ARTIFACT}\"\n\
             id = \"{id}\"\n\
             title = {title_toml}\n\
             type = \"constants\"\n\
             \n\
             [values.R_n]\n\
             value = 1.0\n\
             unit = \"Ом\"\n\
             display = \"R_н\"\n"
        )),
        NewObjectKind::Source => {
            let manifest = format!(
                "schema = \"{SCHEMA_SOURCE}\"\n\
                 id = \"{id}\"\n\
                 title = {title_toml}\n\
                 kind = \"file\"\n\
                 file = \"source.md\"\n"
            );
            let path = dir.join("source.toml");
            fs::write(&path, manifest).map_err(|e| format!("записать source.toml: {e}"))?;
            validate_manifest(ManifestKind::Source, &path)?;
            fs::write(dir.join("source.md"), format!("# {title}\n\nДобавьте конспект источника.\n"))
                .map_err(|e| format!("создать source.md: {e}"))?;
            Ok(())
        }
        NewObjectKind::Note => {
            let manifest = format!(
                "schema = \"{SCHEMA_NOTE}\"\n\
                 id = \"{id}\"\n\
                 title = {title_toml}\n\
                 file = \"note.md\"\n"
            );
            let path = dir.join("note.toml");
            fs::write(&path, manifest).map_err(|e| format!("записать note.toml: {e}"))?;
            validate_manifest(ManifestKind::Note, &path)?;
            fs::write(dir.join("note.md"), format!("# {title}\n\n")).map_err(|e| format!("создать note.md: {e}"))?;
            Ok(())
        }
        NewObjectKind::Repository => {
            let manifest = format!(
                "schema = \"{SCHEMA_REPOSITORY}\"\n\
                 id = \"{id}\"\n\
                 title = {title_toml}\n\
                 kind = \"external\"\n\
                 path = \".\"\n"
            );
            let path = dir.join("repo.toml");
            fs::write(&path, manifest).map_err(|e| format!("записать repo.toml: {e}"))?;
            validate_manifest(ManifestKind::Repository, &path)?;
            Ok(())
        }
    };
    if let Err(error) = result {
        let _ = fs::remove_dir_all(&dir);
        return Err(error);
    }
    Ok(dir)
}

fn write_artifact(dir: &Path, id: &str, _title: &str, kind: &str, manifest: &str) -> Result<(), String> {
    debug_assert!(kind == "compute" || kind == "data" || kind == "constants");
    let path = dir.join("artifact.toml");
    fs::write(&path, manifest).map_err(|e| format!("записать artifact.toml: {e}"))?;
    validate_manifest(ManifestKind::Artifact, &path)?;
    let _ = id;
    Ok(())
}

fn validate_manifest(kind: ManifestKind, path: &Path) -> Result<(), String> {
    let text = fs::read_to_string(path).map_err(|e| format!("прочитать {}: {e}", path.display()))?;
    let value: toml::Value = toml::from_str(&text).map_err(|e| format!("TOML {}: {e}", path.display()))?;
    let errors = manifest::validate(kind, &value);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("{} не проходит JSON Schema: {}", path.display(), errors.join("; ")))
    }
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Переместить объект в `.rde/trash/` (без безвозвратного удаления) и записать метаданные.
pub fn delete_object(project: &crate::Project, id: &str) -> Result<PathBuf, String> {
    let object = project.object(id).ok_or_else(|| format!("объект {id} не найден"))?;
    let dir = object.dir.clone();
    let trash_root = project.root.join(".rde/trash");
    fs::create_dir_all(&trash_root).map_err(|e| format!("создать .rde/trash: {e}"))?;
    let stamp = now_unix();
    let target = trash_root.join(format!("{stamp}-{id}"));
    fs::rename(&dir, &target).map_err(|e| format!("переместить в корзину: {e}"))?;
    let meta = serde_json::json!({
        "id": id,
        "title": object.title,
        "kind": format!("{:?}", object.kind),
        "deleted_unix": stamp,
        "original": dir.display().to_string(),
    });
    let meta_path = trash_root.join(format!("{stamp}-{id}.meta.json"));
    let _ = fs::write(meta_path, serde_json::to_vec_pretty(&meta).unwrap_or_default());
    Ok(target)
}

/// Переименовать объект: меняет только `title` верхнего уровня в манифесте (id/каталог не меняются).
pub fn rename_object(project: &crate::Project, id: &str, new_title: &str) -> Result<(), String> {
    let new_title = new_title.trim();
    if new_title.is_empty() {
        return Err("название не может быть пустым".to_owned());
    }
    let object = project.object(id).ok_or_else(|| format!("объект {id} не найден"))?;
    let manifest_name = match &object.data {
        crate::ObjectData::Document(_) => "document.toml",
        crate::ObjectData::Artifact(_) => "artifact.toml",
        crate::ObjectData::Source(_) => "source.toml",
        crate::ObjectData::Note(_) => "note.toml",
        crate::ObjectData::Repository(_) => "repo.toml",
    };
    let path = object.dir.join(manifest_name);
    let text = fs::read_to_string(&path).map_err(|e| format!("прочитать {}: {e}", path.display()))?;
    let new_line = format!("title = {}", toml_string(new_title));
    let mut replaced = false;
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        // title верхнего уровня — без отступов; вложенные `[reference] title = ...` не трогаем.
        if !replaced && line.starts_with("title") && line.trim_start().starts_with("title =") && !line.starts_with(' ') {
            lines.push(new_line.clone());
            replaced = true;
        } else {
            lines.push(line.to_owned());
        }
    }
    if !replaced {
        return Err(format!("в {} нет поля title верхнего уровня", path.display()));
    }
    let mut out = lines.join("\n");
    out.push('\n');
    fs::write(&path, out).map_err(|e| format!("записать {}: {e}", path.display()))?;
    let kind = match &object.data {
        crate::ObjectData::Document(_) => ManifestKind::Document,
        crate::ObjectData::Artifact(_) => ManifestKind::Artifact,
        crate::ObjectData::Source(_) => ManifestKind::Source,
        crate::ObjectData::Note(_) => ManifestKind::Note,
        crate::ObjectData::Repository(_) => ManifestKind::Repository,
    };
    validate_manifest(kind, &path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::load_project;

    fn minimal_project(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rde-core-ops-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("rde.toml"),
            "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"Тест операций\"\n",
        )
        .unwrap();
        root
    }

    #[test]
    fn create_compute_artifact_loads_and_is_stale() {
        let root = minimal_project("compute");
        let dir = create_object(&root, &CreateRequest { kind: NewObjectKind::ComputeArtifact, title: "Новый расчёт".into() }).unwrap();
        assert!(dir.join("artifact.toml").is_file());
        assert!(dir.join("script.py").is_file());
        let project = load_project(&root).unwrap();
        let object = project.objects.iter().find(|o| o.title == "Новый расчёт").expect("объект должен загрузиться");
        assert!(matches!(object.freshness, crate::Freshness::Stale | crate::Freshness::Failed));
        assert_eq!(project.issues.len(), 0, "issues: {:?}", project.issues);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn create_all_kinds_roundtrip() {
        let root = minimal_project("all");
        for kind in NewObjectKind::ALL {
            let dir = create_object(&root, &CreateRequest { kind, title: format!("Объект {}", kind.label_ru()) }).unwrap();
            assert!(dir.is_dir(), "{kind:?}");
        }
        let project = load_project(&root).unwrap();
        assert_eq!(project.objects.len(), NewObjectKind::ALL.len());
        assert!(project.issues.is_empty(), "{:?}", project.issues);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn rename_changes_title_and_keeps_id() {
        let root = minimal_project("rename");
        let dir = create_object(&root, &CreateRequest { kind: NewObjectKind::Note, title: "Старое".into() }).unwrap();
        let project = load_project(&root).unwrap();
        let id = project.objects[0].id.clone();
        rename_object(&project, &id, "Новое название").unwrap();
        let project = load_project(&root).unwrap();
        assert_eq!(project.objects[0].title, "Новое название");
        assert_eq!(project.objects[0].id, id);
        assert_eq!(project.objects[0].dir, dir);
        assert!(rename_object(&project, &id, "  ").is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_moves_object_to_trash_with_metadata() {
        let root = minimal_project("delete");
        let dir = create_object(&root, &CreateRequest { kind: NewObjectKind::Source, title: "Удаляемый".into() }).unwrap();
        let project = load_project(&root).unwrap();
        let id = project.objects[0].id.clone();
        let trash = delete_object(&project, &id).unwrap();
        assert!(trash.is_dir());
        assert!(!dir.exists());
        assert!(root.join(format!(".rde/trash/{}-{id}.meta.json", trash.file_name().unwrap().to_string_lossy().split('-').next().unwrap())).exists() || trash.parent().unwrap().read_dir().unwrap().count() >= 2);
        let project = load_project(&root).unwrap();
        assert!(project.objects.is_empty());
        let _ = fs::remove_dir_all(&root);
    }
}
