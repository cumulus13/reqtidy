//! Parser and evaluator for [PEP 508] environment markers such as
//! `python_version >= "3.8" and sys_platform == "win32"`.
//!
//! [PEP 508]: https://peps.python.org/pep-0508/

use std::collections::HashMap;
use std::fmt;
use std::str::FromStr;

use crate::pep440::{Operator, Specifier, Version};

/// Error returned for a malformed marker expression.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid environment marker: {0}")]
pub struct MarkerParseError(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    LParen,
    RParen,
    Ident(String),
    Str(String),
    Op(String),
    And,
    Or,
    In,
    Not,
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '.'
}

fn is_op_char(c: char) -> bool {
    matches!(c, '<' | '>' | '=' | '!' | '~')
}

fn tokenize(src: &str) -> Result<Vec<Token>, MarkerParseError> {
    let chars: Vec<char> = src.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '(' {
            tokens.push(Token::LParen);
            i += 1;
        } else if c == ')' {
            tokens.push(Token::RParen);
            i += 1;
        } else if c == '\'' || c == '"' {
            let start = i + 1;
            let mut j = start;
            while j < chars.len() && chars[j] != c {
                j += 1;
            }
            if j >= chars.len() {
                return Err(MarkerParseError("unterminated string literal".into()));
            }
            tokens.push(Token::Str(chars[start..j].iter().collect()));
            i = j + 1;
        } else if is_op_char(c) {
            let start = i;
            while i < chars.len() && is_op_char(chars[i]) {
                i += 1;
            }
            tokens.push(Token::Op(chars[start..i].iter().collect()));
        } else if is_word_char(c) {
            let start = i;
            while i < chars.len() && is_word_char(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            tokens.push(match word.as_str() {
                "and" => Token::And,
                "or" => Token::Or,
                "in" => Token::In,
                "not" => Token::Not,
                _ => Token::Ident(word),
            });
        } else {
            return Err(MarkerParseError(format!("unexpected character {c:?}")));
        }
    }
    Ok(tokens)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Operand {
    Var(String),
    Lit(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CmpOp {
    Version(Operator),
    In,
    NotIn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Expr {
    Cmp {
        lhs: Operand,
        op: CmpOp,
        rhs: Operand,
    },
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
}

/// Maps deprecated marker variable spellings onto their PEP 508 names.
fn canonical_var(name: &str) -> String {
    let canonical = match name {
        "os.name" => "os_name",
        "sys.platform" => "sys_platform",
        "platform.version" => "platform_version",
        "platform.machine" => "platform_machine",
        "platform.python_implementation" | "python_implementation" => {
            "platform_python_implementation"
        }
        other => other,
    };
    canonical.to_string()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.pos).cloned();
        if token.is_some() {
            self.pos += 1;
        }
        token
    }

    fn parse_or(&mut self) -> Result<Expr, MarkerParseError> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Token::Or)) {
            self.pos += 1;
            let right = self.parse_and()?;
            left = Expr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<Expr, MarkerParseError> {
        let mut left = self.parse_atom()?;
        while matches!(self.peek(), Some(Token::And)) {
            self.pos += 1;
            let right = self.parse_atom()?;
            left = Expr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_atom(&mut self) -> Result<Expr, MarkerParseError> {
        if matches!(self.peek(), Some(Token::LParen)) {
            self.pos += 1;
            let inner = self.parse_or()?;
            return match self.advance() {
                Some(Token::RParen) => Ok(inner),
                other => Err(MarkerParseError(format!("expected ')', found {other:?}"))),
            };
        }
        let lhs = self.parse_operand()?;
        let op = self.parse_op()?;
        let rhs = self.parse_operand()?;
        Ok(Expr::Cmp { lhs, op, rhs })
    }

    fn parse_operand(&mut self) -> Result<Operand, MarkerParseError> {
        match self.advance() {
            Some(Token::Str(s)) => Ok(Operand::Lit(s)),
            Some(Token::Ident(name)) => Ok(Operand::Var(canonical_var(&name))),
            other => Err(MarkerParseError(format!(
                "expected a variable or string, found {other:?}"
            ))),
        }
    }

    fn parse_op(&mut self) -> Result<CmpOp, MarkerParseError> {
        match self.advance() {
            Some(Token::In) => Ok(CmpOp::In),
            Some(Token::Not) => match self.advance() {
                Some(Token::In) => Ok(CmpOp::NotIn),
                other => Err(MarkerParseError(format!(
                    "expected 'in' after 'not', found {other:?}"
                ))),
            },
            Some(Token::Op(op)) => {
                let parsed = match op.as_str() {
                    "==" => Operator::Equal,
                    "!=" => Operator::NotEqual,
                    "<=" => Operator::LessEq,
                    ">=" => Operator::GreaterEq,
                    "<" => Operator::Less,
                    ">" => Operator::Greater,
                    "~=" => Operator::Compatible,
                    "===" => Operator::Arbitrary,
                    _ => return Err(MarkerParseError(format!("unknown operator {op:?}"))),
                };
                Ok(CmpOp::Version(parsed))
            }
            other => Err(MarkerParseError(format!(
                "expected a comparison operator, found {other:?}"
            ))),
        }
    }
}

fn resolve(operand: &Operand, env: &HashMap<String, String>) -> String {
    match operand {
        Operand::Lit(s) => s.clone(),
        Operand::Var(name) => env.get(name).cloned().unwrap_or_default(),
    }
}

fn compare(lhs: &str, op: CmpOp, rhs: &str) -> bool {
    match op {
        CmpOp::In => rhs.contains(lhs),
        CmpOp::NotIn => !rhs.contains(lhs),
        CmpOp::Version(vop) => {
            // Like pip: compare as versions when both sides parse, otherwise as strings.
            if vop != Operator::Arbitrary {
                if let (Ok(left), Ok(spec)) = (Version::from_str(lhs), Specifier::new(vop, rhs)) {
                    return spec.contains(&left);
                }
            }
            match vop {
                Operator::Equal | Operator::Arbitrary => lhs == rhs,
                Operator::NotEqual => lhs != rhs,
                Operator::LessEq => lhs <= rhs,
                Operator::GreaterEq => lhs >= rhs,
                Operator::Less => lhs < rhs,
                Operator::Greater => lhs > rhs,
                Operator::Compatible => false,
            }
        }
    }
}

impl Expr {
    fn eval(&self, env: &HashMap<String, String>) -> bool {
        match self {
            Expr::And(a, b) => a.eval(env) && b.eval(env),
            Expr::Or(a, b) => a.eval(env) || b.eval(env),
            Expr::Cmp { lhs, op, rhs } => compare(&resolve(lhs, env), *op, &resolve(rhs, env)),
        }
    }
}

/// A parsed environment marker.
#[derive(Debug, Clone)]
pub struct Marker {
    expr: Expr,
    source: String,
}

impl Marker {
    /// Evaluates the marker against a marker environment (see
    /// [`crate::python::Interpreter::markers`]). Unknown variables, including
    /// `extra`, evaluate as the empty string.
    pub fn evaluate(&self, env: &HashMap<String, String>) -> bool {
        self.expr.eval(env)
    }

    /// The marker text as written.
    pub fn source(&self) -> &str {
        &self.source
    }
}

impl fmt::Display for Marker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.source)
    }
}

impl FromStr for Marker {
    type Err = MarkerParseError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let source = input.trim();
        let tokens = tokenize(source)?;
        if tokens.is_empty() {
            return Err(MarkerParseError("empty marker".into()));
        }
        let mut parser = Parser { tokens, pos: 0 };
        let expr = parser.parse_or()?;
        if parser.pos != parser.tokens.len() {
            return Err(MarkerParseError(format!(
                "unexpected trailing token {:?}",
                parser.tokens[parser.pos]
            )));
        }
        Ok(Marker {
            expr,
            source: source.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
            .collect()
    }

    fn eval(marker: &str, environment: &HashMap<String, String>) -> bool {
        Marker::from_str(marker)
            .unwrap_or_else(|e| panic!("{e}"))
            .evaluate(environment)
    }

    #[test]
    fn platform_markers() {
        let win = env(&[("sys_platform", "win32"), ("os_name", "nt")]);
        let linux = env(&[("sys_platform", "linux"), ("os_name", "posix")]);
        assert!(eval("sys_platform == \"win32\"", &win));
        assert!(!eval("sys_platform == \"win32\"", &linux));
        assert!(eval("sys_platform != 'win32'", &linux));
        assert!(eval("os_name == 'nt' or sys_platform == 'darwin'", &win));
        assert!(eval("\"win\" in sys_platform", &win));
        assert!(eval("sys_platform not in 'win32 cygwin'", &linux));
    }

    #[test]
    fn python_version_uses_version_semantics() {
        let py39 = env(&[("python_version", "3.9")]);
        let py310 = env(&[("python_version", "3.10")]);
        // A plain string comparison would get both of these wrong.
        assert!(eval("python_version < \"3.10\"", &py39));
        assert!(!eval("python_version < \"3.10\"", &py310));
        assert!(eval("python_version >= '3.8'", &py310));
    }

    #[test]
    fn boolean_logic_and_grouping() {
        let e = env(&[("python_version", "3.11"), ("sys_platform", "linux")]);
        assert!(eval(
            "python_version >= '3.8' and sys_platform == 'linux'",
            &e
        ));
        assert!(!eval(
            "python_version >= '3.8' and sys_platform == 'win32'",
            &e
        ));
        assert!(eval(
            "(sys_platform == 'win32' or sys_platform == 'linux') and python_version > '3.0'",
            &e
        ));
    }

    #[test]
    fn extra_is_empty_and_legacy_names_work() {
        let e = env(&[("sys_platform", "linux")]);
        assert!(!eval("extra == 'dev'", &e));
        assert!(eval("sys.platform == 'linux'", &e));
    }

    #[test]
    fn rejects_malformed_markers() {
        for bad in [
            "",
            "python_version >=",
            "sys_platform == 'x",
            "a == 'b' c",
            "(a == 'b'",
        ] {
            assert!(Marker::from_str(bad).is_err(), "{bad:?} should not parse");
        }
    }
}
