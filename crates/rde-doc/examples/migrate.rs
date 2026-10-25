//! Миграция документа проекта из `.docx` в `.doc.rde` (M3).
//!
//! ```bash
//! cargo run -p rde-doc --example migrate -- [каталог-проекта]
//! ```
//! Делает: `rde:doc` из WordCraft-модели (блоки/подписи/рисунки), авто-привязку рисунков к
//! image-выходам артефактов (`rde://artifact/...`), запись пакета, правку `document.toml`
//! (`file = "report.doc.rde"`) и удаление исходного `.docx` (он целиком внутри пакета).

use std::path::PathBuf;

use rde_doc::{build_rde_doc, load_rde_doc, write_package};
use rde_hash::sha256;

fn main() {
    let root = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(rde_core::demo_project_dir);
    println!("проект: {}", root.display());
    let project = rde_core::load_project(&root).expect("загрузить проект");
    let document_object = project.document_object().expect("в проекте нет документа");
    let rde_core::ObjectData::Document(manifest) = &document_object.data else { panic!("не документ") };
    let source = document_object.dir.join(&manifest.file);
    if source.extension().and_then(|ext| ext.to_str()) == Some("rde") {
        println!("документ уже в .doc.rde: {}", source.display());
        return;
    }
    let mut rde_doc = build_rde_doc(&source, &manifest.id, &manifest.template).expect("собрать rde:doc");

    // Авто-привязка рисунков к image-выходам compute-артефактов (первый подходящий output).
    let image_outputs: Vec<(String, String)> = project
        .artifacts()
        .filter(|artifact| artifact.kind == "compute")
        .flat_map(|artifact| artifact.outputs.iter().map(move |output| (artifact.id.clone(), output.clone())))
        .filter(|(_, output)| {
            let lower = output.to_ascii_lowercase();
            [".png", ".svg", ".jpg", ".jpeg", ".webp"].iter().any(|ext| lower.ends_with(ext))
        })
        .collect();
    let mut linked = 0;
    for insert in rde_doc.inserts.iter_mut().filter(|insert| insert.kind == "figure") {
        let Some((artifact_id, selector)) = image_outputs.first() else { continue };
        insert.target = format!("rde://artifact/{artifact_id}/{selector}");
        if let Some(artifact_object) = project.artifact_object(artifact_id) {
            let path = artifact_object.dir.join(selector);
            if let Ok(bytes) = std::fs::read(&path) {
                insert.hash = Some(sha256(&bytes).hex());
            }
        }
        linked += 1;
    }

    let package = document_object.dir.join("report.doc.rde");
    write_package(&source, &mut rde_doc, &package).expect("записать .doc.rde");
    let check = load_rde_doc(&package).expect("прочитать обратно");

    // Обновляем document.toml (file = "report.doc.rde").
    let manifest_path = document_object.dir.join("document.toml");
    let text = std::fs::read_to_string(&manifest_path).expect("document.toml");
    let old_line = format!("file = \"{}\"", manifest.file);
    let new_text = text.replace(&old_line, "file = \"report.doc.rde\"");
    assert_ne!(new_text, text, "в document.toml нет строки {old_line}");
    std::fs::write(&manifest_path, new_text).expect("обновить document.toml");

    // Проверяем, что проект снова загружается и документ — пакет.
    let reloaded = rde_core::load_project(&root).expect("перезагрузить проект");
    let path = reloaded.document_path().expect("документ после миграции");
    assert_eq!(path.extension().and_then(|ext| ext.to_str()), Some("rde"));

    std::fs::remove_file(&source).expect("удалить исходный .docx (он внутри .doc.rde)");
    println!("готово: {}", package.display());
    println!(
        "блоков: {}, вставок: {} (рисунков привязано: {linked}), doc_id: {}, body_hash: {}",
        check.blocks.len(),
        check.inserts.len(),
        check.doc_id,
        &check.body_hash[..12.min(check.body_hash.len())]
    );
    for insert in &check.inserts {
        if insert.kind == "figure" {
            println!("  figure[{}] target={} caption={:?}", insert.id, insert.target, insert.caption);
        }
    }
}
