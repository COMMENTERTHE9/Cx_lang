use std::collections::HashMap;
use crate::frontend::ast::{Op, Type};

#[derive(Debug, Clone)]
pub enum Value {
    Num(u128),
    Float(f64),
    Str(String),
    Bool(bool),
    Char(char),
    Container(HashMap<String, Value>),
}


#[derive(Debug)]
pub enum RuntimeError {
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

#[derive(Debug, Clone)]
pub struct VarEntry {
    pub ty: Option<Type>,
    pub val: Option<Value>,
}

