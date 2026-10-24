//! Демонстрация M2.3: статусы из хешей, реальное обновление цепочки артефактов,
//! повторный запуск без работы, транзакции в `.rde/updates`.
//!
//! ```bash
//! cargo run -p rde-dag --example update-demo
//! ```

use rde_core::{demo_project_dir, load_project};
use rde_dag::{evaluate, update, UpdateOptions};
use rde_store::Store;

fn print_statuses(project: &rde_core::Project, store: &Store) {
    let statuses = evaluate(project, store).expect("оценить статусы");
    for status in statuses.values() {
        println!(
            "  {:<32} {:<12} {:?}{}",
            status.title,
            status.kind,
            status.status.label_ru(),
            status.status.reason().map(|reason| format!(" — {reason}")).unwrap_or_default()
        );
    }
}

fn main() {
    let root = demo_project_dir();
    println!("проект: {}", root.display());
    let project = load_project(&root).expect("загрузить проект");
    let mut store = Store::open(&root).expect("открыть хранилище");

    println!("\n== до обновления");
    print_statuses(&project, &store);

    let report = update(&project, &mut store, &UpdateOptions::default()).expect("обновить артефакты");
    println!("\n== отчёт обновления: выполнено {}, пропущено {}, ошибок {}", report.ran(), report.skipped(), report.failed());
    for artifact in &report.artifacts {
        println!(
            "  [{}] {:<32} {} (act_key {})",
            match artifact.action {
                rde_dag::UpdateAction::Ran => "запуск",
                rde_dag::UpdateAction::Skipped => "пропуск",
                rde_dag::UpdateAction::Failed => "ошибка",
            },
            artifact.title,
            artifact.message,
            artifact.act_key.as_deref().map(|key| &key[..12.min(key.len())]).unwrap_or("—"),
        );
    }
    if !report.errors.is_empty() {
        println!("  ошибки: {:?}", report.errors);
    }

    let project = load_project(&root).expect("перезагрузить проект");
    println!("\n== после обновления");
    print_statuses(&project, &store);

    let second = update(&project, &mut store, &UpdateOptions::default()).expect("повторное обновление");
    println!("\n== повторное обновление: выполнено {}, пропущено {}, транзакций {}", second.ran(), second.skipped(), report.tx_ids.len());
    println!("CAS: {:?}", store.stats().expect("статистика"));
    println!(".rde: {}", root.join(".rde").display());
}
