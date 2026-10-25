//! Служебные команды RDE (G1, P5): `cargo run -p xtask -- <команда>`.
//!
//! - `ci` — тесты крейтов + docx-гейты + проверка схем (для CI);
//! - `schemas` — перегенерировать `docs/schema/*.json` из Rust-типов;
//! - `docx-gates` — fidelity/OPC-гейты по корпусу `crates/rde-docx/tests/corpus`;
//! - `fidelity` — гейт по демо-документу (`docs/fidelity/demo-report.json`);
//! - `perf` — замер открытия проекта на 10 000 объектов (`docs/perf/10k.json`);
//! - `demo` — обновить артефакты демо, привязать вставки, прогнать fidelity;
//! - `check` — быстрый `cargo check --workspace`.

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).parent().expect("корень репозитория").to_path_buf()
}

fn run(program: &str, args: &[&str]) -> bool {
    let status = Command::new(program).args(args).current_dir(repo_root()).status();
    matches!(status, Ok(status) if status.success())
}

fn cargo(args: &[&str]) -> bool {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    run(&cargo, args)
}

fn demo_document() -> Option<String> {
    let docs = repo_root().join("demo-project/objects/documents");
    let entries = std::fs::read_dir(docs).ok()?;
    for entry in entries.flatten() {
        let path = entry.path().join("report.doc.rde");
        if path.is_file() {
            return Some(path.display().to_string());
        }
    }
    None
}

fn main() {
    let command = std::env::args().nth(1).unwrap_or_else(|| "help".to_owned());
    let ok = match command.as_str() {
        "ci" => {
            let core = cargo(&[
                "test", "-p", "rde-core", "-p", "rde-hash", "-p", "rde-store", "-p", "rde-run", "-p", "rde-env", "-p",
                "rde-dag", "-p", "rde-doc", "-p", "rde-docx", "-p", "rde-templates", "-p", "rde-sources", "-p", "rde-vcs",
            ]);
            let gates = run_gates();
            let schemas = regenerate_schemas() && schemas_clean();
            core && gates && schemas
        }
        "check" => cargo(&["check", "--workspace"]),
        "schemas" => regenerate_schemas() && schemas_clean(),
        "docx-gates" => run_gates(),
        "fidelity" => match demo_document() {
            Some(document) => run("cargo", &["run", "-q", "-p", "rde-docx", "--example", "fidelity", "--", &document, "docs/fidelity/demo-report.json"]),
            None => {
                eprintln!("в demo-project нет документа");
                false
            }
        },
        "perf" => cargo(&["run", "-q", "-p", "rde-core", "--example", "perf-10k", "--", "docs/perf/10k.json"]),
        "demo" => {
            let update = run("cargo", &["run", "-q", "-p", "rde-dag", "--example", "update", "--", "demo-project"]);
            let link = run("cargo", &["run", "-q", "-p", "rde-doc", "--example", "link-demo", "--", "demo-project"]);
            let fidelity = match demo_document() {
                Some(document) => run("cargo", &["run", "-q", "-p", "rde-docx", "--example", "fidelity", "--", &document, "docs/fidelity/demo-report.json"]),
                None => false,
            };
            update && link && fidelity
        }
        "help" | "--help" | "-h" => {
            println!("xtask: ci | check | schemas | docx-gates | fidelity | perf | demo");
            true
        }
        other => {
            eprintln!("неизвестная команда `{other}`; доступно: ci | check | schemas | docx-gates | fidelity | perf | demo");
            false
        }
    };
    if !ok {
        std::process::exit(1);
    }
}

fn run_gates() -> bool {
    run(
        "cargo",
        &[
            "run", "-q", "-p", "rde-docx", "--example", "docx-gates", "--", "crates/rde-docx/tests/corpus",
            "docs/fidelity/corpus-report.json",
        ],
    )
}

fn regenerate_schemas() -> bool {
    run("cargo", &["run", "-q", "-p", "rde-core", "--example", "dump-schemas", "--", "docs/schema"])
}

/// Схемы должны быть перегенерированы (нет незакоммиченных отличий) — гейт G1.
fn schemas_clean() -> bool {
    let status = Command::new("git").args(["status", "--porcelain", "--", "docs/schema"]).current_dir(repo_root()).output();
    match status {
        Ok(output) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout);
            if text.trim().is_empty() {
                true
            } else {
                eprintln!("docs/schema не соответствует Rust-типам:\n{text}");
                false
            }
        }
        _ => {
            eprintln!("git недоступен — проверка схем пропущена");
            true
        }
    }
}
