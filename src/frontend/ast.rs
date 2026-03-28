#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Plus,
    Minus,
    Mul,
    Div,
    Mod,
    EqEq,
    NotEq,
    Lt,
    Gt,
    LtEq,
    GtEq,
    Not,
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
    F64,
    Bool,
    Str,
    StrRef,
    Container,
    Char,
    Enum(String),
    Unknown,
    Handle(Box<Type>),
    Array(usize, Box<Type>),
    TypeParam(String),
    Struct(String),
}

// AST-level value - owned, no arena lifetime
// Used by parser and AST nodes only
#[derive(Debug, Clone)]
pub enum AstValue {
    Num(i128),
    Float(f64),
    Str(String),
    Bool(bool),
    Char(char),
    EnumVariant(String, String),
    StructInstance(String, Vec<Type>, Vec<(String, Expr)>),
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
Unary(Op, Box<Expr>, usize),
    Bin(Box<Expr>, Op, usize, Box<Expr>),
    ArrayLit(Vec<Expr>),
    Index(Box<Expr>, Box<Expr>, usize),
    MethodCall(String, String, Vec<CallArg>, usize),
    When(Box<Expr>, Vec<WhenArm>, usize),
}

#[derive(Debug, Clone)]
pub enum WhenPattern {
    Literal(AstValue),
    Range(AstValue, AstValue, bool),
    EnumVariant(String, String),
    Group(String, String),
    Catchall,
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

#[derive(Debug, Clone)]
pub enum AssignTarget {
    Var(String),
    Field(String, String), // container_name, field_name
}

#[derive(Debug, Clone)]
pub struct WhileInChain {
    pub arr: String,
    pub start_slot: usize,
    pub range_start: Expr,
    pub range_end: Expr,
    pub inclusive: bool,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub enum CxMacro {
    Test,
    Inline,
    Reactive,
    Deprecated(Option<String>),
    Cfg(String),
    Unknown(String),
}

#[derive(Debug, Clone)]
pub struct ImportDecl {
    pub alias: String,
    pub path: String,
    pub pos: usize,
}

// AST statements produced by the parser
#[derive(Debug, Clone)]
pub enum Stmt {
    ImportBlock {
        imports: Vec<ImportDecl>,
        pos: usize,
    },
    StructDef {
        name: String,
        type_params: Vec<String>,
        fields: Vec<(String, Type)>,
        is_pub: bool,
        pos: usize,
    },
    ImplBlock {
        name: String,
        aliases: Vec<(String, Type)>,
        methods: Vec<(String, Vec<ParamKind>, Option<Type>, Vec<Stmt>, Option<Expr>)>,
        is_pub: bool,
        pos: usize,
    },
    ConstDecl {
        name: String,
        ty: Type,
        value: Expr,
        is_pub: bool,
        pos: usize,
    },
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
        target: AssignTarget,
        op: Op,
        operand: Expr,
        pos: usize,
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
        type_params: Vec<String>,
        params: Vec<ParamKind>,
        ret_ty: Option<Type>,
        body: Vec<Stmt>,
        ret_expr: Option<Expr>,
        is_pub: bool,
        macros: Vec<CxMacro>,
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
    IfElse {
        condition: Expr,
        then_body: Vec<Stmt>,
        else_ifs: Vec<(Expr, Vec<Stmt>)>,
        else_body: Option<Vec<Stmt>>,
        pos: usize,
    },
    WhileIn {
        arr: String,
        start_slot: usize,
        range_start: Expr,
        range_end: Expr,
        inclusive: bool,
        body: Vec<Stmt>,
        then_chains: Vec<WhileInChain>,
        result: Option<Expr>,
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
