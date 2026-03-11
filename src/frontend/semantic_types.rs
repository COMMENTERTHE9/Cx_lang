use crate::frontend::ast::{Op, Type};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BindingId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FunctionId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EnumVariantId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticType {
    I8,
    I16,
    I32,
    I64,
    I128,
    F64,
    Bool,
    Str,
    StrRef,
    Container,
    Char,
    Enum(String),
    Unknown,
    Handle(Box<SemanticType>),
    Numeric,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SemanticValue {
    Num(u128),
    Float(f64),
    Str(String),
    Bool(bool),
    Char(char),
    EnumVariant {
        enum_name: String,
        variant_name: String,
        enum_id: Option<EnumId>,
        variant_id: Option<EnumVariantId>,
    },
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticExpr {
    pub ty: SemanticType,
    pub kind: SemanticExprKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SemanticExprKind {
    Value(SemanticValue),
    VarRef {
        binding: BindingId,
        name: String,
    },
    DotAccess {
        binding: Option<BindingId>,
        container: String,
        field: String,
    },
    HandleNew {
        value: Box<SemanticExpr>,
        pos: usize,
    },
    HandleVal {
        binding: BindingId,
        name: String,
        pos: usize,
    },
    HandleDrop {
        binding: BindingId,
        name: String,
        pos: usize,
    },
    Call {
        callee: String,
        function: FunctionId,
        args: Vec<SemanticCallArg>,
    },
    Range {
        start: Box<SemanticExpr>,
        end: Box<SemanticExpr>,
        inclusive: bool,
    },
    Unary {
        op: Op,
        expr: Box<SemanticExpr>,
        pos: usize,
    },
    Binary {
        lhs: Box<SemanticExpr>,
        op: Op,
        pos: usize,
        rhs: Box<SemanticExpr>,
    },
    Cast {
        expr: Box<SemanticExpr>,
        from: SemanticType,
        to: SemanticType,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SemanticCallArg {
    Expr(SemanticExpr),
    Copy { binding: BindingId, name: String },
    CopyFree { binding: BindingId, name: String },
    CopyInto(Vec<ResolvedBinding>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedBinding {
    pub binding: BindingId,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SemanticLValue {
    Binding {
        binding: BindingId,
        name: String,
        ty: SemanticType,
    },
    DotAccess {
        binding: Option<BindingId>,
        container: String,
        field: String,
        ty: SemanticType,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SemanticWhenPattern {
    Literal(SemanticValue),
    Range(SemanticValue, SemanticValue, bool),
    EnumVariant {
        enum_name: String,
        variant_name: String,
        enum_id: Option<EnumId>,
        variant_id: Option<EnumVariantId>,
    },
    Catchall,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticWhenArm {
    pub pattern: SemanticWhenPattern,
    pub body: Vec<SemanticStmt>,
    pub pos: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SemanticParamKind {
    Typed,
    Copy,
    CopyFree,
    CopyInto,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticParam {
    pub binding: BindingId,
    pub name: String,
    pub kind: SemanticParamKind,
    pub ty: Option<SemanticType>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticEnumGroup {
    pub name: String,
    pub variants: Vec<EnumVariantId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticEnumVariant {
    pub id: EnumVariantId,
    pub name: String,
    pub enum_id: EnumId,
    pub group: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticEnum {
    pub id: EnumId,
    pub name: String,
    pub declared_ty: Type,
    pub variants: Vec<SemanticEnumVariant>,
    pub groups: Vec<SemanticEnumGroup>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticFunction {
    pub id: FunctionId,
    pub name: String,
    pub params: Vec<SemanticParam>,
    pub return_ty: Option<SemanticType>,
    pub body: Vec<SemanticStmt>,
    pub ret_expr: Option<SemanticExpr>,
    pub pos: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SemanticStmt {
    EnumDef {
        enum_id: EnumId,
        name: String,
        variants: Vec<String>,
        pos: usize,
    },
    Decl {
        binding: BindingId,
        name: String,
        ty: Option<SemanticType>,
        pos: usize,
    },
    Assign {
        target: SemanticLValue,
        expr: SemanticExpr,
        pos_eq: usize,
    },
    TypedAssign {
        binding: BindingId,
        name: String,
        ty: SemanticType,
        expr: SemanticExpr,
        pos_type: usize,
    },
    CompoundAssign {
        binding: Option<BindingId>,
        name: String,
        op: Op,
        operand: SemanticExpr,
        pos: usize,
    },
    Print {
        expr: SemanticExpr,
        pos: usize,
    },
    PrintInline {
        expr: SemanticExpr,
        pos: usize,
    },
    ExprStmt {
        expr: SemanticExpr,
        pos: usize,
    },
    Return {
        expr: Option<SemanticExpr>,
        pos: usize,
    },
    FuncDef(SemanticFunction),
    Block {
        stmts: Vec<SemanticStmt>,
        pos: usize,
    },
    While {
        cond: SemanticExpr,
        body: Vec<SemanticStmt>,
        pos: usize,
    },
    For {
        binding: BindingId,
        var: String,
        start: SemanticExpr,
        end: SemanticExpr,
        inclusive: bool,
        body: Vec<SemanticStmt>,
        pos: usize,
    },
    Loop {
        body: Vec<SemanticStmt>,
        pos: usize,
    },
    Break {
        pos: usize,
    },
    Continue {
        pos: usize,
    },
    When {
        expr: SemanticExpr,
        arms: Vec<SemanticWhenArm>,
        pos: usize,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct SemanticProgram {
    pub stmts: Vec<SemanticStmt>,
    pub enums: Vec<SemanticEnum>,
}
