//! kill -9-харнесс: убиваем процесс посреди записи в CAS и проверяем, что хранилище
//! остаётся целым (все блобы соответствуют именам; индекс пересобирается из файлов).

use std::process::{Command, Stdio};
use std::time::Duration;

use rde_store::Store;

#[test]
fn kill9_leaves_store_consistent() {
    if std::env::var_os("RDE_STORE_KILL9_CHILD").is_some() {
        // Ребёнок: непрерывно пишет блобы, пока родитель не убьёт его SIGKILL.
        let dir = std::env::var_os("RDE_STORE_KILL9_DIR").expect("RDE_STORE_KILL9_DIR в ребёнке");
        let mut store = Store::open(std::path::Path::new(&dir)).expect("child store");
        let mut i: u64 = 0;
        loop {
            store.put_blob(format!("kill9-child-{i}").as_bytes()).expect("child put_blob");
            i += 1;
        }
    }

    let dir = std::env::temp_dir().join(format!("rde-store-kill9-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let exe = std::env::current_exe().unwrap();
    let mut child = Command::new(exe)
        .args(["--exact", "kill9_leaves_store_consistent", "--nocapture"])
        .env("RDE_STORE_KILL9_CHILD", "1")
        .env("RDE_STORE_KILL9_DIR", &dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn child");
    std::thread::sleep(Duration::from_millis(300));
    child.kill().expect("SIGKILL child"); // на Unix это SIGKILL, процесс не успевает корректно завершиться
    let _ = child.wait();

    let mut store = Store::open(&dir).expect("store opens after kill -9");
    let report = store.rebuild_index().expect("rebuild index");
    assert!(report.mismatched.is_empty(), "битые блобы после kill -9: {:?}", report.mismatched);
    let stats = store.stats().unwrap();
    assert!(stats.blobs > 0, "ребёнок не успел записать ни одного блоба");
    // Владение/перезапись: все блобы проходят проверку содержимого.
    for hash in store.blob_hashes().unwrap() {
        assert!(store.verify_blob(hash).unwrap(), "блоб {hash} повреждён");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
