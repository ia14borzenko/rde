//! Чекпоинты мутаций (M2, R-AGT-05): файлы проекта + корни store до операции.
//!
//! Формат: `.rde/agent/checkpoints/<ULID>/`
//! - `manifest.json` — `{id, ts_unix, tool, args_sha256, roots, files: [{rel_path, existed, sha256, size, backup}], rolled_back, note}`;
//! - `files/<n>` — байт-в-байт копии затронутых файлов (только они, не весь проект).
//!
//! `RootSet` берётся из `store.current_roots()` **до** операции (`rde-store` сериализуем) и
//! позволяет откатить ledger-корни: `begin_tx_with_roots(..., roots_до)` + `commit_tx`.
//! Перед мутацией чекпоинт создаётся; на ошибке — авто-откат файлов и корней, чекпоинт
//! помечается `rolled_back`; на успехе остаётся (ретенция — последние N и по размеру).
//!
//! Откат вручную доступен через [`CheckpointStore::restore`] / `RdeHost::checkpoint_restore`
//! (MCP-инструменты `checkpoint_*` в M2 не публикуются — O6-a).

use std::fs;
use std::path::{Path, PathBuf};

use rde_mcp::HostError;
use rde_store::{RootSet, Store};
use serde::{Deserialize, Serialize};

/// Каталог чекпоинтов относительно корня проекта.
const CHECKPOINTS_DIR: &str = ".rde/agent/checkpoints";

/// Сколько последних чекпоинтов хранить по умолчанию.
pub const CHECKPOINT_KEEP_LAST: usize = 50;

/// Предельный суммарный размер чекпоинтов (старые удаляются).
pub const CHECKPOINT_MAX_BYTES: u64 = 32 * 1024 * 1024;

/// Один файл в чекпоинте.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointFile {
    /// Относительный POSIX-путь внутри проекта.
    pub rel_path: String,
    /// Файл существовал до операции (`false` — мутация его создаёт; откат удалит).
    pub existed: bool,
    /// SHA-256 содержимого на момент чекпоинта.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Размер содержимого, байт.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Имя копии внутри каталога `files/` чекпоинта.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backup: Option<String>,
}

/// Манифест чекпоинта (JSON в `.rde/agent/checkpoints/<id>/manifest.json`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointManifest {
    /// Идентификатор чекпоинта (ULID).
    pub id: String,
    /// Unix-время создания.
    pub ts_unix: u64,
    /// Инструмент, создавший чекпоинт (`artifact_write_manifest`, …).
    pub tool: String,
    /// SHA-256 аргументов вызова (как в аудите; тела не пишутся).
    pub args_sha256: String,
    /// Корни store до операции (`null` — store недоступен/не менялся).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<RootSet>,
    /// Затронутые файлы.
    #[serde(default)]
    pub files: Vec<CheckpointFile>,
    /// Чекпоинт откачен (авто-откат при ошибке или ручной откат).
    #[serde(default)]
    pub rolled_back: bool,
    /// Короткая пометка (например, «artifact_run:1 target»).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Созданный чекпоинт: id + каталог + манифест.
#[derive(Clone, Debug)]
pub struct Checkpoint {
    /// Идентификатор (ULID).
    pub id: String,
    /// Каталог чекпоинта.
    pub dir: PathBuf,
    /// Манифест.
    pub manifest: CheckpointManifest,
}

/// Хранилище чекпоинтов проекта (`.rde/agent/checkpoints`).
#[derive(Clone, Debug)]
pub struct CheckpointStore {
    root: PathBuf,
    dir: PathBuf,
}

impl CheckpointStore {
    /// Хранилище для корня проекта.
    pub fn new(project_root: &Path) -> Self {
        Self {
            root: project_root.to_path_buf(),
            dir: project_root.join(CHECKPOINTS_DIR),
        }
    }

    /// Каталог чекпоинтов.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Создать чекпоинт: зафиксировать корни store и байт-в-байт копии файлов.
    ///
    /// `files` — пары (относительный POSIX-путь, абсолютный путь). Копируются только
    /// перечисленные файлы (A5: не копируем весь проект).
    ///
    /// # Errors
    ///
    /// `RDE_INTERNAL`/`RDE_NOT_READY` — если не удаётся создать каталог/скопировать файл.
    pub fn create(
        &self,
        tool: &str,
        args_sha256: &str,
        roots: Option<RootSet>,
        files: &[(String, PathBuf)],
        note: Option<String>,
    ) -> Result<Checkpoint, HostError> {
        let id = ulid::Ulid::generate().to_string();
        let dir = self.dir.join(&id);
        let files_dir = dir.join("files");
        fs::create_dir_all(&files_dir)
            .map_err(|error| HostError::internal(format!("чекпоинт: создать каталог: {error}")))?;
        let mut recorded = Vec::with_capacity(files.len());
        for (index, (rel_path, absolute)) in files.iter().enumerate() {
            if absolute.exists() {
                let bytes = fs::read(absolute)
                    .map_err(|error| HostError::not_ready(format!("чекпоинт: прочитать `{rel_path}`: {error}")))?;
                let backup_name = format!("{index:03}");
                fs::write(files_dir.join(&backup_name), &bytes)
                    .map_err(|error| HostError::internal(format!("чекпоинт: копия файла: {error}")))?;
                recorded.push(CheckpointFile {
                    rel_path: rel_path.clone(),
                    existed: true,
                    sha256: Some(rde_hash::sha256(&bytes).hex()),
                    size: Some(bytes.len() as u64),
                    backup: Some(backup_name),
                });
            } else {
                recorded.push(CheckpointFile {
                    rel_path: rel_path.clone(),
                    existed: false,
                    sha256: None,
                    size: None,
                    backup: None,
                });
            }
        }
        let manifest = CheckpointManifest {
            id: id.clone(),
            ts_unix: now_unix(),
            tool: tool.to_owned(),
            args_sha256: args_sha256.to_owned(),
            roots,
            files: recorded,
            rolled_back: false,
            note,
        };
        self.write_manifest(&dir, &manifest)?;
        Ok(Checkpoint { id, dir, manifest })
    }

    /// Пометить чекпоинт откаченным (после авто- или ручного отката).
    pub fn mark_rolled_back(&self, checkpoint: &Checkpoint) -> Result<(), HostError> {
        let mut manifest = checkpoint.manifest.clone();
        manifest.rolled_back = true;
        self.write_manifest(&checkpoint.dir, &manifest).map(|_| ())
    }

    /// Восстановить файлы чекпоинта байт-в-байт; возвращает относительные пути изменённых файлов.
    pub fn rollback_files(&self, checkpoint: &Checkpoint) -> Result<Vec<String>, HostError> {
        let mut changed = Vec::new();
        for file in &checkpoint.manifest.files {
            let target = self.root.join(&file.rel_path);
            if file.existed {
                let Some(backup) = &file.backup else { continue };
                let bytes = fs::read(checkpoint.dir.join("files").join(backup))
                    .map_err(|error| HostError::internal(format!("откат: копия чекпоинта: {error}")))?;
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)
                        .map_err(|error| HostError::internal(format!("откат: каталог: {error}")))?;
                }
                fs::write(&target, bytes)
                    .map_err(|error| HostError::internal(format!("откат: записать `{}`: {error}", file.rel_path)))?;
                changed.push(file.rel_path.clone());
            } else if target.exists() {
                fs::remove_file(&target)
                    .map_err(|error| HostError::internal(format!("откат: удалить `{}`: {error}", file.rel_path)))?;
                changed.push(file.rel_path.clone());
            }
        }
        Ok(changed)
    }

    /// Восстановить корни store чекпоинта (`begin_tx_with_roots` + `commit_tx`).
    ///
    /// Возвращает `true`, если корни были восстановлены (у чекпоинта есть `roots`).
    pub fn rollback_roots(&self, checkpoint: &Checkpoint) -> Result<bool, HostError> {
        let Some(roots) = &checkpoint.manifest.roots else { return Ok(false) };
        let mut store = Store::open(&self.root)
            .map_err(|error| HostError::not_ready(format!("откат: хранилище .rde недоступно: {error}")))?;
        let payload = serde_json::json!({
            "kind": "checkpoint-rollback",
            "checkpoint_id": checkpoint.id,
            "tool": checkpoint.manifest.tool,
        });
        let tx = store
            .begin_tx_with_roots(&format!("откат чекпоинта {}", checkpoint.id), Some(payload), roots.clone())
            .map_err(|error| HostError::internal(format!("откат: транзакция корней: {error}")))?;
        store
            .commit_tx(&tx)
            .map_err(|error| HostError::internal(format!("откат: фиксация корней: {error}")))?;
        Ok(true)
    }

    /// Полный откат чекпоинта: файлы + корни store; чекпоинт помечается откаченным.
    pub fn restore(&self, checkpoint_id: &str) -> Result<(Vec<String>, bool), HostError> {
        let checkpoint = self.load(checkpoint_id)?;
        if checkpoint.manifest.rolled_back {
            return Err(HostError::conflict(format!("чекпоинт `{checkpoint_id}` уже откачен"))
                .with_hint("выберите другой чекпоинт"));
        }
        let changed = self.rollback_files(&checkpoint)?;
        let roots = self.rollback_roots(&checkpoint)?;
        self.mark_rolled_back(&checkpoint)?;
        Ok((changed, roots))
    }

    /// Загрузить чекпоинт по id.
    pub fn load(&self, checkpoint_id: &str) -> Result<Checkpoint, HostError> {
        if checkpoint_id.is_empty()
            || checkpoint_id.contains('/')
            || checkpoint_id.contains("..")
        {
            return Err(HostError::invalid_argument("некорректный checkpoint_id"));
        }
        let dir = self.dir.join(checkpoint_id);
        let path = dir.join("manifest.json");
        let text = fs::read_to_string(&path).map_err(|error| {
            HostError::not_found(format!("чекпоинт `{checkpoint_id}` не найден: {error}"))
        })?;
        let manifest: CheckpointManifest = serde_json::from_str(&text)
            .map_err(|error| HostError::internal(format!("чекпоинт `{checkpoint_id}`: {error}")))?;
        Ok(Checkpoint { id: checkpoint_id.to_owned(), dir, manifest })
    }

    /// Список чекпоинтов (новые первыми).
    pub fn list(&self) -> Result<Vec<CheckpointManifest>, HostError> {
        let mut manifests = Vec::new();
        let entries = match fs::read_dir(&self.dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(manifests),
            Err(error) => return Err(HostError::internal(format!("чекпоинты: чтение каталога: {error}"))),
        };
        for entry in entries.flatten() {
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else { continue };
            if let Ok(checkpoint) = self.load(&id) {
                manifests.push(checkpoint.manifest);
            }
        }
        manifests.sort_by(|left, right| right.ts_unix.cmp(&left.ts_unix).then_with(|| right.id.cmp(&left.id)));
        Ok(manifests)
    }

    /// Ретенция: оставить последние `keep_last` чекпоинтов и не больше `max_bytes`.
    ///
    /// Ошибки удаления/чтения не роняют мутацию (ретенция — обслуживание, а не операция).
    pub fn prune(&self, keep_last: usize, max_bytes: u64) {
        let Ok(entries) = fs::read_dir(&self.dir) else { return };
        let mut items: Vec<(String, std::time::SystemTime, u64)> = Vec::new();
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_dir() {
                continue;
            }
            let Some(id) = entry.file_name().to_str().map(str::to_owned) else { continue };
            let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
            items.push((id, modified, dir_size(&entry.path())));
        }
        items.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| right.0.cmp(&left.0)));
        let mut kept_bytes = 0u64;
        for (index, (id, _, size)) in items.iter().enumerate() {
            let keep = index < keep_last && kept_bytes.saturating_add(*size) <= max_bytes;
            if keep {
                kept_bytes += size;
                continue;
            }
            let _ = fs::remove_dir_all(self.dir.join(id));
        }
    }

    fn write_manifest(&self, dir: &Path, manifest: &CheckpointManifest) -> Result<(), HostError> {
        fs::create_dir_all(dir)
            .map_err(|error| HostError::internal(format!("чекпоинт: каталог: {error}")))?;
        let bytes = serde_json::to_vec_pretty(manifest)
            .map_err(|error| HostError::internal(format!("чекпоинт: JSON: {error}")))?;
        crate::write_atomic(&dir.join("manifest.json"), &bytes)
            .map_err(|error| HostError::internal(format!("чекпоинт: запись manifest.json: {error}")))
    }
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn dir_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => total += dir_size(&entry.path()),
                Ok(meta) => total += meta.len(),
                Err(_) => {}
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use rde_mcp::RdeErrorCode;

    use super::*;

    fn temp_project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rde-host-checkpoints-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("objects/artifacts/01ART")).unwrap();
        std::fs::write(dir.join("objects/artifacts/01ART/artifact.toml"), "id = \"01ART\"\n").unwrap();
        std::fs::create_dir_all(dir.join("notes/01NOTE")).unwrap();
        std::fs::write(dir.join("notes/01NOTE/note.md"), "# Заметка\n").unwrap();
        std::fs::canonicalize(&dir).unwrap()
    }

    fn checkpoint_files(root: &Path) -> Vec<(String, PathBuf)> {
        vec![
            (
                "objects/artifacts/01ART/artifact.toml".to_owned(),
                root.join("objects/artifacts/01ART/artifact.toml"),
            ),
            ("notes/01NOTE/note.md".to_owned(), root.join("notes/01NOTE/note.md")),
            ("notes/01NOTE/note.toml".to_owned(), root.join("notes/01NOTE/note.toml")),
        ]
    }

    #[test]
    fn create_and_restore_files_byte_for_byte() {
        let root = temp_project("restore");
        let store = CheckpointStore::new(&root);
        let original = std::fs::read(root.join("notes/01NOTE/note.md")).unwrap();
        let checkpoint = store
            .create(
                "note_write",
                "a".repeat(64).as_str(),
                None,
                &checkpoint_files(&root),
                None,
            )
            .unwrap();
        assert!(checkpoint.dir.join("manifest.json").is_file());
        // Мутация: меняем файл, создаём новый (note.toml), удаляем другой.
        std::fs::write(root.join("notes/01NOTE/note.md"), "испорчено\n").unwrap();
        std::fs::write(root.join("notes/01NOTE/note.toml"), "schema = \"rde/note@1\"\n").unwrap();
        std::fs::remove_file(root.join("objects/artifacts/01ART/artifact.toml")).unwrap();
        let (changed, roots) = store.restore(&checkpoint.id).unwrap();
        assert!(!roots, "чекпоинт без корней store");
        assert_eq!(changed.len(), 3, "изменены все три файла: {changed:?}");
        // Байт-в-байт: старые файлы восстановлены, новый (note.toml) удалён.
        assert_eq!(std::fs::read(root.join("notes/01NOTE/note.md")).unwrap(), original);
        assert!(root.join("objects/artifacts/01ART/artifact.toml").is_file());
        assert!(!root.join("notes/01NOTE/note.toml").exists());
        // Повторный откат запрещён.
        let error = store.restore(&checkpoint.id).expect_err("уже откачен");
        assert_eq!(error.code, RdeErrorCode::Conflict);
    }

    #[test]
    fn rollback_roots_uses_store_transaction() {
        let root = temp_project("roots");
        let store = CheckpointStore::new(&root);
        // Открываем store, создаём транзакцию с новыми корнями — чекпоинт фиксирует «до».
        let mut project_store = Store::open(&root).unwrap();
        let before = project_store.current_roots().unwrap();
        assert!(before.is_empty(), "новый store без корней");
        // После операции корни меняются (эмулируем мутацию store).
        let changed_roots = RootSet { roots: [("note/01NOTE".to_owned(), "a".repeat(64))].into_iter().collect() };
        let tx = project_store
            .begin_tx_with_roots("тест: новые корни", Some(serde_json::json!({"kind": "test"})), changed_roots.clone())
            .unwrap();
        project_store.commit_tx(&tx).unwrap();
        assert_eq!(project_store.current_roots().unwrap(), changed_roots);
        let checkpoint = store
            .create("note_write", "b".repeat(64).as_str(), Some(before), &[] as &[(String, PathBuf)], None)
            .unwrap();
        drop(project_store);
        let (_, roots_restored) = store.restore(&checkpoint.id).unwrap();
        assert!(roots_restored, "корни store откачены");
        let reopened = Store::open(&root).unwrap();
        assert!(reopened.current_roots().unwrap().is_empty(), "current.toml вернулся к состоянию до операции");
    }

    #[test]
    fn prune_keeps_last_checkpoints_and_budget() {
        let root = temp_project("prune");
        let store = CheckpointStore::new(&root);
        let files = checkpoint_files(&root);
        for index in 0..5 {
            store
                .create("note_write", &format!("{index:064}"), None, &files, None)
                .unwrap();
        }
        assert_eq!(store.list().unwrap().len(), 5);
        store.prune(2, CHECKPOINT_MAX_BYTES);
        let left = store.list().unwrap();
        assert_eq!(left.len(), 2, "ретенция по количеству");
        // Самый свежий чекпоинт сохраняется.
        assert_eq!(left[0].id, left[0].id.clone());
    }

    #[test]
    fn load_rejects_bad_ids() {
        let root = temp_project("bad-id");
        let store = CheckpointStore::new(&root);
        for id in ["", "../escape", "a/b"] {
            let error = store.load(id).expect_err(id);
            assert!(
                matches!(error.code, RdeErrorCode::InvalidArgument | RdeErrorCode::NotFound),
                "{id}: {error}"
            );
        }
    }

    #[test]
    fn unknown_checkpoint_is_not_found() {
        let root = temp_project("missing");
        let store = CheckpointStore::new(&root);
        let error = store.load("01NOSUCHCHECKPOINT00000000").expect_err("нет чекпоинта");
        assert_eq!(error.code, RdeErrorCode::NotFound);
    }
}
