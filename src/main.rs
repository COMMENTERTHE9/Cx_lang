#![allow(unused)]
//extern crate core;

mod diagnostics;
mod parser;
mod semantic;

use chumsky::input::{Input, Stream};
use chumsky::prelude::SimpleSpan;
use chumsky::Parser;
use logos::Logos;
use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
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
        s[1..s.len()-1].to_string()
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
        "(" => Token::PunctParenOpen,
        ")" => Token::PunctParenClose,
        "+" => Token::OpAdd,
        "-" => Token::OpSub,
        "*" => Token::OpMul,
        "/" => Token::OpDiv,
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

// Simple driver that tokenizes, parses, and executes a .cx file.
fn main() {
    let mut args = env::args().skip(1);
    let first = args.next();
    let debug = matches!(first.as_deref(), Some("--debug"));
    if debug {
        args.next();
    }

    let path = if debug {
        args.next()
            .unwrap_or_else(|| "src/tests/test.cx".to_string())
    } else {
        first.unwrap_or_else(|| "src/tests/test.cx".to_string())
    };

    let input = fs::read_to_string(&path).expect("failed to read .cx file");
    if debug {
        eprintln!("RAW INPUT = {:?}", input);
    }
    let tok_list = match tok_collector(&input) {
        Ok(tok_list) => tok_list,
        Err(err) => {
            diagnostics::print_parse(&input, &err);
            return;
        }
    };
    if debug {
        eprintln!("TOKENS = {:?}", tok_list);
    }

    let program = match parse_program_with_fallback(&tok_list, &input, debug) {
        Ok(program) => program,
        Err(err) => {
            diagnostics::print_parse(&input, &err);
            return;
        }
    };
    if let Err(e) = semantic::analyze_program(&program) {
        diagnostics::print_custom(&input, &e.msg, e.pos);
        return;
    }

    let mut rt = RunTime::new();

    for stmt in program.stmts {
        if let Err(err) = run_stmt(&mut rt, stmt) {
            diagnostics::print_runtime(&input, &err);
            break;
        }
    }
    if debug {
        println!("\nScopes: {:?}", rt.scopes);
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
        Stmt::Assign { name, expr, pos_eq } => {
            let value = rt.eval_expr(&expr)?;
            rt.set_var(name, value, pos_eq)
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
                Value::Str(s) => println!("{}", expand_template(rt, &s, pos)?),
                Value::Bool(b) => println!("{}", b),
                Value::Char(c) => println!("{}", c),
            }
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
    }
}

fn type_of_value(v: &Value) -> Type {
    match v {
        Value::Num(_) => Type::T128,
        Value::Float(_) => Type::T64,
        Value::Str(_) => Type::Str,
        Value::Bool(_) => Type::Bool,
        Value::Char(_) => Type::Char,
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
}

#[derive(Debug, Clone, Copy)]
enum Op {
    Plus,
    Minus,
    Mul,
    Div,
    EqEq,
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
    Call(String, Vec<Expr>, usize),
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
        name: String,
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
        params: Vec<(String, Type)>,
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
    EarlyReturn(Value),
}

// Runtime environment that stores variable values during program execution
#[derive(Debug, Clone)]
struct VarEntry {
    ty: Option<Type>,
    val: Option<Value>,
}

#[derive(Debug, Clone)]
struct FuncDef {
    params: Vec<(String, Type)>,
    ret_ty: Option<Type>,
    body: Vec<Stmt>,
    ret_expr: Option<Expr>,
}

type Scope = HashMap<String, VarEntry>;

struct RunTime {
    scopes: Vec<Scope>,
    order: Vec<String>,
    seen: HashSet<String>,
    funcs: HashMap<String, FuncDef>,
}

impl RunTime {
    fn new() -> Self {
        Self {
            scopes: vec![HashMap::new()],
            order: Vec::new(),
            seen: HashSet::new(),
            funcs: HashMap::new(),
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(HashMap::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare(&mut self, name: String, ty: Option<Type>, pos: usize) -> Result<(), RuntimeError> {
        let scope = self.scopes.last_mut().unwrap();

        if scope.contains_key(&name) {
            return Err(RuntimeError::AlreadyDeclared { pos, name });
        }

        if self.seen.insert(name.clone()) {
            self.order.push(name.clone());
        }

        scope.insert(name, VarEntry { ty, val: None });
        Ok(())
    }

    fn set_var(&mut self, name: String, value: Value, pos: usize) -> Result<(), RuntimeError> {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(entry) = scope.get_mut(&name) {
                if entry.ty.is_none() {
                    entry.ty = Some(type_of_value(&value));
                }

                let expected = entry.ty.unwrap();
                let got = type_of_value(&value);
                if !value_matches_type(&value, expected) {
                    return Err(RuntimeError::TypeMismatch { pos, expected, got });
                }

                entry.val = Some(value);
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
        let got = type_of_value(&value);
        if !value_matches_type(&value, ty) {
            return Err(RuntimeError::TypeMismatch {
                pos,
                expected: ty,
                got,
            });
        }

        let scope = self.scopes.last_mut().unwrap();
        if scope.contains_key(&name) {
            return Err(RuntimeError::AlreadyDeclared { pos, name });
        }

        if self.seen.insert(name.clone()) {
            self.order.push(name.clone());
        }

        scope.insert(
            name,
            VarEntry {
                ty: Some(ty),
                val: Some(value),
            },
        );
        Ok(())
    }

    fn get_var(&self, name: &str, pos: usize) -> Result<Value, RuntimeError> {
        for scope in self.scopes.iter().rev() {
            if let Some(entry) = scope.get(name) {
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
            Expr::Call(name, args, pos) => {
                let func = self
                    .funcs
                    .get(name)
                    .cloned()
                    .ok_or_else(|| RuntimeError::UndefinedVar {
                        pos: *pos,
                        name: name.clone(),
                    })?;

                let arg_vals: Vec<Value> = args
                    .iter()
                    .map(|a| self.eval_expr(a))
                    .collect::<Result<_, _>>()?;

                self.push_scope();

                let call_result = (|| -> Result<Value, RuntimeError> {
                    for ((pname, pty), val) in func.params.iter().zip(arg_vals.into_iter()) {
                        self.set_var_typed(pname.clone(), *pty, val, *pos)?;
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
            Op::EqEq => match (&left, &right) {
                (Value::Num(a), Value::Num(b)) => Ok(Value::Bool(a == b)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(a == b)),
                (Value::Num(a), Value::Float(b)) => Ok(Value::Bool((*a as f64) == *b)),
                (Value::Float(a), Value::Num(b)) => Ok(Value::Bool(*a == (*b as f64))),
                (Value::Str(a), Value::Str(b)) => Ok(Value::Bool(a == b)),
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

