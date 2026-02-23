use crate::{ParseError, RuntimeError};

pub(crate) const ERR_BAD_DECLARATION: &str = "bad declaration";
pub(crate) const ERR_BAD_PRINT_STATEMENT: &str = "bad print statement";
pub(crate) const ERR_FAILED_STATEMENT: &str = "failed to parse statement";
pub(crate) const ERR_EXPECTED_TYPE_AFTER_COLON: &str = "expected type after ':'";
pub(crate) const ERR_EXPECTED_EQ_AFTER_TYPE: &str = "expected '=' after type";
pub(crate) const ERR_MISSING_SEMICOLON: &str = "missing semicolon";
pub(crate) const ERR_LET_CANNOT_INIT: &str =
    "let declarations cannot initialize; use x = ...; or x: TYPE = ...;";
pub(crate) const ERR_EXPECTED_RPAREN: &str = "expected ')'";
pub(crate) const ERR_EXPECTED_RCURLY: &str = "expected '}'";
pub(crate) const ERR_EXPECTED_EXPRESSION: &str = "expected expression";

pub(crate) fn lexer_error_message(slice: &str) -> String {
    format!("lexer error near {:?}", slice)
}

pub(crate) fn unresolved_var_error(pos: usize, name: String, was_seen: bool) -> RuntimeError {
    if was_seen {
        RuntimeError::OutOfScope { pos, name }
    } else {
        RuntimeError::UndefinedVar { pos, name }
    }
}

pub(crate) fn print_at(src: &str, title: &str, msg: &str, pos: usize) {
    let bytes = src.as_bytes();
    let safe_pos = pos.min(bytes.len());

    let mut line_start = safe_pos;
    while line_start > 0 && bytes[line_start - 1] != b'\n' {
        line_start -= 1;
    }

    let mut line_end = safe_pos;
    while line_end < bytes.len() && bytes[line_end] != b'\n' {
        line_end += 1;
    }

    let line_no = src[..line_start]
        .bytes()
        .filter(|&b| b == b'\n')
        .count()
        + 1;

    let mut line = &src[line_start..line_end];
    if let Some(stripped) = line.strip_suffix('\r') {
        line = stripped;
    }

    let col = safe_pos - line_start;

    eprintln!("{} (line {}): {}", title, line_no, msg);
    eprintln!("{}", line);
    eprintln!("{:>width$}^", "", width = col + 1);
}

pub(crate) fn print_parse(src: &str, err: &ParseError) {
    print_at(src, "PARSE ERROR", &err.msg, err.pos);
}

pub(crate) fn print_custom(src: &str, msg: &str, pos: usize) {
    print_at(src, "SEMANTIC ERROR", msg, pos);
}

pub(crate) fn print_runtime(src: &str, err: &RuntimeError) {
    match err {
        RuntimeError::DivByZero { pos } => {
            print_at(src, "RUNTIME ERROR", "division by zero", *pos);
        }
        RuntimeError::BadOperands {
            pos,
            op,
            left,
            right,
        } => {
            let msg = format!("bad operands for {:?}: {:?} and {:?}", op, left, right);
            print_at(src, "RUNTIME ERROR", &msg, *pos);
        }
        RuntimeError::TypeMismatch { pos, expected, got } => {
            let msg = format!("type mismatch: expected {:?}, got {:?}", expected, got);
            print_at(src, "RUNTIME ERROR", &msg, *pos);
        }
        RuntimeError::AlreadyDeclared { pos, name } => {
            let msg = format!("variable already declared in this scope: {}", name);
            print_at(src, "RUNTIME ERROR", &msg, *pos);
        }
        RuntimeError::UndefinedVar { pos, name } => {
            let msg = format!("undefined variable: {}", name);
            print_at(src, "RUNTIME ERROR", &msg, *pos);
        }
        RuntimeError::OutOfScope { pos, name } => {
            let msg = format!("variable is out of scope: {}", name);
            print_at(src, "RUNTIME ERROR", &msg, *pos);
        }
        RuntimeError::UninitializedVar { pos, name } => {
            let msg = format!("uninitialized variable: {}", name);
            print_at(src, "RUNTIME ERROR", &msg, *pos);
        }
        RuntimeError::TemplateInvalidPlaceholder { pos, placeholder } => {
            let msg = format!(
                "invalid template placeholder '{{{}}}'. Only {{NAME}} or {{NAME:?}} allowed.",
                placeholder
            );
            print_at(src, "RUNTIME ERROR", &msg, *pos);
        }
        RuntimeError::TemplateInvalidFormat { pos, spec } => {
            let msg = format!("invalid template format ':{}' (only ':?' allowed).", spec);
            print_at(src, "RUNTIME ERROR", &msg, *pos);
        }
        RuntimeError::EarlyReturn(_) => {
            print_at(
                src,
                "RUNTIME ERROR",
                "return used outside of function body",
                0,
            );
        }
    }
}
