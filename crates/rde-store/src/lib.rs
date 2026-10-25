//! RDE project store (M2.2b): content-addressed storage, atomic writes, transaction journal and a
//! rebuildable SQLite index.
//!
//! Раскладка (tz-02 §8.3–8.4):
//! ```text
//! <project>/.rde/
//!   index.db                     # SQLite: blobs/trees/txs; всегда пересоздаётся из файлов
//!   cas/sha256/ab/cd/<hash>      # immutable блобы (крупные — zstd, маркер `RDEZ`)
//!   trees/<tree_hash>.json       # версии файлов (путь, режим, блоб)
//!   updates/<txid>.json          # журнал транзакций (prepared/committed/failed + roots)
//!   current.toml                 # текущие корни (имя → tree_hash); смена атомарная
//!   snapshots/<txid>.json        # предыдущие корни транзакции (окно отката)
//!   pins.json                    # pin'ы корней (экспорты/шаринги), GC их не удаляет
//!   lock                         # эксклюзивная блокировка писателя (pid + liveness)
//!   trash/                       # корзина объектов
//! ```
//!
//! Транзакция обновления (§7.3 п.7): `prepare` (журнал + новые roots) → выполнение → импорт
//! выходов в CAS → `commit` (снапшот текущих корней → атомарная запись current.toml → журнал
//! committed). Откат — O(1): `rollback(txid)` переключает `current.toml` на предыдущие корни
//! из снапшота (и сам фиксируется транзакцией, чтобы откат тоже можно было откатить).
//! Источник истины — файлы: `Store::open` пересобирает индекс при необходимости, `rebuild_index`
//! проверяет целостность CAS/деревьев/журнала. Атомарная запись: temp + fsync + rename + fsync(dir).

use std::collections::{BTreeMap, BTreeSet};
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
    /// Произвольный payload транзакции (например, ledger состояний артефактов для rde-dag).
    #[serde(default)]
    pub data: Option<serde_json::Value>,
    /// Новые корни, которые транзакция переключает при commit (None — транзакция без корней).
    #[serde(default)]
    pub roots: Option<RootSet>,
    /// Транзакция была откачена (rollback) — для истории/UI.
    #[serde(default)]
    pub rolled_back: bool,
}

/// Набор корней хранилища: имя → tree_hash. Имена задаёт вызывающий слой
/// (например, `artifact/<id>`, `document/<id>`), store их не интерпретирует.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootSet {
    #[serde(default)]
    pub roots: BTreeMap<String, String>,
}

impl RootSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, name: &str) -> Option<Hash> {
        self.roots.get(name).and_then(|hex| Hash::from_hex(hex).ok())
    }

    pub fn insert(&mut self, name: impl Into<String>, tree: Hash) {
        self.roots.insert(name.into(), tree.hex());
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.roots.remove(name).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }

    pub fn len(&self) -> usize {
        self.roots.len()
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.roots.keys().map(String::as_str)
    }

    /// Пары (имя, tree_hash) с проверкой hex.
    pub fn trees(&self) -> Vec<(String, Hash)> {
        self.roots
            .iter()
            .filter_map(|(name, hex)| Hash::from_hex(hex).ok().map(|hash| (name.clone(), hash)))
            .collect()
    }
}

#[derive(Serialize, Deserialize)]
struct CurrentFile {
    #[serde(default)]
    txid: String,
    #[serde(default)]
    updated_unix: u64,
    #[serde(default)]
    roots: BTreeMap<String, String>,
}

/// Снапшот корней транзакции: что было до и что стало (для отката O(1)).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub txid: String,
    pub prev_roots: RootSet,
    pub new_roots: RootSet,
    pub committed_unix: u64,
}

/// Результат отката транзакции.
#[derive(Clone, Debug)]
pub struct RollbackReport {
    /// Транзакция отката (сама зафиксирована — её тоже можно откатить).
    pub rollback_tx: String,
    pub rolled_back_tx: String,
    pub previous: RootSet,
    pub restored: RootSet,
}

/// Кандидат на откат (для UI «Задачи»).
#[derive(Clone, Debug)]
pub struct RollbackCandidate {
    pub txid: String,
    pub note: String,
    pub updated_unix: Option<u64>,
    pub rolled_back: bool,
    pub roots: usize,
}

/// Pin корня: GC не удаляет помеченные деревья/блобы (экспорты, шаринги, R-ART-11).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pin {
    pub name: String,
    pub tree: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub created_unix: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct PinsFile {
    #[serde(default)]
    pins: BTreeMap<String, Pin>,
}

/// Параметры GC (mark-and-sweep от корней).
#[derive(Clone, Debug)]
pub struct GcOptions {
    /// Сколько последних транзакций с корнями всегда остаются в окне отката.
    pub keep_last_txs: usize,
    /// Окно отката по времени (дни).
    pub keep_days: u64,
    /// Только отчёт, ничего не удалять.
    pub dry_run: bool,
    /// Удалять failed-транзакции старше окна.
    pub prune_failed_txs: bool,
}

impl Default for GcOptions {
    fn default() -> Self {
        Self { keep_last_txs: 20, keep_days: 7, dry_run: false, prune_failed_txs: true }
    }
}

/// Отчёт GC.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GcReport {
    pub dry_run: bool,
    /// Транзакции, чьи корни остались в окне отката.
    pub kept_txs: usize,
    pub kept_snapshots: usize,
    pub removed_snapshots: usize,
    pub kept_pins: usize,
    pub marked_trees: usize,
    pub removed_trees: usize,
    pub marked_blobs: usize,
    pub removed_blobs: usize,
    pub freed_bytes: u64,
    pub pruned_failed_txs: usize,
}

/// Блокировка писателя: файл `.rde/lock` с pid; устаревшая блокировка (процесс убит) снимается.
///
/// Внутри одного процесса повторное открытие того же хранилища разрешено (общий реестр),
/// чтобы UI/тесты/инструменты не конфликтовали сами с собой; между процессами — эксклюзивно.
struct LockGuard {
    path: PathBuf,
    pid: u32,
    /// true — блокировка уже была взята этим процессом (файл не наш, не удаляем).
    shared: bool,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        if self.shared {
            return;
        }
        // Снимаем только свою блокировку (файл мог быть перехвачен после аварии).
        if let Ok(text) = fs::read_to_string(&self.path) {
            if text.trim() == self.pid.to_string() {
                let _ = fs::remove_file(&self.path);
            }
        }
        if let Ok(mut locks) = process_locks().lock() {
            locks.remove(&self.path);
        }
    }
}

/// Маркер zstd-сжатого блоба в CAS (имя файла по-прежнему sha256 от несжатых байт).
const BLOB_MAGIC: &[u8; 4] = b"RDEZ";
/// Минимальный размер блоба, который имеет смысл сжимать.
const BLOB_COMPRESS_MIN: usize = 1024;

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
    /// Эксклюзивная блокировка писателя; снимается при drop.
    _lock: LockGuard,
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
        let lock = acquire_lock(&store_dir)?;
        let conn = Connection::open(store_dir.join("index.db"))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\n\
             PRAGMA synchronous = NORMAL;\n\
             CREATE TABLE IF NOT EXISTS blobs (hash TEXT PRIMARY KEY, size INTEGER NOT NULL);\n\
             CREATE TABLE IF NOT EXISTS trees (hash TEXT PRIMARY KEY, entries INTEGER NOT NULL);\n\
             CREATE TABLE IF NOT EXISTS txs (id TEXT PRIMARY KEY, status TEXT NOT NULL, note TEXT NOT NULL, started_unix INTEGER NOT NULL, updated_unix INTEGER);",
        )?;
        let mut store = Self { root: project_root.to_path_buf(), store_dir, conn, recovery: RecoveryReport::default(), _lock: lock };
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
    ///
    /// Крупные и сжимаемые блобы хранятся zstd (маркер `RDEZ`); имя файла — sha256
    /// несжатых байт, поэтому адресация/дедупликация не меняются (tz-02 §8.4).
    pub fn put_blob(&mut self, bytes: &[u8]) -> Result<Hash> {
        let hash = sha256(bytes);
        if !self.has_blob(hash) {
            let path = self.cas_path(hash);
            let stored = encode_blob(bytes)?;
            write_atomic(&path, &stored)?;
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
        Ok(Some(decode_blob(&fs::read(path)?)?))
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
        self.begin_tx_with(note, None)
    }

    /// Начать транзакцию с payload (например, ledger состояний артефактов).
    pub fn begin_tx_with(&mut self, note: &str, data: Option<serde_json::Value>) -> Result<String> {
        let id = ulid::Ulid::generate().to_string();
        let record = TxRecord {
            id: id.clone(),
            status: TxStatus::Prepared,
            note: note.to_owned(),
            started_unix: now_unix(),
            updated_unix: None,
            data,
            roots: None,
            rolled_back: false,
        };
        self.write_tx(&record)?;
        Ok(id)
    }

    /// Зафиксировать транзакцию. Если у неё заданы новые корни — сначала сохраняется снапшот
    /// текущих корней, затем атомарно переключается `current.toml`, затем пишется журнал
    /// `committed` (§7.3 п.7). Такой порядок позволяет восстановиться после kill -9 на любом шаге.
    pub fn commit_tx(&mut self, id: &str) -> Result<()> {
        let mut record = self.read_tx(id)?.ok_or_else(|| StoreError::new(format!("транзакция {id} не найдена")))?;
        if record.status != TxStatus::Prepared {
            return Err(StoreError::new(format!("транзакция {id} уже завершена ({:?})", record.status)));
        }
        if let Some(new_roots) = record.roots.clone() {
            let prev_roots = self.current_roots()?;
            let snapshot = Snapshot {
                txid: id.to_owned(),
                prev_roots,
                new_roots: new_roots.clone(),
                committed_unix: now_unix(),
            };
            self.write_snapshot(&snapshot)?;
            self.write_current(&new_roots, id)?;
        }
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
        let mut records: Vec<TxRecord> = Vec::new();
        if let Ok(entries) = fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("json") {
                    continue;
                }
                let Ok(bytes) = fs::read(&path) else { continue };
                let Ok(record) = serde_json::from_slice::<TxRecord>(&bytes) else { continue };
                records.push(record);
            }
        }
        records.sort_by(|a, b| a.started_unix.cmp(&b.started_unix).then_with(|| a.id.cmp(&b.id)));
        for record in &records {
            report.reconciled += 1;
            if record.status == TxStatus::Prepared {
                // Транзакция с корнями: kill -9 мог прервать её на шаге переключения current.
                // Если current уже равен новым корням — commit фактически состоялся, дозавершаем;
                // иначе транзакция не переключила корни — помечаем failed.
                let resolved = if let Some(new_roots) = &record.roots {
                    if self.read_snapshot(&record.id)?.is_some() && self.current_roots()? == *new_roots {
                        let mut record = record.clone();
                        record.status = TxStatus::Committed;
                        record.updated_unix = Some(now_unix());
                        self.write_tx(&record)?;
                        true
                    } else {
                        let mut record = record.clone();
                        record.status = TxStatus::Failed;
                        record.updated_unix = Some(now_unix());
                        record.note = format!("{}; прервано до переключения корней", record.note);
                        self.write_tx(&record)?;
                        true
                    }
                } else {
                    false
                };
                if !resolved {
                    report.incomplete.push(record.id.clone());
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
            let Ok(stored) = fs::read(&path) else { continue };
            let Ok(bytes) = decode_blob(&stored) else {
                report.mismatched.push(path.display().to_string());
                continue;
            };
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

/// Корни, снапшоты, откат, pins и GC (P2): транзакционное ядро хранилища.
impl Store {
    fn current_path(&self) -> PathBuf {
        self.store_dir.join("current.toml")
    }

    fn snapshot_path(&self, txid: &str) -> PathBuf {
        self.store_dir.join("snapshots").join(format!("{txid}.json"))
    }

    fn pins_path(&self) -> PathBuf {
        self.store_dir.join("pins.json")
    }

    /// Текущие корни (`current.toml`); пустой набор, если файла ещё нет.
    pub fn current_roots(&self) -> Result<RootSet> {
        let path = self.current_path();
        if !path.is_file() {
            return Ok(RootSet::new());
        }
        let text = fs::read_to_string(&path)?;
        let file: CurrentFile = toml::from_str(&text).map_err(|e| StoreError::new(format!("current.toml: {e}")))?;
        Ok(RootSet { roots: file.roots })
    }

    /// Транзакция последнего переключения корней (для UI/диагностики).
    pub fn current_txid(&self) -> Result<Option<String>> {
        let path = self.current_path();
        if !path.is_file() {
            return Ok(None);
        }
        let text = fs::read_to_string(&path)?;
        let file: CurrentFile = toml::from_str(&text).map_err(|e| StoreError::new(format!("current.toml: {e}")))?;
        Ok(if file.txid.is_empty() { None } else { Some(file.txid) })
    }

    fn write_current(&self, roots: &RootSet, txid: &str) -> Result<()> {
        let file = CurrentFile {
            txid: txid.to_owned(),
            updated_unix: now_unix(),
            roots: roots.roots.clone(),
        };
        let text = toml::to_string(&file).map_err(|e| StoreError::new(format!("current.toml: {e}")))?;
        write_atomic(&self.current_path(), text.as_bytes())
    }

    fn write_snapshot(&self, snapshot: &Snapshot) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(snapshot).map_err(|e| StoreError::new(format!("json снапшота: {e}")))?;
        write_atomic(&self.snapshot_path(&snapshot.txid), &bytes)
    }

    pub fn read_snapshot(&self, txid: &str) -> Result<Option<Snapshot>> {
        let path = self.snapshot_path(txid);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = fs::read(&path)?;
        Ok(Some(serde_json::from_slice(&bytes).map_err(|e| StoreError::new(format!("json снапшота {txid}: {e}")))?))
    }

    /// Начать транзакцию с новыми корнями: commit переключит current.toml (см. [`Store::commit_tx`]).
    pub fn begin_tx_with_roots(&mut self, note: &str, data: Option<serde_json::Value>, roots: RootSet) -> Result<String> {
        let id = ulid::Ulid::generate().to_string();
        let record = TxRecord {
            id: id.clone(),
            status: TxStatus::Prepared,
            note: note.to_owned(),
            started_unix: now_unix(),
            updated_unix: None,
            data,
            roots: Some(roots),
            rolled_back: false,
        };
        self.write_tx(&record)?;
        Ok(id)
    }

    /// Кандидаты на откат: зафиксированные транзакции со снапшотом корней (новые первыми).
    pub fn rollback_candidates(&self) -> Result<Vec<RollbackCandidate>> {
        let mut result = Vec::new();
        for record in self.list_txs()? {
            if record.status != TxStatus::Committed || record.roots.is_none() {
                continue;
            }
            if self.read_snapshot(&record.id)?.is_none() {
                continue;
            }
            result.push(RollbackCandidate {
                txid: record.id.clone(),
                note: record.note.clone(),
                updated_unix: record.updated_unix,
                rolled_back: record.rolled_back,
                roots: record.roots.as_ref().map(|roots| roots.len()).unwrap_or(0),
            });
        }
        Ok(result)
    }

    /// Откатить транзакцию: O(1) — current.toml переключается на предыдущие корни из снапшота.
    /// Сам откат фиксируется транзакцией (его тоже можно откатить).
    pub fn rollback(&mut self, txid: &str) -> Result<RollbackReport> {
        let record = self.read_tx(txid)?.ok_or_else(|| StoreError::new(format!("транзакция {txid} не найдена")))?;
        if record.status != TxStatus::Committed {
            return Err(StoreError::new(format!("откатить можно только зафиксированную транзакцию ({txid}: {:?})", record.status)));
        }
        if record.rolled_back {
            return Err(StoreError::new(format!("транзакция {txid} уже откачена")));
        }
        let snapshot = self
            .read_snapshot(txid)?
            .ok_or_else(|| StoreError::new(format!("у транзакции {txid} нет снапшота корней — откат невозможен")))?;
        let previous = self.current_roots()?;
        let payload = serde_json::json!({ "kind": "rollback", "rolled_back": txid });
        let rollback_tx = self.begin_tx_with_roots(&format!("откат: {}", record.note), Some(payload), snapshot.prev_roots.clone())?;
        self.commit_tx(&rollback_tx)?;
        let mut original = record;
        original.rolled_back = true;
        original.updated_unix = Some(now_unix());
        self.write_tx(&original)?;
        Ok(RollbackReport {
            rollback_tx,
            rolled_back_tx: txid.to_owned(),
            previous,
            restored: snapshot.prev_roots,
        })
    }

    /// Откатить последнюю зафиксированную транзакцию с корнями.
    pub fn rollback_latest(&mut self) -> Result<Option<RollbackReport>> {
        let candidate = self
            .rollback_candidates()?
            .into_iter()
            .find(|candidate| !candidate.rolled_back);
        match candidate {
            Some(candidate) => Ok(Some(self.rollback(&candidate.txid)?)),
            None => Ok(None),
        }
    }

    /// Все pins (экспорты/шаринги, R-ART-11).
    pub fn pins(&self) -> Result<Vec<Pin>> {
        let path = self.pins_path();
        if !path.is_file() {
            return Ok(Vec::new());
        }
        let bytes = fs::read(&path)?;
        let file: PinsFile = serde_json::from_slice(&bytes).map_err(|e| StoreError::new(format!("pins.json: {e}")))?;
        Ok(file.pins.into_values().collect())
    }

    fn write_pins(&self, pins: &BTreeMap<String, Pin>) -> Result<()> {
        let bytes = serde_json::to_vec_pretty(&PinsFile { pins: pins.clone() })
            .map_err(|e| StoreError::new(format!("pins.json: {e}")))?;
        write_atomic(&self.pins_path(), &bytes)
    }

    /// Закрепить корень: GC не удалит его дерево и блобы.
    pub fn pin_root(&mut self, name: &str, tree: Hash, note: &str) -> Result<()> {
        let mut pins: BTreeMap<String, Pin> = self.pins()?.into_iter().map(|pin| (pin.name.clone(), pin)).collect();
        pins.insert(
            name.to_owned(),
            Pin { name: name.to_owned(), tree: tree.hex(), note: note.to_owned(), created_unix: now_unix() },
        );
        self.write_pins(&pins)
    }

    /// Снять pin. Возвращает `true`, если pin был.
    pub fn unpin_root(&mut self, name: &str) -> Result<bool> {
        let mut pins: BTreeMap<String, Pin> = self.pins()?.into_iter().map(|pin| (pin.name.clone(), pin)).collect();
        let removed = pins.remove(name).is_some();
        if removed {
            self.write_pins(&pins)?;
        }
        Ok(removed)
    }

    /// Mark-and-sweep GC от корней (§7.3 п.8): current roots + окно отката + pins.
    /// Pins не удаляются никогда; отчёт — до/после. `dry_run` ничего не удаляет.
    pub fn gc(&mut self, options: &GcOptions) -> Result<GcReport> {
        let mut report = GcReport { dry_run: options.dry_run, ..GcReport::default() };
        let now = now_unix();
        let keep_secs = options.keep_days.saturating_mul(24 * 3600);
        let current = self.current_roots()?;
        let pins = self.pins()?;
        report.kept_pins = pins.len();

        let txs = self.list_txs()?;
        let mut kept_roots: BTreeSet<Hash> = BTreeSet::new();
        for (_, tree) in current.trees() {
            kept_roots.insert(tree);
        }
        for pin in &pins {
            if let Ok(tree) = Hash::from_hex(&pin.tree) {
                kept_roots.insert(tree);
            }
        }

        // Окно отката: последние N транзакций с корнями + всё моложе keep_days.
        let mut with_roots: Vec<&TxRecord> =
            txs.iter().filter(|tx| tx.status == TxStatus::Committed && tx.roots.is_some()).collect();
        with_roots.sort_by(|a, b| b.started_unix.cmp(&a.started_unix).then_with(|| b.id.cmp(&a.id)));
        let mut keep_tx_ids: BTreeSet<String> = BTreeSet::new();
        for (index, tx) in with_roots.iter().enumerate() {
            // keep_days = 0 — окно по времени отключено (остаётся только keep_last_txs).
            let fresh = options.keep_days > 0 && now.saturating_sub(tx.started_unix) <= keep_secs;
            if index < options.keep_last_txs || fresh {
                keep_tx_ids.insert(tx.id.clone());
            }
        }
        report.kept_txs = keep_tx_ids.len();

        // Снапшоты: keep — из окна; остальные удаляем; деревья из kept-снапшотов защищаем.
        let snapshots_dir = self.store_dir.join("snapshots");
        if let Ok(entries) = fs::read_dir(&snapshots_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(txid) = path.file_stem().and_then(|name| name.to_str()) else { continue };
                if keep_tx_ids.contains(txid) {
                    report.kept_snapshots += 1;
                    if let Some(snapshot) = self.read_snapshot(txid)? {
                        for (_, tree) in snapshot.prev_roots.trees().into_iter().chain(snapshot.new_roots.trees()) {
                            kept_roots.insert(tree);
                        }
                    }
                } else {
                    report.removed_snapshots += 1;
                    if !options.dry_run {
                        let _ = fs::remove_file(&path);
                    }
                }
            }
        }

        // Блобы, защищённые ledger-состояниями транзакций окна отката.
        let mut kept_blobs: BTreeSet<Hash> = BTreeSet::new();
        for tx in &txs {
            if !keep_tx_ids.contains(&tx.id) {
                continue;
            }
            let Some(data) = &tx.data else { continue };
            let Some(items) = data.get("artifacts").and_then(|value| value.as_array()) else { continue };
            for item in items {
                let Some(outputs) = item.get("outputs").and_then(|value| value.as_array()) else { continue };
                for output in outputs {
                    if let Some(blob) = output.get("blob").and_then(|value| value.as_str()) {
                        if let Ok(hash) = Hash::from_hex(blob) {
                            kept_blobs.insert(hash);
                        }
                    }
                }
            }
        }

        // Деревья: защищённые читаем и помечаем их блобы; остальные — кандидаты на удаление.
        for tree in &kept_roots {
            if let Some(entries) = self.get_tree(*tree)? {
                for entry in entries {
                    kept_blobs.insert(entry.blob);
                }
            }
        }
        report.marked_trees = kept_roots.len();
        report.marked_blobs = kept_blobs.len();

        let trees_root = self.store_dir.join("trees");
        let mut tree_files = Vec::new();
        collect_files(&trees_root, &mut tree_files);
        for path in tree_files {
            let Some(stem) = path.file_stem().and_then(|name| name.to_str()) else { continue };
            let Ok(hash) = Hash::from_hex(stem) else { continue };
            if kept_roots.contains(&hash) {
                continue;
            }
            report.removed_trees += 1;
            if !options.dry_run {
                let _ = fs::remove_file(&path);
            }
        }

        let cas_root = self.store_dir.join("cas/sha256");
        let mut blob_files = Vec::new();
        collect_files(&cas_root, &mut blob_files);
        for path in blob_files {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
            if name.starts_with(".rde-tmp-") {
                continue;
            }
            let Ok(hash) = Hash::from_hex(name) else { continue };
            if kept_blobs.contains(&hash) {
                continue;
            }
            report.removed_blobs += 1;
            report.freed_bytes += path.metadata().map(|meta| meta.len()).unwrap_or(0);
            if !options.dry_run {
                let _ = fs::remove_file(&path);
            }
        }

        // Failed-транзакции старше окна (по желанию) — журнал не хранит мусор.
        if options.prune_failed_txs {
            for tx in &txs {
                if tx.status != TxStatus::Failed {
                    continue;
                }
                let old = now.saturating_sub(tx.started_unix) > keep_secs;
                if old && !options.dry_run {
                    let _ = fs::remove_file(self.tx_path(&tx.id));
                    report.pruned_failed_txs += 1;
                } else if old {
                    report.pruned_failed_txs += 1;
                }
            }
        }

        if !options.dry_run {
            // Индекс пересобирается из файлов — источник истины остаётся файловым.
            self.rebuild_index()?;
        }
        Ok(report)
    }
}

/// zstd-сжатие блоба (если выгодно): `RDEZ` + zstd-кадр.
fn encode_blob(bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() >= BLOB_COMPRESS_MIN {
        if let Ok(compressed) = zstd::bulk::compress(bytes, 3) {
            if compressed.len() + BLOB_MAGIC.len() < bytes.len() {
                let mut stored = Vec::with_capacity(BLOB_MAGIC.len() + compressed.len());
                stored.extend_from_slice(BLOB_MAGIC);
                stored.extend_from_slice(&compressed);
                return Ok(stored);
            }
        }
    }
    Ok(bytes.to_vec())
}

/// Декодирование блоба: снимаем `RDEZ`-обёртку при наличии.
fn decode_blob(stored: &[u8]) -> Result<Vec<u8>> {
    if stored.starts_with(BLOB_MAGIC) {
        return zstd::stream::decode_all(std::io::Cursor::new(&stored[BLOB_MAGIC.len()..]))
            .map_err(|e| StoreError::new(format!("zstd-блоб повреждён: {e}")));
    }
    Ok(stored.to_vec())
}

/// Реестр хранилищ, уже открытых этим процессом (для повторного открытия без конфликта).
fn process_locks() -> &'static std::sync::Mutex<BTreeSet<PathBuf>> {
    static LOCKS: std::sync::OnceLock<std::sync::Mutex<BTreeSet<PathBuf>>> = std::sync::OnceLock::new();
    LOCKS.get_or_init(|| std::sync::Mutex::new(BTreeSet::new()))
}

/// Взять эксклюзивную блокировку писателя. Устаревший lock (процесс убит) снимается.
fn acquire_lock(store_dir: &Path) -> Result<LockGuard> {
    let path = store_dir.join("lock");
    let own_pid = std::process::id();
    // Повторное открытие в том же процессе: разделяем уже взятую блокировку.
    if let Ok(locks) = process_locks().lock() {
        if locks.contains(&path) {
            return Ok(LockGuard { path, pid: own_pid, shared: true });
        }
    }
    for _ in 0..3 {
        match fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                let _ = writeln!(file, "{own_pid}");
                let _ = file.sync_all();
                if let Ok(mut locks) = process_locks().lock() {
                    locks.insert(path.clone());
                }
                return Ok(LockGuard { path, pid: own_pid, shared: false });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                let holder = fs::read_to_string(&path).ok().and_then(|text| text.trim().parse::<u32>().ok());
                if let Some(pid) = holder {
                    if process_alive(pid) {
                        return Err(StoreError::new(format!(
                            "хранилище занято другим процессом RDE (pid {pid}); закройте его.                              Если процесс завершился аварийно — удалите {}",
                            path.display()
                        )));
                    }
                }
                let _ = fs::remove_file(&path);
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(StoreError::new(format!("не удалось взять блокировку {}", path.display())))
}

/// Жив ли процесс-держатель блокировки (Linux: /proc; иначе — консервативно считаем живым).
fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
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
