//! Сгенерировать синтетический корпус `.docx` для fidelity-гейтов (S6):
//! `cargo run -p rde-docx --example make-corpus -- crates/rde-docx/tests/corpus`
//!
//! Корпус синтетический (WordCraft-модель → OOXML): разные объёмы/структуры/шрифтовые
//! варианты. Реальный корпус Word-файлов подключается отдельно (лицензии/размеры), см.
//! `docs/fidelity/README.md`.

use std::path::PathBuf;

use wordcraft_engine::doc::{Block, CharProps, Paragraph, Path, StoryRef};
use wordcraft_engine::doc::Document;

fn paragraph(text: &str, style: Option<&str>, bold: bool) -> Paragraph {
    let mut paragraph = Paragraph::new();
    let mut props = CharProps::default();
    props.bold = Some(bold);
    paragraph.insert_text(0, text, &props).expect("текст в абзац");
    paragraph.props.style = style.map(str::to_owned);
    paragraph
}

fn document_with(paragraphs: Vec<Paragraph>) -> Document {
    let mut doc = Document::default();
    for (index, paragraph) in paragraphs.into_iter().enumerate() {
        doc.insert_block(StoryRef::Body, &Path::top(index), Block::Para(paragraph)).expect("вставить абзац");
    }
    doc
}

fn main() {
    let out_dir = std::env::args().nth(1).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("crates/rde-docx/tests/corpus"));
    std::fs::create_dir_all(&out_dir).expect("каталог корпуса");
    let variants: Vec<(String, Document)> = vec![
        ("01-empty.docx".to_owned(), document_with(vec![])),
        ("02-one-paragraph.docx".to_owned(), document_with(vec![paragraph("Один абзац простого текста.", None, false)])),
        (
            "03-headings.docx".to_owned(),
            document_with(vec![
                paragraph("1 Введение", Some("Heading1"), true),
                paragraph("Текст введения с обычным форматированием.", None, false),
                paragraph("2 Экспериментальная часть", Some("Heading1"), true),
                paragraph("Подраздел 2.1", Some("Heading2"), true),
                paragraph("Текст подраздела.", None, false),
            ]),
        ),
        (
            "04-unicode.docx".to_owned(),
            document_with(vec![
                paragraph("Кириллица, «кавычки», тире — и символы: α, β, λ, ∂, ∑, ≤, ≥.", None, false),
                paragraph("English text with accents: café, naïve, façade; 日本語/中文 — bidi не гарантируется.", None, false),
            ]),
        ),
        (
            "05-long-100.docx".to_owned(),
            document_with((0..100).map(|index| paragraph(&format!("Абзац {} — проверка длинного документа и вёрстки.", index + 1), None, false)).collect()),
        ),
        (
            "06-long-300.docx".to_owned(),
            document_with((0..300).map(|index| paragraph(&format!("Строка {} длинного документа для проверки пагинации и roundtrip.", index + 1), None, false)).collect()),
        ),
        (
            "07-bold-italic.docx".to_owned(),
            document_with(vec![
                paragraph("Обычный текст.", None, false),
                paragraph("Жирный текст.", None, true),
                paragraph("Смешанный: обычный + жирный.", None, false),
            ]),
        ),
        (
            "08-caption-like.docx".to_owned(),
            document_with(vec![
                paragraph("Рисунок 1 — Схема обработки данных", None, false),
                paragraph("Текст после подписи.", None, false),
            ]),
        ),
        (
            "09-many-headings.docx".to_owned(),
            document_with(
                (0..40)
                    .flat_map(|index| {
                        vec![
                            paragraph(&format!("{} Раздел {}", index + 1, index + 1), Some("Heading1"), true),
                            paragraph(&format!("Текст раздела {}.", index + 1), None, false),
                        ]
                    })
                    .collect(),
            ),
        ),
        (
            "10-empty-paragraphs.docx".to_owned(),
            document_with(vec![
                paragraph("Первый.", None, false),
                Paragraph::new(),
                Paragraph::new(),
                paragraph("После пустых.", None, false),
            ]),
        ),
        (
            "11-numeric.docx".to_owned(),
            document_with(vec![paragraph(
                "Числа и единицы: 1,25 Ом; 3,5 кГц; 0,001 с; 10⁶ Па; ±5 %; 42.",
                None,
                false,
            )]),
        ),
        (
            "12-mixed.docx".to_owned(),
            document_with(vec![
                paragraph("Отчёт", Some("Heading1"), true),
                paragraph("Введение и цель работы.", None, false),
                paragraph("1 Методика", Some("Heading1"), true),
                paragraph("Описание методики с формулами: E = mc².", None, false),
                paragraph("Заключение.", Some("Heading1"), true),
            ]),
        ),
    ];
    for (name, doc) in variants {
        let path = out_dir.join(&name);
        wordcraft_engine::io::save_path(&path, &doc).expect("сохранить .docx");
        let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        println!("{name}: {size} байт");
    }
    // Реальный (наш демо-) документ как элемент корпуса: копия стабильной фикстуры.
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../rde-doc/tests/fixtures/base-report.docx");
    if fixture.is_file() {
        let target = out_dir.join("13-demo-report.docx");
        std::fs::copy(&fixture, &target).expect("копия демо-документа в корпус");
        println!("13-demo-report.docx: скопирован");
    }
    println!("корпус: {}", out_dir.display());
}
