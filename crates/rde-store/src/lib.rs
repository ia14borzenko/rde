//! RDE project store (M2.2b): content-addressed storage, atomic writes, transaction journal and a
//! rebuildable SQLite index.
//!
//! Раскладка (tz-02 §8.3–8.4):
//! ```text
//! <project>/.rde/
//!   index.db                     # SQLite: blobs/trees/txs; всегда пересоздаётся из файлов
//!   cas/sha256/ab/cd/<hash>      # immutable блобы
//!   trees/<tree_hash>.json       # версии файлов (путь, режим, блоб)
//!   updates/<txid>.json          # журнал транзакций (prepared/committed/failed)
//!   snapshots/  trash/           # резерв под следующие вехи
//! ```
//! Источник истины — файлы: `Store::open` пересобирает индекс при необходимости, `rebuild_index`
//! проверяет целостность CAS/деревьев/журнала. Атомарная запись: temp + fsync + rename + fsync(dir).

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rde_hash::{Hash, TreeEntry, sha256, tree_hash};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

pub const STORE_DIR: &str = ".rde";

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Ошибка хранилища (строка с контекстом; UI/CLI показывают её как есть).
#[derive(Debug)]
pub struct StoreError(pub String);

impl StoreError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl std::fmt::Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StoreError {}

impl From<std::io::Error> for StoreError {
    fn from(e: std::io::Error) -> Self {
        Self::new(format!("ввод-вывод: {e}"))
    }
}

impl From<rusqlite::Error> for StoreError {
    fn from(e: rusqlite::Error) -> Self {
        Self::new(format!("индекс: {e}"))
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Статус транзакции (журнал `.rde/updates/<txid>.json`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TxStatus {
    Prepared,
    Committed,
    Failed,
}

impl TxStatus {
    fn as_str(self) -> &'static str {
        match self {
            TxStatus::Prepared => "prepared",
            TxStatus::Committed => "committed",
            TxStatus::Failed => "failed",
        }
    }
}

/// Запись журнала транзакций.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TxRecord {
    pub id: String,
    pub status: TxStatus,
    pub note: String,
    pub started_unix: u64,
    #[serde(default)]
    pub updated_unix: Option<u64>,
}

/// Сводка восстановления при открытии проекта.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Транзакции, оставшиеся `prepared` (были прерваны kill -9).
    pub incomplete: Vec<String>,
    /// Сколько записей индекса подтянуто из файлов при открытии.
    pub reconciled: usize,
}

/// Отчёт пересборки индекса из файлов.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexReport {
    pub blobs: usize,
    pub trees: usize,
    pub txs: usize,
    /// Файлы, содержимое которых не совпало с именем-хешем (битые блобы).
    pub mismatched: Vec<String>,
}

/// Статистика по индексу.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StoreStats {
    pub blobs: u64,
    pub blob_bytes: u64,
    pub trees: u64,
    pub txs: u64,
}

#[derive(Serialize, Deserialize)]
struct TreeFile {
    entries: Vec<TreeJsonEntry>,
}

#[derive(Serialize, Deserialize)]
struct TreeJsonEntry {
    path: String,
    mode: u32,
    blob: String,
}

/// Хранилище проекта.
pub struct Store {
    root: PathBuf,
    store_dir: PathBuf,
    conn: Connection,
    /// Результат проверки журнала при `open`.
    pub recovery: RecoveryReport,
}

impl Store {
    /// Открыть (или создать) хранилище проекта. Файлы проекта — источник истины.
    pub fn open(project_root: &Path) -> Result<Self> {
        if !project_root.is_dir() {
            return Err(StoreError::new(format!("каталог проекта не найден: {}", project_root.display())));
        }
        let store_dir = project_root.join(STORE_DIR);
        for dir in ["cas/sha256", "trees", "updates", "snapshots", "trash"] {
            fs::create_dir_all(store_dir.join(dir))?;
        }
        let conn = Connection::open(store_dir.join("index.db"))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\n\
             PRAGMA synchronous = NORMAL;\n\
             CREATE TABLE IF NOT EXISTS blobs (hash TEXT PRIMARY KEY, size INTEGER NOT NULL);\n\
             CREATE TABLE IF NOT EXISTS trees (hash TEXT PRIMARY KEY, entries INTEGER NOT NULL);\n\
             CREATE TABLE IF NOT EXISTS txs (id TEXT PRIMARY KEY, status TEXT NOT NULL, note TEXT NOT NULL, started_unix INTEGER NOT NULL, updated_unix INTEGER);",
        )?;
        let mut store = Self { root: project_root.to_path_buf(), store_dir, conn, recovery: RecoveryReport::default() };
        // kill -9 мог оставить недописанный temp-файл атомарной записи: он не является блобом.
        cleanup_temp_files(&store.store_dir);
        store.recovery = store.recover()?;
        Ok(store)
    }

    /// Корень проекта, к которому относится хранилище.
    pub fn project_root(&self) -> &Path {
        &self.root
    }

    /// Каталог `.rde`.
    pub fn store_dir(&self) -> &Path {
        &self.store_dir
    }

    fn cas_path(&self, hash: Hash) -> PathBuf {
        let hex = hash.hex();
        self.store_dir.join("cas/sha256").join(&hex[0..2]).join(&hex[2..4]).join(&hex)
    }

    fn tree_path(&self, hash: Hash) -> PathBuf {
        self.store_dir.join("trees").join(format!("{}.json", hash.hex()))
    }

    fn tx_path(&self, id: &str) -> PathBuf {
        self.store_dir.join("updates").join(format!("{id}.json"))
    }

    /// Положить блоб в CAS (дедупликация по содержимому). Возвращает SHA-256.
    pub fn put_blob(&mut self, bytes: &[u8]) -> Result<Hash> {
        let hash = sha256(bytes);
        if !self.has_blob(hash) {
            let path = self.cas_path(hash);
            write_atomic(&path, bytes)?;
            self.conn.execute("INSERT OR REPLACE INTO blobs (hash, size) VALUES (?1, ?2)", params![hash.hex(), bytes.len() as i64])?;
        }
        Ok(hash)
    }

    /// Положить содержимое файла проекта в CAS.
    pub fn put_file(&mut self, path: &Path) -> Result<Hash> {
        let bytes = fs::read(path)?;
        self.put_blob(&bytes)
    }

    pub fn has_blob(&self, hash: Hash) -> bool {
        self.cas_path(hash).is_file()
    }

    pub fn get_blob(&self, hash: Hash) -> Result<Option<Vec<u8>>> {
        let path = self.cas_path(hash);
        if !path.is_file() {
            return Ok(None);
        }
        Ok(Some(fs::read(path)?))
    }

    /// Проверить, что содержимое блоба совпадает с его именем-хешем.
    pub fn verify_blob(&self, hash: Hash) -> Result<bool> {
        match self.get_blob(hash)? {
            Some(bytes) => Ok(sha256(&bytes) == hash),
            None => Ok(false),
        }
    }

    /// Сохранить версию файлов (Merkle-дерево) и вернуть `tree_hash`. Порядок входа не важен.
    pub fn put_tree(&mut self, entries: &[TreeEntry]) -> Result<Hash> {
        let hash = tree_hash(entries);
        let path = self.tree_path(hash);
        if !path.is_file() {
            let json = TreeFile {
                entries: entries
                    .iter()
                    .map(|e| TreeJsonEntry { path: e.path.clone(), mode: e.mode, blob: e.blob.hex() })
                    .collect(),
            };
            let bytes = serde_json::to_vec_pretty(&json).map_err(|e| StoreError::new(format!("json дерева: {e}")))?;
            write_atomic(&path, &bytes)?;
        }
        self.conn.execute("INSERT OR REPLACE INTO trees (hash, entries) VALUES (?1, ?2)", params![hash.hex(), entries.len() as i64])?;
        Ok(hash)
    }

    /// Прочитать версию файлов по `tree_hash`.
    pub fn get_tree(&self, hash: Hash) -> Result<Option<Vec<TreeEntry>>> {
        let path = self.tree_path(hash);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(path)?;
        let file: TreeFile = serde_json::from_slice(&bytes).map_err(|e| StoreError::new(format!("json дерева: {e}")))?;
        let mut entries = Vec::with_capacity(file.entries.len());
        for entry in file.entries {
            let blob = Hash::from_hex(&entry.blob).map_err(StoreError::new)?;
            entries.push(TreeEntry { path: entry.path, mode: entry.mode, blob });
        }
        Ok(Some(entries))
    }

    /// Начать транзакцию: запись в журнал со статусом `prepared`.
    pub fn begin_tx(&mut self, note: &str) -> Result<String> {
        let id = ulid::Ulid::generate().to_string();
        let record = TxRecord {
            id: id.clone(),
            status: TxStatus::Prepared,
            note: note.to_owned(),
            started_unix: now_unix(),
            updated_unix: None,
        };
        self.write_tx(&record)?;
        Ok(id)
    }

    /// Зафиксировать транзакцию.
    pub fn commit_tx(&mut self, id: &str) -> Result<()> {
        let mut record = self.read_tx(id)?.ok_or_else(|| StoreError::new(format!("транзакция {id} не найдена")))?;
        record.status = TxStatus::Committed;
        record.updated_unix = Some(now_unix());
        self.write_tx(&record)
    }

    /// Пометить транзакцию неуспешной (откат выполнения).
    pub fn fail_tx(&mut self, id: &str, reason: &str) -> Result<()> {
        let mut record = self.read_tx(id)?.ok_or_else(|| StoreError::new(format!("транзакция {id} не найдена")))?;
        record.status = TxStatus::Failed;
        record.updated_unix = Some(now_unix());
        if !reason.is_empty() {
            record.note = format!("{}; failed: {reason}", record.note);
        }
        self.write_tx(&record)
    }

    pub fn read_tx(&self, id: &str) -> Result<Option<TxRecord>> {
        let path = self.tx_path(id);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(path)?;
        let record = serde_json::from_slice(&bytes).map_err(|e| StoreError::new(format!("json транзакции {id}: {e}")))?;
        Ok(Some(record))
    }

    pub fn list_txs(&self) -> Result<Vec<TxRecord>> {
        let mut stmt = self.conn.prepare("SELECT id FROM txs ORDER BY started_unix DESC, id DESC")?;
        let ids: Vec<String> = stmt.query_map([], |row| row.get::<_, String>(0))?.collect::<std::result::Result<_, _>>()?;
        ids.into_iter().map(|id| self.read_tx(&id)?.ok_or_else(|| StoreError::new(format!("транзакция {id} пропала из файлов")))).collect()
    }

    fn write_tx(&mut self, record: &TxRecord) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(record).map_err(|e| StoreError::new(format!("json транзакции: {e}")))?;
        write_atomic(&self.tx_path(&record.id), &bytes)?;
        self.conn.execute(
            "INSERT OR REPLACE INTO txs (id, status, note, started_unix, updated_unix) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![record.id, record.status.as_str(), record.note, record.started_unix as i64, record.updated_unix.map(|v| v as i64)],
        )?;
        Ok(())
    }

    fn recover(&mut self) -> Result<RecoveryReport> {
        let mut report = RecoveryReport::default();
        let dir = self.store_dir.join("updates");
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Ok(bytes) = fs::read(&path) else { continue };
                let Ok(record) = serde_json::from_slice::<TxRecord>(&bytes) else { continue };
                self.conn.execute(
                    "INSERT OR REPLACE INTO txs (id, status, note, started_unix, updated_unix) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![record.id, record.status.as_str(), record.note, record.started_unix as i64, record.updated_unix.map(|v| v as i64)],
                )?;
                report.reconciled += 1;
                if record.status == TxStatus::Prepared {
                    report.incomplete.push(record.id);
                }
            }
        }
        // Индекс мог быть потерян/устареть: если blobs/trees пусты, а файлы есть — пересобрать.
        let blobs_in_index: i64 = self.conn.query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get(0))?;
        if blobs_in_index == 0 && self.cas_file_count() > 0 {
            let rebuilt = self.rebuild_index()?;
            report.reconciled += rebuilt.blobs + rebuilt.trees;
        }
        Ok(report)
    }

    fn cas_file_count(&self) -> usize {
        fn walk(dir: &Path) -> usize {
            let Ok(entries) = fs::read_dir(dir) else { return 0 };
            entries
                .flatten()
                .map(|e| {
                    let p = e.path();
                    if p.is_dir() { walk(&p) } else { 1 }
                })
                .sum()
        }
        walk(&self.store_dir.join("cas/sha256"))
    }

    /// Пересобрать индекс из файлов и проверить целостность CAS/деревьев/журнала.
    pub fn rebuild_index(&mut self) -> Result<IndexReport> {
        self.conn.execute_batch("DELETE FROM blobs; DELETE FROM trees; DELETE FROM txs;")?;
        let mut report = IndexReport::default();

        let cas_root = self.store_dir.join("cas/sha256");
        let mut files = Vec::new();
        collect_files(&cas_root, &mut files);
        for path in files {
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else { continue };
            if name.starts_with(".rde-tmp-") {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else { continue };
            let hash = sha256(&bytes);
            if hash.hex() != name {
                report.mismatched.push(path.display().to_string());
                continue;
            }
            self.conn.execute("INSERT OR REPLACE INTO blobs (hash, size) VALUES (?1, ?2)", params![name, bytes.len() as i64])?;
            report.blobs += 1;
        }

        let trees_root = self.store_dir.join("trees");
        let mut tree_files = Vec::new();
        collect_files(&trees_root, &mut tree_files);
        for path in tree_files {
            let Some(name) = path.file_stem().and_then(|n| n.to_str()) else { continue };
            let Ok(bytes) = fs::read(&path) else { continue };
            let Ok(file) = serde_json::from_slice::<TreeFile>(&bytes) else {
                report.mismatched.push(path.display().to_string());
                continue;
            };
            let mut entries = Vec::new();
            let mut valid = true;
            for entry in &file.entries {
                match Hash::from_hex(&entry.blob) {
                    Ok(blob) => entries.push(TreeEntry { path: entry.path.clone(), mode: entry.mode, blob }),
                    Err(_) => valid = false,
                }
            }
            if !valid || tree_hash(&entries).hex() != name {
                report.mismatched.push(path.display().to_string());
                continue;
            }
            self.conn.execute("INSERT OR REPLACE INTO trees (hash, entries) VALUES (?1, ?2)", params![name, file.entries.len() as i64])?;
            report.trees += 1;
        }

        let updates_root = self.store_dir.join("updates");
        let mut tx_files = Vec::new();
        collect_files(&updates_root, &mut tx_files);
        for path in tx_files {
            let Ok(bytes) = fs::read(&path) else { continue };
            let Ok(record) = serde_json::from_slice::<TxRecord>(&bytes) else {
                report.mismatched.push(path.display().to_string());
                continue;
            };
            self.conn.execute(
                "INSERT OR REPLACE INTO txs (id, status, note, started_unix, updated_unix) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![record.id, record.status.as_str(), record.note, record.started_unix as i64, record.updated_unix.map(|v| v as i64)],
            )?;
            report.txs += 1;
        }
        Ok(report)
    }

    /// Статистика из индекса.
    pub fn stats(&self) -> Result<StoreStats> {
        let blobs: i64 = self.conn.query_row("SELECT COUNT(*) FROM blobs", [], |row| row.get(0))?;
        let blob_bytes: i64 = self.conn.query_row("SELECT COALESCE(SUM(size), 0) FROM blobs", [], |row| row.get(0))?;
        let trees: i64 = self.conn.query_row("SELECT COUNT(*) FROM trees", [], |row| row.get(0))?;
        let txs: i64 = self.conn.query_row("SELECT COUNT(*) FROM txs", [], |row| row.get(0))?;
        Ok(StoreStats { blobs: blobs as u64, blob_bytes: blob_bytes as u64, trees: trees as u64, txs: txs as u64 })
    }

    /// Список незавершённых транзакций (prepared) — для диагностики «починить».
    pub fn incomplete_txs(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare("SELECT id FROM txs WHERE status = 'prepared' ORDER BY started_unix")?;
        let ids = stmt.query_map([], |row| row.get::<_, String>(0))?.collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(ids)
    }

    /// Все известные блобы (для GC в следующих вехах).
    pub fn blob_hashes(&self) -> Result<Vec<Hash>> {
        let mut stmt = self.conn.prepare("SELECT hash FROM blobs ORDER BY hash")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut set = BTreeSet::new();
        for row in rows {
            set.insert(Hash::from_hex(&row?).map_err(StoreError::new)?);
        }
        Ok(set.into_iter().collect())
    }
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Удалить недописанные temp-файлы атомарных записей (после kill -9).
fn cleanup_temp_files(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            cleanup_temp_files(&path);
        } else if path.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(".rde-tmp-")) {
            let _ = fs::remove_file(&path);
        }
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, out);
        } else {
            out.push(path);
        }
    }
    out.sort();
}

/// Атомарная запись: temp в том же каталоге → fsync файла → rename → fsync каталога.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or_else(|| StoreError::new(format!("у пути нет каталога: {}", path.display())))?;
    fs::create_dir_all(dir)?;
    let unique = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".rde-tmp-{}-{unique}", std::process::id()));
    {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    sync_dir(dir)?;
    Ok(())
}

fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        File::open(dir)?.sync_all()?;
    }
    let _ = dir;
    Ok(())
}
