//! Выгрузить JSON Schema всех манифестов проекта в каталог (по умолчанию `docs/schema`).
//!
//! ```bash
//! cargo run -p rde-core --example dump-schemas -- docs/schema
//! ```

use std::path::PathBuf;

use rde_core::manifest::ManifestKind;

fn main() {
    let dir = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("docs/schema"));
    std::fs::create_dir_all(&dir).expect("создать каталог схем");
    let kinds = [
        ("project", ManifestKind::Project),
        ("artifact", ManifestKind::Artifact),
        ("document", ManifestKind::Document),
        ("source", ManifestKind::Source),
        ("note", ManifestKind::Note),
        ("repository", ManifestKind::Repository),
        ("formula", ManifestKind::Formula),
    ];
    for (name, kind) in kinds {
        let schema = kind.schema();
        let json = serde_json::to_string_pretty(&schema).expect("сериализовать схему");
        let path = dir.join(format!("{name}.schema.json"));
        std::fs::write(&path, json).expect("записать схему");
        println!("{}", path.display());
    }
}
