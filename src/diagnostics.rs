use crate::{CallArg, Expr, ParamKind, ParseError, Program, RuntimeError, ScopeEvent, Stmt, Token};
use colored::Colorize;

pub(crate) const ERR_BAD_DECLARATION: &str = "bad declaration";
pub(crate) const ERR_BAD_PRINT_STATEMENT: &str = "bad print statement";
pub(crate) const ERR_FAILED_STATEMENT: &str = "failed to parse statement";
pub(crate) const ERR_EXPECTED_TYPE_AFTER_COLON: &str = "expected type after ':'";
pub(crate) const ERR_EXPECTED_EQ_AFTER_TYPE: &str = "expected '=' after type";
pub(crate) const ERR_MISSING_SEMICOLON: &str = "missing semicolon";
pub(crate) const ERR_LET_CANNOT_INIT: &str =
    "let declarations cannot initialize; use 'x = ...;' or 'x: TYPE = ...;'";
pub(crate) const ERR_EXPECTED_RPAREN: &str = "expected closing ')'";
pub(crate) const ERR_EXPECTED_RCURLY: &str = "expected closing '}'";
pub(crate) const ERR_EXPECTED_EXPRESSION: &str = "expected an expression";

pub(crate) fn lexer_error_message(slice: &str) -> String {
    format!("unrecognized token {:?} — this character is not valid in Cx", slice)
}

pub(crate) fn unresolved_var_error(pos: usize, name: String, was_seen: bool) -> RuntimeError {
    if was_seen {
        RuntimeError::OutOfScope { pos, name }
    } else {
        RuntimeError::UndefinedVar { pos, name }
    }
}

// ── Core print function ──────────────────────────────────────────

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

    let line_no = src[..line_start].bytes().filter(|&b| b == b'\n').count() + 1;

    let mut line = &src[line_start..line_end];
    if let Some(stripped) = line.strip_suffix('\r') {
        line = stripped;
    }

    let col = safe_pos - line_start;

    let colored_title = match title {
        "PARSE ERROR" => title.truecolor(220, 20, 60).bold(),
        "SEMANTIC ERROR" => title.truecolor(220, 20, 60).bold(),
        "RUNTIME ERROR" => title.truecolor(220, 20, 60).bold(),
        "WARNING" => title.yellow().bold(),
        _ => title.white().bold(),
    };

    eprintln!(
        "{} {} {}",
        colored_title,
        format!("(line {}):", line_no).white().dimmed(),
        msg.white()
    );
    eprintln!("{}", line.white().dimmed());
    eprintln!("{}", format!("{:>width$}^", "", width = col + 1).cyan().bold());
}

// ── Parse errors ─────────────────────────────────────────────────

pub(crate) fn print_parse(src: &str, err: &ParseError) {
    print_at(src, "PARSE ERROR", &err.msg, err.pos);
}

// ── Semantic errors ───────────────────────────────────────────────

pub(crate) fn print_custom(src: &str, msg: &str, pos: usize) {
    print_at(src, "SEMANTIC ERROR", msg, pos);
}

// ── Runtime errors ────────────────────────────────────────────────

pub(crate) fn runtime_error_message(err: &RuntimeError) -> (String, usize) {
    match err {
        RuntimeError::DivByZero { pos } => (
            "division by zero — the right-hand side of '/' evaluated to 0".to_string(),
            *pos,
        ),
        RuntimeError::BadOperands {
            pos,
            op,
            left,
            right,
        } => (
            format!(
                "operator '{:?}' cannot be applied to '{:?}' and '{:?}' — types are incompatible",
                op, left, right
            ),
            *pos,
        ),
        RuntimeError::TypeMismatch { pos, expected, got } => (
            format!(
                "type mismatch — expected '{:?}' but got '{:?}'",
                expected, got
            ),
            *pos,
        ),
        RuntimeError::AlreadyDeclared { pos, name } => (
            format!(
                "variable '{}' was already declared in this scope — use a different name or remove the duplicate",
                name
            ),
            *pos,
        ),
        RuntimeError::UndefinedVar { pos, name } => (
            format!(
                "variable '{}' has not been declared — declare it with 'let {}' or '{}: TYPE = value' before use",
                name, name, name
            ),
            *pos,
        ),
        RuntimeError::OutOfScope { pos, name } => (
            format!(
                "variable '{}' was declared in a different scope and is not accessible here",
                name
            ),
            *pos,
        ),
        RuntimeError::UninitializedVar { pos, name } => (
            format!(
                "variable '{}' was declared but never assigned a value before this use",
                name
            ),
            *pos,
        ),
        RuntimeError::TemplateInvalidPlaceholder { pos, placeholder } => (
            format!(
                "invalid template placeholder '{{{}}}' — only {{NAME}} or {{NAME:?}} are allowed",
                placeholder
            ),
            *pos,
        ),
        RuntimeError::TemplateInvalidFormat { pos, spec } => (
            format!(
                "invalid template format specifier ':{}'  — only ':?' is supported",
                spec
            ),
            *pos,
        ),
        RuntimeError::BadAssignTarget { pos } => (
            "invalid assignment target — only variables and container fields (t.x) can be assigned to".to_string(),
            *pos,
        ),
        RuntimeError::NotAContainer { pos, name } => (
            format!(
                "'{}' is not a container — dot access is only valid on copy_into containers",
                name
            ),
            *pos,
        ),
        RuntimeError::EarlyReturn(_) => (
            "return statement used outside of a function body".to_string(),
            0,
        ),
    }
}

pub(crate) fn print_runtime(src: &str, err: &RuntimeError) {
    let (msg, pos) = runtime_error_message(err);
    print_at(src, "RUNTIME ERROR", &msg, pos);
}

// ── Summary line ──────────────────────────────────────────────────

pub(crate) fn print_summary(error_count: usize) {
    if error_count == 0 {
        return;
    }
    let label = if error_count == 1 {
        "── 1 error found ──".to_string()
    } else {
        format!("── {} errors found ──", error_count)
    };
    eprintln!("{}", label.truecolor(255, 191, 0).bold());
}

pub fn print_token_table(tokens: &[(Token, std::ops::Range<usize>)], src: &str) {
    eprintln!(
        "{}",
        "── TOKENS ──────────────────────────────────────────".cyan().bold()
    );
    eprintln!(
        "{}",
        format!(
            " {:<4} {:<20} {:<15} {:<6} {:<6} {}",
            "#", "TOKEN", "VALUE", "LINE", "COL", "BYTES"
        )
        .white()
        .bold()
    );

    for (i, (tok, span)) in tokens.iter().enumerate() {
        let slice = &src[span.clone()];
        let line_no = src[..span.start].bytes().filter(|&b| b == b'\n').count() + 1;
        let line_start = src[..span.start].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let col = span.start - line_start + 1;
        let bytes = format!("{}..{}", span.start, span.end);

        eprintln!(
            " {:<4} {:<20} {:<15} {:<6} {:<6} {}",
            i + 1,
            format!("{:?}", tok).split('(').next().unwrap_or(""),
            slice,
            line_no,
            col,
            bytes.dimmed()
        );
    }
    eprintln!();
}

pub fn print_ast(program: &Program) {
    eprintln!(
        "{}",
        "── AST ─────────────────────────────────────────────".cyan().bold()
    );
    for stmt in &program.stmts {
        print_stmt(stmt, 0);
    }
    eprintln!();
}

fn indent(depth: usize) -> String {
    "  ".repeat(depth)
}

fn print_stmt(stmt: &Stmt, depth: usize) {
    let pad = indent(depth);
    match stmt {
        Stmt::Decl { name, ty, .. } => {
            eprintln!("{}Decl({}: {:?})", pad, name, ty);
        }
        Stmt::Assign { target, expr, .. } => {
            eprintln!("{}Assign", pad);
            eprintln!("{}  target:", pad);
            print_expr(target, depth + 2);
            eprintln!("{}  value:", pad);
            print_expr(expr, depth + 2);
        }
        Stmt::TypedAssign { name, ty, expr, .. } => {
            eprintln!("{}TypedAssign({}: {:?})", pad, name, ty);
            print_expr(expr, depth + 1);
        }
        Stmt::Print { expr, .. } => {
            eprintln!("{}Print", pad);
            print_expr(expr, depth + 1);
        }
        Stmt::PrintInline { expr, .. } => {
            eprintln!("{}PrintInline", pad);
            print_expr(expr, depth + 1);
        }
        Stmt::Return { expr, .. } => {
            eprintln!("{}Return", pad);
            if let Some(e) = expr {
                print_expr(e, depth + 1);
            }
        }
        Stmt::FuncDef {
            name,
            params,
            ret_ty,
            body,
            ret_expr,
            ..
        } => {
            let ret = ret_ty
                .map(|t| format!("{:?}", t))
                .unwrap_or("void".to_string());
            eprintln!("{}FuncDef({}) -> {}", pad, name, ret);
            for param in params {
                match param {
                    ParamKind::Typed(pname, pty) => {
                        eprintln!("{}  Param({}: {:?})", pad, pname, pty);
                    }
                    ParamKind::Copy(pname) => {
                        eprintln!("{}  Param({}.copy)", pad, pname);
                    }
                    ParamKind::CopyFree(pname) => {
                        eprintln!("{}  Param({}.copy.free)", pad, pname);
                    }
                }
            }
            eprintln!("{}  Body", pad);
            for s in body {
                print_stmt(s, depth + 2);
            }
            if let Some(e) = ret_expr {
                eprintln!("{}  ImplicitReturn", pad);
                print_expr(e, depth + 2);
            }
        }
        Stmt::Block { stmts, .. } => {
            eprintln!("{}Block", pad);
            for s in stmts {
                print_stmt(s, depth + 1);
            }
        }
        Stmt::ExprStmt { expr, .. } => {
            eprintln!("{}ExprStmt", pad);
            print_expr(expr, depth + 1);
        }
    }
}

fn print_expr(expr: &Expr, depth: usize) {
    let pad = indent(depth);
    match expr {
        Expr::Val(v) => eprintln!("{}Val({:?})", pad, v),
        Expr::Ident(name, _) => eprintln!("{}Ident({})", pad, name),
        Expr::DotAccess(con, field) => eprintln!("{}DotAccess({}.{})", pad, con, field),
        Expr::Call(name, args, _) => {
            eprintln!("{}Call({})", pad, name);
            for a in args {
                match a {
                    CallArg::Expr(expr) => print_expr(expr, depth + 1),
                    CallArg::Copy(name) => eprintln!("{}  ArgCopy({})", pad, name),
                    CallArg::CopyFree(name) => eprintln!("{}  ArgCopyFree({})", pad, name),
                    CallArg::CopyInto(names) => eprintln!("{}  ArgCopyInto({:?})", pad, names),
                }
            }
        }
        Expr::Bin(lhs, op, _, rhs) => {
            eprintln!("{}BinOp({:?})", pad, op);
            print_expr(lhs, depth + 1);
            print_expr(rhs, depth + 1);
        }
    }
}

pub fn print_scope_event(event: &ScopeEvent) {
    match event {
        ScopeEvent::Open(name) => {
            eprintln!("{}", format!("[SCOPE OPEN]  {}", name).green().bold());
        }
        ScopeEvent::Close(name) => {
            eprintln!("{}", format!("[SCOPE CLOSE] {}", name).yellow().bold());
        }
        ScopeEvent::Add(name, val) => {
            eprintln!("  {}", format!("+ {}  = {:?}", name, val).green());
        }
        ScopeEvent::Mutate(name, val) => {
            eprintln!("  {}", format!("~ {}  = {:?}", name, val).yellow());
        }
        ScopeEvent::Free(name) => {
            eprintln!("  {}", format!("- {}  = freed", name).red());
        }
        ScopeEvent::BleedBack(name, val) => {
            eprintln!("  {}", format!("~ {}  = {:?}  (bled back)", name, val).cyan());
        }
    }
}

pub fn emit_scope_event(event: ScopeEvent) {
    print_scope_event(&event);
}

pub fn print_stmt_summary(stmt: &Stmt) {
    let label = match stmt {
        Stmt::Decl { name, .. } => format!("Decl {}", name),
        Stmt::Assign { .. } => "Assign".to_string(),
        Stmt::TypedAssign { name, .. } => format!("TypedAssign {}", name),
        Stmt::Print { .. } => "Print".to_string(),
        Stmt::PrintInline { .. } => "PrintInline".to_string(),
        Stmt::ExprStmt { .. } => "ExprStmt".to_string(),
        Stmt::Return { .. } => "Return".to_string(),
        Stmt::FuncDef { name, .. } => format!("FuncDef {}", name),
        Stmt::Block { .. } => "Block".to_string(),
    };
    eprintln!("{}", format!("  [stmt] {}", label).white().dimmed());
}
