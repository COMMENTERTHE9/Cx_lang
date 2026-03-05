use crate::frontend::types::*;

#[derive(Debug, Clone, Copy)]
pub enum Op {
    Plus,
    Minus,
    Mul,
    Div,
    Mod,
    EqEq,
}

#[derive(Debug, Clone)]
pub enum CallArg {
    Expr(Expr),
    Copy(String),
    CopyFree(String),
    CopyInto(Vec<String>),
}

#[derive(Debug, Clone)]
pub enum ParamKind {
    Typed(String, Type),
    Copy(String),
    CopyFree(String),
    CopyInto(String, Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Type {
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
pub enum Expr {
    Val(Value),
    Ident(String, usize),
    DotAccess(String, String),
    Call(String, Vec<CallArg>, usize),
    Bin(Box<Expr>, Op, usize, Box<Expr>),
}

// AST statements produced by the parser
#[derive(Debug, Clone)]
pub enum Stmt {
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

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}
