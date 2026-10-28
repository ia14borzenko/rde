//! Компиляция project-level формулы (ADR-0005 P2): эффективный LaTeX → OMML-фрагмент, SVG и
//! PNG-fallback. Кэши сами по себе — блобы CAS store (см. [`store_formula_caches`]); запись
//! состояния формулы в ledger — слой `rde-dag` (`commit_formula_update`), вставка в документ — P3.
//!
//! Вектор: мини-документ WordCraft с одним уравнением → векторный PDF (krilla) → `pdftocairo -svg`
//! (poppler CLI, уже принятое отклонение §15-2) → кроп по bbox формулы → SVG без шрифтов
//! (глифы путями), проверено спайком B (`docs/spikes/formulas-latex.md`). PNG-fallback — resvg.

use std::io::Read;
use std::path::Path;
use std::process::Command;

use crate::rasterize_svg;
use serde_json::json;
use wordcraft_engine::doc::Document;
use wordcraft_engine::Session;

/// Идентификатор рендерера формул для `env_hash` (ADR-0005 §4.1): версия WordCraft/пайплайна.
/// Менять при обновлении пина `vendor/wordcraft` — кэши формул инвалидируются осознанно.
pub const FORMULA_RENDERER_ID: &str = "wordcraft";
/// Версия рендерера: WordCraft v0.10.0 (пин форка) + RDE-пайплайн SVG.
/// `rde2` — кроп кэшей по ink-bbox PDF (скобки/линейки формул рисуются путями, а не текстом);
/// ранее отрендеренные SVG/PNG невалидны для новой версии и пересобираются по `renderer`-хэшу.
pub const FORMULA_RENDERER_VERSION: &str = "0.10.0+rde2";

/// Скомпилированные кэши формулы.
#[derive(Clone, Debug, Default)]
pub struct FormulaCaches {
    /// OMML-фрагмент: `m:oMathPara` (display) или `m:oMath` (inline).
    pub omml: Vec<u8>,
    /// Векторный SVG (кроп формулы), если пайплайн доступен.
    pub svg: Option<Vec<u8>>,
    /// PNG-fallback для WordCraft/старых Word.
    pub png: Option<Vec<u8>>,
    pub omml_hash: String,
    pub svg_hash: Option<String>,
    pub png_hash: Option<String>,
}

impl FormulaCaches {
    /// Имена/содержимое частей кэш-дерева store (`formula/<fid>`).
    pub fn entries(&self) -> Vec<(String, &[u8])> {
        let mut entries: Vec<(String, &[u8])> = vec![("formula.omml".to_owned(), &self.omml)];
        if let Some(svg) = &self.svg {
            entries.push(("formula.svg".to_owned(), svg));
        }
        if let Some(png) = &self.png {
            entries.push(("formula.png".to_owned(), png));
        }
        entries
    }
}

/// Скомпилировать формулу из эффективного LaTeX.
pub fn compile_formula(effective_latex: &str, display: bool) -> Result<FormulaCaches, String> {
    if effective_latex.trim().is_empty() {
        return Err("пустой LaTeX формулы".to_owned());
    }
    let mut session = Session::new(Document::new());
    session
        .run("insert.equation", &json!({"latex": effective_latex, "display": display}))
        .map_err(|e| format!("LaTeX не разобран движком: {e}"))?;
    session.run("equation.exit", &json!({})).map_err(|e| e.to_string())?;
    // 1) OMML: сохраняем мини-документ и вытаскиваем фрагмент уравнения.
    let docx = wordcraft_engine::io::save_bytes("formula.docx", &session.doc).map_err(|e| format!("docx: {e}"))?;
    let document_xml = zip_part(&docx, "word/document.xml").ok_or("в мини-документе нет word/document.xml")?;
    let omml = extract_omml_fragment(&String::from_utf8_lossy(&document_xml), display)?;
    let omml_hash = rde_hash::sha256(&omml).hex();
    // 2) SVG/PNG: мини-документ → векторный PDF → pdftocairo -svg → кроп; PNG — resvg.
    let pdf = wordcraft_engine::io::save_bytes_opts("formula.pdf", &session.doc, &Default::default())
        .map_err(|e| format!("PDF формулы: {e}"))?;
    let (svg, png) = pdf_to_formula_svg_png(&pdf)?;
    Ok(FormulaCaches {
        svg_hash: Some(rde_hash::sha256(&svg).hex()),
        png_hash: png.as_ref().map(|png| rde_hash::sha256(png).hex()),
        omml,
        svg: Some(svg),
        png,
        omml_hash,
    })
}

/// Положить кэши формулы в CAS и вернуть Merkle-дерево (`formula/<fid>` root).
pub fn store_formula_caches(
    store: &mut rde_store::Store,
    caches: &FormulaCaches,
) -> Result<rde_hash::Hash, String> {
    let mut entries = Vec::new();
    for (name, bytes) in caches.entries() {
        let blob = store.put_blob(bytes).map_err(|e| e.to_string())?;
        entries.push(rde_hash::TreeEntry::new(name, 0o644, blob));
    }
    store.put_tree(&entries).map_err(|e| e.to_string())
}

/// Достать первую часть zip-пакета (мини-документ docx).
fn zip_part(bytes: &[u8], name: &str) -> Option<Vec<u8>> {
    let reader = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(reader).ok()?;
    let mut entry = archive.by_name(name).ok()?;
    let mut out = Vec::new();
    entry.read_to_end(&mut out).ok()?;
    Some(out)
}

/// Извлечь OMML-фрагмент уравнения из `word/document.xml` мини-документа.
/// display → `m:oMathPara`, inline → `m:oMath` (без `Para`).
pub fn extract_omml_fragment(document_xml: &str, display: bool) -> Result<Vec<u8>, String> {
    let (start, close) = if display {
        match document_xml.find("<m:oMathPara") {
            Some(start) => (start, "</m:oMathPara>"),
            None => return Err("в документе нет display-уравнения (m:oMathPara)".to_owned()),
        }
    } else {
        // Inline: `<m:oMath>` или `<m:oMath ...>` (но не `<m:oMathPara`).
        let start = document_xml
            .find("<m:oMath>")
            .or_else(|| document_xml.find("<m:oMath "))
            .ok_or_else(|| "в документе нет inline-уравнения (m:oMath)".to_owned())?;
        (start, "</m:oMath>")
    };
    let end = document_xml[start..]
        .find(close)
        .ok_or_else(|| "OMML-уравнение не закрыто".to_owned())?
        + start
        + close.len();
    Ok(document_xml[start..end].as_bytes().to_vec())
}

/// PDF мини-документа формулы → кропнутый SVG (poppler) → PNG-fallback (resvg).
fn pdf_to_formula_svg_png(pdf: &[u8]) -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
    let dir = std::env::temp_dir().join(format!("rde-formula-{}-{}", std::process::id(), nanos()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("создать {}: {e}", dir.display()))?;
    let pdf_path = dir.join("formula.pdf");
    let svg_path = dir.join("formula.svg");
    std::fs::write(&pdf_path, pdf).map_err(|e| format!("записать pdf: {e}"))?;
    let result = (|| -> Result<(Vec<u8>, Option<Vec<u8>>), String> {
        run_checked(Command::new("pdftocairo").arg("-svg").arg(&pdf_path).arg(&svg_path))?;
        let svg = std::fs::read(&svg_path).map_err(|e| format!("прочитать svg: {e}"))?;
        let svg_text = String::from_utf8_lossy(&svg).to_string();
        let (x, y, w, h) = pdf_ink_bbox(&pdf_path, &dir, 2.0)?;
        let cropped = crop_svg(&svg_text, x, y, w, h)?;
        let png = rasterize_svg(cropped.as_bytes()).ok();
        Ok((cropped.into_bytes(), png))
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn run_checked(command: &mut Command) -> Result<(), String> {
    let output = command.output().map_err(|e| format!("запустить {}: {e}", command.get_program().to_string_lossy()))?;
    if !output.status.success() {
        return Err(format!(
            "{}: {}",
            command.get_program().to_string_lossy(),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

/// Bbox всего, что нарисовано на странице PDF формулы, в pt + отступ: (x, y, w, h).
///
/// Раньше bbox брался из текстового слоя (`pdftotext -bbox`), но MathType-подобные скобки,
/// линейки дробей и знаки больших операторов в векторном PDF идут **путями**, а не текстом, и
/// обрезались: формулы с `\left( ... \right)` теряли закрывающую скобку. Растеризуем страницу
/// (poppler `pdftoppm`) и берём ink-bbox по непустым пикселям — это покрывает и текст, и пути.
const BBOX_DPI: u32 = 150;

fn pdf_ink_bbox(pdf: &Path, work: &Path, pad: f64) -> Result<(f64, f64, f64, f64), String> {
    let prefix = work.join("bbox");
    run_checked(Command::new("pdftoppm").arg("-r").arg(BBOX_DPI.to_string()).arg("-gray").arg("-png").arg("-singlefile").arg(pdf).arg(&prefix))?;
    let page = prefix.with_extension("png");
    let image = image::open(&page).map_err(|e| format!("decode pdf raster: {e}"))?.to_luma8();
    let (width, height) = image.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
    for (x, y, pixel) in image.enumerate_pixels() {
        // Фон страницы белый; всё темнее почти белого — «чернила» (антиалиасинг тоже ловим).
        if pixel.0[0] < 250 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 == u32::MAX || x1 < x0 || y1 < y0 {
        return Err("в PDF формула не нарисована (нет ни одного пикселя)".to_owned());
    }
    let scale = 72.0 / f64::from(BBOX_DPI);
    let (page_w, page_h) = (f64::from(width) * scale, f64::from(height) * scale);
    let (min_x, min_y) = (f64::from(x0) * scale, f64::from(y0) * scale);
    // +1px, чтобы антиалиасинг на границе не обрезался.
    let (max_x, max_y) = (f64::from(x1 + 1) * scale, f64::from(y1 + 1) * scale);
    let x = (min_x - pad).max(0.0);
    let y = (min_y - pad).max(0.0);
    let w = (max_x - min_x + 2.0 * pad).min(page_w - x).max(1.0);
    let h = (max_y - min_y + 2.0 * pad).min(page_h - y).max(1.0);
    Ok((x, y, w, h))
}

/// Обрезать SVG до bbox формулы правкой `viewBox`/`width`/`height` (pdftocairo -svg не кропает
/// на стенде надёжно; содержимое остаётся в координатах страницы).
fn crop_svg(svg: &str, x: f64, y: f64, w: f64, h: f64) -> Result<String, String> {
    let start = svg.find("<svg").ok_or("в SVG нет корня <svg>")?;
    let end = svg[start..].find('>').ok_or("корень <svg> не закрыт")? + start;
    let header = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w}pt\" height=\"{h}pt\" viewBox=\"{x} {y} {w} {h}\">"
    );
    Ok(format!("{}{}{}", &svg[..start], header, &svg[end + 1..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_omml_fragment_display_and_inline() {
        let xml = "<w:document><w:body><w:p><m:oMathPara><m:oMath><m:r><m:t>a</m:t></m:r></m:oMath></m:oMathPara></w:p></w:body></w:document>";
        let display = extract_omml_fragment(xml, true).unwrap();
        assert!(display.starts_with(b"<m:oMathPara"));
        assert!(display.ends_with(b"</m:oMathPara>"));
        let inline = extract_omml_fragment(xml, false).unwrap();
        assert!(inline.starts_with(b"<m:oMath>"));
        assert!(extract_omml_fragment("<w:document/>", true).is_err());
    }

    /// Спайк B, воспроизводимый как тест: OMML σ=F/A → SVG без `<image>` (глифы путями) и PNG.
    /// Требует poppler CLI; без него тест честно пропускается.
    #[test]
    fn compile_formula_end_to_end_when_poppler_available() {
        if !command_available("pdftocairo") {
            return;
        }
        let caches = compile_formula("\\sigma=\\frac{F}{A}", true).expect("компиляция формулы");
        let omml = String::from_utf8_lossy(&caches.omml).to_string();
        assert!(omml.contains("<m:oMathPara"), "{omml}");
        assert!(omml.contains("<m:f>"), "дробь в OMML: {omml}");
        let svg = String::from_utf8_lossy(caches.svg.as_deref().expect("SVG")).to_string();
        assert!(!svg.contains("<image"), "SVG формулы должен быть векторным (без растра)");
        assert!(svg.contains("<path"), "глифы SVG — путями: {}", &svg[..svg.len().min(200)]);
        assert!(caches.png.is_some(), "PNG-fallback");
        assert_eq!(caches.omml_hash.len(), 64);
        assert_eq!(caches.svg_hash.as_deref().map(str::len), Some(64));
        assert_eq!(caches.png_hash.as_deref().map(str::len), Some(64));
    }

    fn command_available(name: &str) -> bool {
        Command::new(name).arg("-v").output().map(|output| output.status.success()).unwrap_or(false)
    }
}
