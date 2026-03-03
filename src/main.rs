#![allow(unused)]
//extern crate core;

mod diagnostics;
mod parser;
mod semantic;

use chumsky::input::{Input, Stream};
use chumsky::prelude::SimpleSpan;
use chumsky::Parser;
use colored::Colorize;
use logos::Logos;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::time::Instant;

fn unescape_string(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some('r') => out.push('\r'),
                Some('t') => out.push('\t'),
                Some('\\') => out.push('\\'),
                Some('"') => out.push('"'),
                Some('0') => out.push('\0'),
                Some(c) => {
                    out.push('\\');
                    out.push(c);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}
// Token kinds recognized by the lexer; whitespace is skipped via a Logos attribute.
#[derive(Logos, Debug, PartialEq, Clone)]
#[logos(skip r"[ \t\r\n\f]+")] // skip whitespace
pub enum Token {
    // ── Comments ────────────────────────────────
    #[regex(r"//[^\r\n]*", logos::skip, allow_greedy = true)]
    LineComment,

    // ── Keywords ────────────────────────────────
    #[token("let")]
    KeywordLet,

    #[token("print")]
    KeywordPrint,

    #[token("print!")]
    KeywordPrintInline,

    #[token("fnc")]
    KeywordFnc,

    #[token("return")]
    KeywordReturn,

    #[token("true")]
    KeywordTrue,

    #[token("false")]
    KeywordFalse,

    // ── Type Keywords ────────────────────────────
    #[token("t8")]
    TypeT8,

    #[token("t16")]
    TypeT16,

    #[token("t32")]
    TypeT32,

    #[token("t64")]
    TypeT64,

    #[token("t128")]
    TypeT128,

    #[token("bool")]
    TypeBool,

    #[token("str")]
    TypeStr,

    #[token("char")]
    TypeChar,

    // ── Literals ─────────────────────────────────
    #[regex(r#""([^"\\]|\\.)*""#, |lex| {
        let s = lex.slice();
        Some(unescape_string(&s[1..s.len()-1]))
    })]
    LiteralString(String),

    #[regex(r"[0-9]+\.[0-9]+", |lex| lex.slice().parse::<f64>().ok())]
    LiteralFloat(f64),

    #[regex(r"[0-9]+", |lex| lex.slice().parse::<u128>().ok())]
    LiteralInt(u128),

    #[regex(r"'([^'\\]|\\.)'", |lex| {
        let s = lex.slice();
        let inner = &s[1..s.len()-1];
        let ch = if inner.starts_with('\\') {
            match inner.chars().nth(1).unwrap_or('\\') {
                'n'  => '\n',
                'r'  => '\r',
                't'  => '\t',
                '\\' => '\\',
                '\'' => '\'',
                '0'  => '\0',
                other => other,
            }
        } else {
            inner.chars().next().unwrap_or('\0')
        };
        ch
    })]
    LiteralChar(char),

    // ── Identifiers ───────────────────────────────
    #[regex(r"[_\p{L}][_\p{L}\p{N}]*", |lex| lex.slice().to_string())]
    Identifier(String),

    // ── Arithmetic Operators ──────────────────────
    #[token("+")]
    OpAdd,

    #[token("-")]
    OpSub,

    #[token("*")]
    OpMul,

    #[token("/")]
    OpDiv,

    #[token("%")]
    OpMod,

    // ── Comparison Operators ──────────────────────
    #[token("==")]
    OpEqualEqual,

    #[token(">")]
    OpGreaterThan,

    #[token("<")]
    OpLessThan,

    // ── Assignment ───────────────────────────────
    #[token("=")]
    OpAssign,

    // ── Arrow ────────────────────────────────────
    #[token("->")]
    PunctArrow,

    // ── Punctuation ──────────────────────────────
    #[token(":")]
    PunctColon,

    #[token(";")]
    PunctSemicolon,

    #[token(",")]
    PunctComma,

    #[token(".")]
    PunctDot,

    #[token("(")]
    PunctParenOpen,

    #[token(")")]
    PunctParenClose,

    #[token("{")]
    PunctBraceOpen,

    #[token("}")]
    PunctBraceClose,

    #[end]
    EndOfFile,
}

// Helper that mirrors the lexer rules and is handy for tests or manual inspection.
pub fn tok_match(string: &str) -> Token {
    match string {
        "let" => Token::KeywordLet,
        "print" => Token::KeywordPrint,
        "print!" => Token::KeywordPrintInline,
        "fnc" => Token::KeywordFnc,
        "return" => Token::KeywordReturn,
        "true" => Token::KeywordTrue,
        "false" => Token::KeywordFalse,
        "t8" => Token::TypeT8,
        "t16" => Token::TypeT16,
        "t32" => Token::TypeT32,
        "t64" => Token::TypeT64,
        "t128" => Token::TypeT128,
        "bool" => Token::TypeBool,
        "str" => Token::TypeStr,
        "char" => Token::TypeChar,
        "=" => Token::OpAssign,
        "==" => Token::OpEqualEqual,
        ">" => Token::OpGreaterThan,
        "<" => Token::OpLessThan,
        "->" => Token::PunctArrow,
        ":" => Token::PunctColon,
        ";" => Token::PunctSemicolon,
        "," => Token::PunctComma,
        "." => Token::PunctDot,
        "(" => Token::PunctParenOpen,
        ")" => Token::PunctParenClose,
        "+" => Token::OpAdd,
        "-" => Token::OpSub,
        "*" => Token::OpMul,
        "/" => Token::OpDiv,
        "%" => Token::OpMod,
        _ => {
            if let Ok(x) = string.parse::<f64>() {
                Token::LiteralFloat(x)
            } else if let Ok(n) = string.parse::<u128>() {
                Token::LiteralInt(n)
            } else {
                Token::Identifier(string.to_string())
            }
        }
    }
}

#[derive(Debug, Default)]
struct DebugFlags {
    tokens: bool,
    ast: bool,
    scope: bool,
    step: bool,
    phase: bool,
}

impl DebugFlags {
    fn from_args(args: &[String]) -> Self {
        let all = args.contains(&"--debug".to_string());
        Self {
            tokens: all || args.contains(&"--debug-tokens".to_string()),
            ast: all || args.contains(&"--debug-ast".to_string()),
            scope: all || args.contains(&"--debug-scope".to_string()),
            step: all || args.contains(&"--debug-step".to_string()),
            phase: all || args.contains(&"--debug-phase".to_string()),
        }
    }

    fn any(&self) -> bool {
        self.tokens || self.ast || self.scope || self.step || self.phase
    }
}

struct PhaseTimer {
    label: &'static str,
    start: Instant,
}

impl PhaseTimer {
    fn start(label: &'static str) -> Self {
        Self {
            label,
            start: Instant::now(),
        }
    }

    fn finish(self, detail: &str) {
        let ms = self.start.elapsed().as_secs_f64() * 1000.0;
        eprintln!(
            "{}",
            format!("[{:<10}] {:<30} {:.2}ms", self.label, detail, ms)
                .cyan()
                .dimmed()
        );
    }
}

fn wait_for_step() {
    use std::io::Write;
    eprint!("{}", "  [step] press enter to continue...".dimmed());
    std::io::stderr().flush().ok();

    #[cfg(unix)]
    {
        use std::io::BufRead;
        let tty = std::fs::File::open("/dev/tty").unwrap();
        let mut reader = std::io::BufReader::new(tty);
        let mut line = String::new();
        reader.read_line(&mut line).ok();
    }
    #[cfg(windows)]
    {
        use std::io::BufRead;
        let tty = std::fs::File::open("CONIN$").unwrap();
        let mut reader = std::io::BufReader::new(tty);
        let mut line = String::new();
        reader.read_line(&mut line).ok();
    }
}

// Simple driver that tokenizes, parses, and executes a .cx file.
fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let flags = DebugFlags::from_args(&args);
    let path = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "src/tests/test.cx".to_string());

    let input = fs::read_to_string(&path).expect("failed to read .cx file");

    // ── LEXER PHASE ──────────────────────────────
    let lex_timer = flags.phase.then(|| PhaseTimer::start("LEXER"));
    let tok_list = match tok_collector(&input) {
        Ok(t) => t,
        Err(e) => {
            diagnostics::print_parse(&input, &e);
            return;
        }
    };
    if let Some(t) = lex_timer {
        t.finish(&format!("{} tokens", tok_list.len()));
    }
    if flags.tokens {
        let pairs: Vec<_> = tok_list
            .iter()
            .map(|t| (t.kind.clone(), t.span.clone()))
            .collect();
        diagnostics::print_token_table(&pairs, &input);
    }

    // ── PARSER PHASE ─────────────────────────────
    let parse_timer = flags.phase.then(|| PhaseTimer::start("PARSER"));
    let program = match parse_program_with_fallback(&tok_list, &input, false) {
        Ok(p) => p,
        Err(e) => {
            diagnostics::print_parse(&input, &e);
            return;
        }
    };
    if let Some(t) = parse_timer {
        t.finish(&format!("{} statements", program.stmts.len()));
    }
    if flags.ast {
        diagnostics::print_ast(&program);
    }

    // ── SEMANTIC PHASE ────────────────────────────
    let sem_timer = flags.phase.then(|| PhaseTimer::start("SEMANTIC"));
    let semantic_errors = semantic::analyze_program(&program);
    if let Some(t) = sem_timer {
        t.finish(&format!("{} errors", semantic_errors.len()));
    }
    if !semantic_errors.is_empty() {
        for e in &semantic_errors {
            diagnostics::print_custom(&input, &e.msg, e.pos);
        }
        diagnostics::print_summary(semantic_errors.len());
        return;
    }

    // ── RUNTIME PHASE ─────────────────────────────
    let rt_timer = flags.phase.then(|| PhaseTimer::start("RUNTIME"));
    let mut rt = RunTime::new();
    rt.debug_scope = flags.scope;
    let mut step_count = 0;

    for stmt in program.stmts {
        if flags.step {
            eprintln!("{}", format!("\n[STEP {}]", step_count + 1).cyan().bold());
            diagnostics::print_stmt_summary(&stmt);
            wait_for_step();
        }
        if let Err(err) = run_stmt(&mut rt, stmt) {
            diagnostics::print_runtime(&input, &err);
            diagnostics::print_summary(1);
            break;
        }
        step_count += 1;
    }
    if let Some(t) = rt_timer {
        t.finish(&format!("{} steps", step_count));
    }
}

fn parse_program_with_fallback(tok_list: &[Tok], src: &str, _debug: bool) -> Result<Program, ParseError> {
    match parse_program_chumsky(tok_list, src) {
        Ok(program) => Ok(program),
        Err(chumsky_errs) => Err(chumsky_errs.into_iter().next().unwrap_or(ParseError {
            msg: diagnostics::ERR_FAILED_STATEMENT.to_string(),
            pos: src.len(),
        })),
    }
}

fn parse_program_chumsky(tok_list: &[Tok], src: &str) -> Result<Program, Vec<ParseError>> {
    let token_iter = tok_list
        .iter()
        .map(|t| {
            (
                t.kind.clone(),
                (t.span.start..t.span.end).into(),
            )
        });

    let eoi: SimpleSpan = (src.len()..src.len()).into();
    let input = Stream::from_iter(token_iter).map(eoi, |(token, span): (_, _)| (token, span));

    match parser::program_parser().parse(input).into_result() {
        Ok(program) => Ok(program),
        Err(errs) => {
            let mapped = errs
                .into_iter()
                .map(|e| ParseError {
                    msg: format!("{:?}", e.reason()),
                    pos: e.span().start,
                })
                .collect::<Vec<ParseError>>();
            Err(mapped)
        }
    }
}

fn run_stmt(rt: &mut RunTime, stmt: Stmt) -> Result<(), RuntimeError> {
    match stmt {
        Stmt::Decl { name, ty, pos } => rt.declare(name, ty, pos),
        Stmt::Assign {
            target,
            expr,
            pos_eq,
        } => {
            let value = rt.eval_expr(&expr)?;
            match target {
                Expr::Ident(name, _) => rt.set_var(name, value, pos_eq),
                Expr::DotAccess(container, field) => {
                    rt.set_container_field(&container, &field, value, pos_eq)
                }
                _ => Err(RuntimeError::BadAssignTarget { pos: pos_eq }),
            }
        }
        Stmt::TypedAssign {
            name,
            ty,
            expr,
            pos_type,
        } => {
            let value = rt.eval_expr(&expr)?;
            rt.set_var_typed(name, ty, value, pos_type)
        }
        Stmt::Print { expr, pos } => {
            let value = rt.eval_expr(&expr)?;
            match value {
                Value::Num(n) => println!("{}", n),
                Value::Float(x) => println!("{}", x),
                Value::Str(s) => println!("{}", s),
                Value::Bool(b) => println!("{}", b),
                Value::Char(c) => println!("{}", c),
                Value::Container(map) => println!("{:?}", map),
            }
            Ok(())
        }
        Stmt::PrintInline { expr, pos: _ } => {
            let value = rt.eval_expr(&expr)?;
            match value {
                Value::Num(n) => print!("{}", n),
                Value::Float(x) => print!("{}", x),
                Value::Str(s) => print!("{}", s),
                Value::Bool(b) => print!("{}", b),
                Value::Char(c) => print!("{}", c),
                Value::Container(map) => print!("{:?}", map),
            }
            use std::io::Write;
            std::io::stdout().flush().ok();
            Ok(())
        }
        Stmt::ExprStmt { expr, .. } => {
            rt.eval_expr(&expr)?;
            Ok(())
        }
        Stmt::Return { expr, .. } => {
            let val = match expr {
                Some(e) => rt.eval_expr(&e)?,
                None => Value::Num(0),
            };
            Err(RuntimeError::EarlyReturn(val))
        }
        Stmt::FuncDef {
            name,
            params,
            ret_ty,
            body,
            ret_expr,
            ..
        } => {
            rt.funcs.insert(
                name,
                FuncDef {
                    params,
                    ret_ty,
                    body,
                    ret_expr,
                },
            );
            Ok(())
        }
        Stmt::Block { stmts, .. } => {
            rt.push_scope();
            for stmt in stmts {
                if let Err(err) = run_stmt(rt, stmt) {
                    rt.pop_scope();
                    return Err(err);
                }
            }
            rt.pop_scope();
            Ok(())
        }
    }
}

fn value_to_string(v: Value) -> String {
    match v {
        Value::Num(n) => n.to_string(),
        Value::Float(x) => x.to_string(),
        Value::Str(s) => s,
        Value::Bool(b) => b.to_string(),
        Value::Char(c) => c.to_string(),
        Value::Container(map) => format!("{:?}", map),
    }
}

fn type_of_value(v: &Value) -> Type {
    match v {
        Value::Num(_) => Type::T128,
        Value::Float(_) => Type::T64,
        Value::Str(_) => Type::Str,
        Value::Bool(_) => Type::Bool,
        Value::Char(_) => Type::Char,
        Value::Container(_) => Type::Str,
    }
}

fn value_matches_type(v: &Value, t: Type) -> bool {
    match (v, t) {
        (Value::Num(_), Type::T8) => true,
        (Value::Num(_), Type::T16) => true,
        (Value::Num(_), Type::T32) => true,
        (Value::Num(_), Type::T64) => true,
        (Value::Num(_), Type::T128) => true,
        (Value::Float(_), Type::T8) => true,
        (Value::Float(_), Type::T16) => true,
        (Value::Float(_), Type::T32) => true,
        (Value::Float(_), Type::T64) => true,
        (Value::Float(_), Type::T128) => true,
        (Value::Str(_), Type::Str) => true,
        (Value::Bool(_), Type::Bool) => true,
        (Value::Char(_), Type::Char) => true,
        _ => false,
    }
}

fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_alphanumeric())
}

fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Num(n) => Some(*n as f64),
        Value::Float(x) => Some(*x),
        _ => None,
    }
}

fn expand_template(rt: &RunTime, s: &str, pos: usize) -> Result<String, RuntimeError> {
    let mut out = String::new();
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            let mut spec = String::new();
            let mut in_spec = false;
            while let Some(&ch) = chars.peek() {
                chars.next();
                if ch == '}' {
                    break;
                }
                if ch == ':' {
                    in_spec = true
                } else if in_spec {
                    spec.push(ch);
                } else {
                    name.push(ch);
                }
            }
            let key = name.trim();
            if !is_ident(key) {
                return Err(RuntimeError::TemplateInvalidPlaceholder {
                    pos,
                    placeholder: key.to_string(),
                });
            }
            if !(spec.is_empty() || spec == "?") {
                return Err(RuntimeError::TemplateInvalidFormat {
                    pos,
                    spec: spec.to_string(),
                });
            }
            let v = rt.get_var(key, pos)?;
            if spec == "?" {
                out.push_str(&format!("{:?}", v));
            } else {
                out.push_str(&value_to_string(v));
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

// Tokens carry both their kind and the byte span for error reporting.
#[derive(Debug, Clone)]
struct Tok {
    kind: Token,
    span: std::ops::Range<usize>,
}

// Drive the Logos lexer to collect tokens and emit diagnostics on bad input.
fn tok_collector(input: &str) -> Result<Vec<Tok>, ParseError> {
    let mut lex_in = Token::lexer(input);
    let mut lex_out = Vec::new();

    while let Some(res) = lex_in.next() {
        match res {
            Ok(kind) => {
                lex_out.push(Tok {
                    kind,
                    span: lex_in.span(),
                });
            }
            Err(_) => {
                let span = lex_in.span();
                return Err(ParseError {
                    msg: diagnostics::lexer_error_message(lex_in.slice()),
                    pos: span.start,
                });
            }
        }
    }
    Ok(lex_out)
}

#[derive(Debug, Clone)]
enum Value {
    Num(u128),
    Float(f64),
    Str(String),
    Bool(bool),
    Char(char),
    Container(HashMap<String, Value>),
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Plus,
    Minus,
    Mul,
    Div,
    Mod,
    EqEq,
}

#[derive(Debug, Clone)]
enum CallArg {
    Expr(Expr),
    Copy(String),
    CopyFree(String),
    CopyInto(Vec<String>),
}

#[derive(Debug, Clone)]
enum ParamKind {
    Typed(String, Type),
    Copy(String),
    CopyFree(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Type {
    T8,
    T16,
    T32,
    T64,
    T128,
    Bool,
    Str,
    Char,
}

#[derive(Debug, Clone)]
enum Expr {
    Val(Value),
    Ident(String, usize),
    DotAccess(String, String),
    Call(String, Vec<CallArg>, usize),
    Bin(Box<Expr>, Op, usize, Box<Expr>),
}

// AST statements produced by the parser
#[derive(Debug, Clone)]
enum Stmt {
    Decl {
        name: String,
        ty: Option<Type>,
        pos: usize,
    },
    Assign {
        target: Expr,
        expr: Expr,
        pos_eq: usize,
    },
    TypedAssign {
        name: String,
        ty: Type,
        expr: Expr,
        pos_type: usize,
    },
    Print {
        expr: Expr,
        pos: usize,
    },
    PrintInline {
        expr: Expr,
        pos: usize,
    },
    ExprStmt {
        expr: Expr,
        pos: usize,
    },
    Return {
        expr: Option<Expr>,
        pos: usize,
    },
    FuncDef {
        name: String,
        params: Vec<ParamKind>,
        ret_ty: Option<Type>,
        body: Vec<Stmt>,
        ret_expr: Option<Expr>,
        pos: usize,
    },
    Block {
        stmts: Vec<Stmt>,
        pos: usize,
    },
}

struct Program {
    stmts: Vec<Stmt>,
}

struct ParseError {
    msg: String,
    pos: usize,
}

#[derive(Debug, Clone)]
pub struct SemanticError {
    pub msg: String,
    pub pos: usize,
}

#[derive(Debug)]
enum RuntimeError {
    DivByZero {
        pos: usize,
    },
    BadOperands {
        pos: usize,
        op: Op,
        left: Value,
        right: Value,
    },
    TypeMismatch {
        pos: usize,
        expected: Type,
        got: Type,
    },
    AlreadyDeclared {
        pos: usize,
        name: String,
    },
    UndefinedVar {
        pos: usize,
        name: String,
    },
    OutOfScope {
        pos: usize,
        name: String,
    },
    UninitializedVar {
        pos: usize,
        name: String,
    },
    TemplateInvalidPlaceholder {
        pos: usize,
        placeholder: String,
    },
    TemplateInvalidFormat {
        pos: usize,
        spec: String,
    },
    BadAssignTarget {
        pos: usize,
    },
    NotAContainer {
        pos: usize,
        name: String,
    },
    EarlyReturn(Value),
}

#[derive(Debug)]
pub(crate) enum ScopeEvent {
    Open(String),
    Close(String),
    Add(String, Value),
    Mutate(String, Value),
    Free(String),
    BleedBack(String, Value),
}

// Runtime environment that stores variable values during program execution
#[derive(Debug, Clone)]
struct VarEntry {
    ty: Option<Type>,
    val: Option<Value>,
}

#[derive(Debug, Clone)]
struct FuncDef {
    params: Vec<ParamKind>,
    ret_ty: Option<Type>,
    body: Vec<Stmt>,
    ret_expr: Option<Expr>,
}

#[derive(Debug, Clone)]
struct ScopeFrame {
    vars: HashMap<String, VarEntry>,
    freed: HashSet<String>,
    bleed_back: HashMap<String, (usize, String)>,
    // inner param name -> (outer scope index, outer var name)
}

struct RunTime {
    scopes: Vec<ScopeFrame>,
    order: Vec<String>,
    seen: HashSet<String>,
    funcs: HashMap<String, FuncDef>,
    debug_scope: bool,
}

impl RunTime {
    fn resolve_assigned_value(&self, value: Value, pos: usize) -> Result<Value, RuntimeError> {
        match value {
            Value::Str(s) => Ok(Value::Str(expand_template(self, &s, pos)?)),
            other => Ok(other),
        }
    }

    fn new() -> Self {
        Self {
            scopes: vec![ScopeFrame {
                vars: HashMap::new(),
                freed: HashSet::new(),
                bleed_back: HashMap::new(),
            }],
            order: Vec::new(),
            seen: HashSet::new(),
            funcs: HashMap::new(),
            debug_scope: false,
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(ScopeFrame {
            vars: HashMap::new(),
            freed: HashSet::new(),
            bleed_back: HashMap::new(),
        });
        if self.debug_scope {
            diagnostics::print_scope_event(&ScopeEvent::Open(format!(
                "scope#{}",
                self.scopes.len() - 1
            )));
        }
    }

    fn pop_scope(&mut self) {
        if let Some(frame) = self.scopes.last() {
            // bleed-back - write final values back to outer scope
            let bleeds: Vec<(String, usize, String)> = frame
                .bleed_back
                .iter()
                .filter(|(param_name, _)| !frame.freed.contains(*param_name))
                .map(|(param_name, (outer_idx, outer_name))| {
                    (param_name.clone(), *outer_idx, outer_name.clone())
                })
                .collect();

            let bleed_values: Vec<(usize, String, Value)> = bleeds
                .iter()
                .filter_map(|(param_name, outer_idx, outer_name)| {
                    frame
                        .vars
                        .get(param_name)
                        .and_then(|entry| entry.val.clone())
                        .map(|val| (*outer_idx, outer_name.clone(), val))
                })
                .collect();

            let bleed_events: Vec<(String, Value)> = bleeds
                .iter()
                .filter_map(|(param_name, _, outer_name)| {
                    frame
                        .vars
                        .get(param_name)
                        .and_then(|entry| entry.val.clone())
                        .map(|val| (outer_name.clone(), val))
                })
                .collect();

            let free_names: Vec<String> = frame
                .vars
                .keys()
                .filter(|name| !frame.freed.contains(*name))
                .cloned()
                .collect();

            // run normal cleanup
            for (name, _val) in &frame.vars {
                if !frame.freed.contains(name) {
                    // cleanup - currently just drop
                }
            }

            let close_label = format!("scope#{}", self.scopes.len() - 1);

            // pop the frame
            let _ = frame;
            self.scopes.pop();

            // write bleed-back values AFTER pop so borrow checker is happy
            for (outer_idx, outer_name, val) in bleed_values {
                if let Some(outer_frame) = self.scopes.get_mut(outer_idx) {
                    if let Some(entry) = outer_frame.vars.get_mut(&outer_name) {
                        entry.val = Some(val);
                    }
                }
            }
            if self.debug_scope {
                for name in &free_names {
                    diagnostics::print_scope_event(&ScopeEvent::Free(name.clone()));
                }
                for (name, val) in &bleed_events {
                    diagnostics::print_scope_event(&ScopeEvent::BleedBack(name.clone(), val.clone()));
                }
                diagnostics::print_scope_event(&ScopeEvent::Close(close_label));
            }
        } else {
            self.scopes.pop();
        }
    }

    fn free_variable(&mut self, name: &str) {
        if let Some(frame) = self.scopes.last_mut() {
            if !frame.freed.contains(name) {
                frame.vars.remove(name);
                frame.freed.insert(name.to_string());
                if self.debug_scope {
                    diagnostics::print_scope_event(&ScopeEvent::Free(name.to_string()));
                }
            }
        }
    }

    fn declare(&mut self, name: String, ty: Option<Type>, pos: usize) -> Result<(), RuntimeError> {
        let frame = self.scopes.last_mut().unwrap();

        if frame.vars.contains_key(&name) {
            return Err(RuntimeError::AlreadyDeclared { pos, name });
        }

        if self.seen.insert(name.clone()) {
            self.order.push(name.clone());
        }

        frame.vars.insert(name, VarEntry { ty, val: None });
        Ok(())
    }

    fn set_var(&mut self, name: String, value: Value, pos: usize) -> Result<(), RuntimeError> {
        let value = self.resolve_assigned_value(value, pos)?;
        for frame in self.scopes.iter_mut().rev() {
            if let Some(entry) = frame.vars.get_mut(&name) {
                let was_initialized = entry.val.is_some();
                if entry.ty.is_none() {
                    entry.ty = Some(type_of_value(&value));
                }

                let expected = entry.ty.unwrap();
                let got = type_of_value(&value);
                if !value_matches_type(&value, expected) {
                    return Err(RuntimeError::TypeMismatch { pos, expected, got });
                }

                entry.val = Some(value);
                if self.debug_scope {
                    let logged = entry.val.clone().unwrap();
                    if was_initialized {
                        diagnostics::print_scope_event(&ScopeEvent::Mutate(name.clone(), logged));
                    } else {
                        diagnostics::print_scope_event(&ScopeEvent::Add(name.clone(), logged));
                    }
                }
                return Ok(());
            }
        }
        let was_seen = self.seen.contains(&name);
        Err(diagnostics::unresolved_var_error(pos, name, was_seen))
    }

    fn set_var_typed(
        &mut self,
        name: String,
        ty: Type,
        value: Value,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        let value = self.resolve_assigned_value(value, pos)?;
        let logged = value.clone();
        let got = type_of_value(&value);
        if !value_matches_type(&value, ty) {
            return Err(RuntimeError::TypeMismatch {
                pos,
                expected: ty,
                got,
            });
        }

        let frame = self.scopes.last_mut().unwrap();
        if frame.vars.contains_key(&name) {
            return Err(RuntimeError::AlreadyDeclared { pos, name });
        }

        if self.seen.insert(name.clone()) {
            self.order.push(name.clone());
        }

        frame.vars.insert(
            name.clone(),
            VarEntry {
                ty: Some(ty),
                val: Some(value),
            },
        );
        if self.debug_scope {
            diagnostics::print_scope_event(&ScopeEvent::Add(name, logged));
        }
        Ok(())
    }

    fn set_container_field(
        &mut self,
        container: &str,
        field: &str,
        value: Value,
        pos: usize,
    ) -> Result<(), RuntimeError> {
        let logged = value.clone();
        for frame in self.scopes.iter_mut().rev() {
            if let Some(entry) = frame.vars.get_mut(container) {
                if let Some(Value::Container(map)) = &mut entry.val {
                    map.insert(field.to_string(), value);
                    if self.debug_scope {
                        diagnostics::print_scope_event(&ScopeEvent::Mutate(
                            format!("{}.{}", container, field),
                            logged,
                        ));
                    }
                    return Ok(());
                } else {
                    return Err(RuntimeError::NotAContainer {
                        pos,
                        name: container.to_string(),
                    });
                }
            }
        }
        Err(RuntimeError::UndefinedVar {
            pos,
            name: container.to_string(),
        })
    }

    fn get_var(&self, name: &str, pos: usize) -> Result<Value, RuntimeError> {
        for frame in self.scopes.iter().rev() {
            if let Some(entry) = frame.vars.get(name) {
                if let Some(value) = &entry.val {
                    return Ok(value.clone());
                }
                return Err(RuntimeError::UninitializedVar {
                    pos,
                    name: name.to_string(),
                });
            }
        }
        let owned = name.to_string();
        let was_seen = self.seen.contains(&owned);
        Err(diagnostics::unresolved_var_error(pos, owned, was_seen))
    }

    fn eval_expr(&mut self, expr: &Expr) -> Result<Value, RuntimeError> {
        match expr {
            Expr::Val(v) => Ok(v.clone()),
            Expr::Ident(name, pos) => self.get_var(name, *pos),
            Expr::DotAccess(container, field) => {
                for frame in self.scopes.iter().rev() {
                    if let Some(entry) = frame.vars.get(container) {
                        if let Some(Value::Container(map)) = &entry.val {
                            return map.get(field).cloned().ok_or_else(|| RuntimeError::UndefinedVar {
                                pos: 0,
                                name: format!("{}.{}", container, field),
                            });
                        } else {
                            return Err(RuntimeError::NotAContainer {
                                pos: 0,
                                name: container.to_string(),
                            });
                        }
                    }
                }
                Err(RuntimeError::UndefinedVar {
                    pos: 0,
                    name: container.to_string(),
                })
            }
            Expr::Call(name, args, pos) => {
                let func = self
                    .funcs
                    .get(name)
                    .cloned()
                    .ok_or_else(|| RuntimeError::UndefinedVar {
                        pos: *pos,
                        name: name.clone(),
                    })?;

                let outer_scope_idx = self.scopes.len() - 1;
                let mut resolved_args: Vec<(String, Value, Option<String>)> = Vec::new();
                // (inner param name, value, bleed_back outer name if .copy)

                for (param, arg) in func.params.iter().zip(args.iter()) {
                    match (param, arg) {
                        (ParamKind::Typed(pname, _pty), CallArg::Expr(expr)) => {
                            let val = self.eval_expr(expr)?;
                            resolved_args.push((pname.clone(), val, None));
                        }
                        (ParamKind::Copy(pname), CallArg::Copy(outer_name)) => {
                            let val = self.get_var(outer_name, *pos)?;
                            resolved_args.push((pname.clone(), val, Some(outer_name.clone())));
                        }
                        (ParamKind::CopyFree(pname), CallArg::CopyFree(outer_name)) => {
                            let val = self.get_var(outer_name, *pos)?;
                            resolved_args.push((pname.clone(), val, None));
                            // no bleed-back - copy.free is isolated
                        }
                        _ => return Err(RuntimeError::BadAssignTarget { pos: *pos }),
                    }
                }

                self.push_scope();

                let call_result = (|| -> Result<Value, RuntimeError> {
                    for (pname, val, bleed_outer) in resolved_args {
                        let ty = type_of_value(&val);
                        self.set_var_typed(pname.clone(), ty, val, *pos)?;
                        if let Some(outer_name) = bleed_outer {
                            if let Some(frame) = self.scopes.last_mut() {
                                frame.bleed_back.insert(pname, (outer_scope_idx, outer_name));
                            }
                        }
                    }

                    for stmt in &func.body {
                        match run_stmt(self, stmt.clone()) {
                            Ok(_) => {}
                            Err(RuntimeError::EarlyReturn(val)) => return Ok(val),
                            Err(e) => return Err(e),
                        }
                    }

                    if let Some(expr) = &func.ret_expr {
                        self.eval_expr(expr)
                    } else {
                        Ok(Value::Num(0))
                    }
                })();

                self.pop_scope();
                call_result
            }
            Expr::Bin(lhs, op, pos, rhs) => {
                let left = self.eval_expr(lhs)?;
                let right = self.eval_expr(rhs)?;
                self.apply_op(left, *op, *pos, right)
            }
        }
    }

    fn apply_op(
        &self,
        left: Value,
        op: Op,
        pos: usize,
        right: Value,
    ) -> Result<Value, RuntimeError> {
        match op {
            Op::Plus => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a.saturating_add(*b))),
                _ => {
                    if let (Some(a), Some(b)) = (as_f64(&left), as_f64(&right)) {
                        Ok(Value::Float(a + b))
                    } else {
                        Err(RuntimeError::BadOperands {
                            pos,
                            op,
                            left,
                            right,
                        })
                    }
                }
            },
            Op::Minus => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a.saturating_sub(*b))),
                _ => {
                    if let (Some(a), Some(b)) = (as_f64(&left), as_f64(&right)) {
                        Ok(Value::Float(a - b))
                    } else {
                        Err(RuntimeError::BadOperands {
                            pos,
                            op,
                            left,
                            right,
                        })
                    }
                }
            },
            Op::Mul => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => {
                    Ok(Value::Num(a.checked_mul(*b).unwrap_or(u128::MAX)))
                }
                _ => {
                    if let (Some(a), Some(b)) = (as_f64(&left), as_f64(&right)) {
                        Ok(Value::Float(a * b))
                    } else {
                        Err(RuntimeError::BadOperands {
                            pos,
                            op,
                            left,
                            right,
                        })
                    }
                }
            },
            Op::Div => match (&left, &right) {
                (Value::Num(_), Value::Num(0)) => Err(RuntimeError::DivByZero { pos }),
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a / b)),
                _ => {
                    if let (Some(a), Some(b)) = (as_f64(&left), as_f64(&right)) {
                        if b == 0.0 {
                            Err(RuntimeError::DivByZero { pos })
                        } else {
                            Ok(Value::Float(a / b))
                        }
                    } else {
                        Err(RuntimeError::BadOperands {
                            pos,
                            op,
                            left,
                            right,
                        })
                    }
                }
            },
            Op::Mod => match (&left, &right) {
                (Value::Num(_), Value::Num(0)) => Err(RuntimeError::DivByZero { pos }),
                (Value::Num(a), Value::Num(b)) => Ok(Value::Num(a % b)),
                _ => {
                    if let (Some(a), Some(b)) = (as_f64(&left), as_f64(&right)) {
                        if b == 0.0 {
                            Err(RuntimeError::DivByZero { pos })
                        } else {
                            Ok(Value::Float(a % b))
                        }
                    } else {
                        Err(RuntimeError::BadOperands {
                            pos,
                            op,
                            left,
                            right,
                        })
                    }
                }
            },
            Op::EqEq => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Bool(a == b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a == b)),
                (Value::Num(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) == *b)),
                (Value::Float(a), Value::Num(b)) => Ok(Value::Bool(*a == (*b as f64))),
                (Value::Str(a), Value::Str(b)) => Ok(Value::Bool(a == b)),
                (Value::Char(a), Value::Char(b)) => Ok(Value::Bool(a == b)),
                (Value::Bool(a), Value::Bool(b)) => Ok(Value::Bool(a == b)),
                (l, r) => Err(RuntimeError::BadOperands {
                    pos,
                    op,
                    left: l.clone(),
                    right: r.clone(),
                }),
            },
        }
    }
}

