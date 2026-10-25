//! Headless-обновление артефактов проекта (без UI): `cargo run -p rde-dag --example update -- <project>`
//!
//! Печатает отчёт по каждому артефакту и сводку; используется для проверки DAG в CI/скриптах
//! и для подготовки демо-снимков. Проект берётся из аргумента или `RDE_PROJECT`.

use std::path::PathBuf;

use rde_dag::{update, UpdateAction, UpdateOptions};

fn main() {
    let mut force = false;
    let mut root_arg: Option<PathBuf> = None;
    for arg in std::env::args().skip(1) {
        if arg == "--force" {
            force = true;
        } else {
            root_arg = Some(PathBuf::from(arg));
        }
    }
    let root = root_arg
        .or_else(|| std::env::var_os("RDE_PROJECT").map(PathBuf::from))
        .unwrap_or_else(|| rde_core::demo_project_dir());
    let project = match rde_core::load_project(&root) {
        Ok(project) => project,
        Err(error) => {
            eprintln!("проект {} не загружается: {error}", root.display());
            std::process::exit(1);
        }
    };
    let mut store = match rde_store::Store::open(&root) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("хранилище не открывается: {error}");
            std::process::exit(1);
        }
    };
    let options = UpdateOptions { force, ..UpdateOptions::default() };
    let report = match update(&project, &mut store, &options) {
        Ok(report) => report,
        Err(error) => {
            eprintln!("обновление не выполнено: {error}");
            std::process::exit(1);
        }
    };
    for artifact in &report.artifacts {
        let action = match artifact.action {
            UpdateAction::Ran => "обновлён",
            UpdateAction::Skipped => "актуален",
            UpdateAction::Failed => "ошибка",
        };
        println!("{action:>9}  {}  ({})", artifact.title, artifact.message);
    }
    for error in &report.errors {
        eprintln!("ошибка: {error}");
    }
    println!(
        "итого: выполнено {}, пропущено {}, ошибок {}; транзакций: {}",
        report.ran(),
        report.skipped(),
        report.failed(),
        report.tx_ids.len()
    );
    if report.failed() > 0 {
        std::process::exit(2);
    }
}
