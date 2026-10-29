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
            ))?;
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
            ))?;
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
    // R-ART-01: у артефакта есть readme и собственный .gitignore (выходы не коммитятся).
    if matches!(
        request.kind,
        NewObjectKind::ComputeArtifact | NewObjectKind::DataArtifact | NewObjectKind::ConstantsArtifact
    ) {
        if let Err(error) = write_artifact_scaffold(&dir, request.kind, title) {
            let _ = fs::remove_dir_all(&dir);
            return Err(error);
        }
    }
    Ok(dir)
}

/// readme.md + `.gitignore` (+ `out/.gitignore`) артефакта: R-ART-01, R-VCS-02.
fn write_artifact_scaffold(dir: &Path, kind: NewObjectKind, title: &str) -> Result<(), String> {
    let (type_line, run_line) = match kind {
        NewObjectKind::ComputeArtifact => (
            "compute — скрипт, запускаемый RDE; выходы объявлены в artifact.toml",
            "Входы hashed-групп материализуются в `inputs/`, скрипт запускается в изолированном рабочем каталоге; выходы объявляются в `artifact.toml`.",
        ),
        NewObjectKind::DataArtifact => (
            "data — статические входные данные проекта",
            "Файлы данных лежат в `inputs/` и входят в `in_hash` как hashed-группа.",
        ),
        _ => (
            "constants — типизированные значения проекта (без скрипта)",
            "Значения задаются в `artifact.toml` (`[values.<имя>]`) и доступны документам и другим артефактам как вход `{ constants = \"<id>\" }`.",
        ),
    };
    let readme = format!(
        "# {title}\n\nТип: {type_line}.\n\n{run_line}\n\nМанифест: `artifact.toml`.\n"
    );
    fs::write(dir.join("readme.md"), readme).map_err(|e| format!("создать readme.md: {e}"))?;
    // Выходы не коммитятся: их идентичность фиксирует out_hash в artifact.toml (R-VCS-02).
    let gitignore = "out/*\n!out/.gitignore\n__pycache__/\n*.pyc\n.rde-tmp*\n";
    fs::write(dir.join(".gitignore"), gitignore).map_err(|e| format!("создать .gitignore: {e}"))?;
    if matches!(kind, NewObjectKind::ComputeArtifact) {
        fs::create_dir_all(dir.join("out")).map_err(|e| format!("создать out/: {e}"))?;
        fs::write(dir.join("out/.gitignore"), "").map_err(|e| format!("создать out/.gitignore: {e}"))?;
    }
    Ok(())
}

/// Перезаписать `artifact.toml` из типизированной модели (редактор артефакта в UI).
/// Комментарии/порядок при этом не сохраняются (полноценный `toml_edit` — v0.2), но схема
/// валидируется; скрипты/выходы при сохранении манифеста не трогаются.
pub fn write_artifact_manifest(root: &Path, manifest: &crate::manifest::ArtifactManifest) -> Result<(), String> {
    let dir = root.join("objects/artifacts").join(&manifest.id);
    if !dir.is_dir() {
        return Err(format!("каталог артефакта {} не найден", dir.display()));
    }
    let text = toml::to_string_pretty(manifest).map_err(|e| format!("artifact.toml: {e}"))?;
    let path = dir.join("artifact.toml");
    fs::write(&path, text).map_err(|e| format!("записать {}: {e}", path.display()))?;
    validate_manifest(ManifestKind::Artifact, &path)
}

/// Записать список авторов проекта в `rde.toml` (R-SET-01, R-UI-06): заменяет `[[authors]]`-блоки,
/// остальной файл не трогает. Пустой список удаляет авторов.
pub fn set_project_authors(root: &Path, authors: &[crate::manifest::AuthorManifest]) -> Result<(), String> {
    let path = root.join("rde.toml");
    let text = fs::read_to_string(&path).map_err(|e| format!("прочитать {}: {e}", path.display()))?;
    // Удаляем все существующие [[authors]]-секции (до следующего заголовка таблицы).
    let mut kept: Vec<String> = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("[[authors]]") {
            skipping = true;
            continue;
        }
        if skipping {
            let is_table = trimmed.starts_with('[') && !trimmed.starts_with("[[authors]]");
            if is_table {
                skipping = false;
            } else {
                continue;
            }
        }
        kept.push(line.to_owned());
    }
    // Убираем возможные пустые хвостовые строки перед добавлением.
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    out.push('\n');
    for author in authors {
        out.push_str(&format!(
            "\n[[authors]]\nid = {}\nname = {}\ninitials = {}\nemail = {}\nkind = {}\n",
            toml_string(&author.id),
            toml_string(&author.name),
            toml_string(&author.initials),
            toml_string(&author.email),
            toml_string(&author.kind),
        ));
    }
    fs::write(&path, out).map_err(|e| format!("записать {}: {e}", path.display()))?;
    let text = fs::read_to_string(&path).map_err(|e| format!("прочитать {}: {e}", path.display()))?;
    let value: toml::Value = toml::from_str(&text).map_err(|e| format!("TOML {}: {e}", path.display()))?;
    let errors = manifest::validate(ManifestKind::Project, &value);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("{} не проходит JSON Schema: {}", path.display(), errors.join("; ")))
    }
}

/// Записать `trusted` в `rde.toml`, не трогая остальной файл (R-RUN-06): согласие пользователя
/// на запуск скриптов проекта. Ключ верхнего уровня — вставляется перед первой таблицей.
/// Каталог бэкапов проекта: `.rde/backups/`.
pub fn project_backups_dir(root: &Path) -> PathBuf {
    root.join(".rde").join("backups")
}

/// Сделать zip-бэкап проекта (R-VCS-04/R-SET-06): все файлы проекта, кроме служебных
/// (`.rde`, `exports`, `.git`, `__pycache__`, `.venv`, `node_modules`).
/// Возвращает путь к архиву.
pub fn backup_project(root: &Path) -> Result<PathBuf, String> {
    let dir = project_backups_dir(root);
    fs::create_dir_all(&dir).map_err(|e| format!("создать {}: {e}", dir.display()))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let mut out = dir.join(format!("project-{stamp}.zip"));
    let mut suffix = 1u32;
    while out.exists() {
        out = dir.join(format!("project-{stamp}-{suffix}.zip"));
        suffix += 1;
    }
    let file = fs::File::create(&out).map_err(|e| format!("создать {}: {e}", out.display()))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = fs::read_dir(&current).map_err(|e| format!("прочитать {}: {e}", current.display()))?;
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if matches!(name.as_str(), ".rde" | "exports" | ".git" | "__pycache__" | ".venv" | "node_modules") {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path.strip_prefix(root).map_err(|e| e.to_string())?.to_string_lossy().replace('\\', "/");
            zip.start_file(relative, options).map_err(|e| format!("zip {}: {e}", path.display()))?;
            let mut source = fs::File::open(&path).map_err(|e| format!("открыть {}: {e}", path.display()))?;
            std::io::copy(&mut source, &mut zip).map_err(|e| format!("zip {}: {e}", path.display()))?;
        }
    }
    zip.finish().map_err(|e| format!("завершить zip: {e}"))?;
    // Ротация: держим последние 10 бэкапов (R-VCS-04).
    let mut backups = list_project_backups(root)?;
    backups.sort();
    while backups.len() > 10 {
        let oldest = backups.remove(0);
        let _ = fs::remove_file(oldest);
    }
    Ok(out)
}

/// Список бэкапов проекта (пути, от старых к новым).
pub fn list_project_backups(root: &Path) -> Result<Vec<PathBuf>, String> {
    let dir = project_backups_dir(root);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut out: Vec<PathBuf> = fs::read_dir(&dir)
        .map_err(|e| format!("прочитать {}: {e}", dir.display()))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("zip"))
        .collect();
    out.sort();
    Ok(out)
}

/// Восстановить проект из zip-бэкапа: сначала бэкап текущего состояния, затем распаковка
/// (служебные каталоги `.rde`/`exports` не перезаписываются).
pub fn restore_project(root: &Path, archive: &Path) -> Result<usize, String> {
    backup_project(root)?;
    let file = fs::File::open(archive).map_err(|e| format!("открыть {}: {e}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| format!("прочитать zip: {e}"))?;
    let mut restored = 0usize;
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|e| format!("zip-запись: {e}"))?;
        let Some(relative) = entry.enclosed_name() else { continue };
        if relative.components().any(|component| {
            matches!(component.as_os_str().to_str(), Some(".rde") | Some("exports") | Some(".git"))
        }) {
            continue;
        }
        let target = root.join(&relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("создать {}: {e}", parent.display()))?;
        }
        let mut out = fs::File::create(&target).map_err(|e| format!("записать {}: {e}", target.display()))?;
        std::io::copy(&mut entry, &mut out).map_err(|e| format!("распаковать {}: {e}", target.display()))?;
        restored += 1;
    }
    Ok(restored)
}

pub fn set_project_trusted(root: &Path, trusted: bool) -> Result<(), String> {
    let path = root.join("rde.toml");
    let text = fs::read_to_string(&path).map_err(|e| format!("прочитать {}: {e}", path.display()))?;
    let new_line = format!("trusted = {trusted}");
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    let mut inserted = false;
    for line in text.lines() {
        let top_level = !line.starts_with(' ') && !line.starts_with('\t');
        let key = line.split('=').next().map(str::trim).unwrap_or("");
        if top_level && key == "trusted" {
            if !replaced {
                lines.push(new_line.clone());
                replaced = true;
            }
            continue;
        }
        if !replaced && !inserted && top_level && line.starts_with('[') {
            lines.push(new_line.clone());
            inserted = true;
        }
        lines.push(line.to_owned());
    }
    if !replaced && !inserted {
        lines.push(new_line);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    fs::write(&path, out).map_err(|e| format!("записать {}: {e}", path.display()))?;
    let text = fs::read_to_string(&path).map_err(|e| format!("прочитать {}: {e}", path.display()))?;
    let value: toml::Value = toml::from_str(&text).map_err(|e| format!("TOML {}: {e}", path.display()))?;
    let errors = manifest::validate(ManifestKind::Project, &value);
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("{} не проходит JSON Schema: {}", path.display(), errors.join("; ")))
    }
}

/// Записать `[state]` формулы (кэш актуальности, ADR-0005 P2) в `formula.toml`, не трогая
/// остальной файл: заменяет существующую таблицу `[state]` целиком; валидирует JSON Schema.
/// Источник истины актуальности — ledger (`rde-dag`), `[state]` — человекочитаемое зеркало
/// (прецедент `out_hash` у артефактов).
pub fn record_formula_state(
    formula_dir: &Path,
    act_key: &str,
    resolved_latex_hash: &str,
    omml_hash: &str,
    svg_hash: Option<&str>,
    png_hash: Option<&str>,
    computed_at: u64,
) -> Result<(), String> {
    let path = formula_dir.join("formula.toml");
    let text = fs::read_to_string(&path).map_err(|e| format!("прочитать {}: {e}", path.display()))?;
    let mut kept: Vec<String> = Vec::new();
    let mut skipping = false;
    for line in text.lines() {
        let top_level = !line.starts_with(' ') && !line.starts_with('\t');
        if top_level && line.starts_with('[') {
            skipping = line.trim() == "[state]";
            if skipping {
                continue;
            }
        }
        if skipping {
            continue;
        }
        kept.push(line.to_owned());
    }
    while kept.last().is_some_and(|line| line.trim().is_empty()) {
        kept.pop();
    }
    let mut out = kept.join("\n");
    out.push_str(&format!(
        "\n\n[state]\nact_key = {}\nresolved_latex_hash = {}\nomml_hash = {}\n",
        toml_string(act_key),
        toml_string(resolved_latex_hash),
        toml_string(omml_hash)
    ));
    if let Some(svg) = svg_hash {
        out.push_str(&format!("svg_hash = {}\n", toml_string(svg)));
    }
    if let Some(png) = png_hash {
        out.push_str(&format!("png_hash = {}\n", toml_string(png)));
    }
    out.push_str(&format!("computed_at = {computed_at}\n"));
    fs::write(&path, out).map_err(|e| format!("записать {}: {e}", path.display()))?;
    validate_manifest(ManifestKind::Formula, &path)
}

/// Убрать `out_hash` из `artifact.toml` (откат к состоянию «артефакт ещё не запускался»).
pub fn clear_artifact_out_hash(artifact_dir: &Path) -> Result<(), String> {
    let path = artifact_dir.join("artifact.toml");
    let text = fs::read_to_string(&path).map_err(|e| format!("прочитать {}: {e}", path.display()))?;
    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        let top_level = !line.starts_with(' ') && !line.starts_with('\t');
        let key = line.split('=').next().map(str::trim).unwrap_or("");
        if top_level && key == "out_hash" {
            continue;
        }
        lines.push(line.to_owned());
    }
    let mut out = lines.join("\n");
    out.push('\n');
    fs::write(&path, out).map_err(|e| format!("записать {}: {e}", path.display()))?;
    validate_manifest(ManifestKind::Artifact, &path)
}

/// Записать `out_hash` последнего успешного обновления в `artifact.toml`, не трогая остальной
/// файл (комментарии и порядок сохраняются): ключ верхнего уровня, вставляется перед первой
/// таблицей `[...]` или заменяет существующий. `out_hash` — идентичность выходов для git (R-VCS-02).
pub fn record_artifact_out_hash(artifact_dir: &Path, out_hash: &str) -> Result<(), String> {
    let path = artifact_dir.join("artifact.toml");
    let text = fs::read_to_string(&path).map_err(|e| format!("прочитать {}: {e}", path.display()))?;
    let new_line = format!("out_hash = {}", toml_string(out_hash));
    let mut lines: Vec<String> = Vec::new();
    let mut replaced = false;
    let mut inserted = false;
    for line in text.lines() {
        let top_level = !line.starts_with(' ') && !line.starts_with('\t');
        let key = line.split('=').next().map(str::trim).unwrap_or("");
        if top_level && key == "out_hash" {
            if !replaced {
                lines.push(new_line.clone());
                replaced = true;
            } else {
                // Повторяющийся ключ TOML не допускает — старую строку убираем.
            }
            continue;
        }
        if !replaced && !inserted && top_level && line.starts_with('[') {
            lines.push(new_line.clone());
            inserted = true;
        }
        lines.push(line.to_owned());
    }
    if !replaced && !inserted {
        lines.push(new_line);
    }
    let mut out = lines.join("\n");
    out.push('\n');
    fs::write(&path, out).map_err(|e| format!("записать {}: {e}", path.display()))?;
    validate_manifest(ManifestKind::Artifact, &path)
}

/// Создать минимальный проект RDE (rde.toml + каталоги objects/**).
pub fn create_project(root: &Path, name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("название проекта не может быть пустым".to_owned());
    }
    if root.join("rde.toml").exists() {
        return Err(format!("в {} уже есть rde.toml", root.display()));
    }
    fs::create_dir_all(root).map_err(|e| format!("создать {}: {e}", root.display()))?;
    for dir in ["artifacts", "documents", "sources", "notes", "repos", "formulas"] {
        fs::create_dir_all(root.join("objects").join(dir)).map_err(|e| format!("создать objects/{dir}: {e}"))?;
    }
    let id = new_id();
    let manifest = format!(
        "schema = \"{}\"\nid = \"{id}\"\nname = {}\ndefault_rebuild = \"manual\"\ndefault_template = \"gost-7.32-2017\"\ntrusted = false\nnetwork_allow = false\ndocument_auto_update = false\n",
        crate::manifest::SCHEMA_PROJECT,
        toml_string(name)
    );
    fs::write(root.join("rde.toml"), manifest).map_err(|e| format!("записать rde.toml: {e}"))?;
    Ok(())
}

/// Сменить схему нумерации формул документа в `document.toml` (ADR-0018): `continuous` —
/// сквозная, `section` — по разделам (ГОСТ). Валидирует манифест после записи.
pub fn set_document_formula_numbering(manifest_path: &Path, scheme: &str) -> Result<(), String> {
    let text = fs::read_to_string(manifest_path).map_err(|e| format!("прочитать {}: {e}", manifest_path.display()))?;
    let mut manifest: crate::manifest::DocumentManifest =
        toml::from_str(&text).map_err(|e| format!("разобрать {}: {e}", manifest_path.display()))?;
    if !matches!(scheme, "continuous" | "section") {
        return Err(format!("неизвестная схема нумерации формул: {scheme} (ожидается continuous|section)"));
    }
    manifest.formula_numbering = scheme.to_owned();
    let text = toml::to_string_pretty(&manifest).map_err(|e| format!("document.toml: {e}"))?;
    fs::write(manifest_path, text).map_err(|e| format!("записать {}: {e}", manifest_path.display()))?;
    validate_manifest(ManifestKind::Document, manifest_path)
}

/// Создать каталог документа и `document.toml` (без OOXML-пакета: пакет собирает вызывающий слой,
/// у которого есть движок WordCraft — см. `RdeApp::create_document_action`).
pub fn create_document_stub(root: &Path, title: &str) -> Result<PathBuf, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("название документа не может быть пустым".to_owned());
    }
    let id = new_id();
    let dir = root.join("objects/documents").join(&id);
    fs::create_dir_all(&dir).map_err(|e| format!("создать каталог документа: {e}"))?;
    let manifest = format!(
        "schema = \"{}\"\nid = \"{id}\"\ntitle = {}\ntemplate = \"gost-7.32-2017\"\nfile = \"report.doc.rde\"\nformula_numbering = \"continuous\"\n",
        crate::manifest::SCHEMA_DOCUMENT,
        toml_string(title)
    );
    let path = dir.join("document.toml");
    fs::write(&path, manifest).map_err(|e| format!("записать document.toml: {e}"))?;
    if let Err(error) = validate_manifest(ManifestKind::Document, &path) {
        let _ = fs::remove_dir_all(&dir);
        return Err(error);
    }
    Ok(dir)
}

/// Создать project-level объект формулы (ADR-0005 P2): `objects/formulas/<id>/formula.toml`
/// с LaTeX-источником (display, политика нумерации по умолчанию — auto).
pub fn create_formula(root: &Path, title: &str, latex: &str) -> Result<PathBuf, String> {
    let title = title.trim();
    if title.is_empty() {
        return Err("название формулы не может быть пустым".to_owned());
    }
    let latex = latex.trim();
    if latex.is_empty() {
        return Err("LaTeX-источник формулы не может быть пустым".to_owned());
    }
    let id = new_id();
    let dir = root.join("objects/formulas").join(&id);
    fs::create_dir_all(&dir).map_err(|e| format!("создать каталог формулы: {e}"))?;
    let manifest = format!(
        "schema = \"{}\"\nid = \"{id}\"\ntitle = {}\nlatex = {}\ndisplay = true\n\n[numbering]\nmode = \"auto\"\nlabel = \"\"\n",
        crate::manifest::SCHEMA_FORMULA,
        toml_string(title),
        toml_string(latex)
    );
    let path = dir.join("formula.toml");
    fs::write(&path, manifest).map_err(|e| format!("записать formula.toml: {e}"))?;
    let readme = format!("# {title}\n\nФормула проекта (ADR-0005). LaTeX-источник — `formula.toml`; кэши OMML/SVG/PNG — в CAS.\n");
    let _ = fs::write(dir.join("readme.md"), readme);
    let _ = fs::write(dir.join(".gitignore"), "");
    if let Err(error) = validate_manifest(ManifestKind::Formula, &path) {
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
        crate::ObjectData::Formula(_) => "formula.toml",
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
        crate::ObjectData::Formula(_) => ManifestKind::Formula,
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
    fn create_project_and_document_stub() {
        let root = std::env::temp_dir().join(format!("rde-core-project-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        create_project(&root, "Новый проект").unwrap();
        let project = load_project(&root).unwrap();
        assert_eq!(project.name, "Новый проект");
        assert_eq!(project.object_count(), 0);
        assert!(root.join("objects/artifacts").is_dir());

        let dir = create_document_stub(&root, "Новый отчёт").unwrap();
        assert!(dir.join("document.toml").is_file());
        let project = load_project(&root).unwrap();
        assert_eq!(project.object_count(), 1);
        assert_eq!(project.document().unwrap().title, "Новый отчёт");
        assert!(project.issues.is_empty(), "{:?}", project.issues);
        assert!(create_project(&root, "Ещё один").is_err(), "повторный create_project должен падать");
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

    #[test]
    fn artifacts_get_readme_and_gitignore() {
        let root = minimal_project("scaffold");
        let compute = create_object(&root, &CreateRequest { kind: NewObjectKind::ComputeArtifact, title: "Расчёт".into() }).unwrap();
        assert!(compute.join("readme.md").is_file());
        let ignore = fs::read_to_string(compute.join(".gitignore")).unwrap();
        assert!(ignore.contains("out/*"), "выходы не коммитятся: {ignore}");
        assert!(compute.join("out/.gitignore").is_file());
        let data = create_object(&root, &CreateRequest { kind: NewObjectKind::DataArtifact, title: "Данные".into() }).unwrap();
        assert!(data.join("readme.md").is_file());
        assert!(data.join(".gitignore").is_file());
        // Проект после появления readme/.gitignore загружается без issues.
        let project = load_project(&root).unwrap();
        assert!(project.issues.is_empty(), "{:?}", project.issues);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn record_out_hash_keeps_manifest_text_and_roundtrips() {
        let root = minimal_project("outhash");
        let dir = create_object(&root, &CreateRequest { kind: NewObjectKind::ComputeArtifact, title: "Расчёт".into() }).unwrap();
        let path = dir.join("artifact.toml");
        // Добавим комментарий, который не должен потеряться.
        let original = fs::read_to_string(&path).unwrap();
        fs::write(&path, format!("# комментарий пользователя\n{original}")).unwrap();
        let hash = "ab".repeat(32);
        record_artifact_out_hash(&dir, &hash).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("# комментарий пользователя"), "комментарии сохраняются");
        assert!(text.contains(&format!("out_hash = \"{hash}\"")));
        // Ключ верхнего уровня должен стоять до первой таблицы.
        let out_pos = text.find("out_hash").unwrap();
        let table_pos = text.find("\n[").unwrap();
        assert!(out_pos < table_pos, "out_hash до первой таблицы:\n{text}");
        // Повторная запись заменяет, а не дублирует.
        let hash2 = "cd".repeat(32);
        record_artifact_out_hash(&dir, &hash2).unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert_eq!(text.matches("out_hash").count(), 1, "{text}");
        assert!(text.contains(&format!("out_hash = \"{hash2}\"")));
        // Загрузка проекта видит out_hash.
        let project = load_project(&root).unwrap();
        assert!(project.issues.is_empty(), "{:?}", project.issues);
        let artifact = project.artifact_object(&project.objects[0].id).unwrap();
        let crate::ObjectData::Artifact(manifest) = &artifact.data else { panic!() };
        assert_eq!(manifest.out_hash.as_deref(), Some(hash2.as_str()));
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod authors_tests {
    use super::*;

    #[test]
    fn set_project_authors_replaces_blocks_and_keeps_rest() {
        let root = std::env::temp_dir().join(format!("rde-core-authors-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("rde.toml"),
            "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"T\"\n# комментарий\ntrusted = true\n\n[[authors]]\nid = \"01M5VJ1JG0YAJDSX5VK7TBXABK\"\nname = \"Старый\"\n",
        )
        .unwrap();
        let authors = vec![
            crate::manifest::AuthorManifest {
                id: "01M5VJ1JG0YAJDSX5VK7TBXABK".into(),
                name: "Илья".into(),
                initials: "И".into(),
                email: "ilunix@pc.ubuntu".into(),
                kind: "human".into(),
            },
            crate::manifest::AuthorManifest {
                id: "01M5VJ1JG0ZZZZZZZZZZZZZZZZ".into(),
                name: "Агент".into(),
                initials: "AI".into(),
                email: String::new(),
                kind: "agent".into(),
            },
        ];
        set_project_authors(&root, &authors).unwrap();
        let text = fs::read_to_string(root.join("rde.toml")).unwrap();
        assert!(text.contains("# комментарий"), "комментарии сохраняются");
        assert!(text.contains("trusted = true"));
        assert!(text.contains("name = \"Илья\""));
        assert!(text.contains("name = \"Агент\""));
        assert!(!text.contains("Старый"), "старые авторы заменены: {text}");
        let project = crate::load_project(&root).unwrap();
        assert_eq!(project.manifest.authors.len(), 2);
        assert_eq!(project.manifest.authors[1].kind, "agent");
        // Пустой список удаляет авторов.
        set_project_authors(&root, &[]).unwrap();
        assert_eq!(crate::load_project(&root).unwrap().manifest.authors.len(), 0);
        let _ = fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod manifest_write_tests {
    use super::*;

    #[test]
    fn write_artifact_manifest_roundtrips_typed_fields() {
        let root = std::env::temp_dir().join(format!("rde-core-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("rde.toml"),
            "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"T\"\ntrusted = true\n",
        )
        .unwrap();
        let dir = create_object(&root, &CreateRequest { kind: NewObjectKind::ComputeArtifact, title: "A".into() }).unwrap();
        let project = crate::load_project(&root).unwrap();
        let object = &project.objects[0];
        let crate::ObjectData::Artifact(mut manifest) = object.data.clone() else { panic!() };
        manifest.params = serde_json::json!({"window": 256, "method": "welch"});
        manifest.outputs = vec!["out/a.csv".into(), "out/b.svg".into()];
        manifest.network_allow = Some(true);
        manifest.groups.push(crate::manifest::GroupManifest {
            name: "refs".into(),
            mode: "ref".into(),
            inputs: vec![crate::manifest::InputSpec::File { file: "inputs/x.csv".into() }],
        });
        write_artifact_manifest(&root, &manifest).unwrap();
        let reloaded = crate::load_project(&root).unwrap();
        let crate::ObjectData::Artifact(again) = reloaded.objects[0].data.clone() else { panic!() };
        assert_eq!(again.params, manifest.params);
        assert_eq!(again.outputs, manifest.outputs);
        assert_eq!(again.network_allow, Some(true));
        assert_eq!(again.groups.len(), 2);
        assert_eq!(again.groups[1].mode, "ref");
        assert!(reloaded.issues.is_empty(), "{:?}", reloaded.issues);
        let _ = fs::remove_dir_all(&root);
    }
}
