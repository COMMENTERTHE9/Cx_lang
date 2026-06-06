// incremental rebuild test 3
use crate::frontend::{ast::*, types::*};
use crate::frontend::semantic_types::*;
use crate::runtime::handle::HandleRegistry;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug)]
#[allow(dead_code)] // HandleAlloc/Drop/Access: handle-lifecycle trace events reserved for future --trace-handles wiring
pub enum ScopeEvent {
    Open(String),
    Close(String),
    Add(String, Value),
    Mutate(String, Value),
    Free(String),
    BleedBack(String, Value),
    HandleAlloc { slot: u32, gen: u32 },
    HandleDrop { slot: u32, gen: u32 },
    HandleAccess { slot: u32, gen: u32, stale: bool },
}

/// Identity hasher for `BindingId` (tracker #009). `BindingId` is a dense u32
/// produced by the semantic phase; hashing it with SipHash (the default) was
/// ~26% of bare-loop runtime (Pillar 1 callgrind). This hasher returns the id
/// directly — no mixing — which is sound because the keys are already unique
/// dense integers. `write` is a non-panicking fallback; for `BindingId` keys
/// only `write_u32` is ever called.
#[derive(Default)]
pub struct IdHasher(u64);

impl std::hash::Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = self.0.rotate_left(8) ^ b as u64;
        }
    }
    fn write_u32(&mut self, n: u32) {
        self.0 = n as u64;
    }
}

pub type BuildIdHasher = std::hash::BuildHasherDefault<IdHasher>;

/// Per-frame variable storage keyed by `BindingId` (identity-hashed).
pub type VarMap = HashMap<BindingId, VarEntry, BuildIdHasher>;

pub struct ScopeFrame {
    /// Variable storage keyed by `BindingId` — the hot lookup path.
    pub vars: VarMap,
    /// name -> BindingId index, used ONLY by name-based cold paths (string
    /// interpolation, container/array name resolution, `.copy` bleed-back).
    /// The hot path (VarRef / Assign / CompoundAssign / For var / params) goes
    /// straight through `vars` by BindingId and never touches this.
    pub by_name: HashMap<String, BindingId>,
    pub freed: HashSet<String>,
    pub bleed_back: HashMap<String, (usize, String)>,
    pub seen: HashSet<String>,
    // inner param name -> (outer scope index, outer var name)
}

impl ScopeFrame {
    pub(crate) fn new() -> Self {
        ScopeFrame {
            vars: VarMap::default(),
            by_name: HashMap::new(),
            freed: HashSet::new(),
            bleed_back: HashMap::new(),
            seen: HashSet::new(),
        }
    }

    /// Resolve a variable entry in this frame by name (cold path).
    pub(crate) fn get_by_name(&self, name: &str) -> Option<&VarEntry> {
        self.by_name.get(name).and_then(|b| self.vars.get(b))
    }

    /// Mutable resolve by name (cold path).
    pub(crate) fn get_by_name_mut(&mut self, name: &str) -> Option<&mut VarEntry> {
        let binding = *self.by_name.get(name)?;
        self.vars.get_mut(&binding)
    }

    /// Whether a name is declared in this frame.
    pub(crate) fn contains_name(&self, name: &str) -> bool {
        self.by_name.contains_key(name)
    }

    /// Insert a fresh variable keyed by its BindingId, with its name indexed.
    pub(crate) fn insert_var(&mut self, binding: BindingId, name: &str, entry: VarEntry) {
        self.vars.insert(binding, entry);
        self.by_name.insert(name.to_string(), binding);
    }
}

pub struct RunTime {
    pub string_arena: Vec<u8>,
    pub handles: HandleRegistry<Value>,
    pub structs: HashMap<String, Vec<(String, Type)>>,
    pub semantic_impls: HashMap<(String, String), (Vec<(BindingId, String, SemanticType)>, Arc<SemanticFunction>)>,
    pub(crate) scopes: Vec<ScopeFrame>,
    pub semantic_funcs: HashMap<String, Arc<SemanticFunction>>,
    pub debug_scope: bool,
    pub consts: HashMap<String, Value>,
}

impl RunTime {
    pub fn register_semantic_func(&mut self, func: SemanticFunction) {
        self.semantic_funcs.insert(func.name.clone(), Arc::new(func));
    }

    pub fn alloc_str(&mut self, s: &str) -> (u32, u32) {
        let offset = self.string_arena.len() as u32;
        self.string_arena.extend_from_slice(s.as_bytes());
        (offset, s.len() as u32)
    }

    pub fn resolve_str(&self, offset: u32, len: u32) -> &str {
        let bytes = &self.string_arena[offset as usize..(offset + len) as usize];
        std::str::from_utf8(bytes).expect("arena string was not valid utf8")
    }

    pub fn new() -> Self {
        Self {
            string_arena: Vec::new(),
            handles: HandleRegistry::new(),
            structs: HashMap::new(),
            semantic_impls: HashMap::new(),
            scopes: vec![ScopeFrame::new()],
            semantic_funcs: HashMap::new(),
            debug_scope: false,
            consts: HashMap::new(),
        }
    }
}


pub(crate) fn value_to_string(rt: &RunTime, v: Value) -> String {
    match v {
        Value::Num(n) => n.to_string(),
        Value::Float(x) => x.to_string(),
        Value::Str(off, len) => rt.resolve_str(off, len).to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::TBool(b) => match b {
            0 => "false".to_string(),
            1 => "true".to_string(),
            _ => "?".to_string(),
        },
        Value::Char(c) => c.to_string(),
        Value::EnumVariant(e, v) => format!("{}::{}", e, v),
        Value::Unknown(_) => "?".to_string(),
        Value::Array(elems) => {
            let parts: Vec<String> = elems
                .iter()
                .map(|v| value_to_string(rt, v.clone()))
                .collect();
            format!("[{}]", parts.join(", "))
        }
        Value::Handle(h) => format!("handle({},{})", h.slot, h.gen),
        Value::Container(map) => format!("{:?}", map),
        Value::Struct(name, map) => {
            let parts: Vec<String> = map.iter().map(|(k, v)| format!("{}: {}", k, value_to_string(rt, v.clone()))).collect();
            format!("{} {{ {} }}", name, parts.join(", "))
        }
        Value::ResultOk(v) => format!("Ok({})", value_to_string(rt, *v)),
        Value::ResultErr(v) => format!("Err({})", value_to_string(rt, *v)),
    }
}

pub(crate) fn apply_numeric_cast(val: Value, to: &SemanticType) -> Value {
    match val {
        Value::Num(n) => {
            let truncated = match to {
                SemanticType::I8   => (n as i8) as i128,
                SemanticType::I16  => (n as i16) as i128,
                SemanticType::I32  => (n as i32) as i128,
                SemanticType::I64  => (n as i64) as i128,
                SemanticType::I128 => n,
                SemanticType::F64  => return Value::Float(n as f64),
                _ => n,
            };
            Value::Num(truncated)
        }
        Value::Float(f) => {
            match to {
                SemanticType::I8   => Value::Num((f as i8) as i128),
                SemanticType::I16  => Value::Num((f as i16) as i128),
                SemanticType::I32  => Value::Num((f as i32) as i128),
                SemanticType::I64  => Value::Num((f as i64) as i128),
                SemanticType::I128 => Value::Num(f as i128),
                SemanticType::F64  => Value::Float(f),
                _ => Value::Float(f),
            }
        }
        other => other,
    }
}

pub(crate) fn type_of_value(v: &Value) -> Type {
    match v {
        Value::Num(_) => Type::T128,
        Value::Float(_) => Type::F64,
        Value::Str(_, _) => Type::Str,
        Value::Bool(_) => Type::Bool,
        Value::TBool(_) => Type::Bool,
        Value::Char(_) => Type::Char,
        Value::EnumVariant(e, _) => Type::Enum(e.clone()),
        Value::Unknown(_) => Type::Unknown,
        Value::Handle(_) => Type::Handle(Box::new(Type::T128)),
        Value::Container(_) => Type::Container,
        Value::Array(_) => Type::Array(0, Box::new(Type::Unknown)),
        Value::Struct(name, _) => Type::Struct(name.clone()),
        Value::ResultOk(_) => Type::Result(Box::new(Type::Unknown)),
        Value::ResultErr(_) => Type::Result(Box::new(Type::Unknown)),
    }
}

pub(crate) fn value_matches_type(v: &Value, t: &Type) -> bool {
    match (v, t) {
        (Value::Num(_), Type::T8) => true,
        (Value::Num(_), Type::T16) => true,
        (Value::Num(_), Type::T32) => true,
        (Value::Num(_), Type::T64) => true,
        (Value::Num(_), Type::T128) => true,
        (Value::Float(_), Type::F64) => true,
        (Value::Float(_), Type::T8) => true,
        (Value::Float(_), Type::T16) => true,
        (Value::Float(_), Type::T32) => true,
        (Value::Float(_), Type::T64) => true,
        (Value::Float(_), Type::T128) => true,
        (Value::Str(_, _), Type::Str) => true,
        (Value::Str(_, _), Type::StrRef) => true,
        (Value::Container(_), Type::Container) => true,
        (Value::Bool(_), Type::Bool) => true,
        (Value::Char(_), Type::Char) => true,
        (Value::EnumVariant(e, _), Type::Enum(t)) if e == t => true,
        (Value::Handle(_), Type::Handle(_)) => true,
        (Value::TBool(_), Type::Bool) => true,
        (Value::Array(_), Type::Array(_, _)) => true,
        (Value::Struct(name, _), Type::Struct(t)) if name == t => true,
        (Value::Unknown(_), _) => true,
        (Value::ResultOk(_), Type::Result(_)) => true,
        (Value::ResultErr(_), Type::Result(_)) => true,
        _ => false,
    }
}

pub(crate) fn is_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_alphanumeric())
}

pub(crate) fn as_f64(v: &Value) -> Option<f64> {
    match v {
        Value::Num(n) => Some(*n as f64),
        Value::Float(x) => Some(*x),
        _ => None,
    }
}

pub(crate) fn expand_template(rt: &RunTime, s: &str, pos: usize) -> Result<String, RuntimeError> {
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
                out.push_str(&value_to_string(rt, v));
            }
        } else {
            out.push(c);
        }
    }
    Ok(out)
}

impl From<SemanticType> for Type {
    fn from(st: SemanticType) -> Type {
        match st {
            SemanticType::I8 => Type::T8,
            SemanticType::I16 => Type::T16,
            SemanticType::I32 => Type::T32,
            SemanticType::I64 => Type::T64,
            SemanticType::I128 => Type::T128,
            SemanticType::F64 => Type::F64,
            SemanticType::Bool => Type::Bool,
            SemanticType::Str => Type::Str,
            SemanticType::StrRef => Type::StrRef,
            SemanticType::Container => Type::Container,
            SemanticType::Char => Type::Char,
            SemanticType::Enum(name) => Type::Enum(name),
            SemanticType::Unknown => Type::Unknown,
            SemanticType::Handle(inner) => Type::Handle(Box::new((*inner).into())),
            SemanticType::Numeric => Type::T128,
            SemanticType::Struct(name) => Type::Struct(name),
            SemanticType::TypeParam(name) => Type::TypeParam(name),
            SemanticType::Array(size, elem_ty) => Type::Array(size, Box::new((*elem_ty).into())),
            SemanticType::Result(inner) => Type::Result(Box::new((*inner).into())),
            SemanticType::Void => Type::Unknown,
        }
    }
}

impl From<SemanticParamKind> for ParamKind {
    fn from(spk: SemanticParamKind) -> ParamKind {
        match spk {
            SemanticParamKind::Typed => ParamKind::Typed(String::new(), Type::Unknown),
            SemanticParamKind::Copy => ParamKind::Copy(String::new()),
            SemanticParamKind::CopyFree => ParamKind::CopyFree(String::new()),
            SemanticParamKind::CopyInto => ParamKind::CopyInto(String::new(), vec![]),
        }
    }
}
