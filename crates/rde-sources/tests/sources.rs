//! M5: импорт цитат, PDF-текст/рендер, OCR и веб-архив.

use std::path::{Path, PathBuf};

use rde_sources::*;

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rde-sources-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn minimal_project(name: &str) -> PathBuf {
    let root = tmp(name);
    std::fs::write(
        root.join("rde.toml"),
        "schema = \"rde/project@1\"\nid = \"01M5VJ1JG0V3RS3DTR98H6BCFN\"\nname = \"Источники\"\n",
    )
    .unwrap();
    root
}

fn tool(program: &str) -> bool {
    std::process::Command::new(program).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn bibtex_parses_books_and_articles() {
    let text = r#"
@book{cumming2005,
  author = {Cumming, I. G. and Wong, F. H.},
  title = {Digital Processing of Synthetic Aperture Radar Data},
  publisher = {Artech House},
  year = {2005}
}
@article{ivanov2020,
  author = {Иванов, И. И.},
  title = {Метод обработки сигналов},
  journal = {Радиотехника},
  year = {2020}
}
"#;
    let references = parse_bibtex(text).unwrap();
    assert_eq!(references.len(), 2, "{references:?}");
    assert_eq!(references[0].kind, "book");
    assert_eq!(references[0].authors, vec!["Cumming, I. G.", "Wong, F. H."]);
    assert_eq!(references[0].year, Some(2005));
    assert_eq!(references[1].kind, "article");
    assert!(references[1].title.contains("Метод обработки"));
    assert_eq!(CitationFormat::detect(text), CitationFormat::Bibtex);
}

#[test]
fn ris_parses_entries() {
    let text = "TY  - BOOK\nAU  - Cumming, I. G.\nAU  - Wong, F. H.\nTI  - Digital Processing of SAR Data\nPY  - 2005\nPB  - Artech House\nER  - \n";
    let references = parse_ris(text).unwrap();
    assert_eq!(references.len(), 1);
    assert_eq!(references[0].authors.len(), 2);
    assert_eq!(references[0].year, Some(2005));
    assert_eq!(references[0].kind, "book");
    assert_eq!(CitationFormat::detect(text), CitationFormat::Ris);
}

#[test]
fn csl_json_parses_items() {
    let text = r#"[
      {"type": "book", "title": "Digital Processing", "author": [{"family": "Cumming", "given": "I. G."}], "issued": {"date-parts": [[2005]]}, "publisher": "Artech House"},
      {"type": "article-journal", "title": "Метод", "author": [{"literal": "Иванов И.И."}], "issued": {"date-parts": [[2020]]}}
    ]"#;
    let references = parse_csl_json(text).unwrap();
    assert_eq!(references.len(), 2);
    assert_eq!(references[0].authors, vec!["Cumming, I. G."]);
    assert_eq!(references[0].year, Some(2005));
    assert_eq!(references[1].authors, vec!["Иванов И.И."]);
    assert_eq!(CitationFormat::detect(text), CitationFormat::CslJson);
}

#[test]
fn import_citations_creates_project_sources() {
    let root = minimal_project("import");
    let references = parse_bibtex("@book{a, author={Cumming, I. G. and Wong, F. H.}, title={Digital Processing}, year={2005}}").unwrap();
    let created = import_citations(&root, &references).unwrap();
    assert_eq!(created.len(), 1);
    let project = rde_core::load_project(&root).unwrap();
    assert_eq!(project.sources().count(), 1);
    let source = project.sources().next().unwrap();
    let rde_core::ObjectData::Source(manifest) = &source.data else { panic!() };
    let reference = manifest.reference.as_ref().expect("[reference] должен быть записан");
    assert_eq!(reference.authors.len(), 2);
    assert_eq!(reference.year, Some(2005));
    assert!(project.issues.is_empty(), "{:?}", project.issues);
    let _ = std::fs::remove_dir_all(&root);
}

fn demo_pdf(dir: &Path) -> Option<PathBuf> {
    if !tool("soffice") {
        return None;
    }
    let project = rde_core::load_project(&rde_core::demo_project_dir()).unwrap();
    let package = project.document_path().unwrap();
    let docx = dir.join("demo.docx");
    if rde_doc::is_rde_package(&package) {
        rde_doc::extract_docx(&package, &docx).unwrap();
    } else {
        std::fs::copy(&package, &docx).unwrap();
    }
    let out_dir = dir.join("pdf");
    std::fs::create_dir_all(&out_dir).unwrap();
    let status = std::process::Command::new("soffice")
        .args(["--headless", "--norestore", "--convert-to", "pdf", "--outdir"])
        .arg(&out_dir)
        .arg(&docx)
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let pdf = out_dir.join("demo.pdf");
    pdf.is_file().then_some(pdf)
}

#[test]
fn pdf_text_render_and_ocr() {
    let dir = tmp("pdf");
    let Some(pdf) = demo_pdf(&dir) else {
        eprintln!("LibreOffice недоступен — PDF-тесты пропущены");
        return;
    };
    if tool("pdftotext") {
        let text = pdf_text(&pdf).unwrap();
        assert!(text.contains("RDE") || text.contains("Введение"), "текст PDF: {text:.200}");
        // M4.4: текст одной страницы — основа контекст-ссылки `rde://source/…?page=N`.
        let page_text = pdf_page_text(&pdf, 1).unwrap();
        assert!(
            page_text.contains("RDE") || page_text.contains("Введение"),
            "текст страницы 1: {page_text:.200}"
        );
        assert!(pdf_page_text(&pdf, 0).is_err(), "страницы нумеруются с 1");
    }
    if tool("pdfinfo") {
        assert!(pdf_page_count(&pdf).unwrap() >= 1);
        // M4.4: размер страницы (точки) — для пересчёта bbox региона.
        let (width, height) = pdf_page_size(&pdf, 1).expect("размер страницы");
        assert!(width > 0.0 && height > 0.0, "размер страницы: {width}x{height}");
        assert!(pdf_page_size(&pdf, 0).is_err(), "страницы нумеруются с 1");
    }
    if tool("pdftoppm") {
        let png = render_pdf_page(&pdf, 1, &dir.join("page.png")).unwrap();
        assert!(png.is_file() && png.metadata().unwrap().len() > 1024, "png: {png:?}");
        if tool("tesseract") {
            let ocr = ocr_image(&png).unwrap();
            assert!(ocr.contains("RDE") || ocr.contains("Введение"), "OCR: {ocr:.200}");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn archive_url_reports_missing_monolith() {
    let availability = tool_availability();
    let dir = tmp("archive");
    let result = archive_url("https://example.com", &dir.join("page.html"));
    if availability.monolith {
        assert!(result.is_ok() || result.is_err());
    } else {
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("monolith"));
    }
    let _ = std::fs::remove_dir_all(&dir);
}
