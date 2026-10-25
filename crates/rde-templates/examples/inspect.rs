//! Диагностика: как шаблон ГОСТ классифицирует и форматирует абзацы демо-документа.
use wordcraft_engine::doc::StoryRef;

fn main() {
    let dir = std::env::temp_dir().join("rde-template-inspect");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let project = rde_core::load_project(&rde_core::demo_project_dir()).unwrap();
    let package = project.document_path().unwrap();
    let clean = dir.join("clean.docx");
    rde_doc::extract_docx(&package, &clean).unwrap();
    let (mut doc, _) = rde_doc::open_document(&clean).unwrap();
    let gost = rde_templates::template_by_id("gost-7.32-2017").unwrap();
    rde_templates::apply(gost, &mut doc);
    for path in doc.para_paths(StoryRef::Body) {
        let Some(p) = doc.para(StoryRef::Body, &path) else { continue };
        let text: String = p.text.chars().take(40).collect();
        let images = p.objects.len();
        println!("{:>4} style={:?} align={:?} img={} text={:?}", path.to_string(), p.props.style, p.props.align, images, text);
    }
}
