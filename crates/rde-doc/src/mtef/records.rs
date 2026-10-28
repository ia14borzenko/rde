//! Чтение записей MTEF v5 в плоский список (порт `readRecord`/`read*` из mtef-go v2.0,
//! Apache-2.0). Дерево из плоского списка собирает [`super::tree`].
//!
//! Заголовок MTEF v5: версия(1), платформа(1), продукт(1), версия продукта(1), подверсия(1),
//! NUL-строка приложения, inline(1); дальше — записи. Записи с тегом ≥ 100 — «future»:
//! за тегом идёт один байт длины, которую надо пропустить.

use super::MtefError;

/// Смещение/чтение с проверкой границ: враждебный вход даёт [`MtefError::BadRecord`].
pub(super) struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub(super) fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub(super) fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    pub(super) fn u8(&mut self) -> Result<u8, MtefError> {
        let byte = *self.data.get(self.pos).ok_or(MtefError::BadRecord)?;
        self.pos += 1;
        Ok(byte)
    }

    pub(super) fn u16(&mut self) -> Result<u16, MtefError> {
        let bytes = self.data.get(self.pos..self.pos + 2).ok_or(MtefError::BadRecord)?;
        self.pos += 2;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(super) fn i16(&mut self) -> Result<i16, MtefError> {
        let bytes = self.data.get(self.pos..self.pos + 2).ok_or(MtefError::BadRecord)?;
        self.pos += 2;
        Ok(i16::from_le_bytes([bytes[0], bytes[1]]))
    }

    pub(super) fn skip(&mut self, len: usize) -> Result<(), MtefError> {
        if self.remaining() < len {
            return Err(MtefError::BadRecord);
        }
        self.pos += len;
        Ok(())
    }

    /// NUL-терминированная строка (байты как есть; имена шрифтов/кодировок — latin-1-ish).
    fn nul_string(&mut self) -> Result<Vec<u8>, MtefError> {
        let start = self.pos;
        while self.u8()? != 0 {}
        Ok(self.data.get(start..self.pos.saturating_sub(1)).unwrap_or_default().to_vec())
    }

    /// Nudge: два байта; 128 в любом = знак «дальше 16-битные значения».
    fn nudge(&mut self) -> Result<(), MtefError> {
        let x = self.u8()?;
        let y = self.u8()?;
        if x == 128 || y == 128 {
            self.i16()?;
            self.i16()?;
        }
        Ok(())
    }

    fn options(&mut self) -> Result<u8, MtefError> {
        self.u8()
    }
}

/// Значимые для дерева/LaTeX записи MTEF. Служебные (`FONT_DEF`, `ENCODING_DEF`, `EQN_PREFS`,
/// `FULL`, `SUB`, `SUB2`, `SYM`, `SUBSYM`, `SIZE`, `COLOR`, `COLOR_DEF`, `FONT_STYLE_DEF`)
/// пропускаются при разборе: на дерево и LaTeX они не влияют (как в mtef-go).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Node {
    End,
    Line { null: bool },
    Char(CharNode),
    Tmpl(TmplNode),
    Pile,
    Matrix { cols: u8 },
    Embell { kind: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CharNode {
    /// Стиль набора: 0x80 + индекс стиля (`fnTEXT`=1, `fnFUNCTION`=2, `fnVARIABLE`=3, …).
    pub typeface: u8,
    /// Unicode-код MathType (MTCode).
    pub mtcode: u16,
    pub bits8: Option<u8>,
    pub bits16: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct TmplNode {
    pub selector: u8,
    pub variation: u16,
}

/// Разобрать MTEF v5-стрим в плоский список записей.
pub(super) fn parse(data: &[u8]) -> Result<Vec<Node>, MtefError> {
    let mut r = Reader::new(data);
    let version = r.u8()?;
    if version != 5 {
        return Err(MtefError::UnsupportedVersion(version));
    }
    // Платформа, продукт, версия продукта, подверсия, приложение, inline.
    r.u8()?;
    r.u8()?;
    r.u8()?;
    r.u8()?;
    r.nul_string()?;
    r.u8()?;
    let mut nodes = Vec::new();
    while r.remaining() > 0 {
        let tag = r.u8()?;
        if tag >= 100 {
            let skip = usize::from(r.u8()?);
            r.skip(skip)?;
            continue;
        }
        match tag {
            0 => nodes.push(Node::End),
            1 => {
                let options = r.options()?;
                if options & 0x08 != 0 {
                    r.nudge()?;
                }
                if options & 0x04 != 0 {
                    r.u8()?;
                }
                if options & 0x02 != 0 {
                    let stops = r.u8()?;
                    for _ in 0..stops {
                        r.u8()?;
                        r.u16()?;
                    }
                }
                nodes.push(Node::Line { null: options & 0x01 != 0 });
            }
            2 => {
                let options = r.options()?;
                if options & 0x08 != 0 {
                    r.nudge()?;
                }
                let typeface = r.u8()?;
                let mtcode = if options & 0x20 == 0 { r.u16()? } else { 0 };
                let bits8 = if options & 0x04 != 0 { Some(r.u8()?) } else { None };
                let bits16 = if options & 0x10 != 0 { Some(r.u16()?) } else { None };
                nodes.push(Node::Char(CharNode { typeface, mtcode, bits8, bits16 }));
            }
            3 => {
                let options = r.options()?;
                if options & 0x08 != 0 {
                    r.nudge()?;
                }
                let selector = r.u8()?;
                let first = r.u8()?;
                let variation = if first & 0x80 != 0 {
                    let second = u16::from(r.u8()?);
                    (u16::from(first) & 0x7F) | (second << 8)
                } else {
                    u16::from(first)
                };
                r.u8()?; // template-specific options
                nodes.push(Node::Tmpl(TmplNode { selector, variation }));
            }
            4 => {
                let options = r.options()?;
                if options & 0x08 != 0 {
                    r.nudge()?;
                }
                r.u8()?; // halign
                r.u8()?; // valign
                nodes.push(Node::Pile);
            }
            5 => {
                let options = r.options()?;
                if options & 0x08 != 0 {
                    r.nudge()?;
                }
                r.u8()?; // valign
                r.u8()?; // h_just
                r.u8()?; // v_just
                r.u8()?; // rows
                let cols = r.u8()?;
                nodes.push(Node::Matrix { cols });
                // Два «пустых LINE»: в mtef-go они вставляются как настоящие (pushable) LINE-узлы,
                // чтобы закрыться двумя END из данных и оставить MATRIX открытым для строк-ячеек,
                // которые идут следом. Без этого строки матрицы стали бы её братьями.
                nodes.push(Node::Line { null: false });
                nodes.push(Node::Line { null: false });
            }
            6 => {
                let options = r.options()?;
                if options & 0x08 != 0 {
                    r.nudge()?;
                }
                let kind = r.u8()?;
                nodes.push(Node::Embell { kind });
            }
            8 => {
                r.u8()?; // font style def index
                r.nul_string()?;
            }
            9 => {
                r.u8()?; // lsize
                r.u8()?; // dsize
            }
            15 => {
                r.u8()?; // color index
            }
            16 => {
                let options = r.options()?;
                let components = if options & 0x01 != 0 { 4 } else { 3 };
                for _ in 0..components {
                    r.u16()?;
                }
                if options & 0x04 != 0 {
                    r.nul_string()?;
                }
            }
            17 => {
                r.u8()?; // encoding def index
                r.nul_string()?;
            }
            18 => eqn_prefs(&mut r)?,
            19 => {
                r.nul_string()?;
            }
            // FULL, SUB, SUB2, SYM, SUBSYM — маркеры порядка объектов без данных; дереву и LaTeX
            // не нужны (в mtef-go `makeAST` их тоже пропускает).
            10..=14 => {}
            _ => return Err(MtefError::BadRecord),
        }
    }
    Ok(nodes)
}
/// `EQN_PREFS`: размеры/отступы/стили. Значения для LaTeX не нужны, но нужно ровно их пропустить:
/// массивы размеров закодированы нибблами, где 0xF — конец очередного числа (как в mtef-go).
fn eqn_prefs(r: &mut Reader<'_>) -> Result<(), MtefError> {
    r.u8()?; // options
    let sizes = r.u8()?;
    skip_dimensions(r, sizes)?;
    let spaces = r.u8()?;
    skip_dimensions(r, spaces)?;
    let styles = r.u8()?;
    for _ in 0..styles {
        if r.u8()? != 0 {
            r.u8()?;
        }
    }
    Ok(())
}

/// Пропустить `count` чисел, закодированных полубайтами: 0xF завершает число, следующий полубайт —
/// единица измерения (mtef-go `readDimensionArrays`).
fn skip_dimensions(r: &mut Reader<'_>, count: u8) -> Result<(), MtefError> {
    let mut done = 0u8;
    let mut unit_pending = false;
    while done < count {
        let byte = r.u8()?;
        for nibble in [byte >> 4, byte & 0x0F] {
            if unit_pending {
                unit_pending = false;
            } else if nibble == 0x0F {
                unit_pending = true;
                done = done.saturating_add(1);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod testing {
    //! Кодировщики MTEF-записей для тестов (собираем синтетические стримы вручную).

    pub fn record(tag: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![tag];
        out.extend_from_slice(payload);
        out
    }

    pub fn header() -> Vec<u8> {
        let mut out = vec![5, 1, 0, 6, 9];
        out.extend_from_slice(b"MTEF\0");
        out.push(1); // inline
        out
    }

    pub fn line() -> Vec<u8> {
        record(1, &[0])
    }

    pub fn char(mtcode: u16, style: u8) -> Vec<u8> {
        let mut out = vec![2, 0, 0x80 | style];
        out.extend_from_slice(&mtcode.to_le_bytes());
        out
    }

    pub fn tmpl(selector: u8, variation: u16) -> Vec<u8> {
        let mut out = vec![3, 0, selector];
        if variation > 0x7F {
            out.push(0x80 | (variation & 0x7F) as u8);
            out.push((variation >> 8) as u8);
        } else {
            out.push(variation as u8);
        }
        out.push(0); // template-specific options
        out
    }

    pub fn embell(kind: u8) -> Vec<u8> {
        record(6, &[0, kind])
    }

    pub fn end() -> Vec<u8> {
        vec![0]
    }
}
