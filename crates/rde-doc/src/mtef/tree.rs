//! Дерево объектов MTEF из плоского списка записей (порт `makeAST` из mtef-go v2.0, Apache-2.0).
//!
//! MTEF — это поток записей в обратной польской нотации: `LINE`/`TMPL`/`PILE`/`MATRIX` открывают
//! вложенный список объектов, `END` закрывает его, `CHAR`/`EMBELL` — листья. `LINE` с флагом
//! «null» — пустая строка-заполнитель (в дерево входит без детей). Акценты (`EMBELL` dot/hat/bar)
//! в mtef-go переставляются перед акцентируемым символом — так LaTeX получает `\dot H`, а не
//! `H\dot`; эта же логика сохранена.

use super::records::{CharNode, Node};
use super::MtefError;

/// Узел дерева MTEF.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Ast {
    Line { null: bool, children: Vec<Ast> },
    Char(CharNode),
    Tmpl { selector: u8, variation: u16, children: Vec<Ast> },
    Pile { children: Vec<Ast> },
    Matrix { cols: u8, children: Vec<Ast> },
    Embell { kind: u8, children: Vec<Ast> },
}

/// Глубина вложенности MTEF, после которой данные считаются враждебными.
const MAX_DEPTH: usize = 64;

/// Построить дерево из плоского списка записей. (`doc comment` — см. модуль.)
pub(super) fn build(nodes: &[Node]) -> Result<Vec<Ast>, MtefError> {
    let mut pos = 0usize;
    let ast = parse_list(nodes, &mut pos, 0)?;
    // Хвост после закрытия корневого списка mtef-go игнорирует — сохраняем это поведение.
    Ok(ast)
}

fn parse_list(nodes: &[Node], pos: &mut usize, depth: usize) -> Result<Vec<Ast>, MtefError> {
    if depth > MAX_DEPTH {
        return Err(MtefError::BadRecord);
    }
    let mut out = Vec::new();
    while let Some(node) = nodes.get(*pos) {
        *pos += 1;
        match *node {
            Node::End => return Ok(out),
            Node::Char(c) => out.push(Ast::Char(c)),
            Node::Line { null } => {
                if null {
                    out.push(Ast::Line { null, children: Vec::new() });
                } else {
                    let children = parse_list(nodes, pos, depth + 1)?;
                    out.push(Ast::Line { null, children });
                }
            }
            Node::Tmpl(t) => {
                let children = parse_list(nodes, pos, depth + 1)?;
                out.push(Ast::Tmpl { selector: t.selector, variation: t.variation, children });
            }
            Node::Pile => {
                let children = parse_list(nodes, pos, depth + 1)?;
                out.push(Ast::Pile { children });
            }
            Node::Matrix { cols } => {
                let children = parse_list(nodes, pos, depth + 1)?;
                out.push(Ast::Matrix { cols, children });
            }
            Node::Embell { kind } => {
                let children = parse_list(nodes, pos, depth + 1)?;
                let embell = Ast::Embell { kind, children };
                // Акцент ставится перед символом: LaTeX-команда — префикс (`\dot H`).
                if matches!(kind, 2 | 9 | 17)
                    && let Some(previous) = out.pop()
                {
                    out.push(embell);
                    out.push(previous);
                    continue;
                }
                out.push(embell);
            }
        }
    }
    Ok(out)
}
