//! Интеграционные тесты хранилища: CAS, деревья, журнал, пересборка индекса.

use std::path::PathBuf;

use rde_hash::{sha256, TreeEntry};
use rde_store::{Store, TxStatus};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-store-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn blob_roundtrip_dedupe_and_cas_layout() {
    let root = tmp("blob");
    let mut store = Store::open(&root).unwrap();
    let hash = store.put_blob(b"hello rde").unwrap();
    assert_eq!(hash, sha256(b"hello rde"));
    assert!(store.has_blob(hash));
    assert_eq!(store.get_blob(hash).unwrap().unwrap(), b"hello rde");
    assert!(store.verify_blob(hash).unwrap());

    // Дедупликация: тот же блоб второй раз не создаёт новый файл.
    let hash2 = store.put_blob(b"hello rde").unwrap();
    assert_eq!(hash, hash2);
    let hex = hash.hex();
    let cas = root.join(".rde/cas/sha256").join(&hex[0..2]).join(&hex[2..4]).join(&hex);
    assert!(cas.is_file(), "CAS-путь не совпал: {}", cas.display());
    // Другие блобы — отдельные файлы.
    let other = store.put_blob(b"other").unwrap();
    assert_ne!(hash, other);
    assert_eq!(store.stats().unwrap().blobs, 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn tree_roundtrip_and_hash_matches_rde_hash() {
    let root = tmp("tree");
    let mut store = Store::open(&root).unwrap();
    let a = store.put_blob(b"a").unwrap();
    let b = store.put_blob(b"b").unwrap();
    let entries = vec![TreeEntry::new("out/a.svg", 0o644, a), TreeEntry::new("out/b.png", 0o644, b)];
    let hash = store.put_tree(&entries).unwrap();
    assert_eq!(hash, rde_hash::tree_hash(&entries));
    let loaded = store.get_tree(hash).unwrap().unwrap();
    assert_eq!(loaded.len(), 2);
    assert_eq!(rde_hash::tree_hash(&loaded), hash);
    assert!(store.get_tree(sha256(b"missing")).unwrap().is_none());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn tx_journal_commit_and_crash_recovery_report() {
    let root = tmp("tx");
    let mut store = Store::open(&root).unwrap();
    let tx = store.begin_tx("test transaction").unwrap();
    assert!(root.join(format!(".rde/updates/{tx}.json")).is_file());
    // Имитация kill -9: открываем хранилище заново, транзакция осталась prepared.
    drop(store);
    let store = Store::open(&root).unwrap();
    assert_eq!(store.recovery.incomplete, vec![tx.clone()]);
    assert_eq!(store.incomplete_txs().unwrap(), vec![tx.clone()]);
    let record = store.read_tx(&tx).unwrap().unwrap();
    assert_eq!(record.status, TxStatus::Prepared);

    let mut store = store;
    store.commit_tx(&tx).unwrap();
    assert_eq!(store.read_tx(&tx).unwrap().unwrap().status, TxStatus::Committed);
    assert!(store.incomplete_txs().unwrap().is_empty());
    drop(store);
    let store = Store::open(&root).unwrap();
    assert!(store.recovery.incomplete.is_empty(), "{:?}", store.recovery);

    let failed = {
        let mut store = store;
        let id = store.begin_tx("will fail").unwrap();
        store.fail_tx(&id, "script exit=1").unwrap();
        id
    };
    let store = Store::open(&root).unwrap();
    assert_eq!(store.read_tx(&failed).unwrap().unwrap().status, TxStatus::Failed);
    assert!(store.list_txs().unwrap().len() >= 2);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn tx_payload_survives_reopen_and_rebuild() {
    let root = tmp("tx-payload");
    let mut store = Store::open(&root).unwrap();
    let payload = serde_json::json!({"artifacts": [{"id": "a1", "act_key": "abc"}]});
    let tx = store.begin_tx_with("ledger", Some(payload.clone())).unwrap();
    store.commit_tx(&tx).unwrap();
    drop(store);

    // payload читается из файла журнала и переживает пересборку индекса.
    let mut store = Store::open(&root).unwrap();
    store.rebuild_index().unwrap();
    let record = store.read_tx(&tx).unwrap().unwrap();
    assert_eq!(record.status, TxStatus::Committed);
    assert_eq!(record.data, Some(payload));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn index_recovers_from_files_and_detects_corruption() {
    let root = tmp("index");
    {
        let mut store = Store::open(&root).unwrap();
        let blob = store.put_blob(b"data").unwrap();
        store.put_tree(&[TreeEntry::new("out/data.bin", 0o644, blob)]).unwrap();
        let _ = store.begin_tx("tx").unwrap();
        assert_eq!(store.stats().unwrap().blobs, 1);
    }
    // Удаляем индекс: Store::open должен пересобрать его из файлов.
    std::fs::remove_file(root.join(".rde/index.db")).unwrap();
    let mut store = Store::open(&root).unwrap();
    let stats = store.stats().unwrap();
    assert_eq!(stats.blobs, 1, "индекс не восстановлен из CAS");
    assert_eq!(stats.trees, 1);
    assert!(stats.txs >= 1);

    // Порча блоба: rebuild_index сообщает о несовпадении содержимого и имени.
    let hash = store.put_blob(b"will corrupt").unwrap();
    let hex = hash.hex();
    let cas = root.join(".rde/cas/sha256").join(&hex[0..2]).join(&hex[2..4]).join(&hex);
    std::fs::write(&cas, b"corrupted").unwrap();
    let report = store.rebuild_index().unwrap();
    assert_eq!(report.mismatched.len(), 1, "{report:?}");
    assert!(report.blobs >= 1 && report.trees == 1);
    let _ = std::fs::remove_dir_all(&root);
}
