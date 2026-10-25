//! Импорт библиографии в проект RDE (M5): BibTeX / RIS / CSL-JSON.
//!
//! ```bash
//! ./scripts/build.sh run -p rde-sources --example import-citations -- <файл.bib|.ris|.json> [каталог-проекта]
//! ```
//! Создаёт объекты-источники с `[reference]` (по умолчанию — в demo-project).

use std::path::PathBuf;

fn main() {
    let Some(input) = std::env::args().nth(1) else {
        eprintln!("usage: import-citations <файл.bib|.ris|.json> [каталог-проекта]");
        std::process::exit(2);
    };
    let input = PathBuf::from(input);
    let root = std::env::args().nth(2).map(PathBuf::from).unwrap_or_else(rde_core::demo_project_dir);
    let text = std::fs::read_to_string(&input).expect("прочитать файл цитат");
    let format = match input.extension().and_then(|ext| ext.to_str()).map(|ext| ext.to_ascii_lowercase()).as_deref() {
        Some("bib") | Some("bibtex") => rde_sources::CitationFormat::Bibtex,
        Some("ris") => rde_sources::CitationFormat::Ris,
        Some("json") => rde_sources::CitationFormat::CslJson,
        _ => rde_sources::CitationFormat::detect(&text),
    };
    let references = format.parse(&text).expect("разобрать цитаты");
    println!("формат {format:?}: {} записей", references.len());
    let created = rde_sources::import_citations(&root, &references).expect("импортировать в проект");
    println!("создано источников: {}", created.len());
    for dir in created {
        println!("  {}", dir.display());
    }
    println!("проект: {}", root.display());
}
