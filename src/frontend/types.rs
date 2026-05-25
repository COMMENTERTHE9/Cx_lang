use crate::frontend::ast::{Op, Type};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum Value {
    Num(i128),
    Float(f64),
    Str(u32, u32),
    Bool(bool),
    TBool(u8),
    Char(char),
    EnumVariant(String, String),
    Unknown(crate::frontend::ast::Type),
    Handle(crate::runtime::handle::Handle),
    Container(HashMap<String, Value>),
    Array(Vec<Value>),
    Struct(String, HashMap<String, Value>),
    ResultOk(Box<Value>),
    ResultErr(Box<Value>),
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Num(a), Value::Num(b)) => a == b,
            (Value::Float(a), Value::Float(b)) => a == b,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::TBool(a), Value::TBool(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Str(a1, a2), Value::Str(b1, b2)) => a1 == b1 && a2 == b2,
            (Value::EnumVariant(a1, a2), Value::EnumVariant(b1, b2)) => a1 == b1 && a2 == b2,
            (Value::Unknown(a), Value::Unknown(b)) => a == b,
            (Value::Array(a), Value::Array(b)) => a == b,
            (Value::Handle(a), Value::Handle(b)) => a == b,
            (Value::Container(a), Value::Container(b)) => a == b,
            (Value::Struct(n1, a), Value::Struct(n2, b)) => n1 == n2 && a == b,
            (Value::ResultOk(a), Value::ResultOk(b)) => a == b,
            (Value::ResultErr(a), Value::ResultErr(b)) => a == b,
            _ => false,
        }
    }
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
    StaleHandle {
        pos: usize,
    },
    // `index` is i64 because Cx permits negative indices and the diagnostic
    // should echo the actual value the user wrote; `length` is usize because an
    // array length is non-negative; `pos` preserves the source location of the
    // offending index expression (two of the three #002 target sites carry a
    // real position today). Constructed by tracker #002 in a later commit (the
    // three UndefinedVar mis-routings at runtime.rs:391/:739/:875); unused until
    // then.
    #[allow(dead_code)] // constructed by tracker #002 in the next commit
    IndexOutOfBounds {
        pos: usize,
        index: i64,
        length: usize,
    },
    BreakSignal,
    ContinueSignal,
    #[allow(dead_code)] // frontend-level variant; IR layer currently enforces via IrValidationError::LoopVariableReassignment
    ReadOnlyLoopVar {
        pos: usize,
        name: String,
    },
    EarlyReturn(Value),
    /// Control-flow signal raised by the `exit(code)` builtin. Not an error
    /// condition — it carries the process exit code requested by the program.
    /// Propagates uncaught to the top-level interpreter loop (or the --test
    /// loop), which translates it to `std::process::exit(code)`. Sibling of
    /// EarlyReturn/BreakSignal/ContinueSignal: RuntimeError is the runtime's
    /// control-flow-signal carrier, not solely an error type.
    Exit(i32),
    AssertionFailed {
        msg: String,
        pos: usize,
    },
}

#[derive(Debug, Clone)]
pub struct VarEntry {
    pub ty: Option<Type>,
    pub val: Option<Value>,
}
