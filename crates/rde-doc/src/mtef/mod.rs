//! MathType **MTEF v5** → LaTeX (R-DOC-44).
//!
//! Причина: OLE-объекты MathType (`Equation.DSMT4`) в чужих `.docx` хранят формулу в приватном
//! бинарном формате MTEF, а в документе видна только preview-картинка (WMF). RDE при импорте
//! извлекает MTEF из OLE-стрима `Equation Native`, переводит его в LaTeX и компилирует обратно
//! в нативную формулу (OMML) существующим пайплайном `formula::compile_formula`.
//!
//! Реализация — порт **mtef-go** v2.0 (Apache-2.0, <https://github.com/zhexiao/mtef-go>), который
//! разбирает MTEF v5 и печатает LaTeX; дополнено шаблонами, которых в mtef-go нет (интегралы,
//! суммы/произведения, деление «слэшем», дуги/тильды и др.), и стилями набора MathType.
//! Таблицы символов — см. [`chars`]; атрибуция и лицензии — в `NOTICE`.
//!
//! Формат MTEF v3 (старый Equation Editor 3.0) не поддерживается: такие объекты остаются
//! preview-картинкой и попадают в отчёт импорта.
//!
//! Всё чтение — по bounds-checked срезу байт: враждебный OLE/MTEF даёт [`MtefError`], не панику.

mod chars;
mod latex;
mod records;
mod tree;

use std::io::Read;

/// Ошибка разбора MathType/MTEF. Никогда не паникуем на враждебных данных — возвращаем её.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MtefError {
    /// Байты не являются OLE Compound File.
    NotOle,
    /// В OLE-объекте нет стрима с уравнением (`Equation Native`/`EquationNative`/`Equation`).
    NoEquationStream,
    /// Заголовок `EQNOLEFILEHDR` или MTEF-заголовок неверны/обрезаны.
    BadHeader,
    /// Версия MTEF не поддерживается (RDE умеет v5; v3 — Equation Editor 3.0, отдельная задача).
    UnsupportedVersion(u8),
    /// Запись MTEF выходит за границы данных или неизвестна.
    BadRecord,
    /// Шаблон MTEF не поддержан; вызывающий слой оставляет preview-картинку.
    UnsupportedTemplate { selector: u8, variation: u16 },
    /// Формула разобрана, но LaTeX пуст.
    Empty,
}

impl std::fmt::Display for MtefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotOle => write!(f, "не OLE Compound File"),
            Self::NoEquationStream => write!(f, "в OLE-объекте нет стрима `Equation Native`"),
            Self::BadHeader => write!(f, "битый заголовок MTEF/EQNOLEFILEHDR"),
            Self::UnsupportedVersion(v) => write!(f, "неподдерживаемая версия MTEF: {v} (нужна v5)"),
            Self::BadRecord => write!(f, "битая или неизвестная запись MTEF"),
            Self::UnsupportedTemplate { selector, variation } => {
                write!(f, "шаблон MTEF не поддержан (selector {selector}, variation 0x{variation:04x})")
            }
            Self::Empty => write!(f, "пустая формула"),
        }
    }
}

impl std::error::Error for MtefError {}

/// Стрим `Equation Native` из OLE-объекта MathType (`word/embeddings/oleObjectN.bin`).
///
/// Возвращает байты после 28-байтового `EQNOLEFILEHDR` — тело MTEF (то, что нужно хранить
/// как скрытый объект RDE, чтобы формулу можно было восстановить/пере-разобрать без OLE).
pub fn equation_native(ole: &[u8]) -> Result<Vec<u8>, MtefError> {
    let mut compound = cfb::CompoundFile::open(std::io::Cursor::new(ole)).map_err(|_| MtefError::NotOle)?;
    let mut stream = ["Equation Native", "EquationNative", "Equation"]
        .iter()
        .find_map(|name| compound.open_stream(name).ok())
        .ok_or(MtefError::NoEquationStream)?;
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).map_err(|_| MtefError::BadHeader)?;
    // EQNOLEFILEHDR: cbHdr(u16)=28, version(u16), cf(u16), cbObject(u32), reserved(16).
    let header_len = usize::from(u16::from_le_bytes([*bytes.first().ok_or(MtefError::BadHeader)?, *bytes.get(1).ok_or(MtefError::BadHeader)?]));
    if header_len != 28 {
        return Err(MtefError::BadHeader);
    }
    let size = bytes.get(8..12).ok_or(MtefError::BadHeader)?;
    let size = usize::try_from(u32::from_le_bytes([size[0], size[1], size[2], size[3]])).map_err(|_| MtefError::BadHeader)?;
    let body = bytes.get(header_len..).ok_or(MtefError::BadHeader)?;
    if body.len() < size {
        return Err(MtefError::BadHeader);
    }
    Ok(body.get(..size).unwrap_or(body).to_vec())
}

/// LaTeX формулы из OLE-объекта MathType.
pub fn latex_from_ole(ole: &[u8]) -> Result<String, MtefError> {
    latex_from_mtef(&equation_native(ole)?)
}

/// LaTeX формулы из сырого MTEF v5-стрима (без `EQNOLEFILEHDR`).
pub fn latex_from_mtef(mtef: &[u8]) -> Result<String, MtefError> {
    let nodes = records::parse(mtef)?;
    let ast = tree::build(&nodes)?;
    let latex = latex::emit(&ast)?;
    let latex = latex.trim().to_string();
    if latex.is_empty() {
        return Err(MtefError::Empty);
    }
    Ok(latex)
}

#[cfg(test)]
mod tests;
