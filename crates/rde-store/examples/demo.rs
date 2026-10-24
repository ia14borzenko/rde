//! Демонстрация хранилища на демо-проекте: кладём rde.toml в CAS, пишем дерево,
//! показываем статистику и отчёт пересборки индекса.
//!
//! ```bash
//! cargo run -p rde-store --example demo
//! ```

use std::path::PathBuf;

use rde_hash::TreeEntry;
use rde_store::Store;

fn main() {
    let root = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../demo-project")
    });
    println!("проект: {}", root.display());
    let mut store = Store::open(&root).expect("открыть хранилище");
    println!("recovery: {:?}", store.recovery);

    let manifest = root.join("rde.toml");
    let blob = store.put_file(&manifest).expect("put rde.toml");
    println!("blob rde.toml: {} ({} байт)", blob, std::fs::metadata(&manifest).map(|m| m.len()).unwrap_or(0));

    let tree = store
        .put_tree(&[TreeEntry::new("rde.toml", 0o644, blob)])
        .expect("put tree");
    println!("tree: {tree}");
    println!("stats: {:?}", store.stats().expect("stats"));
    let report = store.rebuild_index().expect("rebuild index");
    println!("index report: {report:?}");
    println!(".rde: {}", root.join(".rde").display());
}
