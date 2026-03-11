#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Plus,
    Minus,
    Mul,
    Div,
    Mod,
    EqEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    And,
    Or,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    T8,
    T16,
    T32,
    T64,
    T128,
    Bool,
    Str,
    StrRef,
    Container,
    Char,
    Enum(String),
    Unknown,
    Handle(Box<Type>),
}

// AST-level value - owned, no arena lifetime
// Used by parser and AST nodes only
#[derive(Debug, Clone)]
pub enum AstValue {
    Num(u128),
    Float(f64),
    Str(String),
    Bool(bool),
    Char(char),
    EnumVariant(String, String),
    Unknown,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Val(AstValue),
    Ident(String, usize),
    DotAccess(String, String),
    HandleNew(Box<Expr>, usize),
    HandleVal(String, usize),
    HandleDrop(String, usize),
    Call(String, Vec<CallArg>, usize),
    Range(Box<Expr>, Box<Expr>, bool),
    Unary(Op, Box<Expr>, usize),
    Bin(Box<Expr>, Op, usize, Box<Expr>),
}

#[derive(Debug, Clone)]
pub enum WhenPattern {
    Literal(AstValue),
    Range(AstValue, AstValue, bool),
    EnumVariant(String, String),
    Group(String, String),
    Catchall,
    Placeholder,
}

#[derive(Debug, Clone)]
pub enum SuperGroupHandler {
    Stmts(Vec<Stmt>),
    Placeholder,
}

#[derive(Debug, Clone)]
pub enum WhenBody {
    Stmts(Vec<Stmt>),
    SuperGroup(Vec<SuperGroupHandler>),
}

#[derive(Debug, Clone)]
pub struct WhenArm {
    pub pattern: WhenPattern,
    pub body: WhenBody,
    pub pos: usize,
}

// AST statements produced by the parser
#[derive(Debug, Clone)]
pub enum Stmt {
    EnumDef {
        name: String,
        variants: Vec<String>,
        groups: Vec<(String, Vec<String>)>,
        super_groups: Vec<(String, Vec<(String, Vec<String>)>)>,
        pos: usize,
    },
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
    CompoundAssign {
        name: String,
        op: Op,
        operand: Expr,
        pos: usize,
    },
    Print {
        expr: Expr,
        pos: usize,
    },
    PrintInline {
        expr: Expr,
        _pos: usize,
    },
    ExprStmt {
        expr: Expr,
        _pos: usize,
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
        _pos: usize,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        pos: usize,
    },
    For {
        var: String,
        start: Expr,
        end: Expr,
        inclusive: bool,
        body: Vec<Stmt>,
        pos: usize,
    },
    Loop {
        body: Vec<Stmt>,
        pos: usize,
    },
    Break {
        pos: usize,
    },
    Continue {
        pos: usize,
    },
    When {
        expr: Expr,
        arms: Vec<WhenArm>,
        pos: usize,
    },
}

#[derive(Debug, Clone)]
pub struct Program {
    pub stmts: Vec<Stmt>,
}
