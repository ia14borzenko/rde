//! Оформить документ демо-проекта по ГОСТ 7.32-2017 и вставить библиографию (M4).
//!
//! ```bash
//! ./scripts/build.sh run -p rde-templates --example format-demo
//! ```
//! Меняет `demo-project/.../report.doc.rde` и `document.toml` (template = "gost-7.32-2017").

use std::path::PathBuf;

use rde_templates::template_by_id;

fn main() {
    let root = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(rde_core::demo_project_dir);
    let project = rde_core::load_project(&root).expect("загрузить проект");
    let document_object = project.document_object().expect("в проекте нет документа");
    let rde_core::ObjectData::Document(manifest) = &document_object.data else { panic!("не документ") };
    let package = document_object.dir.join(&manifest.file);
    assert!(rde_doc::is_rde_package(&package), "документ должен быть .doc.rde: {}", package.display());

    let work = std::env::temp_dir().join("rde-format-demo");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let clean = work.join("clean.docx");
    rde_doc::extract_docx(&package, &clean).expect("извлечь .docx");
    let (mut doc, _) = rde_doc::open_document(&clean).expect("открыть документ");
    let template = template_by_id("gost-7.32-2017").expect("шаблон ГОСТ");
    let log = rde_templates::apply_with_bibliography(template, &mut doc, &project).expect("оформить");
    let formatted = work.join("formatted.docx");
    wordcraft_engine::io::save_path(&formatted, &doc).expect("сохранить OOXML");
    let saved = rde_doc::save_engine_document(&formatted, &package, &manifest.id, &template.id).expect("записать .doc.rde");

    // Обновляем template в document.toml.
    let manifest_path = document_object.dir.join("document.toml");
    let text = std::fs::read_to_string(&manifest_path).unwrap();
    let mut out = String::new();
    let mut replaced = false;
    for line in text.lines() {
        if !replaced && line.trim_start().starts_with("template =") && !line.starts_with(' ') {
            out.push_str(&format!("template = \"{}\"\n", template.id));
            replaced = true;
        } else {
            out.push_str(line);
            out.push('\n');
        }
    }
    std::fs::write(&manifest_path, out).unwrap();

    println!("оформлено: {} ({} блоков, {} вставок)", package.display(), saved.blocks.len(), saved.inserts.len());
    for line in log {
        println!("  {line}");
    }
    let _ = std::fs::remove_dir_all(&work);
}
