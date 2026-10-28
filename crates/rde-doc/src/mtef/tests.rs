//! Тесты MTEF-парсера: синтетические MTEF-стримы (записи собираются вручную) и OLE-обёртка.

use super::records::testing::{char, embell, end, header, line, record, tmpl};
use super::{MtefError, equation_native, latex_from_mtef, latex_from_ole};

/// MTEF-стрим: заголовок + конкатенация записей.
fn mtef(records: &[Vec<u8>]) -> Vec<u8> {
    let mut out = header();
    for record in records {
        out.extend_from_slice(record);
    }
    out
}

/// OLE Compound File с потоком `Equation Native` (28-байтовый EQNOLEFILEHDR + MTEF).
fn ole(equation: &[u8]) -> Vec<u8> {
    let mut compound = cfb::CompoundFile::create(std::io::Cursor::new(Vec::new())).expect("create compound");
    {
        let mut stream = compound.create_stream("Equation Native").expect("create stream");
        let mut header = vec![28u8, 0]; // cbHdr = 28
        header.extend_from_slice(&[0u8; 6]); // version/cf: значения не нужны читателю RDE
        header.extend_from_slice(&(equation.len() as u32).to_le_bytes());
        header.extend_from_slice(&[0u8; 16]); // reserved
        std::io::Write::write_all(&mut stream, &header).expect("header");
        std::io::Write::write_all(&mut stream, equation).expect("body");
    }
    compound.into_inner().into_inner()
}

fn latex(records: &[Vec<u8>]) -> String {
    latex_from_mtef(&mtef(records)).expect("latex")
}

#[test]
fn plain_characters_become_text() {
    assert_eq!(latex(&[line(), char(b'a' as u16, 3), char(b'b' as u16, 3), end()]), "ab");
}

#[test]
fn fraction_template() {
    // TMPL 11 (дробь): [LINE «a», LINE «b»] → \frac{a}{b}.
    let records = [line(), tmpl(11, 0), line(), char(b'a' as u16, 3), end(), line(), char(b'b' as u16, 3), end(), end()];
    assert_eq!(latex(&records), "\\frac{a}{b}");
}

#[test]
fn slash_variation_gives_an_inline_fraction() {
    let records = [line(), tmpl(11, 0x0002), line(), char(b'2' as u16, 8), end(), line(), char(b'c' as u16, 3), end(), end()];
    assert_eq!(latex(&records), "{2}/{c}");
}

#[test]
fn square_root_and_nth_root() {
    let square = [line(), tmpl(10, 0), line(), char(b'x' as u16, 3), end(), line(), end(), end()];
    assert_eq!(latex(&square), "\\sqrt{x}");
    let nth = [line(), tmpl(10, 1), line(), char(b'x' as u16, 3), end(), line(), char(b'3' as u16, 8), end(), end()];
    assert_eq!(latex(&nth), "\\sqrt[3]{x}");
}

#[test]
fn scripts_after_the_base() {
    // x^{2}: база, затем TMPL 28 (надстрочный) и два слота — пустой индекс и «2».
    let records = [line(), char(b'x' as u16, 3), tmpl(28, 0), line(), end(), line(), char(b'2' as u16, 8), end(), end(), end()];
    assert_eq!(latex(&records), "x^{2}");
}

#[test]
fn integral_with_limits() {
    // TMPL 15 (интеграл): main, lower, upper, operator.
    let records = [
        line(),
        tmpl(15, 0),
        line(),
        char(b'x' as u16, 3),
        end(),
        line(),
        char(b'0' as u16, 8),
        end(),
        line(),
        char(b'1' as u16, 8),
        end(),
        line(),
        end(),
        end(),
    ];
    assert_eq!(latex(&records), "\\int_{0}^{1} x");
}

#[test]
fn embellishment_goes_before_the_accented_char() {
    // H + EMBELL dot: LaTeX-акцент — префикс.
    let records = [line(), char(b'H' as u16, 3), embell(2), end()];
    assert_eq!(latex(&records), "\\dot H");
}

#[test]
fn text_style_is_wrapped_as_upright() {
    let records = [line(), char(b'e' as u16, 1), char(b'x' as u16, 1), end()];
    assert_eq!(latex(&records), "\\mathrm{e}\\mathrm{x}");
}

#[test]
fn characters_map_through_the_latex_table() {
    // 0x0022 в таблице MTCode→LaTeX — двойной штрих.
    assert_eq!(latex(&[line(), char(0x0022, 3), end()]), "''");
}

#[test]
fn equation_native_stream_is_read_from_the_ole_object() {
    let body = mtef(&[line(), char(b'a' as u16, 3), end()]);
    let ole = ole(&body);
    assert_eq!(equation_native(&ole).expect("stream"), body);
    assert_eq!(latex_from_ole(&ole).expect("latex"), "a");
}

#[test]
fn hostile_inputs_are_errors_not_panics() {
    assert_eq!(latex_from_mtef(&[]), Err(MtefError::BadRecord));
    assert_eq!(latex_from_mtef(&[4, 1, 0, 6, 9, 0, 0]), Err(MtefError::UnsupportedVersion(4)));
    assert_eq!(latex_from_ole(b"not an ole file"), Err(MtefError::NotOle));
    // Верный заголовок, но запись обрывается на середине.
    assert_eq!(latex_from_mtef(&mtef(&[line(), record(2, &[0])])), Err(MtefError::BadRecord));
}

#[test]
fn future_records_are_skipped_by_their_length() {
    // FUTURE-запись 200: байт длины 2, два байта данных, затем обычный текст.
    let mut records = vec![line()];
    records.push(record(200, &[2, 0xAA, 0xBB]));
    records.push(char(b'z' as u16, 3));
    records.push(end());
    assert_eq!(latex(&records), "z");
}
