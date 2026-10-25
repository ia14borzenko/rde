//! P2: транзакционное ядро store — roots/snapshots/rollback O(1), GC/pins, zstd, lock.

use std::path::PathBuf;

use rde_hash::{sha256, TreeEntry};
use rde_store::{GcOptions, RootSet, Store, TxStatus};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-store-p2-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn tree(store: &mut Store, name: &str, bytes: &[u8]) -> rde_hash::Hash {
    let blob = store.put_blob(bytes).unwrap();
    store.put_tree(&[TreeEntry::new(name, 0o644, blob)]).unwrap()
}

#[test]
fn blobs_are_compressed_when_worth_it_and_roundtrip() {
    let root = tmp("zstd");
    let mut store = Store::open(&root).unwrap();
    // Сжимаемый крупный блоб: в CAS должен лежать zstd-кадр (маркер RDEZ), меньше исходного.
    let raw = vec![b'a'; 100_000];
    let hash = store.put_blob(&raw).unwrap();
    assert_eq!(hash, sha256(&raw));
    let hex = hash.hex();
    let path = root.join(".rde/cas/sha256").join(&hex[0..2]).join(&hex[2..4]).join(&hex);
    let stored = std::fs::read(&path).unwrap();
    assert!(stored.starts_with(b"RDEZ"), "крупный сжимаемый блоб должен быть zstd");
    assert!(stored.len() < raw.len(), "{} vs {}", stored.len(), raw.len());
    // Чтение/проверка/индекс понимают сжатие.
    assert_eq!(store.get_blob(hash).unwrap().unwrap(), raw);
    assert!(store.verify_blob(hash).unwrap());
    let report = store.rebuild_index().unwrap();
    assert!(report.mismatched.is_empty(), "{report:?}");
    assert_eq!(report.blobs, 1);
    // Мелкий блоб хранится как есть.
    let small = store.put_blob(b"tiny").unwrap();
    let hex = small.hex();
    let small_path = root.join(".rde/cas/sha256").join(&hex[0..2]).join(&hex[2..4]).join(&hex);
    assert_eq!(std::fs::read(small_path).unwrap(), b"tiny");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn commit_switches_roots_and_rollback_is_cheap() {
    let root = tmp("rollback");
    let mut store = Store::open(&root).unwrap();
    let t1 = tree(&mut store, "out/a.txt", b"v1");
    let t2 = tree(&mut store, "out/a.txt", b"v2");

    let mut roots1 = RootSet::new();
    roots1.insert("artifact/a", t1);
    let tx1 = store.begin_tx_with_roots("update a v1", None, roots1).unwrap();
    store.commit_tx(&tx1).unwrap();
    assert_eq!(store.current_roots().unwrap().get("artifact/a"), Some(t1));
    assert_eq!(store.current_txid().unwrap().as_deref(), Some(tx1.as_str()));

    let mut roots2 = store.current_roots().unwrap();
    roots2.insert("artifact/a", t2);
    let tx2 = store.begin_tx_with_roots("update a v2", None, roots2).unwrap();
    store.commit_tx(&tx2).unwrap();
    assert_eq!(store.current_roots().unwrap().get("artifact/a"), Some(t2));
    assert!(store.read_snapshot(&tx2).unwrap().is_some(), "снапшот для отката");

    // O(1)-откат: current переключается на предыдущие корни, снапшот предыдущей транзакции цел.
    let report = store.rollback(&tx2).unwrap();
    assert_eq!(store.current_roots().unwrap().get("artifact/a"), Some(t1));
    assert_eq!(report.restored.get("artifact/a"), Some(t1));
    assert_eq!(report.previous.get("artifact/a"), Some(t2));
    assert!(store.read_tx(&tx2).unwrap().unwrap().rolled_back);
    // Повторный откат той же транзакции запрещён.
    assert!(store.rollback(&tx2).is_err());
    // Откат сам является транзакцией: его тоже можно откатить (возвращает v2).
    let undo = store.rollback(&report.rollback_tx).unwrap();
    assert_eq!(store.current_roots().unwrap().get("artifact/a"), Some(t2));
    assert_eq!(undo.rolled_back_tx, report.rollback_tx);
    // Кандидаты для UI: есть откатываемая транзакция и пометки rolled_back.
    let candidates = store.rollback_candidates().unwrap();
    assert!(candidates.iter().any(|c| c.txid == tx1 && !c.rolled_back));
    assert!(candidates.iter().any(|c| c.txid == tx2 && c.rolled_back));
    // Деревья/блобы обеих версий доступны: откат не удаляет данные.
    assert!(store.get_tree(t1).unwrap().is_some());
    assert!(store.get_tree(t2).unwrap().is_some());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn prepared_tx_with_switched_roots_is_completed_on_recovery() {
    let root = tmp("recover-roots");
    let mut store = Store::open(&root).unwrap();
    let t1 = tree(&mut store, "out/a.txt", b"v1");
    let tx = {
        let mut roots = RootSet::new();
        roots.insert("artifact/a", t1);
        let tx = store.begin_tx_with_roots("update a", None, roots.clone()).unwrap();
        // Имитация kill -9 между переключением current и записью journal "committed":
        // снапшот и current.toml уже записаны, статус транзакции — prepared.
        let snapshot = serde_json::json!({
            "txid": tx,
            "prev_roots": { "roots": {} },
            "new_roots": roots,
            "committed_unix": 1
        });
        std::fs::write(root.join(format!(".rde/snapshots/{tx}.json")), serde_json::to_vec(&snapshot).unwrap()).unwrap();
        std::fs::write(
            root.join(".rde/current.toml"),
            format!("txid = \"{tx}\"\nupdated_unix = 1\n\n[roots]\n\"artifact/a\" = \"{}\"\n", t1.hex()),
        )
        .unwrap();
        tx
    };
    drop(store);
    let store = Store::open(&root).unwrap();
    assert!(store.recovery.incomplete.is_empty(), "транзакция дозавершена: {:?}", store.recovery.incomplete);
    assert_eq!(store.read_tx(&tx).unwrap().unwrap().status, TxStatus::Committed);
    assert_eq!(store.current_roots().unwrap().get("artifact/a"), Some(t1));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn gc_keeps_current_window_pins_and_reports() {
    let root = tmp("gc");
    let mut store = Store::open(&root).unwrap();
    let old_tree = tree(&mut store, "out/old.txt", b"old");
    let current_tree = tree(&mut store, "out/current.txt", b"current");
    let pinned_tree = tree(&mut store, "out/pinned.txt", b"pinned");
    let orphan_blob = store.put_blob(b"orphan blob without tree").unwrap();
    let orphan_tree = store.put_tree(&[TreeEntry::new("out/orphan.txt", 0o644, orphan_blob)]).unwrap();
    let orphan_hash = sha256(b"unreferenced blob");
    store.put_blob(b"unreferenced blob").unwrap();

    // История: tx_old (вне окна) → tx_current (текущие корни).
    let mut roots_old = RootSet::new();
    roots_old.insert("artifact/old", old_tree);
    let tx_old = store.begin_tx_with_roots("old", None, roots_old).unwrap();
    store.commit_tx(&tx_old).unwrap();
    let mut roots_current = RootSet::new();
    roots_current.insert("artifact/current", current_tree);
    let tx_current = store.begin_tx_with_roots("current", None, roots_current).unwrap();
    store.commit_tx(&tx_current).unwrap();
    store.pin_root("export/pinned", pinned_tree, "экспорт").unwrap();
    assert_eq!(store.pins().unwrap().len(), 1);

    // dry-run: ничего не удаляет, но показывает объём.
    let dry = store.gc(&GcOptions { dry_run: true, ..GcOptions::default() }).unwrap();
    assert!(dry.dry_run);
    assert!(dry.removed_blobs >= 1, "{dry:?}");
    assert!(store.get_blob(orphan_hash).unwrap().is_some(), "dry-run не удаляет");
    assert!(store.get_tree(orphan_tree).unwrap().is_some());

    // Реальный GC с окном 0: удаляются только не-корневые/не-pinned деревья и блобы.
    let report = store.gc(&GcOptions { keep_last_txs: 0, keep_days: 0, dry_run: false, prune_failed_txs: true }).unwrap();
    assert!(report.removed_blobs >= 1, "{report:?}");
    assert!(report.removed_trees >= 1, "{report:?}");
    assert!(report.freed_bytes > 0);
    assert_eq!(report.kept_pins, 1);
    assert!(store.get_tree(current_tree).unwrap().is_some(), "current root защищён");
    assert!(store.get_tree(pinned_tree).unwrap().is_some(), "pin защищён");
    assert!(store.get_tree(orphan_tree).unwrap().is_none(), "сиротское дерево удалено");
    assert!(store.get_blob(orphan_hash).unwrap().is_none(), "сиротский блоб удалён");
    assert!(store.get_tree(old_tree).unwrap().is_none(), "старое окно отката истекло");
    // Текущие корни и индекс после GC согласованы.
    assert_eq!(store.current_roots().unwrap().get("artifact/current"), Some(current_tree));
    let stats = store.stats().unwrap();
    assert_eq!(stats.trees, 2, "current + pinned: {stats:?}");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn lock_excludes_other_processes_and_steals_stale() {
    let root = tmp("lock");
    std::fs::create_dir_all(root.join(".rde")).unwrap();
    // Живой pid (текущий процесс) — хранилище занято.
    std::fs::write(root.join(".rde/lock"), format!("{}\n", std::process::id())).unwrap();
    let error = Store::open(&root).err().expect("живой lock должен блокировать открытие");
    assert!(error.to_string().contains("занято"), "{error}");
    // Мёртвый pid — устаревшая блокировка снимается.
    std::fs::write(root.join(".rde/lock"), "999999999\n").unwrap();
    let store = Store::open(&root).unwrap();
    assert!(root.join(".rde/lock").is_file(), "свой lock взят");
    drop(store);
    assert!(!root.join(".rde/lock").is_file(), "lock снят при закрытии");
    let _ = std::fs::remove_dir_all(&root);
}
