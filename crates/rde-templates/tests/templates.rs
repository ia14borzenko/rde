//! M4: встроенные шаблоны, применение ГОСТ 7.32-2017 к демо-документу, библиография и гейт.

use std::path::{Path, PathBuf};

use rde_templates::*;
use wordcraft_engine::doc::props::LineSpacing;
use wordcraft_engine::doc::{Align, InlineObject, StoryRef};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-templates-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn demo_clean_docx(dir: &Path) -> PathBuf {
    let project = rde_core::load_project(&rde_core::demo_project_dir()).unwrap();
    let package = project.document_path().expect("документ демо-проекта");
    let clean = dir.join("clean.docx");
    if package.extension().and_then(|ext| ext.to_str()) == Some("rde") {
        rde_doc::extract_docx(&package, &clean).expect("извлечь чистый .docx");
    } else {
        std::fs::copy(&package, &clean).unwrap();
    }
    clean
}

fn demo_project() -> rde_core::Project {
    rde_core::load_project(&rde_core::demo_project_dir()).unwrap()
}

#[test]
fn builtin_templates_are_valid() {
    let templates = builtin_templates();
    assert_eq!(templates.len(), 3);
    let ids: Vec<&str> = templates.iter().map(|template| template.id.as_str()).collect();
    assert!(ids.contains(&"gost-7.32-2017"));
    assert!(ids.contains(&"lab-report"));
    assert!(template_by_id("gost-7.32-2017").is_some());
    assert!(template_by_id("nope").is_none());
    let gost = template_by_id("gost-7.32-2017").unwrap();
    assert_eq!(gost.page.margin_left_mm, 30.0);
    assert_eq!(gost.bibliography.heading, "СПИСОК ИСПОЛЬЗОВАННЫХ ИСТОЧНИКОВ");
    // Несовместимая schema отвергается.
    assert!(parse_template("schema = \"rde/template@0\"\nid = \"x\"\ntitle = \"x\"").is_err());
}

#[test]
fn apply_gost_sets_page_margins_and_paragraph_styles() {
    let dir = tmp("apply");
    let clean = demo_clean_docx(&dir);
    let (mut doc, _) = rde_doc::open_document(&clean).unwrap();
    let gost = template_by_id("gost-7.32-2017").unwrap();
    let log = apply(gost, &mut doc);
    assert!(log.len() >= 4, "{log:?}");

    let sections = doc.sections();
    let (_, section) = sections.first().expect("секция");
    assert!((section.page_w - 210.0 * 72.0 / 25.4).abs() < 0.5, "page_w={}", section.page_w);
    assert!((section.margin_left - 30.0 * 72.0 / 25.4).abs() < 0.5, "margin_left={}", section.margin_left);
    assert!((section.margin_right - 15.0 * 72.0 / 25.4).abs() < 0.5);

    let story = StoryRef::Body;
    let mut body_checked = false;
    let mut heading_checked = false;
    let mut caption_checked = false;
    for path in doc.para_paths(story) {
        let Some(paragraph) = doc.para(story, &path) else { continue };
        let style = paragraph.props.style.clone().unwrap_or_default();
        if style.eq_ignore_ascii_case("heading1") {
            assert_eq!(paragraph.props.align, Some(Align::Center), "заголовок по центру");
            assert!(paragraph.runs.iter().all(|run| run.props.bold == Some(true)), "заголовок полужирный");
            assert!(paragraph.runs.iter().all(|run| run.props.font.as_deref() == Some("Times New Roman")));
            heading_checked = true;
        } else if paragraph.objects.iter().any(|object| matches!(object, InlineObject::Image { .. }))
            || paragraph.text.trim_start().starts_with("Рисунок")
        {
            assert_eq!(paragraph.props.align, Some(Align::Center));
            if !paragraph.objects.iter().any(|object| matches!(object, InlineObject::Image { .. })) {
                assert!(paragraph.runs.iter().all(|run| run.props.size == Some(12.0)));
                caption_checked = true;
            }
        } else if !paragraph.text.trim().is_empty() {
            assert_eq!(paragraph.props.align, Some(Align::Justify));
            assert!(matches!(paragraph.props.line_spacing, Some(LineSpacing::Multiple(value)) if (value - 1.5).abs() < 0.01));
            let indent = paragraph.props.indent_first.unwrap_or(0.0);
            assert!((indent - 1.25 * 72.0 / 2.54).abs() < 0.5, "отступ={indent}");
            body_checked = true;
        }
    }
    assert!(body_checked && heading_checked && caption_checked, "body={body_checked} heading={heading_checked} caption={caption_checked}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bibliography_formats_demo_sources() {
    let project = demo_project();
    let entries = bibliography_entries(&project);
    assert_eq!(entries.len(), 3, "{entries:?}");
    assert!(entries.iter().any(|entry| entry.contains("Cumming") && entry.contains("2005")), "{entries:?}");
    assert!(entries.iter().any(|entry| entry.contains("ГОСТ 7.32-2017")), "{entries:?}");
    assert!(entries.iter().any(|entry| entry.contains("PDF-версия")), "{entries:?}");
    // Сортировка по алфавиту (латиница раньше кириллицы в byte-порядке — просто проверяем стабильность).
    let mut sorted = entries.clone();
    sorted.sort_by_key(|entry| entry.to_ascii_lowercase());
    assert_eq!(entries, sorted);
}

#[test]
fn insert_bibliography_is_idempotent() {
    // Чистый документ без библиографии (демо-документ уже оформлен по ГОСТ и содержит список).
    let mut doc = wordcraft_engine::doc::Document::default();
    let mut paragraph = wordcraft_engine::doc::Paragraph::new();
    paragraph.insert_text(0, "Введение", &wordcraft_engine::doc::CharProps::default()).unwrap();
    doc.insert_block(StoryRef::Body, &wordcraft_engine::doc::Path::top(0), wordcraft_engine::doc::Block::Para(paragraph)).unwrap();

    let gost = template_by_id("gost-7.32-2017").unwrap();
    let project = demo_project();
    let entries = bibliography_entries(&project);
    let inserted = insert_bibliography(gost, &mut doc, &entries).unwrap();
    assert_eq!(inserted, entries.len() + 1);
    let text = doc.plain_text(StoryRef::Body);
    assert!(text.contains("СПИСОК ИСПОЛЬЗОВАННЫХ ИСТОЧНИКОВ"));
    assert!(text.contains("Cumming"));
    // Повторная вставка не дублирует список.
    let again = insert_bibliography(gost, &mut doc, &entries).unwrap();
    assert_eq!(again, 0);
}

#[test]
fn gost_formatted_document_passes_fidelity_gate() {
    let dir = tmp("gate");
    let clean = demo_clean_docx(&dir);
    let (mut doc, _) = rde_doc::open_document(&clean).unwrap();
    let gost = template_by_id("gost-7.32-2017").unwrap();
    let project = demo_project();
    apply_with_bibliography(gost, &mut doc, &project).unwrap();
    let formatted = dir.join("formatted.docx");
    wordcraft_engine::io::save_path(&formatted, &doc).unwrap();
    let report = rde_docx::run_gate(&formatted, &dir.join("gate-work")).unwrap();
    assert!(report.critical_missing.is_empty(), "{:?}", report.critical_missing);
    assert!(report.media_preserved);
    assert_eq!(report.paragraphs_source, report.paragraphs_roundtrip);
    if report.lo.available {
        assert!(report.lo.pdf_is_valid, "LO: {:?}", report.lo);
        assert!(report.passed(), "{report:?}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}
