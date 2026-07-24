#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt;

use crate::frontend::ast::Op;
use crate::frontend::builtins::{self, BuiltinKind, JitStatus};
use crate::frontend::semantic_types::{
    BindingId, SemanticCallArg, SemanticExpr, SemanticExprKind, SemanticLValue,
    SemanticParamKind, SemanticProgram, SemanticStmt, SemanticType, SemanticValue,
    SemanticWhenArm, SemanticWhenPattern,
};
use crate::ir::builder::IrBuilder;
use crate::ir::instr::{BinaryOp, CompareOp, IrInst, IrTerminator};
use crate::ir::types::{
    compute_array_layout, compute_struct_layout, BlockId, BlockParam, IrBlock, IrFunction, IrModule,
    IrParam, IrType, StructLayout, ValueId,
};

macro_rules! unsupported {
    ($name:literal) => {
        return Err(LoweringError::UnsupportedSemanticConstruct {
            construct: $name.to_string(),
        })
    };
}

macro_rules! unsupported_type {
    ($name:literal) => {
        return Err(LoweringError::UnsupportedSemanticType {
            ty: $name.to_string(),
        })
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoweringError {
    UnsupportedSemanticConstruct { construct: String },
    UnsupportedSemanticType { ty: String },
    UnresolvedSemanticArtifact { artifact: String },
    InternalInvariantViolation { detail: String },
}

impl fmt::Display for LoweringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedSemanticConstruct { construct } => {
                write!(
                    f,
                    "unsupported semantic construct during lowering: {construct}"
                )
            }
            Self::UnsupportedSemanticType { ty } => {
                write!(f, "unsupported semantic type during lowering: {ty}")
            }
            Self::UnresolvedSemanticArtifact { artifact } => {
                write!(
                    f,
                    "unresolved semantic artifact reached lowering: {artifact}"
                )
            }
            Self::InternalInvariantViolation { detail } => {
                write!(f, "lowering invariant violation: {detail}")
            }
        }
    }
}

impl std::error::Error for LoweringError {}

// ---------------------------------------------------------------------------
// Runtime intrinsics boundary — Phase 9 audit
// ---------------------------------------------------------------------------
//
// These names are Cx language builtins. They are recognized at the semantic
// layer (semantic.rs `analyze_call`) and assigned `FunctionId(u32::MAX)` to
// flag them as non-user-defined. They do NOT appear in the `signature_table`,
// which only holds user-defined functions.
//
// Classification (see docs/backend/cx_runtime_intrinsics_v0.1.md for the
// full boundary specification):
//
//   Category          | Builtins          | Status
//   ──────────────────┼───────────────────┼──────────────────────────────────
//   I/O (stdout)      | print, println    | LOWERABLE (I64 only) — routes to cx_printn
//   I/O (stdout)      | printn            | LOWERABLE — see lower_printn_stmt
//   I/O (stdin)       | read, input       | blocked on Phase 8 str layout
//   Debug / assertion | assert, assert_eq | LOWERABLE — Phase 9 sub-packet 3
//
// Builtins that remain in `is_cx_builtin` produce a structured
// `UnsupportedSemanticConstruct` error instead of a misleading
// `UnresolvedSemanticArtifact` from a signature_table miss.
//
// Builtins that are handled before this gate (assert, assert_eq, print, println, printn)
// are intercepted in `lower_stmt` and never reach `is_cx_builtin`.
fn is_cx_builtin(name: &str) -> bool {
    // Builtins the JIT does not yet lower but should reject with a structured
    // error (today: read, input). Derived from the registry's JIT status (#008)
    // rather than a hardcoded name list.
    matches!(
        builtins::lookup(name).map(|d| d.jit),
        Some(JitStatus::GatedUnsupported)
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct LoweredValue {
    value: ValueId,
    ty: IrType,
}

type BindingMap = HashMap<BindingId, LoweredValue>;

#[derive(Clone)]
struct FunctionSignature {
    param_types: Vec<IrType>,
    return_ty: Option<IrType>,
    /// Struct name when the function's semantic return type is a struct.
    /// Struct returns use a caller-allocated return slot: the caller allocas
    /// this struct's layout and passes its address as a hidden trailing Ptr
    /// param (already included in `param_types`); the callee copies the
    /// result through the slot and returns the slot pointer — no pointer
    /// into the callee's dead frame ever escapes (the silent-corruption bug
    /// this convention closes: garbage fields, exit 0, zero prior coverage).
    ret_struct: Option<String>,
}

/// The returned struct's name, when a function returns a struct by value.
fn ret_struct_of(return_ty: &Option<SemanticType>) -> Option<String> {
    match return_ty {
        Some(SemanticType::Struct(s)) => Some(s.clone()),
        _ => None,
    }
}

/// Mangled IR name for an impl-block method: `<FirstAliasStruct>$<method>`.
fn mangle_method(struct_name: &str, method: &str) -> String {
    format!("{}${}", struct_name, method)
}

fn build_signature_table(program: &SemanticProgram) -> HashMap<String, FunctionSignature> {
    let mut table = HashMap::new();
    for stmt in &program.stmts {
        if let SemanticStmt::FuncDef(function) = stmt {
            let mut param_types = Vec::new();
            let mut all_params_ok = true;
            for param in &function.params {
                match param.kind {
                    SemanticParamKind::Typed => {
                        let Some(ref ty) = param.ty else {
                            all_params_ok = false;
                            break;
                        };
                        match lower_type(ty) {
                            Ok(ir_ty) => param_types.push(ir_ty),
                            Err(_) => { all_params_ok = false; break; }
                        }
                    }
                    _ => { all_params_ok = false; break; }
                }
            }
            if !all_params_ok { continue; }

            let return_ty = match &function.return_ty {
                Some(ty) => match lower_return_type(ty) {
                    Ok(opt) => opt,
                    Err(_) => continue,
                },
                None => None,
            };

            let ret_struct = ret_struct_of(&function.return_ty);
            if ret_struct.is_some() {
                param_types.push(IrType::Ptr); // hidden trailing return slot
            }
            table.insert(function.name.clone(), FunctionSignature {
                param_types,
                return_ty,
                ret_struct,
            });
        }
        // Impl-block methods are registered alongside free functions so calls
        // (after Stage 2 desugaring) resolve through the same signature_table
        // lookup. Mangled key: `<FirstAliasStruct>$<method>`. The receiver and
        // any extra aliases are prepended to the user params, mirroring the
        // IrFunction shape emitted in lower_program_inner.
        if let SemanticStmt::ImplBlock { aliases, methods, method_alias_params, .. } = stmt {
            if aliases.is_empty() { continue; }
            let tn = match &aliases[0].1 {
                SemanticType::Struct(name) => name.clone(),
                _ => continue,
            };
            for (i, method) in methods.iter().enumerate() {
                let mut param_types = Vec::new();
                let mut all_params_ok = true;
                // Alias params first (each is a struct → Ptr).
                if let Some(alias_params) = method_alias_params.get(i) {
                    for param in alias_params {
                        match param.kind {
                            SemanticParamKind::Typed => {
                                let Some(ref ty) = param.ty else {
                                    all_params_ok = false;
                                    break;
                                };
                                match lower_type(ty) {
                                    Ok(ir_ty) => param_types.push(ir_ty),
                                    Err(_) => { all_params_ok = false; break; }
                                }
                            }
                            _ => { all_params_ok = false; break; }
                        }
                    }
                } else {
                    all_params_ok = false;
                }
                if !all_params_ok { continue; }
                // Then the user-declared params.
                for param in &method.params {
                    match param.kind {
                        SemanticParamKind::Typed => {
                            let Some(ref ty) = param.ty else {
                                all_params_ok = false;
                                break;
                            };
                            match lower_type(ty) {
                                Ok(ir_ty) => param_types.push(ir_ty),
                                Err(_) => { all_params_ok = false; break; }
                            }
                        }
                        _ => { all_params_ok = false; break; }
                    }
                }
                if !all_params_ok { continue; }

                let return_ty = match &method.return_ty {
                    Some(ty) => match lower_return_type(ty) {
                        Ok(opt) => opt,
                        Err(_) => continue,
                    },
                    None => None,
                };

                let ret_struct = ret_struct_of(&method.return_ty);
                if ret_struct.is_some() {
                    param_types.push(IrType::Ptr); // hidden trailing return slot
                }
                table.insert(
                    mangle_method(&tn, &method.name),
                    FunctionSignature { param_types, return_ty, ret_struct },
                );
            }
        }
        // 0.3.4 slice 5: phen methods register under the same mangled scheme
        // as impl methods — `<ReceiverStruct>$<method>` — which the ownership
        // locked rule makes collision-free by construction (one owner per
        // method name per type; `$` cannot appear in user identifiers). Each
        // phen is already per-concrete-type, so this IS the monomorphized
        // specialization: one function per gene/type combination, called
        // directly by name (decision 1 — no dispatch table).
        //
        // Struct returns are sound as of the caller-allocated-slot fix (the
        // slice-5 guard is lifted). ARRAY returns remain unregistered: the
        // same dangling-frame shape applies and the slot convention has only
        // been built for struct layouts — array-return support is a tracked
        // follow-up, and an unregistered method keeps its call sites on the
        // clean SKIP path rather than corrupting.
        if let SemanticStmt::PhenDef { receiver_type, methods, method_receiver_params, .. } = stmt {
            let tn = match receiver_type {
                SemanticType::Struct(name) => name.clone(),
                _ => continue,
            };
            for (i, method) in methods.iter().enumerate() {
                if matches!(method.return_ty, Some(SemanticType::Array(_, _))) {
                    continue;
                }
                let mut param_types = Vec::new();
                let mut all_params_ok = true;
                if let Some(recv_params) = method_receiver_params.get(i) {
                    for param in recv_params {
                        match param.kind {
                            SemanticParamKind::Typed => {
                                let Some(ref ty) = param.ty else {
                                    all_params_ok = false;
                                    break;
                                };
                                match lower_type(ty) {
                                    Ok(ir_ty) => param_types.push(ir_ty),
                                    Err(_) => { all_params_ok = false; break; }
                                }
                            }
                            _ => { all_params_ok = false; break; }
                        }
                    }
                } else {
                    all_params_ok = false;
                }
                if !all_params_ok { continue; }
                for param in &method.params {
                    match param.kind {
                        SemanticParamKind::Typed => {
                            let Some(ref ty) = param.ty else {
                                all_params_ok = false;
                                break;
                            };
                            match lower_type(ty) {
                                Ok(ir_ty) => param_types.push(ir_ty),
                                Err(_) => { all_params_ok = false; break; }
                            }
                        }
                        _ => { all_params_ok = false; break; }
                    }
                }
                if !all_params_ok { continue; }

                let return_ty = match &method.return_ty {
                    Some(ty) => match lower_return_type(ty) {
                        Ok(opt) => opt,
                        Err(_) => continue,
                    },
                    None => None,
                };

                let ret_struct = ret_struct_of(&method.return_ty);
                if ret_struct.is_some() {
                    param_types.push(IrType::Ptr); // hidden trailing return slot
                }
                table.insert(
                    mangle_method(&tn, &method.name),
                    FunctionSignature { param_types, return_ty, ret_struct },
                );
            }
        }
    }
    table
}

#[derive(Clone)]
struct StructLayoutInfo {
    fields: Vec<(String, IrType)>,
    layout: StructLayout,
}

fn build_struct_table(program: &SemanticProgram) -> HashMap<String, StructLayoutInfo> {
    let mut table = HashMap::new();
    for stmt in &program.stmts {
        if let SemanticStmt::StructDef { name, fields, .. } = stmt {
            let mut ir_fields = Vec::new();
            let mut field_types = Vec::new();
            let mut all_ok = true;
            for (fname, fty) in fields {
                match lower_type(fty) {
                    Ok(ir_ty) => {
                        ir_fields.push((fname.clone(), ir_ty.clone()));
                        field_types.push(ir_ty);
                    }
                    Err(_) => {
                        all_ok = false;
                        break;
                    }
                }
            }
            if !all_ok {
                continue;
            }
            let layout = compute_struct_layout(&field_types);
            table.insert(name.clone(), StructLayoutInfo {
                fields: ir_fields,
                layout,
            });
        }
    }
    table
}

/// Compilation target configuration threaded through the IR lowering pass.
///
/// Controls target-dependent decisions such as the default integer width chosen
/// for unresolved numeric literals (`SemanticType::Numeric`) and the value
/// range accepted without error during literal lowering.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TargetConfig {
    /// Width of the target's native pointer in bits (32 or 64).
    pub pointer_bits: u32,
}

impl TargetConfig {
    /// Construct a `TargetConfig` matching the current compilation host.
    ///
    /// On a 64-bit host this yields `pointer_bits: 64`; on a 32-bit host,
    /// `pointer_bits: 32`.  Cx 0.1 targets only x86-64, so in practice this
    /// always returns the 64-bit configuration.
    pub const fn host() -> Self {
        Self { pointer_bits: usize::BITS }
    }

    /// The [`IrType`] used to represent an unresolved numeric literal on this
    /// target.  `I64` on 64-bit targets, `I32` on 32-bit targets.
    pub fn numeric_literal_ir_type(&self) -> IrType {
        match self.pointer_bits {
            32 => IrType::I32,
            _ => IrType::I64,
        }
    }

    /// Inclusive minimum value that fits in the target's default numeric
    /// literal type (see [`TargetConfig::numeric_literal_ir_type`]).
    pub fn numeric_literal_min(&self) -> i128 {
        match self.pointer_bits {
            32 => i32::MIN as i128,
            _ => i64::MIN as i128,
        }
    }

    /// Inclusive maximum value that fits in the target's default numeric
    /// literal type (see [`TargetConfig::numeric_literal_ir_type`]).
    pub fn numeric_literal_max(&self) -> i128 {
        match self.pointer_bits {
            32 => i32::MAX as i128,
            _ => i64::MAX as i128,
        }
    }
}

struct LoweringCtx {
    builder: IrBuilder,
    finished_blocks: Vec<IrBlock>,
    signature_table: HashMap<String, FunctionSignature>,
    struct_table: HashMap<String, StructLayoutInfo>,
    trace: bool,
    target: TargetConfig,
    /// D2.3d flat name→binding map for print-time string interpolation: every
    /// `Decl`/`TypedAssign`/`For`/`ConstDecl` name in the current function (+ its
    /// params), last-wins. See `lower_interpolated_print` for the FILED
    /// scope-limitation note.
    binding_names: HashMap<String, BindingId>,
}

struct ActiveBlock {
    block: crate::ir::builder::IrBlockBuilder,
    bindings: BindingMap,
    terminated: bool,
    trace: bool,
}

#[derive(Clone, Debug)]
struct LoopContext {
    /// labeled-breaks (b): this loop's own label, or None. `break 'L`/`continue 'L`
    /// walk the loop stack for the context whose label matches; unlabeled jumps
    /// take the innermost (top of stack).
    label: Option<String>,
    header_id: BlockId,
    exit_id: BlockId,
    ordered_bindings: Vec<BindingId>,
    exit_ordered_bindings: Vec<BindingId>,
}

/// labeled-breaks (b): resolve which enclosing loop a break/continue targets in
/// the loop-context stack (innermost last). Unlabeled jumps take the innermost
/// loop (top of stack); a labeled jump walks outward to the nearest enclosing loop
/// whose label matches. The semantic layer guarantees a labeled jump always has a
/// matching enclosing loop, so `None` for a labeled jump can't arise from a valid
/// program; `None` for an unlabeled jump means "break/continue outside any loop".
fn find_target_loop<'a>(stack: &'a [LoopContext], label: &Option<String>) -> Option<&'a LoopContext> {
    match label {
        None => stack.last(),
        Some(target) => stack
            .iter()
            .rev()
            .find(|c| c.label.as_deref() == Some(target.as_str())),
    }
}

struct FunctionLoweringSpec {
    name: String,
    return_ty: Option<IrType>,
    allow_return_stmt: bool,
    /// Caller-allocated return-slot param (value id + struct name) when this
    /// function returns a struct. Every return site copies the result struct
    /// through the slot and returns the slot pointer instead of the local's.
    ret_slot: Option<(ValueId, String)>,
}

impl LoweringCtx {
    fn new(
        signature_table: HashMap<String, FunctionSignature>,
        struct_table: HashMap<String, StructLayoutInfo>,
        trace: bool,
        target: TargetConfig,
    ) -> Self {
        Self {
            builder: IrBuilder::new(),
            finished_blocks: Vec::new(),
            signature_table,
            struct_table,
            trace,
            target,
            binding_names: HashMap::new(),
        }
    }

    fn fresh_value(&mut self) -> ValueId {
        self.builder.fresh_value()
    }

    fn start_block(&mut self, params: Vec<BlockParam>, bindings: BindingMap) -> ActiveBlock {
        ActiveBlock {
            block: self.builder.block(params),
            bindings,
            terminated: false,
            trace: self.trace,
        }
    }

    fn seal_block(&mut self, active: ActiveBlock) -> Result<(), LoweringError> {
        if !active.terminated {
            return Err(LoweringError::InternalInvariantViolation {
                detail: "attempted to finish a block without a terminator".to_string(),
            });
        }

        let block = active
            .block
            .finish()
            .map_err(|err| LoweringError::InternalInvariantViolation {
                detail: format!("failed to finalize block: {err:?}"),
            })?;
        self.finished_blocks.push(block);
        Ok(())
    }
}

impl ActiveBlock {
    fn id(&self) -> crate::ir::types::BlockId {
        self.block.id()
    }

    fn emit(&mut self, inst: IrInst) -> Result<(), LoweringError> {
        if self.terminated {
            return Err(LoweringError::InternalInvariantViolation {
                detail: "attempted to append instruction after terminator".to_string(),
            });
        }
        if self.trace {
            eprintln!("  [trace] {}", crate::ir::printer::print_inst(&inst));
        }
        self.block
            .append_inst(inst);
        Ok(())
    }

    fn terminate(&mut self, term: IrTerminator) -> Result<(), LoweringError> {
        if self.terminated {
            return Err(LoweringError::InternalInvariantViolation {
                detail: "attempted to set a second terminator".to_string(),
            });
        }
        self.block.set_terminator(term).map_err(|err| {
            LoweringError::InternalInvariantViolation {
                detail: format!("failed to set terminator: {err:?}"),
            }
        })?;
        self.terminated = true;
        Ok(())
    }
}

pub fn lower_program_traced(program: &SemanticProgram) -> Result<IrModule, LoweringError> {
    lower_program_inner(program, true)
}

pub fn lower_program(program: &SemanticProgram) -> Result<IrModule, LoweringError> {
    lower_program_inner(program, false)
}

fn lower_program_inner(program: &SemanticProgram, trace: bool) -> Result<IrModule, LoweringError> {
    let reserved_runtime_intrinsics = crate::ir::validate::runtime_intrinsic_names();

    if program.stmts.is_empty() {
        return Ok(IrModule {
            debug_name: "cxir_v0".into(),
            functions: vec![],
        });
    }

    let mut module = IrModule {
        debug_name: "cxir_v0".into(),
        functions: vec![],
    };
    let mut top_level_stmts = Vec::new();
    let mut has_real_main = false;
    let signature_table = build_signature_table(program);
    let struct_table = build_struct_table(program);
    // Single place where the compilation target is chosen; threaded into every
    // lowering context so all target-dependent decisions use the same config.
    let target = TargetConfig::host();

    for stmt in &program.stmts {
        match stmt {
            SemanticStmt::FuncDef(function) => {
                if reserved_runtime_intrinsics.contains(function.name.as_str()) {
                    return Err(LoweringError::UnsupportedSemanticConstruct {
                        construct: format!(
                            "function name '{}' is reserved for runtime intrinsics",
                            function.name
                        ),
                    });
                }
                if function.name == "main" {
                    has_real_main = true;
                }
                module.functions.push(lower_semantic_function(function, &signature_table, &struct_table, trace, target)?);
            }
            // Struct definitions are pre-processed into the struct_table before
            // code lowering begins (see build_struct_table).  They carry no
            // executable semantics and produce no IR, so skip them here rather
            // than routing them into the synthetic-main statement sequence.
            SemanticStmt::StructDef { .. } => {}
            // Enum definitions carry no runtime data — the tag is the variant_id
            // in each enum value / pattern node (D2.2). No IR, like StructDef.
            SemanticStmt::EnumDef { .. } => {}
            // Impl-block methods are emitted as IrFunctions under mangled names.
            // The captured `method_alias_params` carry the BindingIds the body's
            // VarRef/DotAccess nodes reference, so re-prepending them lets
            // lower_semantic_function's bindings map resolve receiver access.
            // The ImplBlock statement itself produces no IR — handled below in
            // lower_stmt; this arm just keeps it out of the synthetic-main
            // statement sequence.
            SemanticStmt::ImplBlock { aliases, methods, method_alias_params, .. } => {
                if aliases.is_empty() { continue; }
                let tn = match &aliases[0].1 {
                    SemanticType::Struct(name) => name.clone(),
                    _ => continue,
                };
                for (i, method) in methods.iter().enumerate() {
                    let mangled = mangle_method(&tn, &method.name);
                    if reserved_runtime_intrinsics.contains(mangled.as_str()) {
                        return Err(LoweringError::UnsupportedSemanticConstruct {
                            construct: format!(
                                "function name '{}' is reserved for runtime intrinsics",
                                mangled
                            ),
                        });
                    }
                    let alias_params = match method_alias_params.get(i) {
                        Some(ap) => ap.clone(),
                        None => continue,
                    };
                    let mut full_params = alias_params;
                    full_params.extend(method.params.iter().cloned());
                    let reconstructed = crate::frontend::semantic_types::SemanticFunction {
                        id: method.id,
                        name: mangled,
                        type_params: method.type_params.clone(),
                        params: full_params,
                        return_ty: method.return_ty.clone(),
                        body: method.body.clone(),
                        ret_expr: method.ret_expr.clone(),
                        is_test: method.is_test,
                        pos: method.pos,
                    };
                    module.functions.push(lower_semantic_function(&reconstructed, &signature_table, &struct_table, trace, target)?);
                }
            }
            // 0.3.4 slice 5: phen methods emit exactly like impl methods —
            // one specialized IrFunction per gene/type combination under the
            // mangled `<ReceiverStruct>$<method>` name, receiver re-prepended
            // from the captured method_receiver_params. Struct/array-returning
            // methods are skipped, matching the signature-table guard (their
            // call sites SKIP cleanly; see build_signature_table). Struct
            // returns are sound via the caller-allocated slot; only ARRAY
            // returns remain guarded (see the signature-table comment).
            SemanticStmt::PhenDef { receiver_type, methods, method_receiver_params, .. } => {
                let tn = match receiver_type {
                    SemanticType::Struct(name) => name.clone(),
                    _ => continue,
                };
                for (i, method) in methods.iter().enumerate() {
                    if matches!(method.return_ty, Some(SemanticType::Array(_, _))) {
                        continue;
                    }
                    let mangled = mangle_method(&tn, &method.name);
                    if reserved_runtime_intrinsics.contains(mangled.as_str()) {
                        return Err(LoweringError::UnsupportedSemanticConstruct {
                            construct: format!(
                                "function name '{}' is reserved for runtime intrinsics",
                                mangled
                            ),
                        });
                    }
                    let recv_params = match method_receiver_params.get(i) {
                        Some(rp) => rp.clone(),
                        None => continue,
                    };
                    let mut full_params = recv_params;
                    full_params.extend(method.params.iter().cloned());
                    let reconstructed = crate::frontend::semantic_types::SemanticFunction {
                        id: method.id,
                        name: mangled,
                        type_params: method.type_params.clone(),
                        params: full_params,
                        return_ty: method.return_ty.clone(),
                        body: method.body.clone(),
                        ret_expr: method.ret_expr.clone(),
                        is_test: method.is_test,
                        pos: method.pos,
                    };
                    module.functions.push(lower_semantic_function(&reconstructed, &signature_table, &struct_table, trace, target)?);
                }
            }
            other => top_level_stmts.push(other),
        }
    }

    if !top_level_stmts.is_empty() {
        if has_real_main {
            return Err(LoweringError::UnsupportedSemanticConstruct {
                construct: "real function 'main' collides with synthetic main".to_string(),
            });
        }
        module
            .functions
            .push(lower_top_level_main(&top_level_stmts, &signature_table, &struct_table, trace, target)?);
    }

    Ok(module)
}

fn lower_top_level_main(stmts: &[&SemanticStmt], signature_table: &HashMap<String, FunctionSignature>, struct_table: &HashMap<String, StructLayoutInfo>, trace: bool, target: TargetConfig) -> Result<IrFunction, LoweringError> {
    let spec = FunctionLoweringSpec {
        name: "main".to_string(),
        return_ty: None,
        allow_return_stmt: false,
        ret_slot: None,
    };
    let mut ctx = LoweringCtx::new(signature_table.clone(), struct_table.clone(), trace, target);
    // D2.3d: flat name→binding map for print-time string interpolation.
    for &stmt in stmts {
        collect_binding_names_stmt(stmt, &mut ctx.binding_names);
    }
    let entry = ctx.start_block(vec![], HashMap::new());
    let current = lower_stmt_sequence(
        stmts.iter().copied(),
        &mut ctx,
        Some(entry),
        &spec,
        &[],
    )?;
    if let Some(active) = current {
        finalize_active_block(&mut ctx, active, IrTerminator::Return { value: None })?;
    }

    Ok(IrFunction {
        name: spec.name,
        params: vec![],
        return_ty: None,
        blocks: ctx.finished_blocks,
    })
}

fn lower_semantic_function(
    function: &crate::frontend::semantic_types::SemanticFunction,
    signature_table: &HashMap<String, FunctionSignature>,
    struct_table: &HashMap<String, StructLayoutInfo>,
    trace: bool,
    target: TargetConfig,
) -> Result<IrFunction, LoweringError> {
    let mut ir_params = Vec::with_capacity(function.params.len());
    let mut block_params = Vec::with_capacity(function.params.len());
    let mut bindings = HashMap::new();
    let return_ty = match &function.return_ty {
        Some(ty) => lower_return_type(ty)?,
        None => None,
    };

    let mut ctx = LoweringCtx::new(signature_table.clone(), struct_table.clone(), trace, target);
    for param in &function.params {
        match (&param.kind, &param.ty) {
            (crate::frontend::semantic_types::SemanticParamKind::Typed, Some(ty)) => {
                let ty = lower_type(ty)?;
                ir_params.push(IrParam {
                    name: param.name.clone(),
                    ty: ty.clone(),
                });
                let value = ctx.fresh_value();
                block_params.push(BlockParam {
                    value,
                    ty: ty.clone(),
                    read_only: false,
                });
                bindings.insert(param.binding, LoweredValue { value, ty });
            }
            (crate::frontend::semantic_types::SemanticParamKind::Typed, None) => {
                return Err(LoweringError::InternalInvariantViolation {
                    detail: format!(
                        "typed parameter '{}' missing semantic type in function '{}'",
                        param.name, function.name
                    ),
                });
            }
            _ => {
                return Err(LoweringError::UnsupportedSemanticConstruct {
                    construct: format!(
                        "unsupported function parameter kind in '{}'",
                        function.name
                    ),
                });
            }
        }
    }

    // Caller-allocated return slot: a struct-returning function receives the
    // caller's slot address as a hidden trailing Ptr param; return sites copy
    // through it (see FunctionSignature::ret_struct for the convention).
    let ret_slot = if let Some(struct_name) = ret_struct_of(&function.return_ty) {
        ir_params.push(IrParam {
            name: "$ret_slot".to_string(),
            ty: IrType::Ptr,
        });
        let slot_value = ctx.fresh_value();
        block_params.push(BlockParam {
            value: slot_value,
            ty: IrType::Ptr,
            read_only: false,
        });
        Some((slot_value, struct_name))
    } else {
        None
    };

    // D2.3d: flat name→binding map (function body + params) for interpolation.
    for stmt in &function.body {
        collect_binding_names_stmt(stmt, &mut ctx.binding_names);
    }
    for param in &function.params {
        ctx.binding_names.insert(param.name.clone(), param.binding);
    }
    let spec = FunctionLoweringSpec {
        name: function.name.clone(),
        return_ty: return_ty.clone(),
        allow_return_stmt: true,
        ret_slot,
    };
    let entry = ctx.start_block(block_params, bindings);
    let current = lower_stmt_sequence(
        function.body.iter(),
        &mut ctx,
        Some(entry),
        &spec,
        &[],
    )?;

    if current.is_none() {
        if function.ret_expr.is_some() {
            return Err(LoweringError::InternalInvariantViolation {
                detail: format!(
                    "function '{}' has both explicit return terminator and trailing return expression",
                    function.name
                ),
            });
        }
    } else if let Some(mut active) = current {
        if let Some(ret_expr) = &function.ret_expr {
            let lowered = lower_expr(ret_expr, &mut ctx, &mut active)?;
            let expected =
                spec.return_ty
                    .clone()
                    .ok_or_else(|| LoweringError::InternalInvariantViolation {
                        detail: format!(
                            "void function '{}' has a trailing return expression",
                            function.name
                        ),
                    })?;
            ensure_type_match("function trailing return", expected, lowered.ty)?;
            let ret_value = emit_return_through_slot(&spec, lowered.value, &mut ctx, &mut active)?;
            finalize_active_block(
                &mut ctx,
                active,
                IrTerminator::Return {
                    value: Some(ret_value),
                },
            )?;
        } else if spec.return_ty.is_some() {
            return Err(LoweringError::InternalInvariantViolation {
                detail: format!(
                    "function '{}' requires a return value but lowering saw no return",
                    function.name
                ),
            });
        } else {
            finalize_active_block(&mut ctx, active, IrTerminator::Return { value: None })?;
        }
    }

    Ok(IrFunction {
        name: function.name.clone(),
        params: ir_params,
        return_ty,
        blocks: ctx.finished_blocks,
    })
}

fn lower_stmt_sequence<'a, I>(
    stmts: I,
    ctx: &mut LoweringCtx,
    mut current: Option<ActiveBlock>,
    spec: &FunctionLoweringSpec,
    loop_ctx: &[LoopContext],
) -> Result<Option<ActiveBlock>, LoweringError>
where
    I: IntoIterator<Item = &'a SemanticStmt>,
{
    for stmt in stmts {
        let active = current.take().ok_or_else(|| LoweringError::InternalInvariantViolation {
            detail: format!(
                "statement appeared after terminator in function '{}'",
                spec.name
            ),
        })?;
        current = lower_stmt(stmt, ctx, active, spec, loop_ctx)?;
    }
    Ok(current)
}

fn lower_stmt(
    stmt: &SemanticStmt,
    ctx: &mut LoweringCtx,
    mut current: ActiveBlock,
    spec: &FunctionLoweringSpec,
    loop_ctx: &[LoopContext],
) -> Result<Option<ActiveBlock>, LoweringError> {
    match stmt {
        SemanticStmt::Noop => Ok(Some(current)),
        // 0.3.4 slice 1: gene/phen declarations are inert at lowering too —
        // nothing to emit. Coherence registration already happened in Pass 0.
        SemanticStmt::GeneDef { .. } => Ok(Some(current)),
        SemanticStmt::PhenDef { .. } => Ok(Some(current)),
        SemanticStmt::Decl { ty, .. } => {
            if let Some(ty) = ty {
                let _ = lower_type(ty)?;
            }
            Ok(Some(current))
        }
        SemanticStmt::Assign { target, expr, .. } => {
            let lowered = lower_expr(expr, ctx, &mut current)?;
            match target {
                SemanticLValue::Binding { binding, ty, .. } => {
                    // Numeric/Unknown: binding type is a placeholder; use the
                    // lowered expression's concrete type as authoritative.
                    let target_ty = if matches!(ty, SemanticType::Numeric | SemanticType::Unknown) {
                        lowered.ty.clone()
                    } else {
                        lower_type(ty)?
                    };
                    ensure_type_match("assign", target_ty.clone(), lowered.ty)?;
                    let dst = ctx.fresh_value();
                    current.emit(IrInst::SsaBind {
                        dst,
                        ty: target_ty.clone(),
                        src: lowered.value,
                    })?;
                    current
                        .bindings
                        .insert(*binding, LoweredValue { value: dst, ty: target_ty });
                    Ok(Some(current))
                }
                SemanticLValue::DotAccess { binding, container, field, ty, struct_name } => {
                    let lowered = lower_expr(expr, ctx, &mut current)?;
                    let (field_ptr, field_ir_ty) = resolve_field_ptr(
                        binding, container, field, struct_name, ty, ctx, &mut current,
                    )?;
                    ensure_type_match("struct field assign", field_ir_ty, lowered.ty)?;
                    current.emit(IrInst::Store {
                        ptr: field_ptr,
                        value: lowered.value,
                    })?;
                    Ok(Some(current))
                }
                SemanticLValue::Index { target, index, .. } => {
                    let (elem_ptr, elem_ir_ty) =
                        resolve_array_element_ptr(&*target, &*index, ctx, &mut current)?;
                    ensure_type_match("array element assign", elem_ir_ty, lowered.ty)?;
                    current.emit(IrInst::Store {
                        ptr: elem_ptr,
                        value: lowered.value,
                    })?;
                    Ok(Some(current))
                }
            }
        }
        SemanticStmt::TypedAssign {
            binding, ty, expr, ..
        } => {
            // D2.x-A1: a bool-typed `?` constructs the three-state-bool unknown
            // wire value (TBool 2, #044), mirroring the interpreter's
            // assignment-site coercion `(Type::Bool, Value::Unknown) => TBool(2)`
            // (runtime/exec.rs). The discriminator is the *declared* target type
            // — the `?` value itself is always SemanticType::Unknown — so the
            // construct lives here, not at lower_value, which keeps rejecting
            // numeric / untyped / expression-position `?` (R5: Path B stays SKIP).
            // TBool is materialised as ConstInt(I8, 2) + Cast (the IR validator
            // restricts ConstInt to non-TBool integer types), the same wire
            // encoding the `unknown` when-arm match uses.
            if matches!(&expr.kind, SemanticExprKind::Value(SemanticValue::Unknown))
                && lower_type(ty)? == IrType::Bool
            {
                let byte = ctx.fresh_value();
                current.emit(IrInst::ConstInt { dst: byte, ty: IrType::I8, value: 2 })?;
                let unknown = ctx.fresh_value();
                current.emit(IrInst::Cast {
                    dst: unknown,
                    from: IrType::I8,
                    to: IrType::TBool,
                    value: byte,
                })?;
                let dst = ctx.fresh_value();
                current.emit(IrInst::SsaBind { dst, ty: IrType::TBool, src: unknown })?;
                current
                    .bindings
                    .insert(*binding, LoweredValue { value: dst, ty: IrType::TBool });
                return Ok(Some(current));
            }
            let lowered = lower_expr(expr, ctx, &mut current)?;
            let target_ty = lower_type(ty)?;
            // D2.x-A2: a `bool` target may receive a three-state TBool value (a
            // Kleene `&&`/`||`/`!` result, e.g. `z: bool = x && y`). Bind it as
            // TBool — the same Bool/TBool relationship A1 gave VarRef and the
            // when-scrutinee. Definite-Bool assignments are unaffected.
            let bind_ty = if target_ty == IrType::Bool && lowered.ty == IrType::TBool {
                IrType::TBool
            } else {
                ensure_type_match("typed assignment", target_ty.clone(), lowered.ty)?;
                target_ty
            };
            let dst = ctx.fresh_value();
            current.emit(IrInst::SsaBind {
                dst,
                ty: bind_ty.clone(),
                src: lowered.value,
            })?;
            current
                .bindings
                .insert(*binding, LoweredValue { value: dst, ty: bind_ty });
            Ok(Some(current))
        }
        SemanticStmt::ExprStmt { expr, .. } => {
            // Phase 9 sub-packets 2 and 3: assert/assert_eq, print, println, and printn are now
            // fully lowerable.  Intercept them before the is_cx_builtin gate so
            // they are routed to the dedicated lowering functions rather than
            // returning a structured error.
            if let SemanticExprKind::Call { callee, args, .. } = &expr.kind {
                // Builtin lowering dispatch keyed on the registry kind (#008).
                match builtins::lookup(callee).map(|d| d.kind) {
                    Some(BuiltinKind::Assert) => return lower_assert_stmt(args, ctx, current),
                    Some(BuiltinKind::AssertEq) => return lower_assert_eq_stmt(args, ctx, current),
                    Some(BuiltinKind::Print) | Some(BuiltinKind::Println) => {
                        return lower_print_stmt(callee.as_str(), args, ctx, current)
                    }
                    Some(BuiltinKind::Printn) => return lower_printn_stmt(args, ctx, current),
                    _ => {}
                }
            }
            // Remaining unimplemented builtin intrinsics (read, input) are not in the
            // signature_table.  Intercept them here so they produce a structured
            // UnsupportedSemanticConstruct rather than falling through to a misleading
            // UnresolvedSemanticArtifact.
            if let SemanticExprKind::Call { callee, .. } = &expr.kind {
                if is_cx_builtin(callee) {
                    return Err(LoweringError::UnsupportedSemanticConstruct {
                        construct: format!(
                            "builtin '{}' is not yet lowerable to IR — codegen pending (Phase 9)",
                            callee
                        ),
                    });
                }
            }
            // Void function calls cannot go through lower_expr because that function
            // must return a LoweredValue, and void calls produce no value.
            // Detect and lower void calls here before falling through to lower_expr.
            if let SemanticExprKind::Call { callee, function: _, args } = &expr.kind {
                let sig_info = ctx.signature_table.get(callee.as_str())
                    .map(|s| (s.return_ty.clone(), s.param_types.clone()));
                if let Some((None, param_types)) = sig_info {
                    let callee = callee.clone();
                    lower_void_call(&callee, args, &param_types, ctx, &mut current)?;
                    return Ok(Some(current));
                }
            }
            // Same void-call detection for MethodCall: build the synthetic
            // receiver-as-arg list (Stage 1 IR param order is
            // [receiver, extra_aliases..., user_params...]) and route through
            // lower_void_call so the receiver lowers through the existing
            // VarRef→Ptr path. Mirrors the Call arm above; same fall-through
            // to lower_expr for non-void.
            if let SemanticExprKind::MethodCall { instance, method, args, instance_binding, struct_name, .. } = &expr.kind {
                let recv_arg = SemanticCallArg::Expr(SemanticExpr {
                    ty: SemanticType::Struct(struct_name.clone()),
                    kind: SemanticExprKind::VarRef {
                        binding: *instance_binding,
                        name: instance.clone(),
                    },
                });
                let mut full_args = Vec::with_capacity(args.len() + 1);
                full_args.push(recv_arg);
                full_args.extend(args.iter().cloned());
                let callee = mangle_method(struct_name, method);
                let sig_info = ctx.signature_table.get(callee.as_str())
                    .map(|s| (s.return_ty.clone(), s.param_types.clone()));
                if let Some((None, param_types)) = sig_info {
                    lower_void_call(&callee, &full_args, &param_types, ctx, &mut current)?;
                    return Ok(Some(current));
                }
            }
            let _ = lower_expr(expr, ctx, &mut current)?;
            Ok(Some(current))
        }
        SemanticStmt::Return { expr, .. } => {
            if !spec.allow_return_stmt {
                return Err(LoweringError::UnsupportedSemanticConstruct {
                    construct: "top-level Return".to_string(),
                });
            }
            match (&spec.return_ty, expr) {
                (Some(expected), Some(expr)) => {
                    let lowered = lower_expr(expr, ctx, &mut current)?;
                    ensure_type_match("function return", expected.clone(), lowered.ty)?;
                    let ret_value = emit_return_through_slot(spec, lowered.value, ctx, &mut current)?;
                    current.terminate(IrTerminator::Return {
                        value: Some(ret_value),
                    })?;
                    ctx.seal_block(current)?;
                    Ok(None)
                }
                (None, None) => {
                    current.terminate(IrTerminator::Return { value: None })?;
                    ctx.seal_block(current)?;
                    Ok(None)
                }
                (Some(_), None) => Err(LoweringError::InternalInvariantViolation {
                    detail: format!(
                        "non-void function '{}' lowered a return without value",
                        spec.name
                    ),
                }),
                (None, Some(_)) => Err(LoweringError::InternalInvariantViolation {
                    detail: format!("void function '{}' lowered a return with value", spec.name),
                }),
            }
        }
        SemanticStmt::CompoundAssign { target, op, operand, .. } => {
    match target {
        SemanticLValue::Binding { binding, ty, .. } => {
            let current_val = current.bindings.get(binding).ok_or_else(|| {
                LoweringError::UnresolvedSemanticArtifact {
                    artifact: format!("compound assign binding {:?}", binding),
                }
            })?.clone();
            // Numeric/Unknown: binding type is a placeholder; use the stored
            // concrete type from the SSA bindings map as authoritative.
            let target_ty = if matches!(ty, SemanticType::Numeric | SemanticType::Unknown) {
                current_val.ty.clone()
            } else {
                lower_type(ty)?
            };
            let rhs = lower_expr(operand, ctx, &mut current)?;
            let bin_op = match op {
                Op::Plus => BinaryOp::Add,
                Op::Minus => BinaryOp::Sub,
                Op::Mul => BinaryOp::Mul,
                Op::Div => BinaryOp::Div,
                Op::Mod => BinaryOp::Rem,
                _ => {
                    return Err(LoweringError::UnsupportedSemanticConstruct {
                        construct: format!("compound assign operator {:?}", op),
                    });
                }
            };
            let result_dst = ctx.fresh_value();
            current.emit(IrInst::Binary {
                dst: result_dst,
                op: bin_op,
                ty: target_ty.clone(),
                lhs: current_val.value,
                rhs: rhs.value,
            })?;
            let bind_dst = ctx.fresh_value();
            current.emit(IrInst::SsaBind {
                dst: bind_dst,
                ty: target_ty.clone(),
                src: result_dst,
            })?;
            current.bindings.insert(*binding, LoweredValue { value: bind_dst, ty: target_ty });
            Ok(Some(current))
        }
        SemanticLValue::DotAccess { binding, container, field, ty, struct_name } => {
            let (field_ptr, field_ir_ty) = resolve_field_ptr(
                binding, container, field, struct_name, ty, ctx, &mut current,
            )?;
            // Read the current field value.
            let current_dst = ctx.fresh_value();
            current.emit(IrInst::Load {
                dst: current_dst,
                ty: field_ir_ty.clone(),
                ptr: field_ptr,
            })?;
            // Lower the operand.
            let rhs = lower_expr(operand, ctx, &mut current)?;
            // Compute the binary result.
            let bin_op = match op {
                Op::Plus => BinaryOp::Add,
                Op::Minus => BinaryOp::Sub,
                Op::Mul => BinaryOp::Mul,
                Op::Div => BinaryOp::Div,
                Op::Mod => BinaryOp::Rem,
                _ => {
                    return Err(LoweringError::UnsupportedSemanticConstruct {
                        construct: format!("compound assign operator {:?}", op),
                    });
                }
            };
            let result_dst = ctx.fresh_value();
            current.emit(IrInst::Binary {
                dst: result_dst,
                op: bin_op,
                ty: field_ir_ty.clone(),
                lhs: current_dst,
                rhs: rhs.value,
            })?;
            // Write back to the field.
            current.emit(IrInst::Store {
                ptr: field_ptr,
                value: result_dst,
            })?;
            Ok(Some(current))
        }
        SemanticLValue::Index { target, index, .. } => {
            let (elem_ptr, elem_ir_ty) =
                resolve_array_element_ptr(&*target, &*index, ctx, &mut current)?;
            // Read the current element value.
            let current_dst = ctx.fresh_value();
            current.emit(IrInst::Load {
                dst: current_dst,
                ty: elem_ir_ty.clone(),
                ptr: elem_ptr,
            })?;
            // Lower the operand.
            let rhs = lower_expr(operand, ctx, &mut current)?;
            // Compute the binary result.
            let bin_op = match op {
                Op::Plus => BinaryOp::Add,
                Op::Minus => BinaryOp::Sub,
                Op::Mul => BinaryOp::Mul,
                Op::Div => BinaryOp::Div,
                Op::Mod => BinaryOp::Rem,
                _ => {
                    return Err(LoweringError::UnsupportedSemanticConstruct {
                        construct: format!("compound assign operator {:?}", op),
                    });
                }
            };
            let result_dst = ctx.fresh_value();
            current.emit(IrInst::Binary {
                dst: result_dst,
                op: bin_op,
                ty: elem_ir_ty.clone(),
                lhs: current_dst,
                rhs: rhs.value,
            })?;
            // Write back to the element.
            current.emit(IrInst::Store {
                ptr: elem_ptr,
                value: result_dst,
            })?;
            Ok(Some(current))
        }
    }
},
        SemanticStmt::FuncDef(_) => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "nested FuncDef".to_string(),
        }),
        // Enum definitions carry no runtime data (the tag is the variant_id in
        // each value/pattern node, D2.2) — no IR, like StructDef below.
        SemanticStmt::EnumDef { .. } => Ok(Some(current)),
SemanticStmt::Block { .. } => { unsupported!("Block") },
        SemanticStmt::WhileIn { .. } => { unsupported!("WhileIn") },
        SemanticStmt::While { label, cond, body, .. } => {
    return match lower_while(label, cond, body, ctx, current, spec, loop_ctx)? {
        Some(new_active) => Ok(Some(new_active)),
        None => Ok(None),
    };
},
        SemanticStmt::For { label, binding, start, end, inclusive, body, .. } => {
    return match lower_for(label, *binding, start, end, *inclusive, body, ctx, current, spec, loop_ctx)? {
        Some(new_active) => Ok(Some(new_active)),
        None => Ok(None),
    };
},
        SemanticStmt::Loop { label, body, .. } => {
    return match lower_loop(label, body, ctx, current, spec, loop_ctx)? {
        Some(new_active) => Ok(Some(new_active)),
        None => Ok(None),
    };
},
        SemanticStmt::Break { label, .. } => {
    // labeled-breaks (b): a labeled break walks the loop stack to the named loop's
    // exit; an unlabeled break takes the innermost (top of stack). Both Jump to
    // that context's exit block, gathering ITS exit bindings from the live SSA
    // environment (the target loop's bindings are a subset of what's live here).
    let ctx_ref = find_target_loop(loop_ctx, label).ok_or_else(|| LoweringError::UnsupportedSemanticConstruct {
        construct: "break outside of loop".to_string(),
    })?;
    let mut exit_args = Vec::new();
    for binding in &ctx_ref.exit_ordered_bindings {
        let val = current.bindings.get(binding).ok_or_else(|| {
            LoweringError::InternalInvariantViolation {
                detail: format!("break: binding {} missing from SSA environment", binding.0),
            }
        })?;
        exit_args.push(val.value);
    }
    current.terminate(IrTerminator::Jump {
        target: ctx_ref.exit_id,
        args: exit_args,
    })?;
    ctx.seal_block(current)?;
    return Ok(None);
},
        SemanticStmt::Continue { label, .. } => {
    // labeled-breaks (b): a labeled continue walks the loop stack to the named
    // loop's header; an unlabeled continue takes the innermost.
    let ctx_ref = find_target_loop(loop_ctx, label).ok_or_else(|| LoweringError::UnsupportedSemanticConstruct {
        construct: "continue outside of loop".to_string(),
    })?;
    let mut header_args = Vec::new();
    for binding in &ctx_ref.ordered_bindings {
        let val = current.bindings.get(binding).ok_or_else(|| {
            LoweringError::InternalInvariantViolation {
                detail: format!("continue: binding {} missing from SSA environment", binding.0),
            }
        })?;
        header_args.push(val.value);
    }
    current.terminate(IrTerminator::Jump {
        target: ctx_ref.header_id,
        args: header_args,
    })?;
    ctx.seal_block(current)?;
    return Ok(None);
},
        SemanticStmt::When { expr, arms, .. } => {
            lower_when_stmt(expr, arms, ctx, current, spec, loop_ctx)
        },
        SemanticStmt::IfElse {
            condition,
            then_body,
            else_ifs,
            else_body,
            ..
        } => lower_if_else(
            condition,
            then_body,
            else_ifs,
            else_body.as_deref(),
            ctx,
            current,
            spec,
            loop_ctx,
        ),
        // Struct definitions are pre-processed into the struct_table before
        // lowering begins; there is no IR to emit for the definition itself.
        SemanticStmt::StructDef { .. } => Ok(Some(current)),
        // Methods are emitted as IrFunctions in lower_program_inner; the
        // ImplBlock statement itself carries no executable IR. This arm is
        // defense-in-depth in case an ImplBlock reaches statement lowering by
        // any path (it currently does not — lower_program_inner filters it out
        // of the synthetic-main statement sequence).
        SemanticStmt::ImplBlock { .. } => Ok(Some(current)),
        SemanticStmt::ConstDecl { .. } => { unsupported!("ConstDecl") },
    }
}

fn lower_if_else(
    condition: &SemanticExpr,
    then_body: &[SemanticStmt],
    else_ifs: &[(SemanticExpr, Vec<SemanticStmt>)],
    else_body: Option<&[SemanticStmt]>,
    ctx: &mut LoweringCtx,
    current: ActiveBlock,
    spec: &FunctionLoweringSpec,
    loop_ctx: &[LoopContext],
) -> Result<Option<ActiveBlock>, LoweringError> {
    let incoming = current.bindings.clone();
    let fallthroughs = lower_if_chain(
        condition,
        then_body,
        else_ifs,
        else_body,
        ctx,
        current,
        &incoming,
        spec,
        loop_ctx,
    )?;

    match fallthroughs.len() {
        0 => Ok(None),
        1 => Ok(Some(fallthroughs.into_iter().next().unwrap())),
        _ => Ok(Some(merge_fallthroughs(ctx, fallthroughs, &incoming)?)),
    }
}

fn lower_if_chain(
    condition: &SemanticExpr,
    then_body: &[SemanticStmt],
    else_ifs: &[(SemanticExpr, Vec<SemanticStmt>)],
    else_body: Option<&[SemanticStmt]>,
    ctx: &mut LoweringCtx,
    mut decision_block: ActiveBlock,
    incoming: &BindingMap,
    spec: &FunctionLoweringSpec,
    loop_ctx: &[LoopContext],
) -> Result<Vec<ActiveBlock>, LoweringError> {
    let cond = lower_expr(condition, ctx, &mut decision_block)?;
    // D2.x-A1: an unknown (TBool) condition traps (cx_trap) — the interpreter's
    // UnknownCondition (#026). No-op for a definite Bool; narrows a definite
    // TBool to Bool so the ensure check below and the Branch see a Bool.
    let cond = guard_unknown_condition(cond, ctx, &mut decision_block)?;
    ensure_type_match("if condition", IrType::Bool, cond.ty.clone())?;

    let then_active = ctx.start_block(vec![], incoming.clone());
    let then_block_id = then_active.id();

    if let Some((next_arm, remaining_else_ifs)) = else_ifs.split_first() {
        let else_active = ctx.start_block(vec![], incoming.clone());
        let else_block_id = else_active.id();

        decision_block.terminate(IrTerminator::Branch {
            cond: cond.value,
            then_block: then_block_id,
            then_args: vec![],
            else_block: else_block_id,
            else_args: vec![],
        })?;
        ctx.seal_block(decision_block)?;

        let mut fallthroughs = Vec::new();
        if let Some(active) = lower_stmt_sequence(then_body.iter(), ctx, Some(then_active), spec, loop_ctx)? {
            fallthroughs.push(active);
        }
        fallthroughs.extend(lower_if_chain(
            &next_arm.0,
            &next_arm.1,
            remaining_else_ifs,
            else_body,
            ctx,
            else_active,
            incoming,
            spec,
            loop_ctx,
        )?);
        Ok(fallthroughs)
    } else {
        let else_active = ctx.start_block(vec![], incoming.clone());
        let else_block_id = else_active.id();

        decision_block.terminate(IrTerminator::Branch {
            cond: cond.value,
            then_block: then_block_id,
            then_args: vec![],
            else_block: else_block_id,
            else_args: vec![],
        })?;
        ctx.seal_block(decision_block)?;

        let mut fallthroughs = Vec::new();
        if let Some(active) = lower_stmt_sequence(then_body.iter(), ctx, Some(then_active), spec, loop_ctx)? {
            fallthroughs.push(active);
        }
        let else_result = if let Some(else_body) = else_body {
            lower_stmt_sequence(else_body.iter(), ctx, Some(else_active), spec, loop_ctx)?
        } else {
            Some(else_active)
        };
        if let Some(active) = else_result {
            fallthroughs.push(active);
        }
        Ok(fallthroughs)
    }
}

fn merge_fallthroughs(
    ctx: &mut LoweringCtx,
    fallthroughs: Vec<ActiveBlock>,
    incoming: &BindingMap,
) -> Result<ActiveBlock, LoweringError> {
    let mut ordered_bindings: Vec<_> = incoming.keys().copied().collect();
    ordered_bindings.sort_by_key(|binding| binding.0);

    let mut merge_param_bindings = Vec::new();
    let mut merge_block_params = Vec::new();
    let mut merged_bindings = HashMap::new();

    for binding in ordered_bindings {
        let incoming_value = incoming
            .get(&binding)
            .cloned()
            .ok_or_else(|| LoweringError::InternalInvariantViolation {
                detail: format!("incoming binding {} missing during merge", binding.0),
            })?;

        let path_values = fallthroughs
            .iter()
            .map(|active| {
                active
                    .bindings
                    .get(&binding)
                    .cloned()
                    .ok_or_else(|| LoweringError::InternalInvariantViolation {
                        detail: format!(
                            "binding {} missing from branch-local SSA environment at merge",
                            binding.0
                        ),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        let first = path_values[0].clone();
        if path_values.iter().any(|value| value.ty != first.ty) {
            return Err(LoweringError::InternalInvariantViolation {
                detail: format!(
                    "binding {} merged with mismatched SSA value types",
                    binding.0
                ),
            });
        }

        if path_values.iter().all(|value| *value == first) {
            merged_bindings.insert(binding, first);
        } else {
            let param_value = ctx.fresh_value();
            let block_param = BlockParam {
                value: param_value,
                ty: incoming_value.ty.clone(),
                read_only: false,
            };
            merge_param_bindings.push(binding);
            merge_block_params.push(block_param.clone());
            merged_bindings.insert(
                binding,
                LoweredValue {
                    value: param_value,
                    ty: block_param.ty,
                },
            );
        }
    }

    let merge_block = ctx.start_block(merge_block_params, merged_bindings);
    let merge_id = merge_block.id();

    for mut active in fallthroughs {
        let args = merge_param_bindings
            .iter()
            .map(|binding| {
                active
                    .bindings
                    .get(binding)
                    .map(|value| value.value)
                    .ok_or_else(|| LoweringError::InternalInvariantViolation {
                        detail: format!(
                            "binding {} missing from branch-local SSA environment when building merge args",
                            binding.0
                        ),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        active.terminate(IrTerminator::Jump {
            target: merge_id,
            args,
        })?;
        ctx.seal_block(active)?;
    }

    Ok(merge_block)
}

fn lower_while(
    label: &Option<String>,
    cond: &SemanticExpr,
    body: &[SemanticStmt],
    ctx: &mut LoweringCtx,
    current: ActiveBlock,
    spec: &FunctionLoweringSpec,
    outer_loop_ctx: &[LoopContext],
) -> Result<Option<ActiveBlock>, LoweringError> {
    let incoming = current.bindings.clone();

    let mut ordered_bindings: Vec<_> = incoming.keys().copied().collect();
    ordered_bindings.sort_by_key(|b| b.0);

    let mut header_params = Vec::new();
    let mut header_bindings = HashMap::new();
    let mut entry_args = Vec::new();

    for binding in &ordered_bindings {
        let val = incoming.get(binding).unwrap();
        let param_value = ctx.fresh_value();
        header_params.push(BlockParam {
            value: param_value,
            ty: val.ty.clone(),
            read_only: false,
        });
        header_bindings.insert(
            *binding,
            LoweredValue {
                value: param_value,
                ty: val.ty.clone(),
            },
        );
        entry_args.push(val.value);
    }

    let mut header = ctx.start_block(header_params, header_bindings.clone());
    let header_id = header.id();

    let mut current = current;
    current.terminate(IrTerminator::Jump {
        target: header_id,
        args: entry_args,
    })?;
    ctx.seal_block(current)?;

    let cond_val = lower_expr(cond, ctx, &mut header)?;

    let body_block = ctx.start_block(vec![], header_bindings.clone());
    let body_id = body_block.id();

    let mut exit_params = Vec::new();
    let mut exit_bindings = HashMap::new();
    for binding in &ordered_bindings {
        let val = incoming.get(binding).unwrap();
        let param_value = ctx.fresh_value();
        exit_params.push(BlockParam {
            value: param_value,
            ty: val.ty.clone(),
            read_only: false,
        });
        exit_bindings.insert(
            *binding,
            LoweredValue {
                value: param_value,
                ty: val.ty.clone(),
            },
        );
    }
    let exit_block = ctx.start_block(exit_params, exit_bindings);
    let exit_id = exit_block.id();

    let mut else_args = Vec::new();
    for binding in &ordered_bindings {
        let val = header.bindings.get(binding).unwrap();
        else_args.push(val.value);
    }
    header.terminate(IrTerminator::Branch {
        cond: cond_val.value,
        then_block: body_id,
        then_args: vec![],
        else_block: exit_id,
        else_args,
    })?;
    ctx.seal_block(header)?;

    let loop_context = LoopContext {
        label: label.clone(),
        header_id,
        exit_id,
        ordered_bindings: ordered_bindings.clone(),
        exit_ordered_bindings: ordered_bindings.clone(),
    };
    let mut loop_stack = outer_loop_ctx.to_vec();
    loop_stack.push(loop_context);
    let body_result = lower_stmt_sequence(
        body.iter(),
        ctx,
        Some(body_block),
        spec,
        &loop_stack,
    )?;

    if let Some(mut body_active) = body_result {
        let mut backedge_args = Vec::new();
        for binding in &ordered_bindings {
            let val = body_active.bindings.get(binding).unwrap();
            backedge_args.push(val.value);
        }
        body_active.terminate(IrTerminator::Jump {
            target: header_id,
            args: backedge_args,
        })?;
        ctx.seal_block(body_active)?;
    }

    Ok(Some(exit_block))
}

fn lower_loop(
    label: &Option<String>,
    body: &[SemanticStmt],
    ctx: &mut LoweringCtx,
    current: ActiveBlock,
    spec: &FunctionLoweringSpec,
    outer_loop_ctx: &[LoopContext],
) -> Result<Option<ActiveBlock>, LoweringError> {
    let incoming = current.bindings.clone();

    let mut ordered_bindings: Vec<_> = incoming.keys().copied().collect();
    ordered_bindings.sort_by_key(|b| b.0);

    let mut header_params = Vec::new();
    let mut header_bindings = HashMap::new();
    let mut entry_args = Vec::new();

    for binding in &ordered_bindings {
        let val = incoming.get(binding).unwrap();
        let param_value = ctx.fresh_value();
        header_params.push(BlockParam {
            value: param_value,
            ty: val.ty.clone(),
            read_only: false,
        });
        header_bindings.insert(
            *binding,
            LoweredValue {
                value: param_value,
                ty: val.ty.clone(),
            },
        );
        entry_args.push(val.value);
    }

    let header = ctx.start_block(header_params, header_bindings.clone());
    let header_id = header.id();

    let mut current = current;
    current.terminate(IrTerminator::Jump {
        target: header_id,
        args: entry_args,
    })?;
    ctx.seal_block(current)?;

    let body_block = ctx.start_block(vec![], header_bindings.clone());

    let mut exit_params = Vec::new();
    let mut exit_bindings = HashMap::new();
    for binding in &ordered_bindings {
        let val = incoming.get(binding).unwrap();
        let param_value = ctx.fresh_value();
        exit_params.push(BlockParam {
            value: param_value,
            ty: val.ty.clone(),
            read_only: false,
        });
        exit_bindings.insert(
            *binding,
            LoweredValue {
                value: param_value,
                ty: val.ty.clone(),
            },
        );
    }
    let exit_block = ctx.start_block(exit_params, exit_bindings);
    let exit_id = exit_block.id();

    // Header unconditionally jumps into body — no condition for infinite loop
    let mut header_mut = header;
    header_mut.terminate(IrTerminator::Jump {
        target: body_block.id(),
        args: vec![],
    })?;
    ctx.seal_block(header_mut)?;

    let loop_context = LoopContext {
        label: label.clone(),
        header_id,
        exit_id,
        ordered_bindings: ordered_bindings.clone(),
        exit_ordered_bindings: ordered_bindings.clone(),
    };
    let mut loop_stack = outer_loop_ctx.to_vec();
    loop_stack.push(loop_context);

    let body_result = lower_stmt_sequence(
        body.iter(),
        ctx,
        Some(body_block),
        spec,
        &loop_stack,
    )?;

    if let Some(mut body_active) = body_result {
        let mut backedge_args = Vec::new();
        for binding in &ordered_bindings {
            let val = body_active.bindings.get(binding).unwrap();
            backedge_args.push(val.value);
        }
        body_active.terminate(IrTerminator::Jump {
            target: header_id,
            args: backedge_args,
        })?;
        ctx.seal_block(body_active)?;
    }

    Ok(Some(exit_block))
}

fn lower_for(
    label: &Option<String>,
    binding: BindingId,
    start: &SemanticExpr,
    end: &SemanticExpr,
    inclusive: bool,
    body: &[SemanticStmt],
    ctx: &mut LoweringCtx,
    current: ActiveBlock,
    spec: &FunctionLoweringSpec,
    outer_loop_ctx: &[LoopContext],
) -> Result<Option<ActiveBlock>, LoweringError> {
    let mut current = current;

    let start_val = lower_expr(start, ctx, &mut current)?;
    let end_val = lower_expr(end, ctx, &mut current)?;

    let incoming = current.bindings.clone();
    let mut ordered_bindings: Vec<_> = incoming.keys().copied().collect();
    ordered_bindings.sort_by_key(|b| b.0);

    // Header: counter + all incoming bindings as block params
    let mut header_params = Vec::new();
    let mut header_bindings = HashMap::new();
    let mut entry_args = Vec::new();

    let counter_param = ctx.fresh_value();
    // The header's counter param is the loop variable — mark read_only so the
    // validator can reject any backedge that passes an SsaBind-produced value
    // here (which would indicate the loop variable was reassigned in the body).
    header_params.push(BlockParam { value: counter_param, ty: start_val.ty.clone(), read_only: true });
    entry_args.push(start_val.value);

    for b in &ordered_bindings {
        let val = incoming.get(b).unwrap();
        let pv = ctx.fresh_value();
        header_params.push(BlockParam { value: pv, ty: val.ty.clone(), read_only: false });
        header_bindings.insert(*b, LoweredValue { value: pv, ty: val.ty.clone() });
        entry_args.push(val.value);
    }

    let mut header = ctx.start_block(header_params, header_bindings.clone());
    let header_id = header.id();

    current.terminate(IrTerminator::Jump { target: header_id, args: entry_args })?;
    ctx.seal_block(current)?;

    // Increment block: counter + bindings as params, increments counter, jumps to header
    let inc_counter_param = ctx.fresh_value();
    // The increment block's counter param receives the loop variable from the
    // body's backedge — also marked read_only so that a body-modified counter
    // value (an SsaBind result) is caught here before it reaches the Add.
    let mut inc_params = vec![BlockParam { value: inc_counter_param, ty: start_val.ty.clone(), read_only: true }];
    let mut inc_bindings = HashMap::new();
    for b in &ordered_bindings {
        let val = incoming.get(b).unwrap();
        let pv = ctx.fresh_value();
        inc_params.push(BlockParam { value: pv, ty: val.ty.clone(), read_only: false });
        inc_bindings.insert(*b, LoweredValue { value: pv, ty: val.ty.clone() });
    }
    let mut inc_block = ctx.start_block(inc_params, inc_bindings);
    let inc_id = inc_block.id();

    let one_dst = ctx.fresh_value();
    inc_block.emit(IrInst::ConstInt { dst: one_dst, ty: start_val.ty.clone(), value: 1 })?;
    let next_dst = ctx.fresh_value();
    inc_block.emit(IrInst::Binary {
        dst: next_dst,
        op: BinaryOp::Add,
        ty: start_val.ty.clone(),
        lhs: inc_counter_param,
        rhs: one_dst,
    })?;
    let mut inc_jump_args = vec![next_dst];
    for b in &ordered_bindings {
        inc_jump_args.push(inc_block.bindings.get(b).unwrap().value);
    }
    inc_block.terminate(IrTerminator::Jump { target: header_id, args: inc_jump_args })?;
    ctx.seal_block(inc_block)?;

    // Compare counter to end on header
    let cmp_dst = ctx.fresh_value();
    header.emit(IrInst::Compare {
        dst: cmp_dst,
        op: if inclusive { CompareOp::Le } else { CompareOp::Lt },
        lhs: counter_param,
        rhs: end_val.value,
    })?;

    // Body block: expose counter as the loop variable binding
    let mut body_bindings = header_bindings.clone();
    body_bindings.insert(binding, LoweredValue { value: counter_param, ty: start_val.ty.clone() });
    let body_block = ctx.start_block(vec![], body_bindings);
    let body_id = body_block.id();

    // Exit block: only regular bindings
    let mut exit_params = Vec::new();
    let mut exit_bindings = HashMap::new();
    for b in &ordered_bindings {
        let val = incoming.get(b).unwrap();
        let pv = ctx.fresh_value();
        exit_params.push(BlockParam { value: pv, ty: val.ty.clone(), read_only: false });
        exit_bindings.insert(*b, LoweredValue { value: pv, ty: val.ty.clone() });
    }
    let exit_block = ctx.start_block(exit_params, exit_bindings);
    let exit_id = exit_block.id();

    let mut else_args = Vec::new();
    for b in &ordered_bindings {
        else_args.push(header.bindings.get(b).unwrap().value);
    }
    header.terminate(IrTerminator::Branch {
        cond: cmp_dst,
        then_block: body_id,
        then_args: vec![],
        else_block: exit_id,
        else_args,
    })?;
    ctx.seal_block(header)?;

    // continue jumps to inc_block (with counter + bindings), break jumps to exit_block (bindings only)
    // We use inc_id as header_id in the LoopContext so continue goes to increment block.
    // Body's natural fallthrough also goes to inc_block.
    let loop_context = LoopContext {
        label: label.clone(),
        header_id: inc_id,
        exit_id,
        ordered_bindings: {
            // Continue needs to pass [counter, ...bindings] to inc_block.
            // Body has `binding` mapped to counter_param, so continue
            // will pick it up if we put it first in ordered_bindings.
            let mut v = vec![binding];
            v.extend(ordered_bindings.iter().copied());
            v
        },
        exit_ordered_bindings: ordered_bindings.clone(),
    };
    let mut loop_stack = outer_loop_ctx.to_vec();
    loop_stack.push(loop_context);

    let body_result = lower_stmt_sequence(
        body.iter(),
        ctx,
        Some(body_block),
        spec,
        &loop_stack,
    )?;

    if let Some(mut body_active) = body_result {
        // Natural fallthrough also jumps to inc_block
        let mut args = vec![body_active.bindings.get(&binding).unwrap().value];
        for b in &ordered_bindings {
            args.push(body_active.bindings.get(b).unwrap().value);
        }
        body_active.terminate(IrTerminator::Jump { target: inc_id, args })?;
        ctx.seal_block(body_active)?;
    }

    Ok(Some(exit_block))
}

fn finalize_active_block(
    ctx: &mut LoweringCtx,
    mut active: ActiveBlock,
    default_term: IrTerminator,
) -> Result<(), LoweringError> {
    if !active.terminated {
        active.terminate(default_term)?;
    }
    ctx.seal_block(active)
}

/// Fold `len(x)` to its constant integer length when `x` has a compile-time-
/// known length (tracker D2.3a) — no string representation needed. `len("…")`
/// folds to the UTF-8 byte count (`str::len`, matching the interpreter's stored
/// arena-slice length — bytes, not chars/graphemes; e.g. `len("é")` == 2, per
/// runtime/call.rs #021). `len(<array>)` folds to the declared element count from
/// the `Array(count, _)` type (the same count the bounds check and array layout
/// read). `len` returns t64, so the folded constant is I64. Returns `None` when
/// the length is not statically known (a dynamic string / strref), leaving the
/// caller's `is_cx_builtin` path to SKIP it cleanly.
fn try_fold_len(
    args: &[SemanticCallArg],
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<Option<LoweredValue>, LoweringError> {
    let arg = match args.first() {
        Some(SemanticCallArg::Expr(e)) => e,
        _ => return Ok(None),
    };
    let n: i128 = match &arg.kind {
        SemanticExprKind::Value(SemanticValue::Str(s)) => s.len() as i128,
        _ => match &arg.ty {
            SemanticType::Array(count, _) => *count as i128,
            _ => return Ok(None),
        },
    };
    let dst = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst, ty: IrType::I64, value: n })?;
    Ok(Some(LoweredValue { value: dst, ty: IrType::I64 }))
}

fn lower_expr(
    expr: &SemanticExpr,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    match &expr.kind {
        SemanticExprKind::Value(value) => lower_value(value, &expr.ty, ctx, active),
        SemanticExprKind::VarRef { binding, name } => {
            let lowered = active.bindings.get(binding).cloned().ok_or_else(|| {
                LoweringError::InternalInvariantViolation {
                    detail: format!(
                        "binding '{name}' ({}) referenced before any SSA value was assigned",
                        binding.0
                    ),
                }
            })?;
            if matches!(expr.ty, SemanticType::Numeric | SemanticType::Unknown) {
                // Placeholder type: the stored binding type (set at first assignment)
                // is authoritative. Mirrors the fallback in lower_array_lit/lower_index.
                Ok(lowered)
            } else {
                let ty = lower_type(&expr.ty)?;
                // D2.x-A1: a `bool`-typed reference may read a binding stored as
                // TBool — a `bool = ?` unknown. Bool and TBool are the two- and
                // three-state forms of the same semantic `bool`; accept the stored
                // TBool so the when / if consumer sees the three-state value (it
                // would otherwise mismatch and SKIP here, before the match/guard).
                if ty == IrType::Bool && lowered.ty == IrType::TBool {
                    Ok(lowered)
                } else {
                    ensure_type_match("var ref", ty, lowered.ty.clone())?;
                    Ok(lowered)
                }
            }
        }
        SemanticExprKind::Binary { lhs, op, rhs, .. } => {
            lower_binary(lhs, *op, rhs, &expr.ty, ctx, active)
        }
        SemanticExprKind::Cast { expr, from, to } => {
            let to_ty = lower_type(to)?;
            if *from == SemanticType::Numeric {
                if ir_int_range(&to_ty).is_some() {
                    // Integer target: fast path — lower the literal directly at
                    // to_ty so it is range-validated against the actual
                    // destination width and emitted without a redundant
                    // default-width → to_ty cast instruction.
                    if let SemanticExprKind::Value(semantic_val) = &expr.kind {
                        let lowered = lower_value(semantic_val, to, ctx, active)?;
                        ensure_type_match("cast source", to_ty.clone(), lowered.ty.clone())?;
                        return Ok(lowered);
                    }
                }
                // Non-integer target (e.g. F64) or non-literal Numeric
                // expression: lower the source at the target-default integer
                // width (I64 on 64-bit), then emit a Cast to the destination
                // type.  lower_type(Numeric) has no IR equivalent, so we use
                // the actual lowered type as the cast source.
                let lowered = lower_expr(expr, ctx, active)?;
                let from_ty = lowered.ty.clone();
                if from_ty == to_ty {
                    return Ok(LoweredValue { value: lowered.value, ty: to_ty });
                }
                let dst = ctx.fresh_value();
                active.emit(IrInst::Cast {
                    dst,
                    from: from_ty,
                    to: to_ty.clone(),
                    value: lowered.value,
                })?;
                return Ok(LoweredValue { value: dst, ty: to_ty });
            }
            let lowered = lower_expr(expr, ctx, active)?;
            let from_ty = lower_type(from)?;
            ensure_type_match("cast source", from_ty.clone(), lowered.ty)?;
            // Skip no-op casts only after validating source type invariants.
            if from_ty == to_ty {
                return Ok(LoweredValue { value: lowered.value, ty: to_ty });
            }
            let dst = ctx.fresh_value();
            active.emit(IrInst::Cast {
                dst,
                from: from_ty,
                to: to_ty.clone(),
                value: lowered.value,
            })?;
            Ok(LoweredValue {
                value: dst,
                ty: to_ty,
            })
        }
        // Direct function call lowering strategy
        //
        // A direct call emits one IrInst::Call instruction per call site.  The
        // lowering proceeds in four phases:
        //
        // 1. Signature resolution
        //    The signature table (built in a pre-pass over all FuncDef statements)
        //    is consulted by callee name.  An unknown callee is a hard error —
        //    semantic analysis guarantees every call is resolved, so a missing
        //    entry indicates an internal invariant violation.
        //
        // 2. Arity and return-type validation
        //    Void-returning callees are rejected at this stage; IrType::Void is
        //    not yet defined (see Phase 8 ABI work).  Arity mismatches between
        //    the call site and the signature are flagged as invariant violations,
        //    since the semantic layer enforces them before lowering runs.
        //
        // 3. Argument lowering
        //    Each SemanticCallArg::Expr argument is recursively lowered through
        //    lower_expr, producing a ValueId.  Type agreement between the lowered
        //    argument and the expected parameter type is verified with
        //    ensure_type_match.  Non-Expr call arguments (Copy, CopyFree,
        //    CopyInto) are not yet supported; they require ABI decisions that
        //    belong to Phase 8.
        //
        // 4. Instruction emission
        //    A fresh ValueId is allocated for the call result.
        //    IrInst::Call { dst: Some(result), callee, args, return_ty } is
        //    emitted into the active block.  The result ValueId is returned to
        //    the caller as a LoweredValue so it can flow into assignments,
        //    return statements, and sub-expressions.
        SemanticExprKind::Call { callee, function: _, args } => {
            // D2.3a: fold `len()` on a compile-time-known operand to its constant
            // integer length — no string representation needed. A `len` whose
            // length is not statically known (a dynamic string, which doesn't
            // lower anyway) returns None and falls through to the SKIP below.
            if callee == "len" {
                if let Some(folded) = try_fold_len(args, ctx, active)? {
                    return Ok(folded);
                }
            }
            // Remaining unimplemented builtin intrinsics (read, input) are not in the
            // signature_table.  Intercept them before the lookup so the error is
            // structured and actionable rather than the generic UnresolvedSemanticArtifact
            // produced by a table miss.
            // Note: assert/assert_eq, print, println, and printn are handled at statement
            // level and should not reach lower_expr in well-formed programs.
            if is_cx_builtin(callee) {
                return Err(LoweringError::UnsupportedSemanticConstruct {
                    construct: format!(
                        "builtin '{}' is not yet lowerable to IR — codegen pending (Phase 9)",
                        callee
                    ),
                });
            }
            let (param_types, return_ty, ret_struct) = {
                let sig = ctx.signature_table.get(callee).ok_or_else(|| {
                    LoweringError::UnresolvedSemanticArtifact {
                        artifact: format!("function '{}'", callee),
                    }
                })?;
                (sig.param_types.clone(), sig.return_ty.clone(), sig.ret_struct.clone())
            };

            let return_ty = return_ty.ok_or_else(|| {
                LoweringError::UnsupportedSemanticConstruct {
                    construct: format!(
                        "void function '{}' used in value position — void calls are only valid as statements",
                        callee
                    ),
                }
            })?;

            // A struct-returning callee's param list ends with the hidden
            // return-slot Ptr, which the caller supplies below — user args
            // exclude it.
            let user_param_count = param_types.len() - usize::from(ret_struct.is_some());
            if args.len() != user_param_count {
                return Err(LoweringError::InternalInvariantViolation {
                    detail: format!(
                        "call to '{}': expected {} arguments, got {}",
                        callee, user_param_count, args.len()
                    ),
                });
            }

            // Arguments are lowered left-to-right (args.iter() order), matching
            // the interpreter's left-to-right argument evaluation in
            // call_semantic_func.  See docs/backend/cx_eval_order.md.
            let mut lowered_args = Vec::new();
            for (i, arg) in args.iter().enumerate() {
                match arg {
                    SemanticCallArg::Expr(expr) => {
                        let lowered = lower_expr(expr, ctx, active)?;
                        ensure_type_match(
                            &format!("argument {} of call to '{}'", i, callee),
                            param_types[i].clone(),
                            lowered.ty,
                        )?;
                        lowered_args.push(lowered.value);
                    }
                    _ => {
                        return Err(LoweringError::UnsupportedSemanticConstruct {
                            construct: format!("non-Expr call argument in call to '{}'", callee),
                        });
                    }
                }
            }

            // Struct-returning callee: alloca the return slot in THIS (caller)
            // frame and pass its address as the hidden trailing arg. The
            // callee copies the result through it and returns this same
            // pointer — so the value the caller uses lives in the caller's
            // own frame, never in the callee's dead one.
            if let Some(ref sname) = ret_struct {
                let layout_info = ctx.struct_table.get(sname).cloned().ok_or_else(|| {
                    LoweringError::UnresolvedSemanticArtifact {
                        artifact: format!("struct type '{}' for return slot of '{}'", sname, callee),
                    }
                })?;
                if layout_info.layout.total_size == 0 {
                    return Err(LoweringError::UnsupportedSemanticConstruct {
                        construct: format!("zero-size struct return from '{}'", callee),
                    });
                }
                let slot = ctx.fresh_value();
                active.emit(IrInst::Alloca {
                    dst: slot,
                    size: layout_info.layout.total_size,
                    align: layout_info.layout.alignment,
                })?;
                lowered_args.push(slot);
            }

            let dst = ctx.fresh_value();
            active.emit(IrInst::Call {
                dst: Some(dst),
                callee: callee.clone(),
                args: lowered_args,
                return_ty: Some(return_ty.clone()),
            })?;

            Ok(LoweredValue {
                value: dst,
                ty: return_ty,
            })
        },
        SemanticExprKind::DotAccess { binding, container, field, struct_name } => {
            lower_dot_access(binding, container, field, struct_name, &expr.ty, ctx, active)
        }
        SemanticExprKind::HandleNew { value, .. } => lower_handle_new(value, ctx, active),
        SemanticExprKind::HandleVal { binding, name, .. } => lower_handle_val(*binding, name, ctx, active),
        SemanticExprKind::HandleDrop { binding, name, .. } => lower_handle_drop(*binding, name, ctx, active),
        SemanticExprKind::Range { .. } => {
            return Err(LoweringError::UnsupportedSemanticConstruct {
                construct: "range expression used as a value — ranges are only supported in for-loop bounds, not as standalone expressions".to_string(),
            });
        },
        // Unary expression lowering strategy
        //
        // The IR has no dedicated unary-negate or unary-not instructions.  Both
        // operators are expressed as two-operand forms so that every code-gen
        // backend only needs to handle a single instruction shape.
        //
        // Op::Minus  — arithmetic negation
        //   Lower as `0 - value`.  A zero literal of the operand's type is
        //   synthesised first; then a Binary/Sub instruction computes the
        //   difference.  This works uniformly for i8/i16/i32/i64 and f64
        //   because each type already has a matching ConstInt/ConstFloat form.
        //
        // Op::Not  — boolean complement
        //   Lower as `value == 0` using Compare/Eq against a Bool-typed zero.
        //   The semantic layer guarantees the operand is Bool, so "false" is
        //   represented as 0 and "true" as 1; equality with zero is therefore
        //   equivalent to logical negation.
        //
        // All other operator tokens are rejected at this stage; they either
        // do not exist in the current grammar or have no unary meaning.
        SemanticExprKind::Unary { op, expr, .. } => {
    let lowered = lower_expr(expr, ctx, active)?;
    match op {
        Op::Minus => {
            // Emit the zero operand in the correct type, then subtract:
            //   dst = 0 - lowered
            let zero = ctx.fresh_value();
            match &lowered.ty {
                IrType::F64 => {
                    active.emit(IrInst::ConstFloat { dst: zero, value: 0.0 })?;
                }
                ty => {
                    active.emit(IrInst::ConstInt { dst: zero, ty: ty.clone(), value: 0 })?;
                }
            }
            let dst = ctx.fresh_value();
            active.emit(IrInst::Binary {
                dst,
                op: BinaryOp::Sub,
                ty: lowered.ty.clone(),
                lhs: zero,
                rhs: lowered.value,
            })?;
            Ok(LoweredValue { value: dst, ty: lowered.ty })
        }
        Op::Not => {
            // D2.x-A2: `!` on a three-state TBool negates per Kleene (!0=1, !1=0,
            // !2=2) on the I8 wire (runtime/ops.rs:256). Definite Bool keeps the
            // compare-against-zero path below.
            if lowered.ty == IrType::TBool {
                return emit_kleene_not(lowered.value, ctx, active);
            }
            // Emit a Bool zero, then compare for equality:
            //   dst = (lowered == 0)
            // Because Bool is canonically 0/1, this flips the value.
            let zero = ctx.fresh_value();
            active.emit(IrInst::ConstInt { dst: zero, ty: IrType::Bool, value: 0 })?;
            let dst = ctx.fresh_value();
            active.emit(IrInst::Compare {
                dst,
                op: CompareOp::Eq,
                lhs: lowered.value,
                rhs: zero,
            })?;
            Ok(LoweredValue { value: dst, ty: IrType::Bool })
        }
        _ => {
            return Err(LoweringError::UnsupportedSemanticConstruct {
                construct: format!("unary operator {:?}", op),
            });
        }
    }
},
        SemanticExprKind::ArrayLit { elements } => {
            lower_array_lit(elements, &expr.ty, ctx, active)
        }
        SemanticExprKind::Index { target, index, .. } => {
            lower_index(target, index, &expr.ty, ctx, active)
        }
        SemanticExprKind::MethodCall { instance, method, args, instance_binding, struct_name, pos: _ } => {
            // Synthesize an equivalent Call and recurse into the existing Call
            // arm (which has the proven signature_table lookup, void-in-value-
            // position rejection, arity check, per-arg ensure_type_match, and
            // IrInst::Call emission). The synthetic receiver arg is a VarRef
            // that lowers to the receiver's Ptr SSA via the existing VarRef
            // arm. Arg order matches Stage 1's IrFunction param order
            // [receiver, extra_aliases..., user_params...].
            let recv_arg = SemanticCallArg::Expr(SemanticExpr {
                ty: SemanticType::Struct(struct_name.clone()),
                kind: SemanticExprKind::VarRef {
                    binding: *instance_binding,
                    name: instance.clone(),
                },
            });
            let mut full_args = Vec::with_capacity(args.len() + 1);
            full_args.push(recv_arg);
            full_args.extend(args.iter().cloned());
            let callee = mangle_method(struct_name, method);
            let synthetic = SemanticExpr {
                ty: expr.ty.clone(),
                kind: SemanticExprKind::Call {
                    callee,
                    function: crate::frontend::semantic_types::FunctionId(u32::MAX),
                    args: full_args,
                },
            };
            lower_expr(&synthetic, ctx, active)
        }
        // Struct literal lowering strategy
        //
        // A struct literal `S { f1: e1, f2: e2, ... }` is lowered to a sequence
        // of memory operations that produce a stack-allocated struct value:
        //
        // 1. Alloca: reserve stack space for the whole struct using the layout
        //    computed by build_struct_table (total_size, alignment).
        //
        // 2. For each field in canonical (definition) order:
        //    a. Lower the field expression to a scalar IR value.
        //    b. If the field's byte offset within the struct is non-zero, emit a
        //       PtrOffset instruction to advance the base pointer by that many bytes.
        //    c. Emit Store to write the field value at the (possibly offset) pointer.
        //
        // 3. Return the base Alloca pointer as IrType::Ptr — the binding that holds
        //    a struct variable holds a pointer to its stack storage.
        //
        // Field ordering in the literal need not match definition order; we look up
        // each canonical field name in the literal's field list by name.
        SemanticExprKind::StructInstance { type_name, fields } => {
            let layout_info = ctx.struct_table.get(type_name).cloned().ok_or_else(|| {
                LoweringError::UnresolvedSemanticArtifact {
                    artifact: format!("struct type '{}'", type_name),
                }
            })?;

            if layout_info.layout.total_size == 0 {
                return Err(LoweringError::UnsupportedSemanticConstruct {
                    construct: "StructInstance with zero-size layout".to_string(),
                });
            }

            let ptr = ctx.fresh_value();
            active.emit(IrInst::Alloca {
                dst: ptr,
                size: layout_info.layout.total_size,
                align: layout_info.layout.alignment,
            })?;

            for (field_idx, (canonical_name, field_ir_ty)) in layout_info.fields.iter().enumerate() {
                let field_offset = layout_info.layout.field_offsets[field_idx];

                let field_expr = fields
                    .iter()
                    .find(|(name, _)| name == canonical_name)
                    .ok_or_else(|| LoweringError::InternalInvariantViolation {
                        detail: format!(
                            "struct '{}' field '{}' missing in literal",
                            type_name, canonical_name
                        ),
                    })?;

                let lowered_field = lower_expr(&field_expr.1, ctx, active)?;

                let field_ptr = if field_offset == 0 {
                    ptr
                } else {
                    let fp = ctx.fresh_value();
                    active.emit(IrInst::PtrOffset {
                        dst: fp,
                        base: ptr,
                        offset: field_offset,
                    })?;
                    fp
                };

                // Narrow the field value to the field's width before storing
                // (tracker Gate-2a — same root as lower_array_lit). A scalar
                // sub-64-bit field receives an i64-typed value; without this the
                // Store overruns the field slot. The cast is a no-op for fields
                // whose value already matches the slot width (arrays/structs are
                // Ptr-typed both sides; t64 scalars are i64 both sides).
                let store_value = if lowered_field.ty != *field_ir_ty {
                    let narrowed = ctx.fresh_value();
                    active.emit(IrInst::Cast {
                        dst: narrowed,
                        from: lowered_field.ty.clone(),
                        to: field_ir_ty.clone(),
                        value: lowered_field.value,
                    })?;
                    narrowed
                } else {
                    lowered_field.value
                };

                active.emit(IrInst::Store {
                    ptr: field_ptr,
                    value: store_value,
                })?;
            }

            Ok(LoweredValue { value: ptr, ty: IrType::Ptr })
        },
        SemanticExprKind::When { expr: scrutinee, arms, .. } => {
            lower_when_expr(scrutinee, arms, &expr.ty, ctx, active)
        },
        // Tracker D2.1 (#046): if-expression lowering — a two-branch value-
        // producing `when`. #046's else-required rule is enforced at semantic
        // time (a missing else is rejected pre-lowering). As of D2.x-A1 a bool
        // `?` lowers (TBool), so an unknown condition IS reachable here; it is
        // trapped via `guard_unknown_condition` (cx_trap), reproducing #026's
        // UnknownCondition. A definite condition lowers to a Bool as before.
        SemanticExprKind::If { condition, then_body, else_body, .. } => {
            lower_if_expr(condition, then_body, else_body, &expr.ty, ctx, active)
        },
        // D2.4a: `Ok(x)` = tag 0, `Err("...")` = tag 1 (packed-i128). The `?`
        // operator (Try) is D2.4b — still SKIP.
        SemanticExprKind::ResultOk { expr } => lower_result_construct(0, expr, ctx, active),
        SemanticExprKind::ResultErr { expr } => lower_result_construct(1, expr, ctx, active),
        // D2.4b: `expr?` unwraps the Ok payload (continuing) or early-returns the
        // whole Err Result. `expr.ty` is the unwrapped Ok type T (semantic.rs sets
        // it); `inner` is the Result<T> operand.
        SemanticExprKind::Try { expr: inner, .. } => lower_try(inner, &expr.ty, ctx, active),
    }
}

/// Returns the inclusive `[min, max]` range of values that fit in `ty` as a
/// signed integer, or `None` if `ty` is not an integer type.
fn ir_int_range(ty: &IrType) -> Option<(i128, i128)> {
    // Integer-width bounds come from the one facts table (tracker D1.1); Bool's
    // `(0, 1)` is not a `tN` width fact and stays a local arm (int_facts scope note).
    if let Some(width) = ty.int_width() {
        let f = width.facts();
        return Some((f.min, f.max));
    }
    match ty {
        IrType::Bool => Some((0, 1)),
        _ => None,
    }
}

fn lower_value(
    value: &SemanticValue,
    semantic_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    // Numeric literals have no explicit type annotation in Cx source.  The
    // semantic layer uses `SemanticType::Numeric` as a placeholder.  The IR
    // default for unresolved numeric literals is the target's native integer
    // width: I64 on 64-bit targets, I32 on 32-bit targets.
    let ty = if *semantic_ty == SemanticType::Numeric {
        ctx.target.numeric_literal_ir_type()
    } else {
        lower_type(semantic_ty)?
    };
    let dst = ctx.fresh_value();

    match value {
        SemanticValue::Num(n) => {
            let value = *n;
            if let Some((min, max)) = ir_int_range(&ty) {
                if value < min || value > max {
                    return Err(LoweringError::UnsupportedSemanticConstruct {
                        construct: format!(
                            "integer literal {value} does not fit in {ty:?} during lowering"
                        ),
                    });
                }
            }
            active.emit(IrInst::ConstInt {
                dst,
                ty: ty.clone(),
                value,
            })?;
            Ok(LoweredValue { value: dst, ty })
        }
        SemanticValue::Float(value) => {
            if ty != IrType::F64 {
                return Err(LoweringError::InternalInvariantViolation {
                    detail: format!("float literal lowered with non-f64 type: {ty:?}"),
                });
            }
            active.emit(IrInst::ConstFloat { dst, value: *value })?;
            Ok(LoweredValue { value: dst, ty })
        }
        SemanticValue::Bool(value) => {
            if ty != IrType::Bool {
                return Err(LoweringError::InternalInvariantViolation {
                    detail: format!("bool literal lowered with non-bool type: {ty:?}"),
                });
            }
            active.emit(IrInst::ConstInt {
                dst,
                ty: IrType::Bool,
                value: i128::from(*value),
            })?;
            Ok(LoweredValue { value: dst, ty })
        }
        SemanticValue::Unknown => Err(LoweringError::UnsupportedSemanticType {
            ty: "Unknown".to_string(),
        }),
        // D2.3b: a static string literal lowers to a descriptor Ptr (shared with
        // the D2.3c concat fold via `construct_static_str`).
        SemanticValue::Str(s) => construct_static_str(s, ctx, active),
        SemanticValue::Char(_) => Err(LoweringError::UnsupportedSemanticType {
            ty: "Char".to_string(),
        }),
        // Construct a tag-only enum value as its tag constant (D2.2). The tag is
        // the declaration-order `variant_id` carried in the semantic node — the
        // same value the match arm compares against. `ty` is the enum's IR type
        // (I8, from lower_type above).
        SemanticValue::EnumVariant { enum_name, variant_name, variant_id, .. } => {
            let tag = match variant_id {
                Some(id) => id.0,
                None => {
                    return Err(LoweringError::UnsupportedSemanticConstruct {
                        construct: format!(
                            "enum variant '{}::{}' has no resolved tag (variant_id)",
                            enum_name, variant_name
                        ),
                    })
                }
            };
            active.emit(IrInst::ConstInt {
                dst,
                ty: ty.clone(),
                value: tag as i128,
            })?;
            Ok(LoweredValue { value: dst, ty })
        }
    }
}

// Binary expression lowering — evaluation order guarantee
//
// Operands are lowered strictly left-to-right: `lhs` is fully lowered
// (all instructions emitted) before `rhs` lowering begins.  This mirrors
// the interpreter's `eval_semantic_expr` which evaluates `lhs` before `rhs`
// at runtime.  Any side effects embedded in either operand (e.g. function
// calls) occur in left-to-right order in both the interpreter path and the
// compiled path.
//
// This ordering is a language guarantee for Cx 0.1 and must not be changed
// by optimisation passes.  See docs/backend/cx_eval_order.md.
//
// Exception: Op::And and Op::Or use short-circuit evaluation and are
// dispatched to lower_logical before any operand is evaluated.
/// Build a static string VALUE (string rep (a), D2.3b): leak `s`'s bytes and a
/// {byte_ptr, byte_len} descriptor to `&'static`, then materialize the descriptor
/// address as a Ptr (ConstInt(I64) + the no-op Cast(I64->Ptr)). Shared by the
/// literal construct (lower_value) and the D2.3c concat fold. Interpolation
/// (`{…}`) defers to D2.3d — a brace-containing result stays SKIP.
fn construct_static_str(
    s: &str,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    if s.contains('{') {
        return Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "string interpolation '{...}' not yet lowered (tracker D2.3d)".to_string(),
        });
    }
    // JIT-only / non-AOT (R6): the leaked address is process-specific and baked
    // into the IR; the bytes outlive the program, so the descriptor never dangles.
    let bytes: &'static [u8] = s.to_owned().into_bytes().leak();
    let descriptor: &'static [i64] =
        vec![bytes.as_ptr() as usize as i64, bytes.len() as i64].leak();
    let desc_addr = descriptor.as_ptr() as usize as i128;
    let addr = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: addr, ty: IrType::I64, value: desc_addr })?;
    let ptr = ctx.fresh_value();
    active.emit(IrInst::Cast { dst: ptr, from: IrType::I64, to: IrType::Ptr, value: addr })?;
    Ok(LoweredValue { value: ptr, ty: IrType::Ptr })
}

/// Recursively evaluate a compile-time-known string: a `str` literal, or a
/// left-associative chain of `+` over known strings. Returns `None` for any
/// runtime operand (VarRef, call, …) — those stay SKIP (R6 / D2.3-DYN).
fn try_const_str(expr: &SemanticExpr) -> Option<String> {
    match &expr.kind {
        SemanticExprKind::Value(SemanticValue::Str(s)) => Some(s.clone()),
        SemanticExprKind::Binary { lhs, op: Op::Plus, rhs, .. }
            if expr.ty == SemanticType::Str =>
        {
            Some(try_const_str(lhs)? + &try_const_str(rhs)?)
        }
        _ => None,
    }
}

/// Lower `str + str` (D2.3c): constant-fold a literal concat to ONE new static
/// descriptor (the same joined bytes the interpreter `alloc_str`s, different
/// storage). A runtime operand stays SKIP (R6 / D2.3-DYN).
fn lower_str_concat(
    lhs: &SemanticExpr,
    rhs: &SemanticExpr,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    match (try_const_str(lhs), try_const_str(rhs)) {
        (Some(l), Some(r)) => construct_static_str(&(l + &r), ctx, active),
        _ => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "runtime string concatenation not yet lowered (R6 / D2.3-DYN)".to_string(),
        }),
    }
}

/// Lower string content equality (D2.3c). Both operands are str descriptor Ptrs;
/// `cx_str_eq` reads (ptr, len) from each and does a length-check + memcmp,
/// returning a Bool. `!=` negates the result. The TBool refusal and scalar
/// `Compare` are separate operand-type-dispatched branches in `lower_binary`.
fn lower_str_eq(
    op: Op,
    lhs: ValueId,
    rhs: ValueId,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let eq = ctx.fresh_value();
    active.emit(IrInst::Call {
        dst: Some(eq),
        callee: "cx_str_eq".to_string(),
        args: vec![lhs, rhs],
        return_ty: Some(IrType::Bool),
    })?;
    if op == Op::NotEq {
        // `!=` is `==` negated: (eq == 0).
        let zero = ctx.fresh_value();
        active.emit(IrInst::ConstInt { dst: zero, ty: IrType::Bool, value: 0 })?;
        let ne = ctx.fresh_value();
        active.emit(IrInst::Compare { dst: ne, op: CompareOp::Eq, lhs: eq, rhs: zero })?;
        Ok(LoweredValue { value: ne, ty: IrType::Bool })
    } else {
        Ok(LoweredValue { value: eq, ty: IrType::Bool })
    }
}

fn lower_binary(
    lhs: &SemanticExpr,
    op: Op,
    rhs: &SemanticExpr,
    result_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    // Short-circuit operators must be dispatched before eager evaluation.
    if matches!(op, Op::And | Op::Or) {
        return lower_logical(lhs, op, rhs, result_ty, ctx, active);
    }

    // D2.3c: `str + str` is concatenation, constant-folded to one static
    // descriptor — handled before the operands lower (the fold needs their string
    // content, not their descriptor Ptrs). A runtime operand stays SKIP (R6).
    if op == Op::Plus && *result_ty == SemanticType::Str {
        return lower_str_concat(lhs, rhs, ctx, active);
    }
    // D2.3c: string `==`/`!=` dispatches on operand type — captured here, before
    // the operands lower to Ptr. str → content comparison (cx_str_eq); TBool → the
    // existing refusal; scalar → Compare. (Computed before the shadowing below.)
    let str_eq = matches!(op, Op::EqEq | Op::NotEq)
        && (matches!(lhs.ty, SemanticType::Str | SemanticType::StrRef)
            || matches!(rhs.ty, SemanticType::Str | SemanticType::StrRef));

    // Left operand lowered first — preserves left-to-right evaluation order.
    let lhs = lower_expr(lhs, ctx, active)?;
    // Right operand lowered second — all lhs side effects are already emitted.
    let rhs = lower_expr(rhs, ctx, active)?;
    let dst = ctx.fresh_value();

    match op {
        Op::Plus | Op::Minus | Op::Mul | Op::Div | Op::Mod => {
            // SemanticType::Numeric is an unresolved placeholder for integer
            // literals without an explicit type annotation.  Resolve it to the
            // target's native integer width (I64 on 64-bit) so that expressions
            // like `0 + 0` or `3 * 4` lower successfully instead of returning
            // an UnsupportedSemanticType error (which propagates as JIT SKIP).
            let ty = if *result_ty == SemanticType::Numeric {
                ctx.target.numeric_literal_ir_type()
            } else {
                lower_type(result_ty)?
            };
            ensure_type_match("binary lhs", ty.clone(), lhs.ty)?;
            ensure_type_match("binary rhs", ty.clone(), rhs.ty)?;
            // Integer division and modulo route through the guard helper —
            // divisor==0 traps cleanly (Gate-1b), and Div additionally guards
            // INT_MIN/-1 overflow (Gate-1a). Float div/mod (no int width) and the
            // other ops fall through unchanged, so their emitted IR is identical.
            if matches!(op, Op::Div | Op::Mod) && ty.int_width().is_some() {
                let binop = if matches!(op, Op::Div) {
                    BinaryOp::Div
                } else {
                    BinaryOp::Rem
                };
                return emit_guarded_idiv(
                    dst,
                    ty.clone(),
                    binop,
                    lhs.value,
                    rhs.value,
                    ctx,
                    active,
                );
            }
            let op = match op {
                Op::Plus => BinaryOp::Add,
                Op::Minus => BinaryOp::Sub,
                Op::Mul => BinaryOp::Mul,
                Op::Div => BinaryOp::Div,
                Op::Mod => BinaryOp::Rem,
                _ => unreachable!(),
            };
            active.emit(IrInst::Binary {
                dst,
                op,
                ty: ty.clone(),
                lhs: lhs.value,
                rhs: rhs.value,
            })?;
            Ok(LoweredValue { value: dst, ty })
        }
        Op::EqEq | Op::NotEq | Op::Lt | Op::LtEq | Op::Gt | Op::GtEq => {
            // D2.x-A1 safety valve: `==` / `!=` on a three-state bool yields TBool
            // (unknown) in the interpreter (runtime/ops.rs), which the IR Compare
            // cannot produce (it yields a definite Bool — the dual-rep seam). The
            // operand-match check below PASSES for TBool == TBool (both equal), so
            // without this guard the comparison would silently miscompile. Refuse
            // until Path B / the seam lowers three-valued equality.
            if lhs.ty == IrType::TBool || rhs.ty == IrType::TBool {
                unsupported!(
                    "equality/comparison on three-state bool (TBool) — deferred to D2.x Path B"
                );
            }
            // D2.3c: string content comparison — str `==`/`!=` operands route to
            // cx_str_eq (length + memcmp), NOT a pointer Compare. str-ness was
            // captured before the operands lowered to Ptr. The TBool refusal above
            // and the scalar Compare below are untouched — purely additive
            // operand-type dispatch.
            if str_eq {
                return lower_str_eq(op, lhs.value, rhs.value, ctx, active);
            }
            // Composite (non-str) Ptr comparison — structs/arrays — stays SKIP.
            if lhs.ty == IrType::Ptr || rhs.ty == IrType::Ptr {
                unsupported!("composite comparison not yet lowered");
            }
            ensure_type_match("compare lhs/rhs", lhs.ty, rhs.ty)?;
            let result_ty = lower_type(result_ty)?;
            if result_ty != IrType::Bool {
                return Err(LoweringError::InternalInvariantViolation {
                    detail: format!("comparison produced non-bool semantic type: {result_ty:?}"),
                });
            }
            let op = match op {
                Op::EqEq => CompareOp::Eq,
                Op::NotEq => CompareOp::Ne,
                Op::Lt => CompareOp::Lt,
                Op::LtEq => CompareOp::Le,
                Op::Gt => CompareOp::Gt,
                Op::GtEq => CompareOp::Ge,
                _ => unreachable!(),
            };
            active.emit(IrInst::Compare {
                dst,
                op,
                lhs: lhs.value,
                rhs: rhs.value,
            })?;
            Ok(LoweredValue {
                value: dst,
                ty: IrType::Bool,
            })
        }
        Op::Not => unreachable!("Op::Not is unary only"),
        Op::And | Op::Or => unreachable!("And/Or dispatched to lower_logical above"),
    }
}

/// Whether `expr` lowers to a three-state TBool value (vs a definite Bool),
/// determined structurally WITHOUT emitting IR — so `lower_logical` can route a
/// TBool operand to the Kleene path before committing to the definite-Bool
/// short-circuit CFG (which lowers rhs eagerly into its own block and would
/// reject a TBool there). TBool arises only from a `bool = ?` binding (read via
/// VarRef) or a Kleene `&&`/`||`/`!` over a TBool operand, so those are the only
/// shapes that propagate it.
fn expr_lowers_to_tbool(expr: &SemanticExpr, active: &ActiveBlock) -> bool {
    match &expr.kind {
        SemanticExprKind::VarRef { binding, .. } => active
            .bindings
            .get(binding)
            .is_some_and(|v| v.ty == IrType::TBool),
        SemanticExprKind::Unary { op: Op::Not, expr: inner, .. } => {
            expr_lowers_to_tbool(inner, active)
        }
        SemanticExprKind::Binary { lhs, op: Op::And | Op::Or, rhs, .. } => {
            expr_lowers_to_tbool(lhs, active) || expr_lowers_to_tbool(rhs, active)
        }
        _ => false,
    }
}

/// Emit the three-valued (Kleene) result of `a <op> b` on the I8 wire encoding
/// (false=0, true=1, unknown=2), matching the interpreter's tables exactly
/// (runtime/ops.rs:212/230). `op` is `And` or `Or`; `a`/`b` are operand wire
/// values (TBool, or a Bool promoted as wire). Returns a TBool. No `Select` IR
/// exists, so each result is selected by a small compare/branch/merge CFG.
///
/// AND: `0` if either operand is `0`; else `1` if both are `1`; else `2`.
///   With `p = a*b` (operands ∈ {0,1,2}): `p==0` ⟺ a factor is `0`; `p==1` ⟺
///   both are `1`. So `result = (p==0)?0 : (p==1)?1 : 2`.
/// OR:  `1` if either operand is `1`; else `0` if both are `0` (`a+b==0`); else `2`.
fn emit_kleene_and_or(
    op: Op,
    a: ValueId,
    b: ValueId,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let incoming = active.bindings.clone();
    let c0 = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: c0, ty: IrType::I8, value: 0 })?;
    let c1 = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: c1, ty: IrType::I8, value: 1 })?;
    let c2 = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: c2, ty: IrType::I8, value: 2 })?;

    let result_param = ctx.fresh_value();
    let merge = ctx.start_block(
        vec![BlockParam { value: result_param, ty: IrType::I8, read_only: false }],
        incoming.clone(),
    );
    let merge_id = merge.id();

    match op {
        Op::And => {
            let p = ctx.fresh_value();
            active.emit(IrInst::Binary { dst: p, op: BinaryOp::Mul, ty: IrType::I8, lhs: a, rhs: b })?;
            let is_p0 = ctx.fresh_value();
            active.emit(IrInst::Compare { dst: is_p0, op: CompareOp::Eq, lhs: p, rhs: c0 })?;
            let mut check1 = ctx.start_block(vec![], incoming.clone());
            let check1_id = check1.id();
            // p==0 → 0; else fall to the p==1 check. Seal decision first (the
            // sub-blocks read `p` / the consts defined here).
            active.terminate(IrTerminator::Branch {
                cond: is_p0,
                then_block: merge_id, then_args: vec![c0],
                else_block: check1_id, else_args: vec![],
            })?;
            let old = std::mem::replace(active, merge);
            ctx.seal_block(old)?;
            let is_p1 = ctx.fresh_value();
            check1.emit(IrInst::Compare { dst: is_p1, op: CompareOp::Eq, lhs: p, rhs: c1 })?;
            let mut leaf2 = ctx.start_block(vec![], incoming.clone());
            let leaf2_id = leaf2.id();
            check1.terminate(IrTerminator::Branch {
                cond: is_p1,
                then_block: merge_id, then_args: vec![c1],
                else_block: leaf2_id, else_args: vec![],
            })?;
            ctx.seal_block(check1)?;
            leaf2.terminate(IrTerminator::Jump { target: merge_id, args: vec![c2] })?;
            ctx.seal_block(leaf2)?;
        }
        Op::Or => {
            let is_a1 = ctx.fresh_value();
            active.emit(IrInst::Compare { dst: is_a1, op: CompareOp::Eq, lhs: a, rhs: c1 })?;
            let mut chk_b1 = ctx.start_block(vec![], incoming.clone());
            let chk_b1_id = chk_b1.id();
            active.terminate(IrTerminator::Branch {
                cond: is_a1,
                then_block: merge_id, then_args: vec![c1],
                else_block: chk_b1_id, else_args: vec![],
            })?;
            let old = std::mem::replace(active, merge);
            ctx.seal_block(old)?;
            let is_b1 = ctx.fresh_value();
            chk_b1.emit(IrInst::Compare { dst: is_b1, op: CompareOp::Eq, lhs: b, rhs: c1 })?;
            let mut chk_both0 = ctx.start_block(vec![], incoming.clone());
            let chk_both0_id = chk_both0.id();
            chk_b1.terminate(IrTerminator::Branch {
                cond: is_b1,
                then_block: merge_id, then_args: vec![c1],
                else_block: chk_both0_id, else_args: vec![],
            })?;
            ctx.seal_block(chk_b1)?;
            let sum = ctx.fresh_value();
            chk_both0.emit(IrInst::Binary { dst: sum, op: BinaryOp::Add, ty: IrType::I8, lhs: a, rhs: b })?;
            let is_sum0 = ctx.fresh_value();
            chk_both0.emit(IrInst::Compare { dst: is_sum0, op: CompareOp::Eq, lhs: sum, rhs: c0 })?;
            let mut leaf2 = ctx.start_block(vec![], incoming.clone());
            let leaf2_id = leaf2.id();
            chk_both0.terminate(IrTerminator::Branch {
                cond: is_sum0,
                then_block: merge_id, then_args: vec![c0],
                else_block: leaf2_id, else_args: vec![],
            })?;
            ctx.seal_block(chk_both0)?;
            leaf2.terminate(IrTerminator::Jump { target: merge_id, args: vec![c2] })?;
            ctx.seal_block(leaf2)?;
        }
        _ => unreachable!("emit_kleene_and_or only handles And/Or"),
    }

    // active is now the merge block; narrow the I8 wire result to TBool.
    let res = ctx.fresh_value();
    active.emit(IrInst::Cast { dst: res, from: IrType::I8, to: IrType::TBool, value: result_param })?;
    Ok(LoweredValue { value: res, ty: IrType::TBool })
}

/// Emit the three-valued (Kleene) `!a` on the I8 wire encoding: `!0=1`, `!1=0`,
/// `!2=2` (runtime/ops.rs:256). Unknown negates to unknown; a definite `0`/`1`
/// flips via `1 - a`. Returns a TBool.
fn emit_kleene_not(
    a: ValueId,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let incoming = active.bindings.clone();
    let c1 = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: c1, ty: IrType::I8, value: 1 })?;
    let c2 = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: c2, ty: IrType::I8, value: 2 })?;
    let is_a2 = ctx.fresh_value();
    active.emit(IrInst::Compare { dst: is_a2, op: CompareOp::Eq, lhs: a, rhs: c2 })?;

    let result_param = ctx.fresh_value();
    let merge = ctx.start_block(
        vec![BlockParam { value: result_param, ty: IrType::I8, read_only: false }],
        incoming.clone(),
    );
    let merge_id = merge.id();
    let mut not_def = ctx.start_block(vec![], incoming);
    let not_def_id = not_def.id();

    // unknown (2) negates to unknown; else flip via 1 - a (a is 0 or 1 here).
    active.terminate(IrTerminator::Branch {
        cond: is_a2,
        then_block: merge_id, then_args: vec![c2],
        else_block: not_def_id, else_args: vec![],
    })?;
    let old = std::mem::replace(active, merge);
    ctx.seal_block(old)?;

    let flipped = ctx.fresh_value();
    not_def.emit(IrInst::Binary { dst: flipped, op: BinaryOp::Sub, ty: IrType::I8, lhs: c1, rhs: a })?;
    not_def.terminate(IrTerminator::Jump { target: merge_id, args: vec![flipped] })?;
    ctx.seal_block(not_def)?;

    let res = ctx.fresh_value();
    active.emit(IrInst::Cast { dst: res, from: IrType::I8, to: IrType::TBool, value: result_param })?;
    Ok(LoweredValue { value: res, ty: IrType::TBool })
}

// Logical AND/OR short-circuit lowering
//
// `a && b` and `a || b` cannot be lowered as eager binary instructions
// because the right operand must not be evaluated when the left operand
// already determines the result.  Instead, we emit a three-block CFG:
//
//   [decision]           — current block; evaluates lhs; terminates with Branch
//       |    \
//   [rhs]  [sc]          — rhs: evaluates rhs; sc: emits the short-circuit constant
//       \    /
//     [merge(result)]    — receives the Bool result via a block parameter
//
// For AND: Branch(lhs, then=rhs, else=sc);  sc emits false (0).
// For OR:  Branch(lhs, then=sc, else=rhs);  sc emits true  (1).
//
// On return, *active has been replaced with the merge block so the caller
// can continue emitting instructions after the logical expression.
fn lower_logical(
    lhs: &SemanticExpr,
    op: Op,
    rhs: &SemanticExpr,
    result_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    ensure_type_match("logical result", IrType::Bool, lower_type(result_ty)?)?;
    let incoming = active.bindings.clone();

    // Evaluate the left operand in the current (decision) block.
    let lhs_val = lower_expr(lhs, ctx, active)?;
    // D2.x-A2: three-valued (Kleene) logic when EITHER operand is a three-state
    // TBool. Compute eagerly on the I8 wire encoding, matching the interpreter's
    // tables (runtime/ops.rs); both operands are treated as wire values, so a
    // Bool operand (`? && false`, `false && ?`) works as 0/1. The rhs type is
    // detected structurally (`expr_lowers_to_tbool`) WITHOUT lowering it, because
    // the definite-Bool short-circuit path below lowers rhs eagerly into its own
    // block and would reject a TBool there. Definite-Bool `&&`/`||` is untouched
    // (neither operand is TBool → falls through). The `==`/`!=` TBool refusal
    // stays in lower_binary — that's the seam's Path-B guard, not A2's.
    if lhs_val.ty == IrType::TBool || expr_lowers_to_tbool(rhs, active) {
        let rhs_val = lower_expr(rhs, ctx, active)?;
        return emit_kleene_and_or(op, lhs_val.value, rhs_val.value, ctx, active);
    }
    ensure_type_match("logical lhs", IrType::Bool, lhs_val.ty.clone())?;

    // Block that evaluates the right operand (reached when short-circuit does not fire).
    let mut rhs_active = ctx.start_block(vec![], incoming.clone());
    let rhs_block_id = rhs_active.id();

    // Block that produces the short-circuit constant (reached when lhs settles the result).
    let mut sc_active = ctx.start_block(vec![], incoming.clone());
    let sc_block_id = sc_active.id();

    // Merge block receives the Bool result from whichever path ran.
    let result_param = ctx.fresh_value();
    let merge_block = ctx.start_block(
        vec![BlockParam { value: result_param, ty: IrType::Bool, read_only: false }],
        incoming,
    );
    let merge_id = merge_block.id();

    // AND: true lhs → evaluate rhs; false lhs → short-circuit to false.
    // OR:  true lhs → short-circuit to true; false lhs → evaluate rhs.
    let (then_id, else_id) = match op {
        Op::And => (rhs_block_id, sc_block_id),
        Op::Or  => (sc_block_id,  rhs_block_id),
        _       => unreachable!(),
    };

    // Terminate the decision block and hand *active over to the merge block
    // so the caller continues emitting into the merge block.
    active.terminate(IrTerminator::Branch {
        cond: lhs_val.value,
        then_block: then_id,
        then_args: vec![],
        else_block: else_id,
        else_args: vec![],
    })?;
    let old_active = std::mem::replace(active, merge_block);
    ctx.seal_block(old_active)?;

    // Lower the right operand and jump to merge with its value.
    let rhs_val = lower_expr(rhs, ctx, &mut rhs_active)?;
    ensure_type_match("logical rhs", IrType::Bool, rhs_val.ty.clone())?;
    rhs_active.terminate(IrTerminator::Jump {
        target: merge_id,
        args: vec![rhs_val.value],
    })?;
    ctx.seal_block(rhs_active)?;

    // Emit the short-circuit constant and jump to merge.
    // AND short-circuits to false (0); OR short-circuits to true (1).
    let sc_const: i128 = match op {
        Op::And => 0,
        Op::Or  => 1,
        _       => unreachable!(),
    };
    let sc_dst = ctx.fresh_value();
    sc_active.emit(IrInst::ConstInt { dst: sc_dst, ty: IrType::Bool, value: sc_const })?;
    sc_active.terminate(IrTerminator::Jump {
        target: merge_id,
        args: vec![sc_dst],
    })?;
    ctx.seal_block(sc_active)?;

    Ok(LoweredValue { value: result_param, ty: IrType::Bool })
}

/// Emit guarded integer division (`op = Div`) or modulo (`op = Rem`) — the single
/// source for "guard a division" (trackers Gate-1a + Gate-1b). Raw Cranelift
/// `sdiv`/`srem` trap in hardware on two inputs; this branches around both so the
/// JIT matches the interpreter:
///
///   - `divisor == 0` (Gate-1b): the interpreter raises a clean `DivByZero`
///     error (R4: not overflow). Route to the `Trap` terminator, which since
///     Gate-1b0 calls `cx_trap` for a clean process exit (126) — NOT a hardware
///     fault. Applies to BOTH Div and Mod.
///   - `INT_MIN / -1` (Gate-1a, Div only): signed overflow; R4 rules it wraps to
///     `INT_MIN`. A `select` would still execute the faulting `sdiv`, so this is a
///     BRANCH that produces `MIN` without dividing, mirroring the interpreter's
///     `if b == -1 { a.wrapping_neg() }` (runtime/ops.rs:93). `MIN` is sourced
///     per-width from the int-facts leaf. Mod needs no INT_MIN guard — `srem`
///     already yields 0 for `INT_MIN % -1` (R4-confirmed), so Rem goes straight
///     to the divide block once the zero-check passes.
///
///   [decision]  is_zero = Eq(divisor, 0);  Branch(is_zero, trap, nonzero)
///       |    \
///    [trap]   \                              trap: clean cx_trap exit (Gate-1b0)
///              \
///           [nonzero]  — Div: the INT_MIN/-1 sub-guard below; Rem: = divide
///   (Div)   is_min → check_neg1 → min_blk(MIN) / divide ; merge ← MIN | quotient
///   (Rem)   divide: srem ; Jump merge(rem)
///     [merge(result)]  — receives the result via a block parameter
///
/// The per-width `MIN` (Div only) is read from `ty`'s int-facts leaf entry inside.
/// Non-integer (F64) div/mod and all other ops do not reach here (the caller
/// gates on `ty.int_width()`), so their IR is unchanged.
fn emit_guarded_idiv(
    dst: ValueId,
    ty: IrType,
    op: BinaryOp,
    dividend: ValueId,
    divisor: ValueId,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let incoming = active.bindings.clone();

    // ── Outer gate (Gate-1b): divisor == 0 → clean trap. ──────────────────────
    // Decision block (current `active`): is_zero = (divisor == 0).
    let zero_const = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: zero_const, ty: ty.clone(), value: 0 })?;
    let is_zero = ctx.fresh_value();
    active.emit(IrInst::Compare {
        dst: is_zero,
        op: CompareOp::Eq,
        lhs: divisor,
        rhs: zero_const,
    })?;

    // Clean-trap block for the zero path (Gate-1b0: Trap → cx_trap, exit 126).
    let mut trap_blk = ctx.start_block(vec![], incoming.clone());
    let trap_blk_id = trap_blk.id();

    // Shared merge (result param) and divide (sdiv/srem) blocks.
    let result_param = ctx.fresh_value();
    let merge_block = ctx.start_block(
        vec![BlockParam { value: result_param, ty: ty.clone(), read_only: false }],
        incoming.clone(),
    );
    let merge_id = merge_block.id();

    let mut divide = ctx.start_block(vec![], incoming.clone());
    let divide_id = divide.id();

    // Non-zero entry: Div gets the INT_MIN/-1 sub-guard (Gate-1a); Rem goes
    // straight to the divide (srem already handles INT_MIN % -1). The sub-guard
    // blocks are CREATED here (their ids feed the branch below) but FILLED only
    // after the decision block is sealed — they read dividend/divisor, which are
    // defined in the decision block and resolve cross-block only once it is
    // sealed (the seal-order Gate-1a relied on as well).
    let is_div = matches!(op, BinaryOp::Div);
    let mut check_min = if is_div {
        Some(ctx.start_block(vec![], incoming.clone()))
    } else {
        None
    };
    let check_min_id = check_min.as_ref().map(|b| b.id());
    let mut check_neg1 = if is_div {
        Some(ctx.start_block(vec![], incoming.clone()))
    } else {
        None
    };
    let check_neg1_id = check_neg1.as_ref().map(|b| b.id());
    let mut min_blk = if is_div {
        Some(ctx.start_block(vec![], incoming.clone()))
    } else {
        None
    };
    let min_blk_id = min_blk.as_ref().map(|b| b.id());

    let nonzero_entry = check_min_id.unwrap_or(divide_id);

    // Terminate + seal the decision block FIRST, so dividend/divisor resolve in
    // the divide and the cross-block sub-guards filled below.
    active.terminate(IrTerminator::Branch {
        cond: is_zero,
        then_block: trap_blk_id,
        then_args: vec![],
        else_block: nonzero_entry,
        else_args: vec![],
    })?;
    let old_active = std::mem::replace(active, merge_block);
    ctx.seal_block(old_active)?;

    // trap_blk: clean exit via cx_trap (Gate-1b0). Never reached for divisor != 0.
    trap_blk.terminate(IrTerminator::Trap)?;
    ctx.seal_block(trap_blk)?;

    // divide: the actual sdiv/srem — never reached for divisor 0 (or INT_MIN/-1
    // for Div). `op` is Div or Rem; both are well-typed at `ty`.
    divide.emit(IrInst::Binary {
        dst,
        op,
        ty: ty.clone(),
        lhs: dividend,
        rhs: divisor,
    })?;
    divide.terminate(IrTerminator::Jump { target: merge_id, args: vec![dst] })?;
    ctx.seal_block(divide)?;

    // Div-only INT_MIN/-1 sub-guard (Gate-1a), filled now that the decision block
    // is sealed: check_min → check_neg1 → min_blk(MIN) / divide.
    if let (Some(mut check_min), Some(mut check_neg1), Some(mut min_blk)) =
        (check_min.take(), check_neg1.take(), min_blk.take())
    {
        let check_neg1_id = check_neg1_id.expect("check_neg1 id present for Div");
        let min_blk_id = min_blk_id.expect("min_blk id present for Div");
        // Per-width INT_MIN from the int-facts leaf (Div reaches here only for an
        // integer `ty`, so int_width() is always Some).
        let min = ty
            .int_width()
            .expect("integer division requires an integer width")
            .facts()
            .min;

        // check_min: is_min = (dividend == MIN) → check_neg1 / divide.
        let min_const = ctx.fresh_value();
        check_min.emit(IrInst::ConstInt { dst: min_const, ty: ty.clone(), value: min })?;
        let is_min = ctx.fresh_value();
        check_min.emit(IrInst::Compare {
            dst: is_min,
            op: CompareOp::Eq,
            lhs: dividend,
            rhs: min_const,
        })?;
        check_min.terminate(IrTerminator::Branch {
            cond: is_min,
            then_block: check_neg1_id,
            then_args: vec![],
            else_block: divide_id,
            else_args: vec![],
        })?;
        ctx.seal_block(check_min)?;

        // check_neg1: reached only when dividend == MIN. is_neg1 = (divisor == -1).
        let neg1_const = ctx.fresh_value();
        check_neg1.emit(IrInst::ConstInt { dst: neg1_const, ty: ty.clone(), value: -1 })?;
        let is_neg1 = ctx.fresh_value();
        check_neg1.emit(IrInst::Compare {
            dst: is_neg1,
            op: CompareOp::Eq,
            lhs: divisor,
            rhs: neg1_const,
        })?;
        check_neg1.terminate(IrTerminator::Branch {
            cond: is_neg1,
            then_block: min_blk_id,
            then_args: vec![],
            else_block: divide_id,
            else_args: vec![],
        })?;
        ctx.seal_block(check_neg1)?;

        // min_blk: INT_MIN / -1 → MIN, without dividing. Re-emit MIN block-locally.
        let min_result = ctx.fresh_value();
        min_blk.emit(IrInst::ConstInt { dst: min_result, ty: ty.clone(), value: min })?;
        min_blk.terminate(IrTerminator::Jump { target: merge_id, args: vec![min_result] })?;
        ctx.seal_block(min_blk)?;
    }

    Ok(LoweredValue { value: result_param, ty })
}

/// Lower a `when` statement (chained-decision CFG modeled on lower_logical).
///
/// SCOPE BOUNDARY (Option A, 0.1):
///
/// The `unknown` arm in a TBool when matches via wire-value comparison: emit
/// ConstInt(I8, 2), Cast(I8 → scrutinee.ty), Compare(Eq, scrutinee, casted).
/// This is pattern-match-as-value, NOT unknown propagation.
///
/// Why ConstInt(I8) + Cast instead of ConstInt(TBool):
///   The IR validator (validate.rs:~294) does NOT accept TBool in ConstInt's
///   type set — only I8/I16/I32/I64/I128/Bool. This is deliberate scope-keeping:
///   adding TBool to ConstInt would commit the IR layer to a global TBool-as-
///   literal semantics that Option A explicitly defers. The two-instruction
///   workaround keeps the validator's accepted set unchanged. Both
///   Cast(I8 → Bool) and Cast(I8 → TBool) are Cranelift-no-ops since both
///   lower to `types::I8`, so this is a single bit-preserving move at machine
///   level.
///
/// Why no fixture exercises the wire-value match end-to-end:
///   No current source program can construct a scrutinee with TBool wire value
///   2 — the `?` expression literal lowers SemanticValue::Unknown through
///   lower_value, which rejects it (lower.rs:~2118). `?`-lowering is part of
///   the deferred Unknown-propagation design surface (cx_abi_v0.1.md:76-79).
///   So the unknown-arm IR shape is reachable at the lowering layer and
///   validator-clean, but unreachable from source until post-0.1 `?` lowering
///   lands. t144 covers structural validity (well-forms, doesn't fire on Bool);
///   true end-to-end wire-value exercise requires post-0.1 work.
///
/// Whether an `unknown` value flowing into arithmetic, comparison, or logical
/// operators infects the result is a separate language-design question (also
/// cx_abi_v0.1.md:76-79) and remains explicitly deferred post-0.1. Nothing in
/// this lowering commits the language to any particular unknown-propagation
/// semantics.
///
/// EnumVariant arms are rejected here with a structured error; that's blocked
/// on Enum IR lowering, separate work.
fn lower_when_stmt(
    scrutinee: &SemanticExpr,
    arms: &[SemanticWhenArm],
    ctx: &mut LoweringCtx,
    mut current: ActiveBlock,
    spec: &FunctionLoweringSpec,
    loop_ctx: &[LoopContext],
) -> Result<Option<ActiveBlock>, LoweringError> {
    let incoming = current.bindings.clone();
    let scrutinee_val = lower_expr(scrutinee, ctx, &mut current)?;

    let mut fallthroughs: Vec<ActiveBlock> = Vec::new();

    for arm in arms.iter() {
        // A whole-scrutinee `as v` binding is a pure SSA-level alias: the
        // scrutinee's already-lowered tag value is reused under the arm's
        // BindingId, scoped to this arm's body block only (enums are
        // tag-only, so there is no separate value to compute).
        let mut body_incoming = incoming.clone();
        if let SemanticWhenPattern::EnumVariant { binding: Some((id, _)), .. } = &arm.pattern {
            body_incoming.insert(*id, scrutinee_val.clone());
        }
        let body_block = ctx.start_block(vec![], body_incoming.clone());
        let body_id = body_block.id();

        match &arm.pattern {
            SemanticWhenPattern::Catchall => {
                match &arm.guard {
                    None => {
                        current.terminate(IrTerminator::Jump {
                            target: body_id,
                            args: vec![],
                        })?;
                        ctx.seal_block(current)?;
                        if let Some(active) = lower_stmt_sequence(
                            arm.body.iter(),
                            ctx,
                            Some(body_block),
                            spec,
                            loop_ctx,
                        )? {
                            fallthroughs.push(active);
                        }
                        // Arms after Catchall are unreachable — we don't allocate
                        // their body blocks. Merge whatever fallthroughs we have.
                        return match fallthroughs.len() {
                            0 => Ok(None),
                            1 => Ok(Some(fallthroughs.into_iter().next().unwrap())),
                            _ => Ok(Some(merge_fallthroughs(ctx, fallthroughs, &incoming)?)),
                        };
                    }
                    Some(guard_expr) => {
                        // A guarded catch-all can fail too — it's the terminal
                        // arm, so its "keep trying" continuation is exactly the
                        // same "no arm matched" tail the unguarded exhaustion
                        // fallthrough below already produces (out of scope to
                        // improve that tail behavior here).
                        let mut guard_block = ctx.start_block(vec![], body_incoming);
                        let guard_block_id = guard_block.id();
                        current.terminate(IrTerminator::Jump {
                            target: guard_block_id,
                            args: vec![],
                        })?;
                        ctx.seal_block(current)?;

                        let guard_val = lower_expr(guard_expr, ctx, &mut guard_block)?;
                        let guard_val = guard_unknown_condition(guard_val, ctx, &mut guard_block)?;
                        ensure_type_match("when guard", IrType::Bool, guard_val.ty.clone())?;

                        let no_match = ctx.start_block(vec![], incoming.clone());
                        let no_match_id = no_match.id();
                        guard_block.terminate(IrTerminator::Branch {
                            cond: guard_val.value,
                            then_block: body_id,
                            then_args: vec![],
                            else_block: no_match_id,
                            else_args: vec![],
                        })?;
                        ctx.seal_block(guard_block)?;

                        if let Some(active) = lower_stmt_sequence(
                            arm.body.iter(),
                            ctx,
                            Some(body_block),
                            spec,
                            loop_ctx,
                        )? {
                            fallthroughs.push(active);
                        }
                        fallthroughs.push(no_match);
                        return match fallthroughs.len() {
                            0 => Ok(None),
                            1 => Ok(Some(fallthroughs.into_iter().next().unwrap())),
                            _ => Ok(Some(merge_fallthroughs(ctx, fallthroughs, &incoming)?)),
                        };
                    }
                }
            }
            // Literal / Range / EnumVariant share one comparison emitter (D1.2a);
            // the stmt mechanism (fallthrough collection) stays local here.
            _ => {
                match &arm.guard {
                    None => {
                        current = emit_when_arm_match(
                            &arm.pattern,
                            scrutinee,
                            &scrutinee_val,
                            body_id,
                            &incoming,
                            current,
                            ctx,
                        )?;
                        if let Some(active) = lower_stmt_sequence(
                            arm.body.iter(),
                            ctx,
                            Some(body_block),
                            spec,
                            loop_ctx,
                        )? {
                            fallthroughs.push(active);
                        }
                    }
                    Some(guard_expr) => {
                        // Redirect the pattern-match's "then" target to an
                        // intermediate guard block (seeded with the same
                        // bindings as the body, so `as v` is visible to the
                        // guard) instead of the real body block. The guard's
                        // "false" edge reuses `current.id()` — the exact same
                        // "try the next arm" continuation the pattern-mismatch
                        // else-edge already produces.
                        let mut guard_block = ctx.start_block(vec![], body_incoming);
                        let guard_block_id = guard_block.id();
                        current = emit_when_arm_match(
                            &arm.pattern,
                            scrutinee,
                            &scrutinee_val,
                            guard_block_id,
                            &incoming,
                            current,
                            ctx,
                        )?;
                        let next_id = current.id();

                        let guard_val = lower_expr(guard_expr, ctx, &mut guard_block)?;
                        let guard_val = guard_unknown_condition(guard_val, ctx, &mut guard_block)?;
                        ensure_type_match("when guard", IrType::Bool, guard_val.ty.clone())?;

                        guard_block.terminate(IrTerminator::Branch {
                            cond: guard_val.value,
                            then_block: body_id,
                            then_args: vec![],
                            else_block: next_id,
                            else_args: vec![],
                        })?;
                        ctx.seal_block(guard_block)?;

                        if let Some(active) = lower_stmt_sequence(
                            arm.body.iter(),
                            ctx,
                            Some(body_block),
                            spec,
                            loop_ctx,
                        )? {
                            fallthroughs.push(active);
                        }
                    }
                }
            }
        }
    }

    // Loop completed without a Catchall: `current` is the tail "no arm matched"
    // block. It falls through to the merge as if it were a no-op arm body.
    fallthroughs.push(current);

    match fallthroughs.len() {
        0 => Ok(None),
        1 => Ok(Some(fallthroughs.into_iter().next().unwrap())),
        _ => Ok(Some(merge_fallthroughs(ctx, fallthroughs, &incoming)?)),
    }
}

/// Emit the comparison CFG for one non-catchall `when` arm (tracker D1.2a) — the
/// pattern->Compare judgment that `lower_when_stmt` and `lower_when_expr` used to
/// duplicate verbatim. On entry `current` is the active decision block; this
/// emits the compare(s), branches to `body_id` on match, and returns the "no
/// match" continuation block the next arm builds from. The caller owns the body
/// lowering (stmt's fallthrough vs expr's value-merge) — only the judgment is
/// shared, the mechanism stays local. Catchall is caller-handled (its control
/// flow differs between the two forms) and never reaches here.
fn emit_when_arm_match(
    pattern: &SemanticWhenPattern,
    scrutinee: &SemanticExpr,
    scrutinee_val: &LoweredValue,
    body_id: crate::ir::types::BlockId,
    incoming: &BindingMap,
    mut current: ActiveBlock,
    ctx: &mut LoweringCtx,
) -> Result<ActiveBlock, LoweringError> {
    match pattern {
        SemanticWhenPattern::Literal(value) => {
            let pat_value = if scrutinee_val.ty == IrType::TBool {
                // D2.x-A1: a three-state TBool scrutinee (a `bool = ?` unknown)
                // compares against TBool wire constants — false=0, true=1,
                // unknown=2 — each materialised as ConstInt(I8) + Cast→TBool
                // (the validator restricts ConstInt to non-TBool types, and
                // Compare requires matching operand types). This mirrors the
                // interpreter matching bool patterns against a TBool value
                // (runtime/exec.rs). A definite-Bool scrutinee keeps the path
                // below byte-for-byte (true/false via lower_value; unknown via
                // I8 + Cast→Bool), so t20 / t144 / every other `when` are unmoved.
                let wire: i128 = match value {
                    SemanticValue::Bool(false) => 0,
                    SemanticValue::Bool(true) => 1,
                    SemanticValue::Unknown => 2,
                    _ => unsupported!(
                        "non-bool when-pattern against a three-state bool scrutinee"
                    ),
                };
                let byte = ctx.fresh_value();
                current.emit(IrInst::ConstInt { dst: byte, ty: IrType::I8, value: wire })?;
                let wire_const = ctx.fresh_value();
                current.emit(IrInst::Cast {
                    dst: wire_const,
                    from: IrType::I8,
                    to: IrType::TBool,
                    value: byte,
                })?;
                wire_const
            } else if matches!(value, SemanticValue::Unknown) {
                // TBool wire-value match — see SCOPE BOUNDARY at lower_when_stmt doc.
                let unknown_byte = ctx.fresh_value();
                current.emit(IrInst::ConstInt {
                    dst: unknown_byte,
                    ty: IrType::I8,
                    value: 2,
                })?;
                let unknown_const = ctx.fresh_value();
                current.emit(IrInst::Cast {
                    dst: unknown_const,
                    from: IrType::I8,
                    to: scrutinee_val.ty.clone(),
                    value: unknown_byte,
                })?;
                unknown_const
            } else {
                let lowered = lower_value(value, &scrutinee.ty, ctx, &mut current)?;
                lowered.value
            };
            let cmp_dst = ctx.fresh_value();
            current.emit(IrInst::Compare {
                dst: cmp_dst,
                op: CompareOp::Eq,
                lhs: scrutinee_val.value,
                rhs: pat_value,
            })?;
            let next_block = ctx.start_block(vec![], incoming.clone());
            let next_id = next_block.id();
            current.terminate(IrTerminator::Branch {
                cond: cmp_dst,
                then_block: body_id,
                then_args: vec![],
                else_block: next_id,
                else_args: vec![],
            })?;
            ctx.seal_block(current)?;
            Ok(next_block)
        }
        SemanticWhenPattern::Range(low, high, inclusive) => {
            let lo_lowered = lower_value(low, &scrutinee.ty, ctx, &mut current)?;
            let lo_cmp = ctx.fresh_value();
            current.emit(IrInst::Compare {
                dst: lo_cmp,
                op: CompareOp::Ge,
                lhs: scrutinee_val.value,
                rhs: lo_lowered.value,
            })?;
            let mut lo_pass = ctx.start_block(vec![], incoming.clone());
            let lo_pass_id = lo_pass.id();
            let no_match = ctx.start_block(vec![], incoming.clone());
            let no_match_id = no_match.id();
            current.terminate(IrTerminator::Branch {
                cond: lo_cmp,
                then_block: lo_pass_id,
                then_args: vec![],
                else_block: no_match_id,
                else_args: vec![],
            })?;
            ctx.seal_block(current)?;

            let hi_lowered = lower_value(high, &scrutinee.ty, ctx, &mut lo_pass)?;
            let hi_op = if *inclusive { CompareOp::Le } else { CompareOp::Lt };
            let hi_cmp = ctx.fresh_value();
            lo_pass.emit(IrInst::Compare {
                dst: hi_cmp,
                op: hi_op,
                lhs: scrutinee_val.value,
                rhs: hi_lowered.value,
            })?;
            lo_pass.terminate(IrTerminator::Branch {
                cond: hi_cmp,
                then_block: body_id,
                then_args: vec![],
                else_block: no_match_id,
                else_args: vec![],
            })?;
            ctx.seal_block(lo_pass)?;
            Ok(no_match)
        }
        SemanticWhenPattern::EnumVariant { enum_name, variant_name, variant_id, .. } => {
            // Match the scrutinee's tag against this variant's tag (D2.2) — the
            // same `variant_id` the construct emits. Mirrors the Literal arm:
            // ConstInt(tag) → Compare(Eq) → Branch. The scrutinee is already the
            // enum's I8 tag.
            let tag = match variant_id {
                Some(id) => id.0,
                None => {
                    return Err(LoweringError::UnsupportedSemanticConstruct {
                        construct: format!(
                            "enum variant '{}::{}' has no resolved tag (variant_id)",
                            enum_name, variant_name
                        ),
                    })
                }
            };
            let pat_value = ctx.fresh_value();
            current.emit(IrInst::ConstInt {
                dst: pat_value,
                ty: scrutinee_val.ty.clone(),
                value: tag as i128,
            })?;
            let cmp_dst = ctx.fresh_value();
            current.emit(IrInst::Compare {
                dst: cmp_dst,
                op: CompareOp::Eq,
                lhs: scrutinee_val.value,
                rhs: pat_value,
            })?;
            let next_block = ctx.start_block(vec![], incoming.clone());
            let next_id = next_block.id();
            current.terminate(IrTerminator::Branch {
                cond: cmp_dst,
                then_block: body_id,
                then_args: vec![],
                else_block: next_id,
                else_args: vec![],
            })?;
            ctx.seal_block(current)?;
            Ok(next_block)
        }
        SemanticWhenPattern::Catchall => {
            // Catchall is caller-handled; it must never be routed through here.
            Err(LoweringError::InternalInvariantViolation {
                detail: "Catchall must be handled by the when caller, not emit_when_arm_match"
                    .to_string(),
            })
        }
    }
}

/// Lower a `when` expression (value-producing form). Same chained-decision CFG
/// as `lower_when_stmt`, but the merge block carries a single block parameter
/// of the result type, and each arm terminates with `Jump(merge, [arm_value])`.
///
/// Arms in expression position must end in an `ExprStmt` whose expression
/// produces the arm's value. Block-bodied expression-form arms (leading stmts
/// before the trailing expression) are accepted as long as the leading stmts
/// lower without diverging; if a leading stmt diverges, that arm contributes
/// no merge edge.
///
/// Same SCOPE BOUNDARY as `lower_when_stmt` applies. See that function's doc
/// for the wire-value match details and the post-0.1 deferred surface.
fn lower_when_expr(
    scrutinee: &SemanticExpr,
    arms: &[SemanticWhenArm],
    result_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    // Bare-literal arm tails type the when-expr as the unresolved `Numeric`
    // placeholder; resolve it to the default integer width like lower_binary /
    // lower_if_expr do (D2.1), so an enclosing typed context narrows the result.
    // Inert for concrete-typed when-exprs (the `else` is the prior behavior).
    let result_ir_ty = if *result_ty == SemanticType::Numeric {
        ctx.target.numeric_literal_ir_type()
    } else {
        lower_type(result_ty)?
    };
    let incoming = active.bindings.clone();
    let scrutinee_val = lower_expr(scrutinee, ctx, active)?;

    // Merge block: single block parameter typed as the result.
    let result_param = ctx.fresh_value();
    let merge_block = ctx.start_block(
        vec![BlockParam {
            value: result_param,
            ty: result_ir_ty.clone(),
            read_only: false,
        }],
        incoming.clone(),
    );
    let merge_id = merge_block.id();

    // Swap caller's `*active` to be the merge block up front. The chain is
    // built in `current` (a local), starting from what was originally `*active`.
    let mut current = std::mem::replace(active, merge_block);

    // Each arm body lowers to a value via the shared `lower_branch_value`
    // (extracted in D2.1 so if-expr branches reuse the same merge machinery —
    // leading stmts via the void path, trailing ExprStmt as the value, Jump to
    // merge with `ensure_type_match`). This thin wrapper keeps the per-arm call
    // sites unchanged, so the `when`-expr emitted IR is byte-identical.
    let lower_arm_body =
        |ctx: &mut LoweringCtx, body_active: ActiveBlock, arm: &SemanticWhenArm|
         -> Result<(), LoweringError> {
            lower_branch_value(&arm.body, &result_ir_ty, merge_id, body_active, ctx)
        };

    for arm in arms.iter() {
        // Same whole-scrutinee alias as the statement form — see the comment
        // in `lower_when_stmt`.
        let mut body_incoming = incoming.clone();
        if let SemanticWhenPattern::EnumVariant { binding: Some((id, _)), .. } = &arm.pattern {
            body_incoming.insert(*id, scrutinee_val.clone());
        }
        let body_block = ctx.start_block(vec![], body_incoming.clone());
        let body_id = body_block.id();

        match &arm.pattern {
            SemanticWhenPattern::Catchall => {
                match &arm.guard {
                    None => {
                        current.terminate(IrTerminator::Jump {
                            target: body_id,
                            args: vec![],
                        })?;
                        ctx.seal_block(current)?;
                        lower_arm_body(ctx, body_block, arm)?;
                        // Arms after Catchall are unreachable.
                        return Ok(LoweredValue {
                            value: result_param,
                            ty: result_ir_ty,
                        });
                    }
                    Some(guard_expr) => {
                        // A guarded catch-all can fail too. A value-producing
                        // `when` must be exhaustive, so its "keep trying"
                        // path has no value to supply the merge — Trap, the
                        // exact same tail behavior the unguarded exhaustion
                        // fallthrough below already produces.
                        let mut guard_block = ctx.start_block(vec![], body_incoming);
                        let guard_block_id = guard_block.id();
                        current.terminate(IrTerminator::Jump {
                            target: guard_block_id,
                            args: vec![],
                        })?;
                        ctx.seal_block(current)?;

                        let guard_val = lower_expr(guard_expr, ctx, &mut guard_block)?;
                        let guard_val = guard_unknown_condition(guard_val, ctx, &mut guard_block)?;
                        ensure_type_match("when guard", IrType::Bool, guard_val.ty.clone())?;

                        let mut no_match = ctx.start_block(vec![], incoming.clone());
                        let no_match_id = no_match.id();
                        guard_block.terminate(IrTerminator::Branch {
                            cond: guard_val.value,
                            then_block: body_id,
                            then_args: vec![],
                            else_block: no_match_id,
                            else_args: vec![],
                        })?;
                        ctx.seal_block(guard_block)?;

                        lower_arm_body(ctx, body_block, arm)?;

                        no_match.terminate(IrTerminator::Trap)?;
                        ctx.seal_block(no_match)?;

                        return Ok(LoweredValue {
                            value: result_param,
                            ty: result_ir_ty,
                        });
                    }
                }
            }
            // Literal / Range / EnumVariant share one comparison emitter (D1.2a);
            // the expr mechanism (value-producing arm body -> merge) stays local.
            _ => {
                match &arm.guard {
                    None => {
                        current = emit_when_arm_match(
                            &arm.pattern,
                            scrutinee,
                            &scrutinee_val,
                            body_id,
                            &incoming,
                            current,
                            ctx,
                        )?;
                        lower_arm_body(ctx, body_block, arm)?;
                    }
                    Some(guard_expr) => {
                        let mut guard_block = ctx.start_block(vec![], body_incoming);
                        let guard_block_id = guard_block.id();
                        current = emit_when_arm_match(
                            &arm.pattern,
                            scrutinee,
                            &scrutinee_val,
                            guard_block_id,
                            &incoming,
                            current,
                            ctx,
                        )?;
                        let next_id = current.id();

                        let guard_val = lower_expr(guard_expr, ctx, &mut guard_block)?;
                        let guard_val = guard_unknown_condition(guard_val, ctx, &mut guard_block)?;
                        ensure_type_match("when guard", IrType::Bool, guard_val.ty.clone())?;

                        guard_block.terminate(IrTerminator::Branch {
                            cond: guard_val.value,
                            then_block: body_id,
                            then_args: vec![],
                            else_block: next_id,
                            else_args: vec![],
                        })?;
                        ctx.seal_block(guard_block)?;

                        lower_arm_body(ctx, body_block, arm)?;
                    }
                }
            }
        }
    }

    // Loop completed without Catchall: a value-producing when must be
    // exhaustive. The tail block has no arm value to supply to the merge.
    // Emit Trap on the no-arm-matched path so reaching it is a runtime abort
    // rather than passing an undefined value to the merge.
    current.terminate(IrTerminator::Trap)?;
    ctx.seal_block(current)?;

    Ok(LoweredValue {
        value: result_param,
        ty: result_ir_ty,
    })
}

/// Lower a value-producing branch body — a `when`-expr arm or an if-expr branch
/// (tracker D2.1) — and `Jump` to `merge_id` with its value. Leading statements
/// lower via the void path; the trailing `ExprStmt` is the branch value, type-
/// checked against `result_ir_ty`. If the body diverges before the trailing
/// expression (a leading stmt that never returns control), no merge edge is
/// added — that branch simply contributes nothing to the merge.
fn lower_branch_value(
    body: &[SemanticStmt],
    result_ir_ty: &IrType,
    merge_id: crate::ir::types::BlockId,
    mut body_active: ActiveBlock,
    ctx: &mut LoweringCtx,
) -> Result<(), LoweringError> {
    if body.is_empty() {
        return Err(LoweringError::InternalInvariantViolation {
            detail: "value-producing branch body is empty".to_string(),
        });
    }
    let last_idx = body.len() - 1;
    // Lower leading stmts in-place. Minimal spec — a value-producing branch
    // shouldn't contain a Return statement; if it does, this errors (→ SKIP).
    for stmt in &body[..last_idx] {
        let inline_spec = FunctionLoweringSpec {
            name: "<branch>".to_string(),
            return_ty: None,
            allow_return_stmt: false,
            ret_slot: None,
        };
        let result = lower_stmt(stmt, ctx, body_active, &inline_spec, &[])?;
        body_active = match result {
            Some(active) => active,
            None => return Ok(()), // diverged — no merge edge
        };
    }
    let trailing_expr = match &body[last_idx] {
        SemanticStmt::ExprStmt { expr, .. } => expr,
        other => {
            return Err(LoweringError::UnsupportedSemanticConstruct {
                construct: format!(
                    "value-producing branch must end in an expression, got {:?}",
                    std::mem::discriminant(other)
                ),
            });
        }
    };
    let value = lower_expr(trailing_expr, ctx, &mut body_active)?;
    ensure_type_match("branch result", result_ir_ty.clone(), value.ty)?;
    body_active.terminate(IrTerminator::Jump {
        target: merge_id,
        args: vec![value.value],
    })?;
    ctx.seal_block(body_active)?;
    Ok(())
}

/// Lower an `if`-EXPRESSION to a value (tracker D2.1, #046). Structurally a
/// two-branch value-producing `when`: branch on the definite condition, each
/// branch produces its trailing-expression value, both merging through one
/// result block parameter — the same machinery `lower_when_expr` uses, via the
/// shared `lower_branch_value`.
///
///   [decision]  Branch(cond, then_block, else_block)
///   [then] lower_branch_value(then_body) → Jump merge(v)
///   [else] lower_branch_value(else_body) → Jump merge(v)
///   [merge(result)]
///
/// `else if` is a nested `If` in `else_body`, handled by recursion through
/// `lower_branch_value`'s trailing-expr lowering. The #026 unknown-condition
/// case is unreachable here — an unknown value SKIPs at `lower_value` before any
/// if (audit D2.1) — so the condition always lowers to a definite Bool.
/// If `cond` is a three-state TBool, trap (`cx_trap`) when its wire value is the
/// unknown state (2) — the interpreter's `UnknownCondition` for an `if` /
/// `if`-expression (#026 / #046) — then narrow the now-definite 0/1 value to
/// Bool for the branch (the IR validator requires `Branch` conditions to be
/// Bool). A definite Bool condition is returned unchanged with **no** emitted IR,
/// so every existing `if` lowers byte-identically. Modeled on `emit_bounds_check`
/// (trap-branch + continue block, seal-decision-first).
fn guard_unknown_condition(
    cond: LoweredValue,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    if cond.ty != IrType::TBool {
        return Ok(cond);
    }
    let incoming = active.bindings.clone();

    // Decision block (current `active`): is the condition the unknown wire (2)?
    let unknown_byte = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: unknown_byte, ty: IrType::I8, value: 2 })?;
    let unknown_const = ctx.fresh_value();
    active.emit(IrInst::Cast {
        dst: unknown_const,
        from: IrType::I8,
        to: IrType::TBool,
        value: unknown_byte,
    })?;
    let is_unknown = ctx.fresh_value();
    active.emit(IrInst::Compare {
        dst: is_unknown,
        op: CompareOp::Eq,
        lhs: cond.value,
        rhs: unknown_const,
    })?;

    let mut trap_blk = ctx.start_block(vec![], incoming.clone());
    let trap_id = trap_blk.id();
    let cont = ctx.start_block(vec![], incoming);
    let cont_id = cont.id();

    // Seal the decision block first (seal-order rule): `cont` reads `cond` from it.
    active.terminate(IrTerminator::Branch {
        cond: is_unknown,
        then_block: trap_id,
        then_args: vec![],
        else_block: cont_id,
        else_args: vec![],
    })?;
    let old_active = std::mem::replace(active, cont);
    ctx.seal_block(old_active)?;

    // trap: clean cx_trap exit (shared with the Gate-1 / Gate-2 guards).
    trap_blk.terminate(IrTerminator::Trap)?;
    ctx.seal_block(trap_blk)?;

    // continue: the value is now known 0/1 — narrow TBool → Bool for the branch.
    let cond_bool = ctx.fresh_value();
    active.emit(IrInst::Cast {
        dst: cond_bool,
        from: IrType::TBool,
        to: IrType::Bool,
        value: cond.value,
    })?;
    Ok(LoweredValue { value: cond_bool, ty: IrType::Bool })
}

fn lower_if_expr(
    condition: &SemanticExpr,
    then_body: &[SemanticStmt],
    else_body: &[SemanticStmt],
    result_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    // #046 types the if-expr as its then-branch's trailing type, which for a
    // bare integer literal (`if c { 100 } else { 5 }`) is the unresolved
    // `Numeric` placeholder. Resolve it to the target's default integer width
    // (I64 on 64-bit) — the same rule lower_binary uses — so the merge param is
    // concrete; an enclosing typed context narrows the I64 result as usual.
    let result_ir_ty = if *result_ty == SemanticType::Numeric {
        ctx.target.numeric_literal_ir_type()
    } else {
        lower_type(result_ty)?
    };
    let incoming = active.bindings.clone();

    // Lower the condition in the decision block (current `active`).
    let cond = lower_expr(condition, ctx, active)?;
    // D2.x-A1: an unknown (TBool) condition traps (cx_trap) — #046 reuses #026.
    // No-op for a definite Bool condition; narrows a definite TBool to Bool.
    let cond = guard_unknown_condition(cond, ctx, active)?;

    // Merge block carries the single result block parameter.
    let result_param = ctx.fresh_value();
    let merge_block = ctx.start_block(
        vec![BlockParam { value: result_param, ty: result_ir_ty.clone(), read_only: false }],
        incoming.clone(),
    );
    let merge_id = merge_block.id();

    // then / else branch blocks.
    let then_block = ctx.start_block(vec![], incoming.clone());
    let then_id = then_block.id();
    let else_block = ctx.start_block(vec![], incoming.clone());
    let else_id = else_block.id();

    // Terminate + seal the decision block FIRST (seal-order rule): the branch
    // bodies read `cond`/other values defined in the decision block.
    active.terminate(IrTerminator::Branch {
        cond: cond.value,
        then_block: then_id,
        then_args: vec![],
        else_block: else_id,
        else_args: vec![],
    })?;
    let old_active = std::mem::replace(active, merge_block);
    ctx.seal_block(old_active)?;

    // Lower each branch to a value merging into the result param.
    lower_branch_value(then_body, &result_ir_ty, merge_id, then_block, ctx)?;
    lower_branch_value(else_body, &result_ir_ty, merge_id, else_block, ctx)?;

    Ok(LoweredValue { value: result_param, ty: result_ir_ty })
}

/// D2.4b: lower `expr?` (the Try operator) on a packed-i128 Result. Unpack the
/// operand (D2.4a `result_unpack`), then branch on the tag: Ok continues with the
/// payload narrowed back to T (reverse of the construct's widen), Err early-returns
/// the WHOLE original Result i128, byte-identical, from the enclosing function.
///
/// The semantic analyser guarantees `?` only appears inside a Result-returning
/// function and that the operand is a Result<T> (semantic.rs rejects t85/t86), so
/// the Err early-return of the i128 is type-correct without consulting the return
/// type here, and `inner` always lowers to an i128.
///
/// `EarlyReturn` reuses the plain `Return` terminator — zero overlap with the
/// unknown-`?`/TBool machinery (the unknown arc is independent, audit-confirmed).
fn lower_try(
    inner_expr: &SemanticExpr,
    result_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    // Word-payload fence (R7): unwrapping a NESTED Result (T is itself a Result,
    // so lower_type(T) == I128) exceeds the word payload — stays SKIP, exactly as
    // the D2.4a construct does. Scalar/str `?` is the target.
    let t_ir = lower_type(result_ty)?;
    if matches!(t_ir, IrType::I128) {
        return Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "? on a nested Result (word-payload limit, R7-deferred)".to_string(),
        });
    }

    // Lower the Result operand and unpack it into (tag, payload word).
    let result = lower_expr(inner_expr, ctx, active)?;
    let (tag, payload) = result_unpack(result.value, ctx, active)?;

    // Ok payload narrowed back to T (i64 word -> T). Identity when T is already I64.
    let unwrapped = if t_ir == IrType::I64 {
        payload
    } else {
        let dst = ctx.fresh_value();
        active.emit(IrInst::Cast { dst, from: IrType::I64, to: t_ir.clone(), value: payload })?;
        dst
    };

    // is_ok = (tag == 0).
    let zero = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: zero, ty: IrType::I64, value: 0 })?;
    let is_ok = ctx.fresh_value();
    active.emit(IrInst::Compare { dst: is_ok, op: CompareOp::Eq, lhs: tag, rhs: zero })?;

    let incoming = active.bindings.clone();
    let cont_block = ctx.start_block(vec![], incoming.clone());
    let cont_id = cont_block.id();
    let mut err_block = ctx.start_block(vec![], incoming);
    let err_id = err_block.id();

    // Seal the decision block first: the Err block reads `result.value` and the
    // continuation reads `unwrapped`, both defined here (they dominate both edges).
    active.terminate(IrTerminator::Branch {
        cond: is_ok,
        then_block: cont_id,
        then_args: vec![],
        else_block: err_id,
        else_args: vec![],
    })?;
    let old = std::mem::replace(active, cont_block);
    ctx.seal_block(old)?;

    // Err: early-return the whole original Result i128, unchanged.
    err_block.terminate(IrTerminator::Return { value: Some(result.value) })?;
    ctx.seal_block(err_block)?;

    // Ok: `active` is now the continuation; the `?`-expression's value is the
    // unwrapped payload at type T.
    Ok(LoweredValue { value: unwrapped, ty: t_ir })
}

// Struct field access lowering strategy
//
// Both reads (DotAccess expressions) and writes (DotAccess l-values in Assign /
// CompoundAssign) share the same pointer-resolution logic:
//
//   1. Resolve the container binding to its SSA value (a Ptr produced by a
//      prior Alloca when the struct was created).
//
//   2. Look up the struct layout from the pre-built struct_table.  The struct
//      type name is carried directly in `struct_name` (populated by the
//      semantic analyser); no run-time type lookup is needed.
//
//   3. Compute the field's byte offset from the layout.  If the offset is
//      greater than zero, emit IrInst::PtrOffset to advance the base pointer
//      to the field's address.  If the offset is zero the base pointer itself
//      already addresses the first field.
//
// `resolve_field_ptr` encapsulates steps 1–3 and returns the field pointer
// ValueId together with the field's IR type.  Callers then emit either a
// Load (for reads) or a Store (for writes).
//
// Field reads produce at most two instructions (PtrOffset + Load).
// Field writes produce at most two instructions (PtrOffset + Store).
// CompoundAssign field writes produce at most four (PtrOffset + Load + Binary + Store).

/// Resolve the address of a struct field, emitting `PtrOffset` when needed.
///
/// Returns `(field_ptr, field_ir_ty)` where `field_ptr` is the ValueId of the
/// pointer to the field and `field_ir_ty` is its IR type.  The caller is
/// responsible for emitting the Load or Store that actually accesses the field.
fn resolve_field_ptr(
    binding: &Option<BindingId>,
    container: &str,
    field: &str,
    struct_name: &str,
    field_sem_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<(ValueId, IrType), LoweringError> {
    // 1. Resolve the container binding to get the struct pointer.
    let binding_id = binding.ok_or_else(|| LoweringError::InternalInvariantViolation {
        detail: format!(
            "field access on '{container}.{field}' has no binding; \
             the semantic analyser must supply one for all lowerable field accesses"
        ),
    })?;

    let base = active.bindings.get(&binding_id).cloned().ok_or_else(|| {
        LoweringError::InternalInvariantViolation {
            detail: format!(
                "binding for '{container}' (id {}) not found in scope for field access '{container}.{field}'",
                binding_id.0
            ),
        }
    })?;

    if base.ty != IrType::Ptr {
        return Err(LoweringError::InternalInvariantViolation {
            detail: format!(
                "field access on '{container}': expected Ptr-typed binding, got {:?}",
                base.ty
            ),
        });
    }

    // 2. Look up the struct layout.
    if struct_name.is_empty() {
        return Err(LoweringError::UnresolvedSemanticArtifact {
            artifact: format!(
                "struct type for '{container}' is unknown; \
                 field access '{container}.{field}' cannot be lowered"
            ),
        });
    }
    let info = ctx.struct_table.get(struct_name).cloned().ok_or_else(|| {
        LoweringError::UnresolvedSemanticArtifact {
            artifact: format!("struct '{struct_name}' in field access '{container}.{field}'"),
        }
    })?;

    // 3. Find the field index, offset, and IR type.
    let field_idx = info
        .fields
        .iter()
        .position(|(name, _)| name == field)
        .ok_or_else(|| LoweringError::UnresolvedSemanticArtifact {
            artifact: format!("field '{field}' on struct '{struct_name}'"),
        })?;

    let field_ir_ty = info.fields[field_idx].1.clone();
    let field_offset = info.layout.field_offsets[field_idx];

    // Verify that the semantic field type agrees with what the struct table says.
    let expected_ir_ty = lower_type(field_sem_ty)?;
    if expected_ir_ty != field_ir_ty {
        return Err(LoweringError::InternalInvariantViolation {
            detail: format!(
                "field access '{container}.{field}': IR type mismatch — \
                 semantic layer says {expected_ir_ty:?}, struct layout says {field_ir_ty:?}"
            ),
        });
    }

    // 4. Advance the pointer to the field's address (skip if offset is 0).
    let field_ptr = if field_offset > 0 {
        let fp = ctx.fresh_value();
        active.emit(IrInst::PtrOffset {
            dst: fp,
            base: base.value,
            offset: field_offset,
        })?;
        fp
    } else {
        base.value
    };

    Ok((field_ptr, field_ir_ty))
}

/// Emit an array bounds check (tracker Gate-2b): trap cleanly via `cx_trap`
/// (exit 126, Gate-1b0) when `idx` is outside `[0, count)`, matching the
/// interpreter's `IndexOutOfBounds` error. On return `*active` is the in-bounds
/// continuation block — the caller emits the element address/access there.
///
/// `idx` is a signed I64, and the IR has only signed compares, so a single
/// `idx >= count` would NOT catch a negative index. Two checks, nested so either
/// routes to the one trap block:
///
///   [decision]  idx < 0 ?      → trap : check_hi
///   [check_hi]  idx >= count ? → trap : continue
///   [trap] cx_trap            [continue] caller emits PtrAdd + load/store
///
/// Decision is sealed before `continue` is filled (the Gate-1b seal-order rule):
/// `idx` and the array base are defined in the decision block and resolve in the
/// dominated continuation only once it is sealed.
fn emit_bounds_check(
    idx: ValueId,
    count: usize,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<(), LoweringError> {
    let incoming = active.bindings.clone();

    // Decision block (current `active`): idx < 0 ?
    let zero = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: zero, ty: IrType::I64, value: 0 })?;
    let idx_neg = ctx.fresh_value();
    active.emit(IrInst::Compare {
        dst: idx_neg,
        op: CompareOp::Lt,
        lhs: idx,
        rhs: zero,
    })?;

    let mut trap_blk = ctx.start_block(vec![], incoming.clone());
    let trap_blk_id = trap_blk.id();
    let mut check_hi = ctx.start_block(vec![], incoming.clone());
    let check_hi_id = check_hi.id();
    let cont = ctx.start_block(vec![], incoming.clone());
    let cont_id = cont.id();

    // Seal the decision block first, then fill the dependent blocks.
    active.terminate(IrTerminator::Branch {
        cond: idx_neg,
        then_block: trap_blk_id,
        then_args: vec![],
        else_block: check_hi_id,
        else_args: vec![],
    })?;
    let old_active = std::mem::replace(active, cont);
    ctx.seal_block(old_active)?;

    // trap: clean cx_trap exit (shared with Gate-1's guards).
    trap_blk.terminate(IrTerminator::Trap)?;
    ctx.seal_block(trap_blk)?;

    // check_hi: idx >= count ? → trap : continue.
    let count_const = ctx.fresh_value();
    check_hi.emit(IrInst::ConstInt {
        dst: count_const,
        ty: IrType::I64,
        value: count as i128,
    })?;
    let idx_ge = ctx.fresh_value();
    check_hi.emit(IrInst::Compare {
        dst: idx_ge,
        op: CompareOp::Ge,
        lhs: idx,
        rhs: count_const,
    })?;
    check_hi.terminate(IrTerminator::Branch {
        cond: idx_ge,
        then_block: trap_blk_id,
        then_args: vec![],
        else_block: cont_id,
        else_args: vec![],
    })?;
    ctx.seal_block(check_hi)?;

    // `*active` is now the `continue` block; the caller emits the access there.
    Ok(())
}

// Array element write lowering strategy
//
// An array element write `arr:[i] = value` where `arr: Array(N, ElemTy)` is
// lowered to:
//
//   1. Lower `arr` to a base Ptr (the Alloca produced when the array was
//      created).
//
//   2. Lower `i` to an integer SSA value; cast to I64 if needed.
//
//   3. Emit ConstInt(stride) then Binary(Mul, I64, i_i64, stride) to compute
//      the byte offset at runtime.
//
//   4. Emit PtrAdd(base, byte_offset) to advance the pointer.
//
//   5. Caller emits Store(elem_ptr, value) to write the element.
//
// `resolve_array_element_ptr` encapsulates steps 1–4 and returns the element
// pointer ValueId together with the element's IR type.  The caller is
// responsible for emitting the Store (for simple writes) or the Load + Binary
// + Store sequence (for compound-assign writes).

/// Resolve the address of an array element, emitting the index arithmetic.
///
/// Returns `(elem_ptr, elem_ir_ty)` where `elem_ptr` is the ValueId of the
/// pointer to the element and `elem_ir_ty` is its IR type.  The caller is
/// responsible for emitting Load/Store as appropriate.
fn resolve_array_element_ptr(
    target: &SemanticExpr,
    index: &SemanticExpr,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<(ValueId, IrType), LoweringError> {
    // 1. Derive element IR type from the array target's declared type.
    let (count, elem_sem_ty) = match &target.ty {
        SemanticType::Array(count, elem_ty) => (*count, elem_ty.as_ref()),
        _ => {
            return Err(LoweringError::InternalInvariantViolation {
                detail: format!(
                    "array element write: target has non-Array type: {:?}",
                    target.ty
                ),
            })
        }
    };

    let elem_ir_ty = lower_type(elem_sem_ty)?;
    let layout = compute_array_layout(&elem_ir_ty, count);

    // 2. Lower the array target to a base pointer.
    let base = lower_expr(target, ctx, active)?;
    if base.ty != IrType::Ptr {
        return Err(LoweringError::InternalInvariantViolation {
            detail: format!(
                "array element write: target must lower to Ptr, got {:?}",
                base.ty
            ),
        });
    }

    // 3. Lower the index expression; cast to I64 if it isn't already.
    let idx = lower_expr(index, ctx, active)?;
    let idx_i64 = if idx.ty == IrType::I64 {
        idx.value
    } else {
        let cast_dst = ctx.fresh_value();
        active.emit(IrInst::Cast {
            dst: cast_dst,
            from: idx.ty.clone(),
            to: IrType::I64,
            value: idx.value,
        })?;
        cast_dst
    };

    // 3b. Bounds-check the index before computing its address (Gate-2b).
    // OOB traps cleanly via cx_trap; `*active` becomes the in-bounds block.
    emit_bounds_check(idx_i64, count, ctx, active)?;

    // 4. Compute byte_offset = idx_i64 * stride.
    let stride_val = ctx.fresh_value();
    active.emit(IrInst::ConstInt {
        dst: stride_val,
        ty: IrType::I64,
        value: layout.stride as i128,
    })?;
    let byte_offset = ctx.fresh_value();
    active.emit(IrInst::Binary {
        dst: byte_offset,
        op: BinaryOp::Mul,
        ty: IrType::I64,
        lhs: idx_i64,
        rhs: stride_val,
    })?;

    // 5. elem_ptr = base + byte_offset  (runtime pointer arithmetic).
    let elem_ptr = ctx.fresh_value();
    active.emit(IrInst::PtrAdd {
        dst: elem_ptr,
        base: base.value,
        offset: byte_offset,
    })?;

    Ok((elem_ptr, elem_ir_ty))
}

fn lower_dot_access(
    binding: &Option<BindingId>,
    container: &str,
    field: &str,
    struct_name: &str,
    field_sem_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let (field_ptr, field_ir_ty) =
        resolve_field_ptr(binding, container, field, struct_name, field_sem_ty, ctx, active)?;

    let dst = ctx.fresh_value();
    active.emit(IrInst::Load {
        dst,
        ty: field_ir_ty.clone(),
        ptr: field_ptr,
    })?;

    Ok(LoweredValue {
        value: dst,
        ty: field_ir_ty,
    })
}

// Array literal lowering strategy
//
// An array literal `[e0, e1, e2, ...]` of type `Array(N, ElemTy)` is lowered
// to a stack-allocated block in the same style as struct literals:
//
//   1. Alloca: reserve N * stride bytes at element alignment.
//
//   2. For each element at position i (in source order, which is also storage
//      order):
//      a. Lower the element expression to a scalar IR value.
//      b. Compute the byte offset: i * stride.
//      c. If offset > 0, emit PtrOffset to advance the base pointer.
//      d. Emit Store to write the element value.
//
//   3. Return the base Alloca pointer as IrType::Ptr.
//
// All offsets are compile-time constants (element index is literal position),
// so PtrOffset suffices here — PtrAdd is only needed for runtime index access.
fn lower_array_lit(
    elements: &[SemanticExpr],
    array_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let (count, elem_sem_ty) = match array_ty {
        SemanticType::Array(count, elem_ty) => (*count, elem_ty.as_ref()),
        _ => {
            return Err(LoweringError::InternalInvariantViolation {
                detail: format!("ArrayLit expression has non-Array type: {:?}", array_ty),
            })
        }
    };

    if count == 0 {
        return Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "ArrayLit with zero-length array".to_string(),
        });
    }

    if elements.len() != count {
        return Err(LoweringError::InternalInvariantViolation {
            detail: format!(
                "ArrayLit: declared length {} but {} elements provided",
                count,
                elements.len()
            ),
        });
    }

    // Numeric and Unknown are placeholder types used by the semantic layer for
    // untyped integer literals.  Map both to the target's default integer width
    // (I64 on 64-bit) so arrays of untyped literals (e.g. [10, 20, 30]) lower
    // cleanly regardless of which placeholder the semantic pass emitted.
    let elem_ir_ty = if matches!(elem_sem_ty, SemanticType::Numeric | SemanticType::Unknown) {
        ctx.target.numeric_literal_ir_type()
    } else {
        lower_type(elem_sem_ty)?
    };

    // 1. ArrayAlloca: reserve stack space for the entire array.
    let ptr = ctx.fresh_value();
    active.emit(IrInst::ArrayAlloca {
        dst: ptr,
        element_type: elem_ir_ty.clone(),
        count,
    })?;

    let layout = compute_array_layout(&elem_ir_ty, count);

    // 2. Store each element at its stride-aligned byte offset.
    for (i, elem_expr) in elements.iter().enumerate() {
        let lowered_elem = lower_expr(elem_expr, ctx, active)?;
        let byte_offset = i * layout.stride;

        let elem_ptr = if byte_offset == 0 {
            ptr
        } else {
            let fp = ctx.fresh_value();
            active.emit(IrInst::PtrOffset {
                dst: fp,
                base: ptr,
                offset: byte_offset,
            })?;
            fp
        };

        // Narrow the element to the array's element width before storing
        // (tracker Gate-2a). `IrInst::Store` carries no width — it stores at the
        // value's type. The element expression lowers at its computed width
        // (i64 for an integer literal), so without this an i64 store into a
        // sub-64-bit slot overruns by up to 7 bytes, corrupting adjacent stack.
        // The cast truncates to the element's low bytes (the correct value); the
        // read path sign-extends on load. A no-op when widths already match
        // (i64 elements, t64 arrays).
        let store_value = if lowered_elem.ty != elem_ir_ty {
            let narrowed = ctx.fresh_value();
            active.emit(IrInst::Cast {
                dst: narrowed,
                from: lowered_elem.ty.clone(),
                to: elem_ir_ty.clone(),
                value: lowered_elem.value,
            })?;
            narrowed
        } else {
            lowered_elem.value
        };

        active.emit(IrInst::Store {
            ptr: elem_ptr,
            value: store_value,
        })?;
    }

    Ok(LoweredValue {
        value: ptr,
        ty: IrType::Ptr,
    })
}

// Array element read lowering strategy
//
// An index expression `arr[i]` where `arr: Array(N, ElemTy)` is lowered to:
//
//   1. Lower `arr` to a base Ptr (the Alloca produced when the array was created).
//
//   2. Lower `i` to an integer SSA value; cast to I64 if needed.
//
//   3. Emit ConstInt(stride) then Binary(Mul, I64, i_i64, stride) to compute
//      the byte offset at runtime.
//
//   4. Emit PtrAdd(base, byte_offset) to advance the pointer.
//
//   5. Emit Load(ElemIrTy, elem_ptr) to read the element value.
//
// PtrAdd (not PtrOffset) is used here because the index is a runtime value.
// The runtime-constant stride is folded into the multiply operand; a later
// constant-folding pass may eliminate the ConstInt + Binary pair when the
// index expression is itself a literal.
fn lower_index(
    target: &SemanticExpr,
    index: &SemanticExpr,
    elem_sem_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    // Derive element IR type and layout from the array target's type.
    let (count, declared_elem_sem_ty) = match &target.ty {
        SemanticType::Array(count, elem_ty) => (*count, elem_ty.as_ref()),
        _ => {
            return Err(LoweringError::InternalInvariantViolation {
                detail: format!(
                    "Index target has non-Array type: {:?}",
                    target.ty
                ),
            })
        }
    };

    // Numeric and Unknown are placeholder types the semantic layer uses for
    // unresolved or partially-resolved array element positions.  Map both to
    // the target's default integer width (I64 on 64-bit) to allow array
    // programs that mix declared and literal types to lower cleanly.
    let elem_ir_ty = if matches!(declared_elem_sem_ty, SemanticType::Numeric | SemanticType::Unknown) {
        ctx.target.numeric_literal_ir_type()
    } else {
        lower_type(declared_elem_sem_ty)?
    };
    let layout = compute_array_layout(&elem_ir_ty, count);

    // Verify the outer expression type is consistent with the element type.
    // When the outer type is Unknown the semantic layer could not resolve it;
    // skip the check and use the declared element type directly.
    if !matches!(elem_sem_ty, SemanticType::Numeric | SemanticType::Unknown) {
        let outer_ir_ty = lower_type(elem_sem_ty)?;
        if outer_ir_ty != elem_ir_ty {
            return Err(LoweringError::InternalInvariantViolation {
                detail: format!(
                    "Index expression type {:?} does not match array element type {:?}",
                    outer_ir_ty, elem_ir_ty
                ),
            });
        }
    }

    // 1. Lower the array target to get the base pointer.
    let base = lower_expr(target, ctx, active)?;
    if base.ty != IrType::Ptr {
        return Err(LoweringError::InternalInvariantViolation {
            detail: format!("Index target must lower to Ptr, got {:?}", base.ty),
        });
    }

    // 2. Lower the index expression; cast to I64 if it isn't already.
    let idx = lower_expr(index, ctx, active)?;
    let idx_i64 = if idx.ty == IrType::I64 {
        idx.value
    } else {
        let cast_dst = ctx.fresh_value();
        active.emit(IrInst::Cast {
            dst: cast_dst,
            from: idx.ty.clone(),
            to: IrType::I64,
            value: idx.value,
        })?;
        cast_dst
    };

    // 2b. Bounds-check the index before computing its address (Gate-2b).
    // OOB traps cleanly via cx_trap; `*active` becomes the in-bounds block.
    emit_bounds_check(idx_i64, count, ctx, active)?;

    // 3. Compute byte_offset = idx_i64 * stride.
    let stride_val = ctx.fresh_value();
    active.emit(IrInst::ConstInt {
        dst: stride_val,
        ty: IrType::I64,
        value: layout.stride as i128,
    })?;
    let byte_offset = ctx.fresh_value();
    active.emit(IrInst::Binary {
        dst: byte_offset,
        op: BinaryOp::Mul,
        ty: IrType::I64,
        lhs: idx_i64,
        rhs: stride_val,
    })?;

    // 4. elem_ptr = base + byte_offset  (runtime pointer arithmetic).
    let elem_ptr = ctx.fresh_value();
    active.emit(IrInst::PtrAdd {
        dst: elem_ptr,
        base: base.value,
        offset: byte_offset,
    })?;

    // 5. Load the element value.
    let dst = ctx.fresh_value();
    active.emit(IrInst::Load {
        dst,
        ty: elem_ir_ty.clone(),
        ptr: elem_ptr,
    })?;

    Ok(LoweredValue {
        value: dst,
        ty: elem_ir_ty,
    })
}

fn lower_type(ty: &SemanticType) -> Result<IrType, LoweringError> {
    match ty {
        SemanticType::I8 => Ok(IrType::I8),
        SemanticType::I16 => Ok(IrType::I16),
        SemanticType::I32 => Ok(IrType::I32),
        SemanticType::I64 => Ok(IrType::I64),
        SemanticType::I128 => Ok(IrType::I128),
        SemanticType::F64 => Ok(IrType::F64),
        SemanticType::Bool => Ok(IrType::Bool),
        SemanticType::Numeric => { unsupported_type!("Numeric") },
        SemanticType::Unknown => { unsupported_type!("Unknown") },
        // D2.5a: Handle{slot,gen} (two u32s, unsigned) packs into a single i64
        // (`slot | (gen << 32)`, no sign-extension risk — unlike Result's i128,
        // no enable_llvm_abi_extensions flag is needed for i64 returns/params).
        // The semantic layer's declared/inferred inner T is hardcoded to I128
        // regardless of the real construct-site payload (Finding 1, tracked, not
        // fixed here) — but that's irrelevant to THIS mapping: a Handle is an
        // opaque reference into the registry, never an inline T, so the OUTER
        // Handle(_) always lowers to I64 no matter what the ignored inner claims.
        SemanticType::Handle(_) => Ok(IrType::I64),
        SemanticType::StrRef => { unsupported_type!("StrRef") },
        SemanticType::Container => { unsupported_type!("Container") },
        // D2.3b: a `str` is rep (a) — a Ptr to a static {ptr, len} descriptor.
        // Reuses the struct/array Ptr convention for bindings, params, and
        // returns. (StrRef stays unsupported — borrowed views are a later step.)
        SemanticType::Str => Ok(IrType::Ptr),
        SemanticType::Char => { unsupported_type!("Char") },
        // Tag-only enums (no payloads in 0.3) erase to the u8 tag's IR type
        // (D2.2). Exhaustiveness is enforced at semantic time, not in the IR.
        SemanticType::Enum(_) => Ok(IrType::I8),
        SemanticType::TypeParam(_) => { unsupported_type!("TypeParam") },
        SemanticType::Struct(_) => Ok(IrType::Ptr),
        SemanticType::Array(_, _) => Ok(IrType::Ptr),
        // D2.4a: a `Result<T>` (Ok(T) | Err(str)) is a packed i128 — tag in the
        // high 64 (Ok=0/Err=1), payload word in the low 64. Single SSA value,
        // returnable, heap-free. WORD-PAYLOAD-ONLY: the payload is a scalar or a
        // static Ptr (Err-str). This rep does NOT generalize to nested/large
        // payloads — `Result<Result<T>>` (R7-deferred) needs a different rep and
        // SKIPs at the construct's `widen_payload_to_i64` (do not force it here).
        SemanticType::Result(_) => Ok(IrType::I128),
        // Void is not a storable type — it is only valid in return position,
        // where `lower_return_type` canonicalises it to `None`.  Letting Void
        // leak into params, block params, locals, or SSA values would violate
        // the non-storable-void invariant.
        SemanticType::Void => Err(LoweringError::InternalInvariantViolation {
            detail: "SemanticType::Void is not storable; use lower_return_type for return-type slots".to_string(),
        }),
    }
}

/// Lower a return-type slot.  `SemanticType::Void` canonicalises to `None`
/// (the absence of a return value); all other types delegate to `lower_type`.
///
/// This is the **only** path that may legitimately accept `SemanticType::Void`.
/// `IrFunction::return_ty` and `FunctionSignature::return_ty` are
/// `Option<IrType>` where `None` already encodes void; `IrType::Void` must
/// never appear in either slot.
fn lower_return_type(ty: &SemanticType) -> Result<Option<IrType>, LoweringError> {
    match ty {
        SemanticType::Void => Ok(None),
        _ => Ok(Some(lower_type(ty)?)),
    }
}

/// Route a struct return through the caller-allocated slot: field-wise copy
/// from `src` (a Ptr to the result struct — a local alloca, a param, wherever
/// it lives) into the hidden slot param, then return the SLOT pointer. For
/// non-struct-returning functions this is a no-op passthrough. The copy uses
/// the same Load/PtrOffset/Store idiom as struct-literal lowering, so no new
/// instruction shapes are involved and nothing wider than a word moves.
fn emit_return_through_slot(
    spec: &FunctionLoweringSpec,
    src: ValueId,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<ValueId, LoweringError> {
    let Some((slot, ref struct_name)) = spec.ret_slot else {
        return Ok(src);
    };
    let layout_info = ctx.struct_table.get(struct_name).cloned().ok_or_else(|| {
        LoweringError::UnresolvedSemanticArtifact {
            artifact: format!("struct type '{}' for return slot", struct_name),
        }
    })?;
    for (field_idx, (_fname, field_ir_ty)) in layout_info.fields.iter().enumerate() {
        let field_offset = layout_info.layout.field_offsets[field_idx];
        let src_ptr = if field_offset == 0 {
            src
        } else {
            let fp = ctx.fresh_value();
            active.emit(IrInst::PtrOffset { dst: fp, base: src, offset: field_offset })?;
            fp
        };
        let val = ctx.fresh_value();
        active.emit(IrInst::Load { dst: val, ty: field_ir_ty.clone(), ptr: src_ptr })?;
        let dst_ptr = if field_offset == 0 {
            slot
        } else {
            let fp = ctx.fresh_value();
            active.emit(IrInst::PtrOffset { dst: fp, base: slot, offset: field_offset })?;
            fp
        };
        active.emit(IrInst::Store { ptr: dst_ptr, value: val })?;
    }
    Ok(slot)
}

fn ensure_type_match(context: &str, expected: IrType, got: IrType) -> Result<(), LoweringError> {
    if expected == got {
        Ok(())
    } else {
        Err(LoweringError::InternalInvariantViolation {
            detail: format!(
                "{context} type mismatch after semantic analysis: expected {expected:?}, got {got:?}"
            ),
        })
    }
}

// ---------------------------------------------------------------------------
// Phase 9 sub-packet 3 — assert / assert_eq lowering
// ---------------------------------------------------------------------------
//
// Both builtins are lowered to a two-branch CFG pattern:
//
//   [current block]
//     ... condition computation ...
//     Branch { cond, then: pass_block, else: trap_block }
//
//   [pass_block]          ← execution continues here after a passing assertion
//     (empty — caller receives this as the new current block)
//
//   [trap_block]
//     Trap                ← abort; maps to Cranelift `trap` in the JIT backend
//
// Supported condition types:
//   - Bool    → used directly as the branch condition
//   - I8/I16/I32/I64 → compared != 0 to produce Bool (truthy-integer assert)
//   - Bool/I8/I16/I32/I64 for assert_eq → compared with Eq to produce Bool
//
// Unsupported types (e.g. Ptr, F64, StrRef) produce a structured
// UnsupportedSemanticConstruct error so the caller gets a clear diagnostic.

/// Route a print argument to the appropriate runtime intrinsic.
///
/// Returns the SSA value to pass and the callee name to invoke:
///   - I64 passes through directly to `cx_printn`
///   - I8/I16/I32 sign-extend to I64 via `IrInst::Cast` and call `cx_printn`
///   - Bool/TBool pass through unmodified to `cx_print_bool` (formats true/false
///     to match the interpreter's `print_value` Bool behavior; convergent stdout)
///   - F64, I128, Ptr, Str, and composites are not yet supported — returns None,
///     the caller emits a structured error.
///
/// This is the single edit point for adding more (callee, value) routing
/// branches when post-0.1 work adds f64/string/composite print intrinsics.
fn route_print_arg(
    value: ValueId,
    value_ty: IrType,
    ctx: &mut LoweringCtx,
    current: &mut ActiveBlock,
) -> Result<Option<(ValueId, &'static str)>, LoweringError> {
    match value_ty {
        IrType::I64 => Ok(Some((value, "cx_printn"))),
        IrType::I8 | IrType::I16 | IrType::I32 => {
            let dst = ctx.fresh_value();
            current.emit(IrInst::Cast {
                dst,
                from: value_ty.clone(),
                to: IrType::I64,
                value,
            })?;
            Ok(Some((dst, "cx_printn")))
        }
        IrType::Bool | IrType::TBool => Ok(Some((value, "cx_print_bool"))),
        _ => Ok(None),
    }
}

/// Lower `printn(n)` to `IrInst::Call { callee: "cx_printn", args: [i64_value] }`.
///
/// `printn` is a Cx builtin that prints an integer to stdout.  At the IR level
/// it lowers to a call to the `cx_printn` runtime intrinsic, which is pre-declared
/// in the JIT module as an imported C-ABI symbol.
///
/// Narrow integer widths (I8/I16/I32) and Bool/TBool are widened to I64 at the
/// boundary via `widen_to_i64_for_print` (mirrors the semantic-phase narrowing
/// idiom for typed sinks).  F64, I128, Ptr, Str, and composite types produce a
/// structured error.
fn lower_printn_stmt(
    args: &[SemanticCallArg],
    ctx: &mut LoweringCtx,
    mut current: ActiveBlock,
) -> Result<Option<ActiveBlock>, LoweringError> {
    if args.len() != 1 {
        return Err(LoweringError::InternalInvariantViolation {
            detail: format!("printn expects 1 argument, got {}", args.len()),
        });
    }
    let arg_expr = match &args[0] {
        SemanticCallArg::Expr(e) => e,
        _ => {
            return Err(LoweringError::UnsupportedSemanticConstruct {
                construct: "non-Expr argument to printn".to_string(),
            });
        }
    };
    let arg = lower_expr(arg_expr, ctx, &mut current)?;
    let (routed_value, callee) = route_print_arg(arg.value, arg.ty.clone(), ctx, &mut current)?
        .ok_or_else(|| LoweringError::UnsupportedSemanticConstruct {
            construct: format!(
                "printn argument must be a numeric integer (I8/I16/I32/I64) or Bool, got {:?} — F64, I128, Str, and composite types not yet supported",
                arg.ty
            ),
        })?;
    current.emit(IrInst::Call {
        dst: None,
        callee: callee.to_string(),
        args: vec![routed_value],
        return_ty: None,
    })?;
    Ok(Some(current))
}

/// Lower `print(n)` or `println(n)` to `IrInst::Call { callee: "cx_printn", args: [i64_value] }`.
///
/// Both `print` and `println` in Cx print a value followed by a newline.  Narrow
/// integer widths (I8/I16/I32) and Bool/TBool are widened to I64 at the boundary
/// via `widen_to_i64_for_print`.  F64, I128, Ptr, Str, and composite types
/// produce a structured error.
/// D2.3d: collect one statement's binding NAME → BindingId pairs (recursing into
/// nested bodies) into the flat interpolation map. Last-wins for repeats.
fn collect_binding_names_stmt(stmt: &SemanticStmt, map: &mut HashMap<String, BindingId>) {
    match stmt {
        SemanticStmt::Decl { binding, name, .. }
        | SemanticStmt::TypedAssign { binding, name, .. }
        | SemanticStmt::ConstDecl { binding, name, .. } => {
            map.insert(name.clone(), *binding);
        }
        SemanticStmt::For { binding, var, body, .. } => {
            map.insert(var.clone(), *binding);
            for s in body {
                collect_binding_names_stmt(s, map);
            }
        }
        SemanticStmt::Block { stmts, .. }
        | SemanticStmt::While { body: stmts, .. }
        | SemanticStmt::Loop { body: stmts, .. }
        | SemanticStmt::WhileIn { body: stmts, .. } => {
            for s in stmts {
                collect_binding_names_stmt(s, map);
            }
        }
        SemanticStmt::IfElse { then_body, else_ifs, else_body, .. } => {
            for s in then_body {
                collect_binding_names_stmt(s, map);
            }
            for (_, b) in else_ifs {
                for s in b {
                    collect_binding_names_stmt(s, map);
                }
            }
            if let Some(b) = else_body {
                for s in b {
                    collect_binding_names_stmt(s, map);
                }
            }
        }
        SemanticStmt::When { arms, .. } => {
            for arm in arms {
                for s in &arm.body {
                    collect_binding_names_stmt(s, map);
                }
            }
        }
        _ => {}
    }
}

/// D2.3d: a `{...}` segment is a bare-variable reference iff its trimmed content
/// is a non-empty identifier — mirrors runtime/print.rs `is_bare_identifier` (#038).
fn is_bare_identifier(s: &str) -> bool {
    let mut chars = s.chars();
    match chars.next() {
        Some(c) if c.is_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_alphanumeric() || c == '_')
}

enum InterpPiece {
    Lit(String),
    Ref(String),
}

/// D2.3d: split an interpolated literal into literal-text and `{identifier}`
/// pieces, mirroring runtime/print.rs `expand_interpolation` exactly (#038: bare
/// identifiers only; an unclosed `{` is a literal brace). Returns `None` if a
/// closed `{...}` is NOT a bare identifier — the interpreter raises
/// BadInterpolation there, so the JIT SKIPs rather than mis-rendering.
fn parse_interpolation(s: &str) -> Option<Vec<InterpPiece>> {
    let mut pieces = Vec::new();
    let mut lit = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '{' {
            let mut name = String::new();
            let mut closed = false;
            for inner in chars.by_ref() {
                if inner == '}' {
                    closed = true;
                    break;
                }
                name.push(inner);
            }
            if closed {
                let trimmed = name.trim();
                if is_bare_identifier(trimmed) {
                    if !lit.is_empty() {
                        pieces.push(InterpPiece::Lit(std::mem::take(&mut lit)));
                    }
                    pieces.push(InterpPiece::Ref(trimmed.to_string()));
                } else {
                    return None;
                }
            } else {
                lit.push('{');
                lit.push_str(&name);
            }
        } else {
            lit.push(c);
        }
    }
    if !lit.is_empty() {
        pieces.push(InterpPiece::Lit(lit));
    }
    Some(pieces)
}

/// D2.3d: pick the no-newline inline print intrinsic for an interpolated value by
/// its IR type, widening narrow ints to I64 (mirrors `route_print_arg`).
fn interp_value_print(
    val: &LoweredValue,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<(ValueId, &'static str), LoweringError> {
    match val.ty {
        IrType::I64 => Ok((val.value, "cx_printn_inline")),
        IrType::I8 | IrType::I16 | IrType::I32 => {
            let dst = ctx.fresh_value();
            active.emit(IrInst::Cast {
                dst,
                from: val.ty.clone(),
                to: IrType::I64,
                value: val.value,
            })?;
            Ok((dst, "cx_printn_inline"))
        }
        IrType::Bool | IrType::TBool => Ok((val.value, "cx_print_bool_inline")),
        IrType::F64 => Ok((val.value, "cx_print_f64_inline")),
        // A Ptr in interpolation is a str descriptor (the only Ptr value a bare
        // identifier resolves to in practice; composites aren't interpolated).
        IrType::Ptr => Ok((val.value, "cx_print_str_inline")),
        _ => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: format!("string interpolation of unsupported type {:?}", val.ty),
        }),
    }
}

/// D2.3d: lower `print("…{x}…")` as a print SEQUENCE — print-time interpolation,
/// NO new string built (R6 untouched). Each literal chunk and each resolved `{x}`
/// prints via a no-newline inline intrinsic; one `cx_print_newline` closes the
/// line (matching the interpreter's single-newline `print`).
///
/// FILED LIMITATION (D2.3d flat map): `{x}` resolves via the function-wide
/// `binding_names` map (last-wins). This DIVERGES from the interpreter's
/// scope-correct resolution on two corners — (1) a shadowed name interpolated
/// after the inner block exits (resolves the inner binding; the interpreter sees
/// the outer), and (2) use-before-declaration (the interpreter errors #038; the
/// map resolves a later declaration). No fixture exercises these; it is the
/// accepted tradeoff tied to the OPEN interpolation-scoping question (a future
/// scope-threaded map or semantic pre-resolution closes it). A shadowing /
/// use-before-decl interpolation bug belongs on THIS note, not a fresh mystery.
fn lower_interpolated_print(
    s: &str,
    ctx: &mut LoweringCtx,
    mut current: ActiveBlock,
) -> Result<Option<ActiveBlock>, LoweringError> {
    let pieces = parse_interpolation(s).ok_or_else(|| {
        LoweringError::UnsupportedSemanticConstruct {
            construct: "string interpolation with a non-identifier `{...}` not supported"
                .to_string(),
        }
    })?;
    for piece in &pieces {
        match piece {
            InterpPiece::Lit(text) => {
                let desc = construct_static_str(text, ctx, &mut current)?;
                current.emit(IrInst::Call {
                    dst: None,
                    callee: "cx_print_str_inline".to_string(),
                    args: vec![desc.value],
                    return_ty: None,
                })?;
            }
            InterpPiece::Ref(name) => {
                let binding = ctx.binding_names.get(name).copied().ok_or_else(|| {
                    LoweringError::UnsupportedSemanticConstruct {
                        construct: format!("string interpolation: `{}` not resolvable at lowering", name),
                    }
                })?;
                let val = current.bindings.get(&binding).cloned().ok_or_else(|| {
                    LoweringError::UnsupportedSemanticConstruct {
                        construct: format!("string interpolation: `{}` not in scope at this print", name),
                    }
                })?;
                let (arg_val, callee) = interp_value_print(&val, ctx, &mut current)?;
                current.emit(IrInst::Call {
                    dst: None,
                    callee: callee.to_string(),
                    args: vec![arg_val],
                    return_ty: None,
                })?;
            }
        }
    }
    current.emit(IrInst::Call {
        dst: None,
        callee: "cx_print_newline".to_string(),
        args: vec![],
        return_ty: None,
    })?;
    Ok(Some(current))
}

/// D2.4a: widen a Result payload to the i64 word slot. Scalars sign/zero-extend
/// via the existing Cast; a Ptr (Err-str descriptor) reinterprets to i64. An
/// I128 payload (nested Result) is REFUSED — the word-payload rep can't hold it
/// (R7-deferred); this is the SKIP that fences `Result<Result<T>>`.
fn widen_payload_to_i64(
    payload: &LoweredValue,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<ValueId, LoweringError> {
    match payload.ty {
        IrType::I64 => Ok(payload.value),
        IrType::I8 | IrType::I16 | IrType::I32 | IrType::Ptr => {
            let dst = ctx.fresh_value();
            active.emit(IrInst::Cast {
                dst,
                from: payload.ty.clone(),
                to: IrType::I64,
                value: payload.value,
            })?;
            Ok(dst)
        }
        _ => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: format!(
                "Result payload type {:?} exceeds the word-payload slot (nested/large Result is R7-deferred)",
                payload.ty
            ),
        }),
    }
}

/// D2.5a: widen a Handle payload to the i64 word `cx_handle_new` takes. Scope is
/// DELIBERATELY narrower than Result's `widen_payload_to_i64` — scalars only,
/// `{I8, I16, I32, I64, Bool}` — with two exclusions that are NOT mere gaps:
///
/// - `Ptr` (this includes `str`, whose rep is a descriptor Ptr) is excluded even
///   though it's mechanically word-fittable: the semantic layer's `Handle<T>`
///   claim is hardcoded to I128 regardless of the real payload (Finding 1), so a
///   later `.val` read site has no way to know a returned i64 is "a raw scalar"
///   vs. "a Ptr to a string descriptor" — accepting Ptr here would silently set
///   up that landmine for D2.5b to trip over.
/// - `F64` is excluded because it's actively unsafe with this mechanism, not
///   just unsupported: `Cast(F64->I64)` lowers to `fcvt_to_sint_sat` (a NUMERIC
///   float-to-int conversion), which would silently store the integer part of
///   the float (e.g. `3` for `3.14`) instead of preserving its bits.
///
/// `Bool` widens via the same `Cast` the backend already uses for Bool/TBool
/// widening (`uextend`, zero-extension — no sign-extension risk, confirmed in
/// the D2.5a investigation even though Result's own helper never exercised it).
fn widen_handle_payload_to_i64(
    payload: &LoweredValue,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<ValueId, LoweringError> {
    match payload.ty {
        IrType::I64 => Ok(payload.value),
        IrType::I8 | IrType::I16 | IrType::I32 | IrType::Bool => {
            let dst = ctx.fresh_value();
            active.emit(IrInst::Cast {
                dst,
                from: payload.ty.clone(),
                to: IrType::I64,
                value: payload.value,
            })?;
            Ok(dst)
        }
        _ => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: format!(
                "Handle payload type {:?} is not a scalar D2.5a supports (Ptr/str would recreate Finding 1's landmine; F64 would corrupt via fcvt_to_sint_sat; TBool/Result/nested Handle are separately deferred)",
                payload.ty
            ),
        }),
    }
}

/// D2.5a: lower `Handle.new(v)` — CONSTRUCT ONLY (`.val`/`.drop` land in D2.5b,
/// which have their own open question: a bare-identifier read site has no
/// data-flow back to the `.new` that produced it, unlike this construct site,
/// which can inspect its own inner expression's real lowered type directly).
/// Widens `v`'s real lowered type (never the semantic layer's hardcoded
/// `Handle<I128>` claim) via `widen_handle_payload_to_i64`, calls the stateful
/// `cx_handle_new` host callback, and returns the packed `Handle{slot,gen}` i64.
/// A non-scalar payload rejects cleanly (SKIP) rather than miscompiling.
fn lower_handle_new(
    value: &SemanticExpr,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let payload = lower_expr(value, ctx, active)?;
    let widened = widen_handle_payload_to_i64(&payload, ctx, active)?;
    let dst = ctx.fresh_value();
    active.emit(IrInst::Call {
        dst: Some(dst),
        callee: "cx_handle_new".to_string(),
        args: vec![widened],
        return_ty: Some(IrType::I64),
    })?;
    Ok(LoweredValue { value: dst, ty: IrType::I64 })
}

/// D2.5b: lower `h.val` — READ. Investigation-confirmed finding: the semantic
/// layer's `HandleVal` claim is hardcoded to `SemanticType::I128` (same landmine
/// as `HandleNew`'s outer-Handle claim), but UNLIKE the construct site, this
/// claim is NOT protected by a T-blind `lower_type` collapse — every reachable
/// consumer (untyped assign, typed-narrowing assign via a semantic-layer-inserted
/// `Cast{from: I128, ...}`, a direct `I128`-typed assign) strictly requires
/// `ensure_type_match` to see an ACTUAL `IrType::I128` value here — there is no
/// cast-insertion anywhere downstream to paper over a bare i64. So this produces
/// a genuine I128, not a raw i64: read the packed-i64 payload via the stateful
/// `cx_handle_val` host callback, branch on its out-parameter validity flag
/// (Trap on invalid — mirrors the interpreter's `RuntimeError::StaleHandle`,
/// deterministic and non-panicking, not byte-identical text), then widen the
/// payload to I128 via the same generic `Cast` (sign-extend) D2.5a's
/// `widen_handle_payload_to_i64` uses in reverse. Sign-extension composes
/// correctly across the two widening stages (I64 was itself sign/zero-extended
/// from the original scalar in `HandleNew`) — a Bool's 0/1 value is always
/// non-negative, so a second-stage sextend is bit-identical to a uextend for it.
fn lower_handle_val(
    binding: BindingId,
    name: &str,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let handle_val = active.bindings.get(&binding).cloned().ok_or_else(|| {
        LoweringError::InternalInvariantViolation {
            detail: format!(
                "HandleVal: binding '{name}' ({}) not found in scope",
                binding.0
            ),
        }
    })?;

    // Out-parameter: a scratch i8 slot (Result's proven Alloca/Store/Load
    // pattern, reused for a 1-byte out-param instead of a 16-byte round-trip).
    let out_valid_slot = ctx.fresh_value();
    active.emit(IrInst::Alloca { dst: out_valid_slot, size: 1, align: 1 })?;

    let payload = ctx.fresh_value();
    active.emit(IrInst::Call {
        dst: Some(payload),
        callee: "cx_handle_val".to_string(),
        args: vec![handle_val.value, out_valid_slot],
        return_ty: Some(IrType::I64),
    })?;

    let valid = ctx.fresh_value();
    active.emit(IrInst::Load { dst: valid, ty: IrType::I8, ptr: out_valid_slot })?;
    let zero = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: zero, ty: IrType::I8, value: 0 })?;
    let is_valid = ctx.fresh_value();
    active.emit(IrInst::Compare { dst: is_valid, op: CompareOp::Ne, lhs: valid, rhs: zero })?;

    let incoming = active.bindings.clone();
    let cont_block = ctx.start_block(vec![], incoming.clone());
    let cont_id = cont_block.id();
    let mut trap_block = ctx.start_block(vec![], incoming);
    let trap_id = trap_block.id();

    // Seal the decision block first (lower_try's pattern): the continuation
    // reads `payload`, defined here, dominating both edges.
    active.terminate(IrTerminator::Branch {
        cond: is_valid,
        then_block: cont_id,
        then_args: vec![],
        else_block: trap_id,
        else_args: vec![],
    })?;
    let old = std::mem::replace(active, cont_block);
    ctx.seal_block(old)?;

    // Invalid: the same clean Trap -> cx_trap exit path assert/when-exhaustiveness
    // use (Gate-1b0) — deterministic non-zero exit, not a panic. No continuation.
    trap_block.terminate(IrTerminator::Trap)?;
    ctx.seal_block(trap_block)?;

    // Valid: `active` is now the continuation. Widen the i64 payload to I128.
    let widened = ctx.fresh_value();
    active.emit(IrInst::Cast {
        dst: widened,
        from: IrType::I64,
        to: IrType::I128,
        value: payload,
    })?;
    Ok(LoweredValue { value: widened, ty: IrType::I128 })
}

/// D2.5c: lower `h.drop()`. A pure side effect — no branching, no fallibility to
/// signal: `cx_handle_drop` calls the shared registry's `remove()`, which
/// safely no-ops on an already-dropped or invalid handle (the same generation
/// check `.val` relies on returns early before touching the free-list again —
/// this is what makes a double-drop non-corrupting, not new logic here).
///
/// `HandleDrop`'s semantic type is `Handle(Box<I128>)` — the same outer-Handle
/// claim `HandleNew` carries, protected the same way (`lower_type(Handle(_))`
/// collapses to I64 regardless of the inner claim). The interpreter always
/// returns `Value::Num(0)` here, discarding `remove()`'s result entirely — so
/// there is no meaningful value to reconstruct. The expression's result is
/// simply the pre-call packed handle i64, unchanged: satisfies the I64 type
/// any consumer expects, with zero new computation.
fn lower_handle_drop(
    binding: BindingId,
    name: &str,
    _ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let handle_val = active.bindings.get(&binding).cloned().ok_or_else(|| {
        LoweringError::InternalInvariantViolation {
            detail: format!(
                "HandleDrop: binding '{name}' ({}) not found in scope",
                binding.0
            ),
        }
    })?;
    active.emit(IrInst::Call {
        dst: None,
        callee: "cx_handle_drop".to_string(),
        args: vec![handle_val.value],
        return_ty: None,
    })?;
    Ok(handle_val)
}

/// D2.4a: pack a Result tag + payload word into the i128 rep via a MEMORY
/// round-trip — store payload @0 and tag @8 into a 16-byte slot, then `Load`
/// the i128. The raw-byte store/load is exactly why a NEGATIVE Ok payload can't
/// corrupt the tag: its sign bits stay in the payload's 8 bytes, never reaching
/// the tag's. (Arithmetic packing would `sextend` them into the tag — the bug
/// this rep avoids.)
fn result_pack(
    tag: i128,
    payload_i64: ValueId,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let slot = ctx.fresh_value();
    active.emit(IrInst::Alloca { dst: slot, size: 16, align: 16 })?;
    active.emit(IrInst::Store { ptr: slot, value: payload_i64 })?;
    let tag_val = ctx.fresh_value();
    active.emit(IrInst::ConstInt { dst: tag_val, ty: IrType::I64, value: tag })?;
    let slot8 = ctx.fresh_value();
    active.emit(IrInst::PtrOffset { dst: slot8, base: slot, offset: 8 })?;
    active.emit(IrInst::Store { ptr: slot8, value: tag_val })?;
    let result = ctx.fresh_value();
    active.emit(IrInst::Load { dst: result, ty: IrType::I128, ptr: slot })?;
    Ok(LoweredValue { value: result, ty: IrType::I128 })
}

/// D2.4a: lower `Ok(x)` (tag 0) / `Err("...")` (tag 1) — lower the payload, widen
/// to the i64 word, pack. Err's str payload is the static D2.3 descriptor Ptr, so
/// nothing runtime-allocated escapes (R7-clean).
fn lower_result_construct(
    tag: i128,
    payload_expr: &SemanticExpr,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let payload = lower_expr(payload_expr, ctx, active)?;
    let payload_i64 = widen_payload_to_i64(&payload, ctx, active)?;
    result_pack(tag, payload_i64, ctx, active)
}

/// D2.4a: unpack a Result i128 into (tag, payload_word). Payload is the low half
/// (`Cast I128->I64` = truncate). Tag is the high half, read from its OWN 8 bytes
/// (store the i128, `Load i64@8`) — so a negative payload's sign bits never reach
/// the tag.
fn result_unpack(
    result: ValueId,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<(ValueId, ValueId), LoweringError> {
    let payload = ctx.fresh_value();
    active.emit(IrInst::Cast {
        dst: payload,
        from: IrType::I128,
        to: IrType::I64,
        value: result,
    })?;
    let slot = ctx.fresh_value();
    active.emit(IrInst::Alloca { dst: slot, size: 16, align: 16 })?;
    active.emit(IrInst::Store { ptr: slot, value: result })?;
    let slot8 = ctx.fresh_value();
    active.emit(IrInst::PtrOffset { dst: slot8, base: slot, offset: 8 })?;
    let tag = ctx.fresh_value();
    active.emit(IrInst::Load { dst: tag, ty: IrType::I64, ptr: slot8 })?;
    Ok((tag, payload))
}

/// D2.4a: print a static literal chunk inline (no newline) — `construct_static_str`
/// + `cx_print_str_inline`, reused from the D2.3/D2.3d print machinery.
fn emit_inline_str(
    text: &str,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<(), LoweringError> {
    let desc = construct_static_str(text, ctx, active)?;
    active.emit(IrInst::Call {
        dst: None,
        callee: "cx_print_str_inline".to_string(),
        args: vec![desc.value],
        return_ty: None,
    })?;
    Ok(())
}

/// D2.4a: print the Ok payload (the i64 word) by the `Result<T>` Ok type — an
/// integer T prints via `cx_printn_inline` (the widened word carries the signed
/// value); a str T reinterprets the word to a descriptor Ptr.
fn emit_ok_payload(
    payload: ValueId,
    ok_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<(), LoweringError> {
    match lower_type(ok_ty)? {
        IrType::I8 | IrType::I16 | IrType::I32 | IrType::I64 => {
            active.emit(IrInst::Call {
                dst: None,
                callee: "cx_printn_inline".to_string(),
                args: vec![payload],
                return_ty: None,
            })?;
        }
        IrType::Ptr => {
            let p = ctx.fresh_value();
            active.emit(IrInst::Cast { dst: p, from: IrType::I64, to: IrType::Ptr, value: payload })?;
            active.emit(IrInst::Call {
                dst: None,
                callee: "cx_print_str_inline".to_string(),
                args: vec![p],
                return_ty: None,
            })?;
        }
        other => {
            return Err(LoweringError::UnsupportedSemanticConstruct {
                construct: format!("print of Result<{other:?}> Ok payload not yet supported"),
            });
        }
    }
    Ok(())
}

/// D2.4a: print(Result) as a tag-branch print SEQUENCE (no new string built —
/// R6-style). Unpack → branch on tag; Ok → `Ok(` + payload-by-type + `)`; Err →
/// `Err(` + the str payload + `)`; one trailing `cx_print_newline`. Matches the
/// interpreter's `Ok(15)` / `Err(cannot be zero)` byte-for-byte.
fn lower_result_print(
    arg_expr: &SemanticExpr,
    ok_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    mut current: ActiveBlock,
) -> Result<Option<ActiveBlock>, LoweringError> {
    let result = lower_expr(arg_expr, ctx, &mut current)?;
    let (tag, payload) = result_unpack(result.value, ctx, &mut current)?;
    let zero = ctx.fresh_value();
    current.emit(IrInst::ConstInt { dst: zero, ty: IrType::I64, value: 0 })?;
    let is_ok = ctx.fresh_value();
    current.emit(IrInst::Compare { dst: is_ok, op: CompareOp::Eq, lhs: tag, rhs: zero })?;

    let incoming = current.bindings.clone();
    let mut ok_block = ctx.start_block(vec![], incoming.clone());
    let ok_id = ok_block.id();
    let mut err_block = ctx.start_block(vec![], incoming.clone());
    let err_id = err_block.id();
    let merge = ctx.start_block(vec![], incoming);
    let merge_id = merge.id();

    // Seal the decision block first (Ok/Err read `payload` from it).
    current.terminate(IrTerminator::Branch {
        cond: is_ok,
        then_block: ok_id,
        then_args: vec![],
        else_block: err_id,
        else_args: vec![],
    })?;
    let old = std::mem::replace(&mut current, merge);
    ctx.seal_block(old)?;

    // Ok( payload )
    emit_inline_str("Ok(", ctx, &mut ok_block)?;
    emit_ok_payload(payload, ok_ty, ctx, &mut ok_block)?;
    emit_inline_str(")", ctx, &mut ok_block)?;
    ok_block.terminate(IrTerminator::Jump { target: merge_id, args: vec![] })?;
    ctx.seal_block(ok_block)?;

    // Err( <str> ) — the Err payload is always a str descriptor Ptr.
    emit_inline_str("Err(", ctx, &mut err_block)?;
    let err_ptr = ctx.fresh_value();
    err_block.emit(IrInst::Cast { dst: err_ptr, from: IrType::I64, to: IrType::Ptr, value: payload })?;
    err_block.emit(IrInst::Call {
        dst: None,
        callee: "cx_print_str_inline".to_string(),
        args: vec![err_ptr],
        return_ty: None,
    })?;
    emit_inline_str(")", ctx, &mut err_block)?;
    err_block.terminate(IrTerminator::Jump { target: merge_id, args: vec![] })?;
    ctx.seal_block(err_block)?;

    // merge: one trailing newline.
    current.emit(IrInst::Call {
        dst: None,
        callee: "cx_print_newline".to_string(),
        args: vec![],
        return_ty: None,
    })?;
    Ok(Some(current))
}

fn lower_print_stmt(
    builtin_name: &str,
    args: &[SemanticCallArg],
    ctx: &mut LoweringCtx,
    mut current: ActiveBlock,
) -> Result<Option<ActiveBlock>, LoweringError> {
    if args.len() != 1 {
        return Err(LoweringError::InternalInvariantViolation {
            detail: format!("{} expects 1 argument, got {}", builtin_name, args.len()),
        });
    }
    let arg_expr = match &args[0] {
        SemanticCallArg::Expr(e) => e,
        _ => {
            return Err(LoweringError::UnsupportedSemanticConstruct {
                construct: format!("non-Expr argument to {}", builtin_name),
            });
        }
    };
    // D2.4a: print(Result) — unpack the packed-i128 and print a tag-branch
    // sequence (`Ok(..)` / `Err(..)`), no new string built.
    if let SemanticType::Result(ok_ty) = &arg_expr.ty {
        return lower_result_print(arg_expr, ok_ty, ctx, current);
    }
    // D2.3d: print-time string interpolation — a str LITERAL containing `{…}`
    // decomposes into a print sequence (literal chunks + resolved `{x}` values +
    // one trailing newline), building NO new string (R6 untouched). A non-literal
    // str arg can't hold `{…}` in a lowerable program (the construct would SKIP),
    // so it falls through to the plain cx_print_str path below.
    if let SemanticExprKind::Value(SemanticValue::Str(s)) = &arg_expr.kind {
        if s.contains('{') {
            return lower_interpolated_print(s, ctx, current);
        }
    }
    // D2.3d: bare f64 print (f64 wasn't routable before) — inline render via
    // `x.to_string()` + one trailing newline, matching the interpreter's print.
    if matches!(arg_expr.ty, SemanticType::F64) {
        let arg = lower_expr(arg_expr, ctx, &mut current)?;
        current.emit(IrInst::Call {
            dst: None,
            callee: "cx_print_f64_inline".to_string(),
            args: vec![arg.value],
            return_ty: None,
        })?;
        current.emit(IrInst::Call {
            dst: None,
            callee: "cx_print_newline".to_string(),
            args: vec![],
            return_ty: None,
        })?;
        return Ok(Some(current));
    }
    // D2.3b: string print — pass the descriptor Ptr to cx_print_str, which reads
    // (byte ptr, len) from it and writes the bytes + newline (matching the
    // interpreter's print/println for a string, byte-for-byte). Interpolation
    // (`{…}`) literals already SKIP at the construct (tracker D2.3d).
    if matches!(arg_expr.ty, SemanticType::Str) {
        let arg = lower_expr(arg_expr, ctx, &mut current)?;
        current.emit(IrInst::Call {
            dst: None,
            callee: "cx_print_str".to_string(),
            args: vec![arg.value],
            return_ty: None,
        })?;
        return Ok(Some(current));
    }
    let arg = lower_expr(arg_expr, ctx, &mut current)?;
    let (routed_value, callee) = route_print_arg(arg.value, arg.ty.clone(), ctx, &mut current)?
        .ok_or_else(|| LoweringError::UnsupportedSemanticConstruct {
            construct: format!(
                "{} argument must be a numeric integer (I8/I16/I32/I64) or Bool, got {:?} — F64, I128, Str, and composite types not yet supported",
                builtin_name, arg.ty
            ),
        })?;
    current.emit(IrInst::Call {
        dst: None,
        callee: callee.to_string(),
        args: vec![routed_value],
        return_ty: None,
    })?;
    Ok(Some(current))
}

/// Lower `assert(cond)` as a void statement.
///
/// Emits a two-way branch: the pass branch continues execution; the fail
/// branch terminates with [`IrTerminator::Trap`].  Returns the pass block
/// as the new active block so lowering of subsequent statements continues
/// in the correct CFG position.
fn lower_assert_stmt(
    args: &[SemanticCallArg],
    ctx: &mut LoweringCtx,
    mut current: ActiveBlock,
) -> Result<Option<ActiveBlock>, LoweringError> {
    if args.len() != 1 {
        return Err(LoweringError::InternalInvariantViolation {
            detail: format!("assert expects 1 argument, got {}", args.len()),
        });
    }

    let cond_expr = match &args[0] {
        SemanticCallArg::Expr(e) => e,
        _ => {
            return Err(LoweringError::UnsupportedSemanticConstruct {
                construct: "non-Expr argument to assert".to_string(),
            });
        }
    };

    let cond = lower_expr(cond_expr, ctx, &mut current)?;

    // Coerce the condition to Bool (IrType::Bool is the required Branch type).
    let bool_val = coerce_to_bool(cond, ctx, &mut current)?;

    emit_assert_branch(bool_val, ctx, current)
}

/// Lower `assert_eq(lhs, rhs)` as a void statement.
///
/// Emits a Compare(Eq) followed by the same two-way branch pattern as
/// [`lower_assert_stmt`].  Both operands must have the same IR type and that
/// type must be equality-comparable (Bool or integer).
fn lower_assert_eq_stmt(
    args: &[SemanticCallArg],
    ctx: &mut LoweringCtx,
    mut current: ActiveBlock,
) -> Result<Option<ActiveBlock>, LoweringError> {
    if args.len() != 2 {
        return Err(LoweringError::InternalInvariantViolation {
            detail: format!("assert_eq expects 2 arguments, got {}", args.len()),
        });
    }

    let lhs_expr = match &args[0] {
        SemanticCallArg::Expr(e) => e,
        _ => {
            return Err(LoweringError::UnsupportedSemanticConstruct {
                construct: "non-Expr first argument to assert_eq".to_string(),
            });
        }
    };
    let rhs_expr = match &args[1] {
        SemanticCallArg::Expr(e) => e,
        _ => {
            return Err(LoweringError::UnsupportedSemanticConstruct {
                construct: "non-Expr second argument to assert_eq".to_string(),
            });
        }
    };

    // D2.3c: assert_eq on strings — content comparison via cx_str_eq (the same
    // operand-type dispatch as `==`), then the standard assert branch. (The
    // existing scalar type check below rejects Ptr, so strings must route here.)
    if matches!(lhs_expr.ty, SemanticType::Str | SemanticType::StrRef)
        || matches!(rhs_expr.ty, SemanticType::Str | SemanticType::StrRef)
    {
        let lhs = lower_expr(lhs_expr, ctx, &mut current)?;
        let rhs = lower_expr(rhs_expr, ctx, &mut current)?;
        let eq = lower_str_eq(Op::EqEq, lhs.value, rhs.value, ctx, &mut current)?;
        return emit_assert_branch(eq.value, ctx, current);
    }

    let lhs = lower_expr(lhs_expr, ctx, &mut current)?;
    let rhs = lower_expr(rhs_expr, ctx, &mut current)?;

    if lhs.ty != rhs.ty {
        return Err(LoweringError::UnsupportedSemanticConstruct {
            construct: format!(
                "assert_eq: lhs has type {:?}, rhs has type {:?} — operand types must match",
                lhs.ty, rhs.ty
            ),
        });
    }

    // Validate that the type supports equality comparison.
    match &lhs.ty {
        IrType::Bool
        | IrType::I8
        | IrType::I16
        | IrType::I32
        | IrType::I64
        | IrType::I128 => {}
        other => {
            return Err(LoweringError::UnsupportedSemanticConstruct {
                construct: format!(
                    "assert_eq: type {:?} is not supported for equality comparison in the IR backend",
                    other
                ),
            });
        }
    }

    let cmp_dst = ctx.fresh_value();
    current.emit(IrInst::Compare {
        dst: cmp_dst,
        op: CompareOp::Eq,
        lhs: lhs.value,
        rhs: rhs.value,
    })?;

    emit_assert_branch(cmp_dst, ctx, current)
}

/// Coerce a [`LoweredValue`] to `IrType::Bool` for use as a branch condition.
///
/// - If the value is already `Bool`, it is returned unchanged.
/// - If the value is an integer type (`I8`/`I16`/`I32`/`I64`/`I128`), a
///   `Compare { Ne, value, 0 }` is emitted to produce a `Bool` result
///   (truthy semantics: any non-zero integer is true).
/// - All other types produce an `UnsupportedSemanticConstruct` error.
fn coerce_to_bool(
    val: LoweredValue,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<ValueId, LoweringError> {
    match &val.ty {
        IrType::Bool => Ok(val.value),
        IrType::I8 | IrType::I16 | IrType::I32 | IrType::I64 | IrType::I128 => {
            let zero = ctx.fresh_value();
            active.emit(IrInst::ConstInt {
                dst: zero,
                ty: val.ty.clone(),
                value: 0,
            })?;
            let cmp_dst = ctx.fresh_value();
            active.emit(IrInst::Compare {
                dst: cmp_dst,
                op: CompareOp::Ne,
                lhs: val.value,
                rhs: zero,
            })?;
            Ok(cmp_dst)
        }
        other => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: format!(
                "assert condition of type {:?} is not supported in the IR backend",
                other
            ),
        }),
    }
}

/// Emit the common Branch + Trap pattern for assertions.
///
/// Terminates `current` with a [`IrTerminator::Branch`] on `bool_val`:
/// - true  → `pass_block` (continues execution; returned as new current)
/// - false → `trap_block` (terminated with [`IrTerminator::Trap`])
fn emit_assert_branch(
    bool_val: ValueId,
    ctx: &mut LoweringCtx,
    current: ActiveBlock,
) -> Result<Option<ActiveBlock>, LoweringError> {
    let incoming = current.bindings.clone();

    let pass_active = ctx.start_block(vec![], incoming.clone());
    let pass_block_id = pass_active.id();

    let mut trap_active = ctx.start_block(vec![], incoming);
    let trap_block_id = trap_active.id();

    let mut current = current;
    current.terminate(IrTerminator::Branch {
        cond: bool_val,
        then_block: pass_block_id,
        then_args: vec![],
        else_block: trap_block_id,
        else_args: vec![],
    })?;
    ctx.seal_block(current)?;

    trap_active.terminate(IrTerminator::Trap)?;
    ctx.seal_block(trap_active)?;

    Ok(Some(pass_active))
}

/// Lower a void function call as a standalone statement.
///
/// Void calls cannot go through `lower_expr` because that function must return
/// a `LoweredValue` and void calls produce no value.  This helper lowers the
/// arguments, performs the usual arity and type checks, and emits
/// `IrInst::Call { dst: None, return_ty: None }` directly into `active`.
fn lower_void_call(
    callee: &str,
    args: &[SemanticCallArg],
    param_types: &[IrType],
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<(), LoweringError> {
    if args.len() != param_types.len() {
        return Err(LoweringError::InternalInvariantViolation {
            detail: format!(
                "call to '{}': expected {} arguments, got {}",
                callee,
                param_types.len(),
                args.len()
            ),
        });
    }

    let mut lowered_args = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        match arg {
            SemanticCallArg::Expr(expr) => {
                let lowered = lower_expr(expr, ctx, active)?;
                ensure_type_match(
                    &format!("argument {} of call to '{}'", i, callee),
                    param_types[i].clone(),
                    lowered.ty,
                )?;
                lowered_args.push(lowered.value);
            }
            _ => {
                return Err(LoweringError::UnsupportedSemanticConstruct {
                    construct: format!("non-Expr call argument in call to '{}'", callee),
                });
            }
        }
    }

    active.emit(IrInst::Call {
        dst: None,
        callee: callee.to_string(),
        args: lowered_args,
        return_ty: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::ast::Op;
    use crate::frontend::semantic_types::{
        FunctionId, SemanticCallArg, SemanticFunction, SemanticLValue, SemanticParam,
        SemanticParamKind,
    };
    use crate::ir::instr::{BinaryOp, CompareOp, IrInst, IrTerminator};
    use crate::ir::validate::validate_module;

    fn int_expr(value: i128, ty: SemanticType) -> SemanticExpr {
        SemanticExpr {
            ty,
            kind: SemanticExprKind::Value(SemanticValue::Num(value)),
        }
    }

    fn float_expr(value: f64) -> SemanticExpr {
        SemanticExpr {
            ty: SemanticType::F64,
            kind: SemanticExprKind::Value(SemanticValue::Float(value)),
        }
    }

    fn bool_expr(value: bool) -> SemanticExpr {
        SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Value(SemanticValue::Bool(value)),
        }
    }

    fn binding_ref(binding: BindingId, name: &str, ty: SemanticType) -> SemanticExpr {
        SemanticExpr {
            ty,
            kind: SemanticExprKind::VarRef {
                binding,
                name: name.to_string(),
            },
        }
    }

    fn typed_assign(
        binding: BindingId,
        name: &str,
        ty: SemanticType,
        expr: SemanticExpr,
    ) -> SemanticStmt {
        SemanticStmt::TypedAssign {
            binding,
            name: name.to_string(),
            ty,
            expr,
            pos_type: 0,
        }
    }

    fn assign(
        binding: BindingId,
        name: &str,
        ty: SemanticType,
        expr: SemanticExpr,
    ) -> SemanticStmt {
        SemanticStmt::Assign {
            target: SemanticLValue::Binding {
                binding,
                name: name.to_string(),
                ty,
            },
            expr,
            pos_eq: 0,
        }
    }

    fn typed_param(binding: BindingId, name: &str, ty: SemanticType) -> SemanticParam {
        SemanticParam {
            binding,
            name: name.to_string(),
            kind: SemanticParamKind::Typed,
            ty: Some(ty),
        }
    }

    fn semantic_function(
        name: &str,
        params: Vec<SemanticParam>,
        return_ty: Option<SemanticType>,
        body: Vec<SemanticStmt>,
        ret_expr: Option<SemanticExpr>,
    ) -> SemanticStmt {
        SemanticStmt::FuncDef(SemanticFunction {
            id: FunctionId(0),
            name: name.to_string(),
            type_params: vec![],
            params,
            return_ty,
            body,
            ret_expr,
            pos: 0,
            is_test: false,
        })
    }

    fn if_stmt(
        condition: SemanticExpr,
        then_body: Vec<SemanticStmt>,
        else_ifs: Vec<(SemanticExpr, Vec<SemanticStmt>)>,
        else_body: Option<Vec<SemanticStmt>>,
    ) -> SemanticStmt {
        SemanticStmt::IfElse {
            condition,
            then_body,
            else_ifs,
            else_body,
            pos: 0,
        }
    }

    fn lower_and_validate(program: &SemanticProgram) -> IrModule {
        let module = match lower_program(program) {
            Ok(m) => m,
            Err(e) => panic!("lowering failed: {}", e),
        };
        if let Err(errors) = validate_module(&module) {
            eprintln!("\n=== IR DUMP ON VALIDATION FAILURE ===");
            eprintln!("{}", crate::ir::printer::print_module(&module));
            eprintln!("=== VALIDATION ERRORS ===");
            for e in &errors {
                eprintln!("  {:?}", e);
            }
            eprintln!("=== END ===\n");
            panic!("IR validation failed with {} error(s)", errors.len());
        }
        module
    }

    #[test]
    fn lowers_top_level_typed_assign_into_synthetic_main() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(1),
                "x",
                SemanticType::I64,
                int_expr(7, SemanticType::I64),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert_eq!(module.functions.len(), 1);
        assert_eq!(module.functions[0].name, "main");
        assert_eq!(module.functions[0].blocks.len(), 1);
    }

    #[test]
    fn declaration_only_decl_does_not_invent_ssa_value() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::Decl {
                binding: BindingId(1),
                name: "x".to_string(),
                ty: Some(SemanticType::I64),
                pos: 0,
            }],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert!(module.functions[0].blocks[0].insts.is_empty());
    }

    #[test]
    fn lowers_arithmetic_to_binary_with_correct_op_and_type() {
        let expr = SemanticExpr {
            ty: SemanticType::I64,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(int_expr(1, SemanticType::I64)),
                op: Op::Plus,
                pos: 0,
                rhs: Box::new(int_expr(2, SemanticType::I64)),
            },
        };
        let program = SemanticProgram {
            stmts: vec![typed_assign(BindingId(1), "x", SemanticType::I64, expr)],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert!(module.functions[0].blocks[0].insts.iter().any(|inst| {
            matches!(
                inst,
                IrInst::Binary {
                    op: BinaryOp::Add,
                    ty: IrType::I64,
                    ..
                }
            )
        }));
    }

    #[test]
    fn lowers_comparisons_to_compare_for_equality_and_ordering() {
        let eq_expr = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(int_expr(1, SemanticType::I64)),
                op: Op::EqEq,
                pos: 0,
                rhs: Box::new(int_expr(1, SemanticType::I64)),
            },
        };
        let lt_expr = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(float_expr(1.0)),
                op: Op::Lt,
                pos: 0,
                rhs: Box::new(float_expr(2.0)),
            },
        };
        let program = SemanticProgram {
            stmts: vec![
                typed_assign(BindingId(1), "eq", SemanticType::Bool, eq_expr),
                typed_assign(BindingId(2), "lt", SemanticType::Bool, lt_expr),
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let insts = &module.functions[0].blocks[0].insts;
        assert!(insts.iter().any(|inst| matches!(
            inst,
            IrInst::Compare {
                op: CompareOp::Eq,
                ..
            }
        )));
        assert!(insts.iter().any(|inst| matches!(
            inst,
            IrInst::Compare {
                op: CompareOp::Lt,
                ..
            }
        )));
        assert!(insts.iter().any(|inst| matches!(
            inst,
            IrInst::SsaBind {
                ty: IrType::Bool,
                ..
            }
        )));
    }

    #[test]
    fn lowers_explicit_cast_to_cast_instruction() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(1),
                "x",
                SemanticType::I64,
                SemanticExpr {
                    ty: SemanticType::I64,
                    kind: SemanticExprKind::Cast {
                        expr: Box::new(int_expr(1, SemanticType::I128)),
                        from: SemanticType::I128,
                        to: SemanticType::I64,
                    },
                },
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert!(module.functions[0].blocks[0].insts.iter().any(|inst| {
            matches!(
                inst,
                IrInst::Cast {
                    from: IrType::I128,
                    to: IrType::I64,
                    ..
                }
            )
        }));
    }

    #[test]
    fn variable_references_use_current_ssa_mapping() {
        let binding = BindingId(1);
        let program = SemanticProgram {
            stmts: vec![
                typed_assign(
                    binding,
                    "x",
                    SemanticType::I64,
                    int_expr(2, SemanticType::I64),
                ),
                SemanticStmt::ExprStmt {
                    expr: binding_ref(binding, "x", SemanticType::I64),
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let insts = &module.functions[0].blocks[0].insts;
        let bind_count = insts
            .iter()
            .filter(|inst| {
                matches!(
                    inst,
                    IrInst::SsaBind {
                        ty: IrType::I64,
                        ..
                    }
                )
            })
            .count();

        assert_eq!(bind_count, 1);
        assert!(matches!(
            insts.as_slice(),
            [
                IrInst::ConstInt {
                    ty: IrType::I64,
                    value: 2,
                    ..
                },
                IrInst::SsaBind {
                    ty: IrType::I64,
                    ..
                }
            ]
        ));
    }

    #[test]
    fn straight_line_block_ends_in_return_none() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::ExprStmt {
                expr: bool_expr(true),
                pos: 0,
            }],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert_eq!(
            module.functions[0].blocks[0].term,
            IrTerminator::Return { value: None }
        );
    }

    #[test]
    fn rejects_numeric_type() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Numeric,
                SemanticExpr {
                    ty: SemanticType::Numeric,
                    kind: SemanticExprKind::Value(SemanticValue::Num(1)),
                },
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should reject unsupported type"),
            LoweringError::UnsupportedSemanticType {
                ty: "Numeric".to_string()
            }
        );
    }

    // VarRef with SemanticType::Numeric falls back to the stored binding type
    // (I64 on 64-bit) rather than calling lower_type(Numeric) which would fail.
    // Mirrors the fallback introduced for array element lowering in CX-121/CX-129.
    #[test]
    fn varref_numeric_type_falls_back_to_binding_type() {
        let binding = BindingId(1);
        let program = SemanticProgram {
            stmts: vec![
                typed_assign(
                    binding,
                    "i",
                    SemanticType::I64,
                    int_expr(0, SemanticType::I64),
                ),
                SemanticStmt::ExprStmt {
                    expr: binding_ref(binding, "i", SemanticType::Numeric),
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let insts = &module.functions[0].blocks[0].insts;
        assert!(
            insts.iter().any(|inst| matches!(inst, IrInst::SsaBind { ty: IrType::I64, .. })),
            "expected SsaBind(I64) for VarRef with Numeric placeholder type"
        );
    }

    // VarRef with SemanticType::Unknown also falls back to the stored binding type.
    #[test]
    fn varref_unknown_type_falls_back_to_binding_type() {
        let binding = BindingId(2);
        let program = SemanticProgram {
            stmts: vec![
                typed_assign(
                    binding,
                    "j",
                    SemanticType::I64,
                    int_expr(5, SemanticType::I64),
                ),
                SemanticStmt::ExprStmt {
                    expr: binding_ref(binding, "j", SemanticType::Unknown),
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let insts = &module.functions[0].blocks[0].insts;
        assert!(
            insts.iter().any(|inst| matches!(inst, IrInst::SsaBind { ty: IrType::I64, .. })),
            "expected SsaBind(I64) for VarRef with Unknown placeholder type"
        );
    }

    #[test]
    fn rejects_unknown_type() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Unknown,
                SemanticExpr {
                    ty: SemanticType::Unknown,
                    kind: SemanticExprKind::Value(SemanticValue::Unknown),
                },
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should reject unsupported type"),
            LoweringError::UnsupportedSemanticType {
                ty: "Unknown".to_string()
            }
        );
    }

    #[test]
    fn rejects_handle_type_mismatch() {
        // D2.5a: Handle now lowers (SemanticType::Handle(_) -> IrType::I64), so
        // this is no longer "Handle types are unsupported" (renamed from
        // rejects_handle_type) -- it's a genuine type mismatch: a Handle-typed
        // binding's rep is I64 (the packed slot+gen word from Handle.new /
        // cx_handle_new), and a raw Bool literal is a different IR type, not a
        // real Handle value. The rejection reason correctly shifted from
        // UnsupportedSemanticType to a type-mismatch InternalInvariantViolation.
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Handle(Box::new(SemanticType::I64)),
                bool_expr(true),
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should reject the type mismatch"),
            LoweringError::InternalInvariantViolation {
                detail: "typed assignment type mismatch after semantic analysis: expected I64, got Bool".to_string()
            }
        );
    }

    #[test]
    fn rejects_str_ref_type() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::StrRef,
                bool_expr(true),
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should reject unsupported type"),
            LoweringError::UnsupportedSemanticType {
                ty: "StrRef".to_string()
            }
        );
    }

    #[test]
    fn rejects_container_type() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Container,
                bool_expr(true),
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should reject unsupported type"),
            LoweringError::UnsupportedSemanticType {
                ty: "Container".to_string()
            }
        );
    }

    #[test]
    fn rejects_compound_assign() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::CompoundAssign {
                target: SemanticLValue::Binding {
                    binding: BindingId(1),
                    name: "x".to_string(),
                    ty: SemanticType::I64,
                },
                op: Op::Plus,
                operand: int_expr(1, SemanticType::I64),
                pos: 0,
            }],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnresolvedSemanticArtifact {
                artifact: "compound assign binding BindingId(1)".to_string()
            }
        );
    }

    #[test]
    fn lowers_simple_function_with_one_parameter_and_return_value() {
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "id",
                vec![typed_param(BindingId(10), "x", SemanticType::I64)],
                Some(SemanticType::I64),
                vec![],
                Some(binding_ref(BindingId(10), "x", SemanticType::I64)),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert_eq!(module.functions.len(), 1);
        let function = &module.functions[0];
        assert_eq!(function.name, "id");
        assert_eq!(function.params.len(), 1);
        assert_eq!(function.blocks.len(), 1);
        assert_eq!(function.blocks[0].params.len(), 1);
        assert_eq!(
            function.blocks[0].term,
            IrTerminator::Return {
                value: Some(function.blocks[0].params[0].value),
            }
        );
    }

    #[test]
    fn lowers_function_with_multiple_parameters() {
        let x = BindingId(10);
        let y = BindingId(11);
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "sum",
                vec![
                    typed_param(x, "x", SemanticType::I64),
                    typed_param(y, "y", SemanticType::I64),
                ],
                Some(SemanticType::I64),
                vec![],
                Some(SemanticExpr {
                    ty: SemanticType::I64,
                    kind: SemanticExprKind::Binary {
                        lhs: Box::new(binding_ref(x, "x", SemanticType::I64)),
                        op: Op::Plus,
                        pos: 0,
                        rhs: Box::new(binding_ref(y, "y", SemanticType::I64)),
                    },
                }),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let function = &module.functions[0];
        assert_eq!(function.params.len(), 2);
        assert_eq!(function.blocks[0].params.len(), 2);
        assert!(function.blocks[0].insts.iter().any(|inst| matches!(
            inst,
            IrInst::Binary {
                op: BinaryOp::Add,
                ty: IrType::I64,
                ..
            }
        )));
    }

    #[test]
    fn function_parameter_references_use_function_local_ssa_map() {
        let x = BindingId(20);
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "use_param",
                vec![typed_param(x, "x", SemanticType::I64)],
                Some(SemanticType::I64),
                vec![typed_assign(
                    BindingId(21),
                    "y",
                    SemanticType::I64,
                    binding_ref(x, "x", SemanticType::I64),
                )],
                Some(binding_ref(BindingId(21), "y", SemanticType::I64)),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let function = &module.functions[0];
        assert!(matches!(
            function.blocks[0].insts.as_slice(),
            [IrInst::SsaBind {
                ty: IrType::I64,
                src,
                ..
            }] if *src == function.blocks[0].params[0].value
        ));
    }

    #[test]
    fn function_return_with_value_lowers_to_return_some() {
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "const_ret",
                vec![],
                Some(SemanticType::I64),
                vec![],
                Some(int_expr(9, SemanticType::I64)),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert!(matches!(
            module.functions[0].blocks[0].term,
            IrTerminator::Return { value: Some(_) }
        ));
    }

    #[test]
    fn typed_assignment_inside_function_lowers_correctly() {
        let param = BindingId(30);
        let local = BindingId(31);
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "bind_local",
                vec![typed_param(param, "x", SemanticType::I64)],
                Some(SemanticType::I64),
                vec![typed_assign(
                    local,
                    "tmp",
                    SemanticType::I64,
                    binding_ref(param, "x", SemanticType::I64),
                )],
                Some(binding_ref(local, "tmp", SemanticType::I64)),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert!(module.functions[0].blocks[0]
            .insts
            .iter()
            .any(|inst| matches!(
                inst,
                IrInst::SsaBind {
                    ty: IrType::I64,
                    ..
                }
            )));
    }

    #[test]
    fn top_level_statements_and_real_functions_lower_together() {
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "const_ret",
                    vec![],
                    Some(SemanticType::I64),
                    vec![],
                    Some(int_expr(1, SemanticType::I64)),
                ),
                typed_assign(
                    BindingId(40),
                    "x",
                    SemanticType::I64,
                    int_expr(2, SemanticType::I64),
                ),
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert_eq!(module.functions.len(), 2);
        assert!(module.functions.iter().any(|f| f.name == "const_ret"));
        assert!(module.functions.iter().any(|f| f.name == "main"));
    }

    #[test]
    fn real_main_plus_top_level_statements_is_rejected() {
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "main",
                    vec![],
                    Some(SemanticType::I64),
                    vec![],
                    Some(int_expr(1, SemanticType::I64)),
                ),
                typed_assign(
                    BindingId(41),
                    "x",
                    SemanticType::I64,
                    int_expr(2, SemanticType::I64),
                ),
            ],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticConstruct {
                construct: "real function 'main' collides with synthetic main".to_string()
            }
        );
    }

    #[test]
    fn rejects_call_to_unresolved_function() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::ExprStmt {
                expr: SemanticExpr {
                    ty: SemanticType::I64,
                    kind: SemanticExprKind::Call {
                        callee: "foo".to_string(),
                        function: FunctionId(0),
                        args: vec![],
                    },
                },
                pos: 0,
            }],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnresolvedSemanticArtifact {
                artifact: "function 'foo'".to_string(),
            }
        );
    }

    #[test]
    fn rejects_call_to_unresolved_function_inside_function() {
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "bad_call",
                vec![],
                None,
                vec![SemanticStmt::ExprStmt {
                    expr: SemanticExpr {
                        ty: SemanticType::I64,
                        kind: SemanticExprKind::Call {
                            callee: "foo".to_string(),
                            function: FunctionId(0),
                            args: vec![],
                        },
                    },
                    pos: 0,
                }],
                None,
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnresolvedSemanticArtifact {
                artifact: "function 'foo'".to_string(),
            }
        );
    }

    #[test]
    fn lowers_direct_call_in_expr_stmt() {
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "get_value",
                    vec![],
                    Some(SemanticType::I64),
                    vec![],
                    Some(int_expr(42, SemanticType::I64)),
                ),
                SemanticStmt::ExprStmt {
                    expr: SemanticExpr {
                        ty: SemanticType::I64,
                        kind: SemanticExprKind::Call {
                            callee: "get_value".to_string(),
                            function: FunctionId(0),
                            args: vec![],
                        },
                    },
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let call_insts: Vec<_> = main_fn.blocks.iter()
            .flat_map(|b| b.insts.iter())
            .filter(|inst| matches!(inst, IrInst::Call { .. }))
            .collect();
        assert_eq!(call_insts.len(), 1);
        match &call_insts[0] {
            IrInst::Call { dst, callee, args, return_ty } => {
                assert!(dst.is_some());
                assert_eq!(callee, "get_value");
                assert!(args.is_empty());
                assert_eq!(*return_ty, Some(IrType::I64));
            }
            _ => panic!("expected Call instruction"),
        }
    }

    #[test]
    fn lowers_call_with_args_and_assignment() {
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "add_one",
                    vec![typed_param(BindingId(0), "x", SemanticType::I64)],
                    Some(SemanticType::I64),
                    vec![],
                    Some(int_expr(0, SemanticType::I64)),
                ),
                SemanticStmt::TypedAssign {
                    binding: BindingId(10),
                    name: "result".to_string(),
                    ty: SemanticType::I64,
                    expr: SemanticExpr {
                        ty: SemanticType::I64,
                        kind: SemanticExprKind::Call {
                            callee: "add_one".to_string(),
                            function: FunctionId(0),
                            args: vec![SemanticCallArg::Expr(int_expr(5, SemanticType::I64))],
                        },
                    },
                    pos_type: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let call_insts: Vec<_> = main_fn.blocks.iter()
            .flat_map(|b| b.insts.iter())
            .filter(|inst| matches!(inst, IrInst::Call { .. }))
            .collect();
        assert_eq!(call_insts.len(), 1);
        match &call_insts[0] {
            IrInst::Call { callee, args, .. } => {
                assert_eq!(callee, "add_one");
                assert_eq!(args.len(), 1);
            }
            _ => panic!("expected Call instruction"),
        }
    }

    #[test]
    fn rejects_call_arity_mismatch() {
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "needs_one",
                    vec![typed_param(BindingId(0), "x", SemanticType::I64)],
                    Some(SemanticType::I64),
                    vec![],
                    Some(int_expr(0, SemanticType::I64)),
                ),
                SemanticStmt::ExprStmt {
                    expr: SemanticExpr {
                        ty: SemanticType::I64,
                        kind: SemanticExprKind::Call {
                            callee: "needs_one".to_string(),
                            function: FunctionId(0),
                            args: vec![],
                        },
                    },
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::InternalInvariantViolation {
                detail: "call to 'needs_one': expected 1 arguments, got 0".to_string(),
            }
        );
    }

    #[test]
    fn lowers_void_call_in_expr_stmt() {
        // A void function called as a statement must produce
        // IrInst::Call { dst: None, return_ty: None } and pass validation.
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "do_nothing",
                    vec![],
                    None, // void return
                    vec![],
                    None,
                ),
                SemanticStmt::ExprStmt {
                    expr: SemanticExpr {
                        ty: SemanticType::Void,
                        kind: SemanticExprKind::Call {
                            callee: "do_nothing".to_string(),
                            function: FunctionId(0),
                            args: vec![],
                        },
                    },
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let call_insts: Vec<_> = main_fn
            .blocks
            .iter()
            .flat_map(|b| b.insts.iter())
            .filter(|inst| matches!(inst, IrInst::Call { .. }))
            .collect();
        assert_eq!(call_insts.len(), 1);
        match &call_insts[0] {
            IrInst::Call { dst, callee, args, return_ty } => {
                assert!(dst.is_none(), "void call must have no destination");
                assert_eq!(callee, "do_nothing");
                assert!(args.is_empty());
                assert!(return_ty.is_none(), "void call must have no return type");
            }
            _ => panic!("expected Call instruction"),
        }
    }

    #[test]
    fn lowers_void_call_with_args() {
        // A void function that accepts arguments: args must be lowered and
        // matched against the parameter types, then emitted with dst: None.
        let binding = BindingId(1);
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "sink",
                    vec![typed_param(BindingId(0), "x", SemanticType::I64)],
                    None, // void return
                    vec![],
                    None,
                ),
                // x: t64 = 5
                SemanticStmt::TypedAssign {
                    binding,
                    name: "x".to_string(),
                    ty: SemanticType::I64,
                    expr: int_expr(5, SemanticType::I64),
                    pos_type: 0,
                },
                // sink(x)
                SemanticStmt::ExprStmt {
                    expr: SemanticExpr {
                        ty: SemanticType::Void,
                        kind: SemanticExprKind::Call {
                            callee: "sink".to_string(),
                            function: FunctionId(0),
                            args: vec![SemanticCallArg::Expr(binding_ref(
                                binding,
                                "x",
                                SemanticType::I64,
                            ))],
                        },
                    },
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let call_insts: Vec<_> = main_fn
            .blocks
            .iter()
            .flat_map(|b| b.insts.iter())
            .filter(|inst| matches!(inst, IrInst::Call { .. }))
            .collect();
        assert_eq!(call_insts.len(), 1);
        match &call_insts[0] {
            IrInst::Call { dst, callee, args, return_ty } => {
                assert!(dst.is_none(), "void call must have no destination");
                assert_eq!(callee, "sink");
                assert_eq!(args.len(), 1);
                assert!(return_ty.is_none(), "void call must have no return type");
            }
            _ => panic!("expected Call instruction"),
        }
    }

    #[test]
    fn rejects_non_expr_call_arg() {
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "takes_one",
                    vec![typed_param(BindingId(0), "x", SemanticType::I64)],
                    Some(SemanticType::I64),
                    vec![],
                    Some(int_expr(0, SemanticType::I64)),
                ),
                SemanticStmt::ExprStmt {
                    expr: SemanticExpr {
                        ty: SemanticType::I64,
                        kind: SemanticExprKind::Call {
                            callee: "takes_one".to_string(),
                            function: FunctionId(0),
                            args: vec![SemanticCallArg::Copy {
                                binding: BindingId(5),
                                name: "y".to_string(),
                            }],
                        },
                    },
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticConstruct {
                construct: "non-Expr call argument in call to 'takes_one'".to_string(),
            }
        );
    }

    #[test]
    fn lowers_call_inside_function_body() {
        // Verifies that a user-defined function can call another user-defined
        // function and use the result as its trailing return expression.
        //
        //   fn get_val() -> i64 { 42 }
        //   fn use_val() -> i64 { get_val() }
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "get_val",
                    vec![],
                    Some(SemanticType::I64),
                    vec![],
                    Some(int_expr(42, SemanticType::I64)),
                ),
                semantic_function(
                    "use_val",
                    vec![],
                    Some(SemanticType::I64),
                    vec![],
                    Some(SemanticExpr {
                        ty: SemanticType::I64,
                        kind: SemanticExprKind::Call {
                            callee: "get_val".to_string(),
                            function: FunctionId(0),
                            args: vec![],
                        },
                    }),
                ),
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let caller = module.functions.iter().find(|f| f.name == "use_val").unwrap();
        let call_insts: Vec<_> = caller.blocks.iter()
            .flat_map(|b| b.insts.iter())
            .filter(|inst| matches!(inst, IrInst::Call { .. }))
            .collect();
        assert_eq!(call_insts.len(), 1);
        match &call_insts[0] {
            IrInst::Call { callee, args, return_ty, .. } => {
                assert_eq!(callee, "get_val");
                assert!(args.is_empty());
                assert_eq!(*return_ty, Some(IrType::I64));
            }
            _ => panic!("expected Call instruction"),
        }
        // The return terminator of use_val must carry the call result value.
        let terms: Vec<_> = caller.blocks.iter()
            .filter(|b| matches!(b.term, IrTerminator::Return { value: Some(_) }))
            .collect();
        assert_eq!(terms.len(), 1, "use_val must have exactly one value-return terminator");
    }

    #[test]
    fn lowers_call_result_in_binary_expr() {
        // Verifies that the ValueId produced by a Call flows correctly as an
        // operand into a subsequent Binary instruction.
        //
        //   fn make_one() -> i64 { 1 }
        //   let x: i64 = make_one() + 2
        let call_expr = SemanticExpr {
            ty: SemanticType::I64,
            kind: SemanticExprKind::Call {
                callee: "make_one".to_string(),
                function: FunctionId(0),
                args: vec![],
            },
        };
        let add_expr = SemanticExpr {
            ty: SemanticType::I64,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(call_expr),
                op: Op::Plus,
                pos: 0,
                rhs: Box::new(int_expr(2, SemanticType::I64)),
            },
        };
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "make_one",
                    vec![],
                    Some(SemanticType::I64),
                    vec![],
                    Some(int_expr(1, SemanticType::I64)),
                ),
                typed_assign(BindingId(10), "x", SemanticType::I64, add_expr),
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let insts: Vec<_> = main_fn.blocks.iter().flat_map(|b| b.insts.iter()).collect();
        assert!(
            insts.iter().any(|inst| matches!(inst, IrInst::Call { callee, .. } if callee == "make_one")),
            "expected a Call to make_one"
        );
        assert!(
            insts.iter().any(|inst| matches!(inst, IrInst::Binary { op: BinaryOp::Add, ty: IrType::I64, .. })),
            "expected a Binary/Add using the call result"
        );
    }

    #[test]
    fn lowers_multi_arg_call() {
        // Verifies that calls with more than one argument lower all arguments
        // and emit them in order on the IrInst::Call.
        //
        //   fn add(a: i64, b: i64) -> i64 { 0 }
        //   let r: i64 = add(10, 20)
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "add",
                    vec![
                        typed_param(BindingId(0), "a", SemanticType::I64),
                        typed_param(BindingId(1), "b", SemanticType::I64),
                    ],
                    Some(SemanticType::I64),
                    vec![],
                    Some(int_expr(0, SemanticType::I64)),
                ),
                typed_assign(
                    BindingId(10),
                    "r",
                    SemanticType::I64,
                    SemanticExpr {
                        ty: SemanticType::I64,
                        kind: SemanticExprKind::Call {
                            callee: "add".to_string(),
                            function: FunctionId(0),
                            args: vec![
                                SemanticCallArg::Expr(int_expr(10, SemanticType::I64)),
                                SemanticCallArg::Expr(int_expr(20, SemanticType::I64)),
                            ],
                        },
                    },
                ),
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let call_insts: Vec<_> = main_fn.blocks.iter()
            .flat_map(|b| b.insts.iter())
            .filter(|inst| matches!(inst, IrInst::Call { .. }))
            .collect();
        assert_eq!(call_insts.len(), 1);
        match &call_insts[0] {
            IrInst::Call { callee, args, .. } => {
                assert_eq!(callee, "add");
                assert_eq!(args.len(), 2);
            }
            _ => panic!("expected Call instruction"),
        }
    }

    #[test]
    fn rejects_declared_but_never_assigned_binding_use() {
        let binding = BindingId(1);
        let program = SemanticProgram {
            stmts: vec![
                SemanticStmt::Decl {
                    binding,
                    name: "x".to_string(),
                    ty: Some(SemanticType::I64),
                    pos: 0,
                },
                SemanticStmt::ExprStmt {
                    expr: binding_ref(binding, "x", SemanticType::I64),
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        match lower_program(&program).expect_err("lowering should fail") {
            LoweringError::InternalInvariantViolation { detail } => {
                assert!(detail.contains("referenced before any SSA value was assigned"));
            }
            other => panic!("expected invariant error, got {other:?}"),
        }
    }

    #[test]
    fn rejects_unsupported_statement() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::WhileIn {
                label: None,
                arr: "arr".to_string(),
                start_slot: 0,
                range_start: int_expr(0, SemanticType::I64),
                range_end: int_expr(10, SemanticType::I64),
                inclusive: false,
                body: vec![],
                then_chains: vec![],
                result: None,
                pos: 0,
            }],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticConstruct {
                construct: "WhileIn".to_string()
            }
        );
    }

    #[test]
    fn rejects_unsupported_statement_inside_function() {
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "bad",
                vec![],
                None,
                vec![SemanticStmt::WhileIn {
                    label: None,
                    arr: "arr".to_string(),
                    start_slot: 0,
                    range_start: int_expr(0, SemanticType::I64),
                    range_end: int_expr(10, SemanticType::I64),
                    inclusive: false,
                    body: vec![],
                    then_chains: vec![],
                    result: None,
                    pos: 0,
                }],
                None,
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticConstruct {
                construct: "WhileIn".to_string()
            }
        );
    }

    #[test]
    fn empty_program_lowers_to_empty_module() {
        let module = lower_and_validate(&SemanticProgram {
            stmts: vec![],
            enums: vec![],
        });

        assert!(module.functions.is_empty());
    }

    #[test]
    fn lowers_plain_assign_using_resolved_binding_target() {
        let binding = BindingId(3);
        let program = SemanticProgram {
            stmts: vec![
                SemanticStmt::Decl {
                    binding,
                    name: "x".to_string(),
                    ty: Some(SemanticType::I64),
                    pos: 0,
                },
                SemanticStmt::Assign {
                    target: SemanticLValue::Binding {
                        binding,
                        name: "x".to_string(),
                        ty: SemanticType::I64,
                    },
                    expr: int_expr(9, SemanticType::I64),
                    pos_eq: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert!(module.functions[0].blocks[0]
            .insts
            .iter()
            .any(|inst| matches!(
                inst,
                IrInst::SsaBind {
                    ty: IrType::I64,
                    ..
                }
            )));
    }

    #[test]
    fn bool_constants_lower_cleanly() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(1),
                "flag",
                SemanticType::Bool,
                bool_expr(true),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert!(module.functions[0].blocks[0].insts.iter().any(|inst| {
            matches!(
                inst,
                IrInst::ConstInt {
                    ty: IrType::Bool,
                    value: 1,
                    ..
                }
            )
        }));
    }

    #[test]
    fn declared_but_never_assigned_binding_use_inside_function_is_rejected() {
        let binding = BindingId(50);
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "bad_ref",
                vec![],
                Some(SemanticType::I64),
                vec![SemanticStmt::Decl {
                    binding,
                    name: "x".to_string(),
                    ty: Some(SemanticType::I64),
                    pos: 0,
                }],
                Some(binding_ref(binding, "x", SemanticType::I64)),
            )],
            enums: vec![],
        };

        match lower_program(&program).expect_err("lowering should fail") {
            LoweringError::InternalInvariantViolation { detail } => {
                assert!(detail.contains("referenced before any SSA value was assigned"));
            }
            other => panic!("expected invariant error, got {other:?}"),
        }
    }

    #[test]
    fn function_local_ssa_maps_do_not_leak_between_functions_and_main() {
        let function_binding = BindingId(60);
        let top_level_binding = BindingId(61);
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "f",
                    vec![typed_param(function_binding, "x", SemanticType::I64)],
                    Some(SemanticType::I64),
                    vec![],
                    Some(binding_ref(function_binding, "x", SemanticType::I64)),
                ),
                SemanticStmt::ExprStmt {
                    expr: binding_ref(function_binding, "x", SemanticType::I64),
                    pos: 0,
                },
                assign(
                    top_level_binding,
                    "y",
                    SemanticType::I64,
                    int_expr(1, SemanticType::I64),
                ),
            ],
            enums: vec![],
        };

        match lower_program(&program).expect_err("lowering should fail") {
            LoweringError::InternalInvariantViolation { detail } => {
                assert!(detail.contains("referenced before any SSA value was assigned"));
            }
            other => panic!("expected invariant error, got {other:?}"),
        }
    }

    #[test]
    fn function_local_ssa_maps_do_not_leak_between_functions() {
        let shared_binding = BindingId(70);
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "f",
                    vec![typed_param(shared_binding, "x", SemanticType::I64)],
                    Some(SemanticType::I64),
                    vec![],
                    Some(binding_ref(shared_binding, "x", SemanticType::I64)),
                ),
                semantic_function(
                    "g",
                    vec![],
                    Some(SemanticType::I64),
                    vec![],
                    Some(binding_ref(shared_binding, "x", SemanticType::I64)),
                ),
            ],
            enums: vec![],
        };

        match lower_program(&program).expect_err("lowering should fail") {
            LoweringError::InternalInvariantViolation { detail } => {
                assert!(detail.contains("referenced before any SSA value was assigned"));
            }
            other => panic!("expected invariant error, got {other:?}"),
        }
    }

    #[test]
    fn top_level_if_lowers_into_valid_multi_block_cfg() {
        let program = SemanticProgram {
            stmts: vec![if_stmt(
                bool_expr(true),
                vec![typed_assign(
                    BindingId(80),
                    "x",
                    SemanticType::I64,
                    int_expr(1, SemanticType::I64),
                )],
                vec![],
                None,
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let main = &module.functions[0];
        assert_eq!(main.name, "main");
        assert!(main.blocks.len() >= 3);
        assert!(main.blocks.iter().any(|block| matches!(
            block.term,
            IrTerminator::Branch { .. }
        )));
    }

    #[test]
    fn top_level_if_else_lowers_and_validates() {
        let program = SemanticProgram {
            stmts: vec![if_stmt(
                bool_expr(true),
                vec![typed_assign(
                    BindingId(81),
                    "x",
                    SemanticType::I64,
                    int_expr(1, SemanticType::I64),
                )],
                vec![],
                Some(vec![typed_assign(
                    BindingId(81),
                    "x",
                    SemanticType::I64,
                    int_expr(2, SemanticType::I64),
                )]),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let main = &module.functions[0];
        assert!(main.blocks.iter().any(|block| matches!(
            block.term,
            IrTerminator::Branch { .. }
        )));
        assert!(main.blocks.iter().any(|block| matches!(
            block.term,
            IrTerminator::Jump { .. }
        )));
    }

    #[test]
    fn top_level_if_else_if_else_lowers_and_validates() {
        let program = SemanticProgram {
            stmts: vec![if_stmt(
                bool_expr(false),
                vec![typed_assign(
                    BindingId(82),
                    "x",
                    SemanticType::I64,
                    int_expr(1, SemanticType::I64),
                )],
                vec![(
                    bool_expr(true),
                    vec![typed_assign(
                        BindingId(82),
                        "x",
                        SemanticType::I64,
                        int_expr(2, SemanticType::I64),
                    )],
                )],
                Some(vec![typed_assign(
                    BindingId(82),
                    "x",
                    SemanticType::I64,
                    int_expr(3, SemanticType::I64),
                )]),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let branch_count = module.functions[0]
            .blocks
            .iter()
            .filter(|block| matches!(block.term, IrTerminator::Branch { .. }))
            .count();
        assert!(branch_count >= 2);
    }

    #[test]
    fn function_body_if_else_lowers_and_validates() {
        let cond = BindingId(83);
        let out = BindingId(84);
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "choose",
                vec![typed_param(cond, "cond", SemanticType::Bool)],
                Some(SemanticType::I64),
                vec![
                    typed_assign(out, "out", SemanticType::I64, int_expr(0, SemanticType::I64)),
                    if_stmt(
                        binding_ref(cond, "cond", SemanticType::Bool),
                        vec![assign(
                            out,
                            "out",
                            SemanticType::I64,
                            int_expr(1, SemanticType::I64),
                        )],
                        vec![],
                        Some(vec![assign(
                            out,
                            "out",
                            SemanticType::I64,
                            int_expr(2, SemanticType::I64),
                        )]),
                    ),
                ],
                Some(binding_ref(out, "out", SemanticType::I64)),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let function = &module.functions[0];
        assert!(function.blocks.len() >= 4);
        assert!(function.blocks.iter().any(|block| matches!(
            block.term,
            IrTerminator::Branch { .. }
        )));
    }

    #[test]
    fn assignment_in_both_branches_merges_correctly() {
        let cond = BindingId(85);
        let x = BindingId(86);
        let program = SemanticProgram {
            stmts: vec![
                typed_assign(cond, "cond", SemanticType::Bool, bool_expr(true)),
                typed_assign(x, "x", SemanticType::I64, int_expr(0, SemanticType::I64)),
                if_stmt(
                    binding_ref(cond, "cond", SemanticType::Bool),
                    vec![assign(
                        x,
                        "x",
                        SemanticType::I64,
                        int_expr(1, SemanticType::I64),
                    )],
                    vec![],
                    Some(vec![assign(
                        x,
                        "x",
                        SemanticType::I64,
                        int_expr(2, SemanticType::I64),
                    )]),
                ),
                SemanticStmt::ExprStmt {
                    expr: binding_ref(x, "x", SemanticType::I64),
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert!(module.functions[0]
            .blocks
            .iter()
            .any(|block| block.params.len() == 1));
        assert!(module.functions[0].blocks.iter().any(|block| matches!(
            block.term,
            IrTerminator::Jump { ref args, .. } if args.len() == 1
        )));
    }

    #[test]
    fn unchanged_bindings_across_branches_stay_valid_after_merge() {
        let cond = BindingId(87);
        let x = BindingId(88);
        let program = SemanticProgram {
            stmts: vec![
                typed_assign(cond, "cond", SemanticType::Bool, bool_expr(true)),
                typed_assign(x, "x", SemanticType::I64, int_expr(5, SemanticType::I64)),
                if_stmt(
                    binding_ref(cond, "cond", SemanticType::Bool),
                    vec![SemanticStmt::ExprStmt {
                        expr: bool_expr(true),
                        pos: 0,
                    }],
                    vec![],
                    Some(vec![SemanticStmt::ExprStmt {
                        expr: bool_expr(false),
                        pos: 0,
                    }]),
                ),
                SemanticStmt::ExprStmt {
                    expr: binding_ref(x, "x", SemanticType::I64),
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert!(module.functions[0]
            .blocks
            .iter()
            .all(|block| block.params.is_empty()));
    }

    #[test]
    fn one_branch_assignment_merges_with_incoming_value() {
        let cond = BindingId(89);
        let x = BindingId(90);
        let program = SemanticProgram {
            stmts: vec![
                typed_assign(cond, "cond", SemanticType::Bool, bool_expr(true)),
                typed_assign(x, "x", SemanticType::I64, int_expr(0, SemanticType::I64)),
                if_stmt(
                    binding_ref(cond, "cond", SemanticType::Bool),
                    vec![assign(
                        x,
                        "x",
                        SemanticType::I64,
                        int_expr(1, SemanticType::I64),
                    )],
                    vec![],
                    None,
                ),
                SemanticStmt::ExprStmt {
                    expr: binding_ref(x, "x", SemanticType::I64),
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        assert!(module.functions[0]
            .blocks
            .iter()
            .any(|block| block.params.len() == 1));
    }

    #[test]
    fn one_branch_returns_and_other_falls_through_lowers_correctly() {
        let cond = BindingId(91);
        let out = BindingId(92);
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "branch_return",
                vec![typed_param(cond, "cond", SemanticType::Bool)],
                Some(SemanticType::I64),
                vec![if_stmt(
                    binding_ref(cond, "cond", SemanticType::Bool),
                    vec![SemanticStmt::Return {
                        expr: Some(int_expr(1, SemanticType::I64)),
                        pos: 0,
                    }],
                    vec![],
                    Some(vec![typed_assign(
                        out,
                        "out",
                        SemanticType::I64,
                        int_expr(2, SemanticType::I64),
                    )]),
                )],
                Some(binding_ref(out, "out", SemanticType::I64)),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let function = &module.functions[0];
        let return_some_count = function
            .blocks
            .iter()
            .filter(|block| matches!(block.term, IrTerminator::Return { value: Some(_) }))
            .count();
        assert!(return_some_count >= 2);
    }

    #[test]
    fn both_branches_return_lowers_correctly_without_invalid_merge() {
        let cond = BindingId(93);
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "both_return",
                vec![typed_param(cond, "cond", SemanticType::Bool)],
                Some(SemanticType::I64),
                vec![if_stmt(
                    binding_ref(cond, "cond", SemanticType::Bool),
                    vec![SemanticStmt::Return {
                        expr: Some(int_expr(1, SemanticType::I64)),
                        pos: 0,
                    }],
                    vec![],
                    Some(vec![SemanticStmt::Return {
                        expr: Some(int_expr(2, SemanticType::I64)),
                        pos: 0,
                    }]),
                )],
                None,
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let function = &module.functions[0];
        assert_eq!(function.blocks.len(), 3);
        assert!(function.blocks.iter().skip(1).all(|block| block.params.is_empty()));
    }

    #[test]
    fn nested_if_lowers_correctly() {
        let cond_a = BindingId(94);
        let cond_b = BindingId(95);
        let x = BindingId(96);
        let program = SemanticProgram {
            stmts: vec![
                typed_assign(cond_a, "a", SemanticType::Bool, bool_expr(true)),
                typed_assign(cond_b, "b", SemanticType::Bool, bool_expr(false)),
                typed_assign(x, "x", SemanticType::I64, int_expr(0, SemanticType::I64)),
                if_stmt(
                    binding_ref(cond_a, "a", SemanticType::Bool),
                    vec![if_stmt(
                        binding_ref(cond_b, "b", SemanticType::Bool),
                        vec![assign(
                            x,
                            "x",
                            SemanticType::I64,
                            int_expr(1, SemanticType::I64),
                        )],
                        vec![],
                        Some(vec![assign(
                            x,
                            "x",
                            SemanticType::I64,
                            int_expr(2, SemanticType::I64),
                        )]),
                    )],
                    vec![],
                    Some(vec![assign(
                        x,
                        "x",
                        SemanticType::I64,
                        int_expr(3, SemanticType::I64),
                    )]),
                ),
                SemanticStmt::ExprStmt {
                    expr: binding_ref(x, "x", SemanticType::I64),
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let branch_count = module.functions[0]
            .blocks
            .iter()
            .filter(|block| matches!(block.term, IrTerminator::Branch { .. }))
            .count();
        assert!(branch_count >= 2);
    }

    #[test]
    fn rejects_unsupported_statement_inside_if_branch() {
        let program = SemanticProgram {
            stmts: vec![if_stmt(
                bool_expr(true),
                vec![SemanticStmt::WhileIn {
                    label: None,
                    arr: "arr".to_string(),
                    start_slot: 0,
                    range_start: int_expr(0, SemanticType::I64),
                    range_end: int_expr(10, SemanticType::I64),
                    inclusive: false,
                    body: vec![],
                    then_chains: vec![],
                    result: None,
                    pos: 0,
                }],
                vec![],
                None,
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticConstruct {
                construct: "WhileIn".to_string()
            }
        );
    }

    #[test]
    fn lowers_simple_while_loop() {
        let program = SemanticProgram {
            stmts: vec![
                SemanticStmt::TypedAssign {
                    binding: BindingId(0),
                    name: "x".to_string(),
                    ty: SemanticType::I64,
                    expr: int_expr(0, SemanticType::I64),
                    pos_type: 0,
                },
                SemanticStmt::While {
                    label: None,
                    cond: bool_expr(true),
                    body: vec![],
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        // Expect at least 4 blocks: entry, header, body, exit
        assert!(main_fn.blocks.len() >= 4, "expected at least 4 blocks, got {}", main_fn.blocks.len());
        // Header block should have a Branch terminator
        let header = &main_fn.blocks[1];
        assert!(matches!(header.term, IrTerminator::Branch { .. }), "header should branch");
        // Body block should jump back to header (backedge)
        let body = &main_fn.blocks[2];
        match &body.term {
            IrTerminator::Jump { target, .. } => {
                assert_eq!(*target, header.id, "body should jump back to header");
            }
            _ => panic!("body should have Jump terminator"),
        }
    }

    #[test]
    fn lowers_while_inside_function() {
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "looper",
                vec![],
                Some(SemanticType::I64),
                vec![SemanticStmt::While {
                    label: None,
                    cond: bool_expr(true),
                    body: vec![],
                    pos: 0,
                }],
                Some(int_expr(0, SemanticType::I64)),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let func = module.functions.iter().find(|f| f.name == "looper").unwrap();
        assert!(func.blocks.len() >= 4, "expected at least 4 blocks, got {}", func.blocks.len());
    }

    #[test]
    fn while_loop_header_has_block_params() {
        let program = SemanticProgram {
            stmts: vec![
                SemanticStmt::TypedAssign {
                    binding: BindingId(0),
                    name: "x".to_string(),
                    ty: SemanticType::I64,
                    expr: int_expr(0, SemanticType::I64),
                    pos_type: 0,
                },
                SemanticStmt::While {
                    label: None,
                    cond: bool_expr(true),
                    body: vec![SemanticStmt::Assign {
                        target: SemanticLValue::Binding {
                            binding: BindingId(0),
                            name: "x".to_string(),
                            ty: SemanticType::I64,
                        },
                        expr: int_expr(1, SemanticType::I64),
                        pos_eq: 0,
                    }],
                    pos: 0,
                },
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let header = &main_fn.blocks[1];
        // Header should have block params for the loop-carried binding
        assert!(!header.params.is_empty(), "header should have block params for loop-carried values");
    }

    #[test]
    fn lowers_simple_for_loop() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::For {
                label: None,
                binding: BindingId(0),
                var: "i".to_string(),
                start: int_expr(0, SemanticType::I64),
                end: int_expr(5, SemanticType::I64),
                inclusive: false,
                body: vec![],
                pos: 0,
            }],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        assert!(main_fn.blocks.len() >= 4);
    }

    #[test]
    fn lowers_inclusive_for_loop() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::For {
                label: None,
                binding: BindingId(0),
                var: "i".to_string(),
                start: int_expr(1, SemanticType::I64),
                end: int_expr(10, SemanticType::I64),
                inclusive: true,
                body: vec![],
                pos: 0,
            }],
            enums: vec![],
        };
        let _ = lower_and_validate(&program);
    }

    #[test]
    fn lowers_for_with_break() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::For {
                label: None,
                binding: BindingId(0),
                var: "i".to_string(),
                start: int_expr(0, SemanticType::I64),
                end: int_expr(10, SemanticType::I64),
                inclusive: false,
                body: vec![SemanticStmt::Break { label: None, pos: 0 }],
                pos: 0,
            }],
            enums: vec![],
        };
        let _ = lower_and_validate(&program);
    }

    #[test]
    fn lowers_for_with_continue() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::For {
                label: None,
                binding: BindingId(0),
                var: "i".to_string(),
                start: int_expr(0, SemanticType::I64),
                end: int_expr(10, SemanticType::I64),
                inclusive: false,
                body: vec![SemanticStmt::Continue { label: None, pos: 0 }],
                pos: 0,
            }],
            enums: vec![],
        };
        let _ = lower_and_validate(&program);
    }

    #[test]
    fn lowers_conditional_return_inside_while() {
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "looper",
                vec![],
                Some(SemanticType::I64),
                vec![SemanticStmt::While {
                    label: None,
                    cond: bool_expr(true),
                    body: vec![if_stmt(
                        bool_expr(true),
                        vec![SemanticStmt::Return {
                            expr: Some(int_expr(7, SemanticType::I64)),
                            pos: 0,
                        }],
                        vec![],
                        None,
                    )],
                    pos: 0,
                }],
                Some(int_expr(0, SemanticType::I64)),
            )],
            enums: vec![],
        };
        let _ = lower_and_validate(&program);
    }

    #[test]
    fn lowers_unary_negate_int() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::TypedAssign {
                binding: BindingId(0),
                name: "x".to_string(),
                ty: SemanticType::I64,
                expr: SemanticExpr {
                    ty: SemanticType::I64,
                    kind: SemanticExprKind::Unary {
                        op: Op::Minus,
                        expr: Box::new(int_expr(42, SemanticType::I64)),
                        pos: 0,
                    },
                },
                pos_type: 0,
            }],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let has_sub = main_fn.blocks.iter().flat_map(|b| b.insts.iter()).any(|inst| {
            matches!(inst, IrInst::Binary { op: BinaryOp::Sub, .. })
        });
        assert!(has_sub, "negate should lower to 0 - value");
    }

    #[test]
    fn lowers_unary_negate_float() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::TypedAssign {
                binding: BindingId(0),
                name: "x".to_string(),
                ty: SemanticType::F64,
                expr: SemanticExpr {
                    ty: SemanticType::F64,
                    kind: SemanticExprKind::Unary {
                        op: Op::Minus,
                        expr: Box::new(float_expr(3.14)),
                        pos: 0,
                    },
                },
                pos_type: 0,
            }],
            enums: vec![],
        };
        let _ = lower_and_validate(&program);
    }

    #[test]
    fn lowers_unary_not_bool() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::TypedAssign {
                binding: BindingId(0),
                name: "x".to_string(),
                ty: SemanticType::Bool,
                expr: SemanticExpr {
                    ty: SemanticType::Bool,
                    kind: SemanticExprKind::Unary {
                        op: Op::Not,
                        expr: Box::new(bool_expr(true)),
                        pos: 0,
                    },
                },
                pos_type: 0,
            }],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let has_cmp = main_fn.blocks.iter().flat_map(|b| b.insts.iter()).any(|inst| {
            matches!(inst, IrInst::Compare { op: CompareOp::Eq, .. })
        });
        assert!(has_cmp, "not should lower to value == 0");
    }

    #[test]
    fn rejects_unsupported_unary_op() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::TypedAssign {
                binding: BindingId(0),
                name: "x".to_string(),
                ty: SemanticType::I64,
                expr: SemanticExpr {
                    ty: SemanticType::I64,
                    kind: SemanticExprKind::Unary {
                        op: Op::Mul,
                        expr: Box::new(int_expr(5, SemanticType::I64)),
                        pos: 0,
                    },
                },
                pos_type: 0,
            }],
            enums: vec![],
        };
        assert!(lower_program(&program).is_err());
    }

    #[test]
    fn lowers_compound_assign_add() {
        let program = SemanticProgram {
            stmts: vec![
                SemanticStmt::TypedAssign {
                    binding: BindingId(0),
                    name: "x".to_string(),
                    ty: SemanticType::I64,
                    expr: int_expr(10, SemanticType::I64),
                    pos_type: 0,
                },
                SemanticStmt::CompoundAssign {
                    target: SemanticLValue::Binding {
                        binding: BindingId(0),
                        name: "x".to_string(),
                        ty: SemanticType::I64,
                    },
                    op: Op::Plus,
                    operand: int_expr(5, SemanticType::I64),
                    pos: 0,
                },
            ],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let has_add = main_fn.blocks.iter().flat_map(|b| b.insts.iter()).any(|inst| {
            matches!(inst, IrInst::Binary { op: BinaryOp::Add, .. })
        });
        assert!(has_add, "compound += should emit Binary::Add");
    }

    #[test]
    fn lowers_compound_assign_sub() {
        let program = SemanticProgram {
            stmts: vec![
                SemanticStmt::TypedAssign {
                    binding: BindingId(0),
                    name: "x".to_string(),
                    ty: SemanticType::I64,
                    expr: int_expr(10, SemanticType::I64),
                    pos_type: 0,
                },
                SemanticStmt::CompoundAssign {
                    target: SemanticLValue::Binding {
                        binding: BindingId(0),
                        name: "x".to_string(),
                        ty: SemanticType::I64,
                    },
                    op: Op::Minus,
                    operand: int_expr(3, SemanticType::I64),
                    pos: 0,
                },
            ],
            enums: vec![],
        };
        let _ = lower_and_validate(&program);
    }

    #[test]
    fn rejects_compound_assign_dot_access_with_no_binding() {
        // binding: None means the semantic analyser failed to resolve the container —
        // lowering must reject this even though DotAccess is now otherwise supported.
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::CompoundAssign {
                target: SemanticLValue::DotAccess {
                    binding: None,
                    container: "obj".to_string(),
                    field: "x".to_string(),
                    ty: SemanticType::I64,
                    struct_name: "Point".to_string(),
                },
                op: Op::Plus,
                operand: int_expr(1, SemanticType::I64),
                pos: 0,
            }],
            enums: vec![],
        };
        assert!(lower_program(&program).is_err());
    }

    #[test]
    fn rejects_unsupported_semantic_type_inside_if_branch() {
        let program = SemanticProgram {
            stmts: vec![if_stmt(
                bool_expr(true),
                vec![typed_assign(
                    BindingId(97),
                    "x",
                    SemanticType::Numeric,
                    SemanticExpr {
                        ty: SemanticType::Numeric,
                        kind: SemanticExprKind::Value(SemanticValue::Num(1)),
                    },
                )],
                vec![],
                None,
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticType {
                ty: "Numeric".to_string()
            }
        );
    }

    #[test]
    fn rejects_unresolved_artifact_inside_if_branch() {
        let program = SemanticProgram {
            stmts: vec![if_stmt(
                bool_expr(true),
                vec![SemanticStmt::CompoundAssign {
                    target: SemanticLValue::Binding {
                        binding: BindingId(98),
                        name: "x".to_string(),
                        ty: SemanticType::I64,
                    },
                    op: Op::Plus,
                    operand: int_expr(1, SemanticType::I64),
                    pos: 0,
                }],
                vec![],
                None,
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnresolvedSemanticArtifact {
                artifact: "compound assign binding BindingId(98)".to_string()
            }
        );
    }

    // ── struct field read tests ───────────────────────────────────────────────

    fn point_struct_def() -> SemanticStmt {
        SemanticStmt::StructDef {
            name: "Point".to_string(),
            type_params: vec![],
            fields: vec![
                ("x".to_string(), SemanticType::I64),
                ("y".to_string(), SemanticType::I64),
            ],
            pos: 0,
        }
    }

    fn dot_access_expr(
        binding: BindingId,
        container: &str,
        field: &str,
        struct_name: &str,
        field_ty: SemanticType,
    ) -> SemanticExpr {
        SemanticExpr {
            ty: field_ty,
            kind: SemanticExprKind::DotAccess {
                binding: Some(binding),
                container: container.to_string(),
                field: field.to_string(),
                struct_name: struct_name.to_string(),
            },
        }
    }

    // Reading the first field (`x` at offset 0) should emit a single Load —
    // no PtrOffset is needed because the base pointer already addresses the field.
    #[test]
    fn lowers_dot_access_first_field_emits_load_only() {
        // fn read_x(p: Point) -> i64 { p.x }
        let program = SemanticProgram {
            stmts: vec![
                point_struct_def(),
                semantic_function(
                    "read_x",
                    vec![typed_param(
                        BindingId(0),
                        "p",
                        SemanticType::Struct("Point".to_string()),
                    )],
                    Some(SemanticType::I64),
                    vec![],
                    Some(dot_access_expr(
                        BindingId(0),
                        "p",
                        "x",
                        "Point",
                        SemanticType::I64,
                    )),
                ),
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let func = module.functions.iter().find(|f| f.name == "read_x").unwrap();
        let insts: Vec<&IrInst> = func.blocks.iter().flat_map(|b| b.insts.iter()).collect();

        // Must have a Load for i64
        let has_load = insts.iter().any(|i| matches!(i, IrInst::Load { ty: IrType::I64, .. }));
        assert!(has_load, "expected Load i64 for first-field DotAccess");

        // Must NOT have a PtrOffset — first field is at offset 0
        let has_ptr_offset = insts.iter().any(|i| matches!(i, IrInst::PtrOffset { .. }));
        assert!(!has_ptr_offset, "unexpected PtrOffset for first field (offset 0)");
    }

    // Reading the second field (`y` at offset 8) must emit PtrOffset(8) then Load.
    // Point { x: i64, y: i64 } — y is at byte offset 8 after alignment padding.
    #[test]
    fn lowers_dot_access_second_field_emits_ptr_offset_then_load() {
        // fn read_y(p: Point) -> i64 { p.y }
        let program = SemanticProgram {
            stmts: vec![
                point_struct_def(),
                semantic_function(
                    "read_y",
                    vec![typed_param(
                        BindingId(0),
                        "p",
                        SemanticType::Struct("Point".to_string()),
                    )],
                    Some(SemanticType::I64),
                    vec![],
                    Some(dot_access_expr(
                        BindingId(0),
                        "p",
                        "y",
                        "Point",
                        SemanticType::I64,
                    )),
                ),
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let func = module.functions.iter().find(|f| f.name == "read_y").unwrap();
        let insts: Vec<&IrInst> = func.blocks.iter().flat_map(|b| b.insts.iter()).collect();

        // Must have PtrOffset with offset 8
        let has_ptr_offset = insts
            .iter()
            .any(|i| matches!(i, IrInst::PtrOffset { offset: 8, .. }));
        assert!(
            has_ptr_offset,
            "expected PtrOffset(8) for second field of Point"
        );

        // Must have a Load for i64
        let has_load = insts.iter().any(|i| matches!(i, IrInst::Load { ty: IrType::I64, .. }));
        assert!(has_load, "expected Load i64 after PtrOffset for second field");
    }

    // Reading a f64 field verifies correct IR type propagation.
    #[test]
    fn lowers_dot_access_f64_field_produces_f64_load() {
        // struct Vec2 { dx: f64, dy: f64 }
        // fn get_dx(v: Vec2) -> f64 { v.dx }
        let program = SemanticProgram {
            stmts: vec![
                SemanticStmt::StructDef {
                    name: "Vec2".to_string(),
                    type_params: vec![],
                    fields: vec![
                        ("dx".to_string(), SemanticType::F64),
                        ("dy".to_string(), SemanticType::F64),
                    ],
                    pos: 0,
                },
                semantic_function(
                    "get_dx",
                    vec![typed_param(
                        BindingId(0),
                        "v",
                        SemanticType::Struct("Vec2".to_string()),
                    )],
                    Some(SemanticType::F64),
                    vec![],
                    Some(dot_access_expr(
                        BindingId(0),
                        "v",
                        "dx",
                        "Vec2",
                        SemanticType::F64,
                    )),
                ),
            ],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let func = module.functions.iter().find(|f| f.name == "get_dx").unwrap();
        let has_f64_load = func
            .blocks
            .iter()
            .flat_map(|b| b.insts.iter())
            .any(|i| matches!(i, IrInst::Load { ty: IrType::F64, .. }));
        assert!(has_f64_load, "expected Load f64 for f64 DotAccess");
    }

    // Reject DotAccess when the binding is missing (None) — lowering cannot
    // resolve the base pointer without a BindingId.
    #[test]
    fn rejects_dot_access_with_no_binding() {
        let program = SemanticProgram {
            stmts: vec![
                point_struct_def(),
                SemanticStmt::ExprStmt {
                    expr: SemanticExpr {
                        ty: SemanticType::I64,
                        kind: SemanticExprKind::DotAccess {
                            binding: None,
                            container: "p".to_string(),
                            field: "x".to_string(),
                            struct_name: "Point".to_string(),
                        },
                    },
                    pos: 0,
                },
            ],
            enums: vec![],
        };
        assert!(lower_program(&program).is_err());
    }

    // Reject DotAccess when struct_name is empty (unknown container type).
    #[test]
    fn rejects_dot_access_with_unknown_struct_name() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::ExprStmt {
                expr: SemanticExpr {
                    ty: SemanticType::I64,
                    kind: SemanticExprKind::DotAccess {
                        binding: Some(BindingId(0)),
                        container: "x".to_string(),
                        field: "val".to_string(),
                        struct_name: String::new(),
                    },
                },
                pos: 0,
            }],
            enums: vec![],
        };
        assert!(lower_program(&program).is_err());
    }

    // Reject DotAccess when the named struct does not exist in the program.
    #[test]
    fn rejects_dot_access_with_missing_struct_def() {
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::ExprStmt {
                expr: SemanticExpr {
                    ty: SemanticType::I64,
                    kind: SemanticExprKind::DotAccess {
                        binding: Some(BindingId(0)),
                        container: "p".to_string(),
                        field: "x".to_string(),
                        struct_name: "Ghost".to_string(),
                    },
                },
                pos: 0,
            }],
            enums: vec![],
        };
        assert!(lower_program(&program).is_err());
    }

    // ── array literal and index lowering tests ────────────────────────────────

    // Helper: build an ArrayLit SemanticExpr with N integer elements.
    fn array_lit_i64(elements: Vec<i128>) -> SemanticExpr {
        let count = elements.len();
        SemanticExpr {
            ty: SemanticType::Array(count, Box::new(SemanticType::I64)),
            kind: SemanticExprKind::ArrayLit {
                elements: elements
                    .into_iter()
                    .map(|v| int_expr(v, SemanticType::I64))
                    .collect(),
            },
        }
    }

    // Helper: build an ArrayLit SemanticExpr with N Numeric-typed elements,
    // matching what the semantic layer emits for untyped integer literals.
    fn array_lit_numeric(elements: Vec<i128>) -> SemanticExpr {
        let count = elements.len();
        SemanticExpr {
            ty: SemanticType::Array(count, Box::new(SemanticType::Numeric)),
            kind: SemanticExprKind::ArrayLit {
                elements: elements
                    .into_iter()
                    .map(|v| int_expr(v, SemanticType::Numeric))
                    .collect(),
            },
        }
    }

    // Helper: build an Index SemanticExpr.
    fn index_expr(
        target: SemanticExpr,
        index: SemanticExpr,
        elem_ty: SemanticType,
    ) -> SemanticExpr {
        SemanticExpr {
            ty: elem_ty,
            kind: SemanticExprKind::Index {
                target: Box::new(target),
                index: Box::new(index),
                pos: 0,
            },
        }
    }

    // ArrayLit lowering: a three-element i64 array must emit exactly one ArrayAlloca
    // (for the whole array), two PtrOffset instructions (elements 1 and 2 —
    // element 0 is at offset 0 so no PtrOffset), and three Stores.
    #[test]
    fn lowers_array_lit_emits_alloca_ptr_offset_store() {
        let arr = array_lit_i64(vec![10, 20, 30]);
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "arr",
                SemanticType::Array(3, Box::new(SemanticType::I64)),
                arr,
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let insts: Vec<&IrInst> = module.functions[0]
            .blocks
            .iter()
            .flat_map(|b| b.insts.iter())
            .collect();

        // One ArrayAlloca for the array storage (3 x I64).
        let alloca_count = insts
            .iter()
            .filter(|i| matches!(i, IrInst::ArrayAlloca { element_type: IrType::I64, count: 3, .. }))
            .count();
        assert_eq!(alloca_count, 1, "expected exactly one ArrayAlloca(I64, 3)");

        // Three Store instructions — one per element.
        let store_count = insts.iter().filter(|i| matches!(i, IrInst::Store { .. })).count();
        assert_eq!(store_count, 3, "expected three Store instructions");

        // PtrOffset for elements at non-zero offsets: offsets 8 and 16.
        let has_ptr_offset_8 = insts
            .iter()
            .any(|i| matches!(i, IrInst::PtrOffset { offset: 8, .. }));
        let has_ptr_offset_16 = insts
            .iter()
            .any(|i| matches!(i, IrInst::PtrOffset { offset: 16, .. }));
        assert!(has_ptr_offset_8, "expected PtrOffset(8) for second element");
        assert!(has_ptr_offset_16, "expected PtrOffset(16) for third element");

        // No PtrOffset for element 0 (offset 0).
        let has_ptr_offset_0 = insts
            .iter()
            .any(|i| matches!(i, IrInst::PtrOffset { offset: 0, .. }));
        assert!(!has_ptr_offset_0, "unexpected PtrOffset(0) for first element");
    }

    // ArrayLit with Numeric placeholder element type must still emit one
    // ArrayAlloca (using the target's default integer width), locking in the
    // SemanticType::Numeric fallback introduced in lower_array_lit.
    #[test]
    fn lowers_array_lit_numeric_elem_uses_default_int() {
        let arr = array_lit_numeric(vec![10, 20, 30]);
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "arr",
                SemanticType::Array(3, Box::new(SemanticType::Numeric)),
                arr,
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let insts: Vec<&IrInst> = module.functions[0]
            .blocks
            .iter()
            .flat_map(|b| b.insts.iter())
            .collect();

        // The fallback maps Numeric → I64 on a 64-bit target; count stays 3.
        let alloca_count = insts
            .iter()
            .filter(|i| matches!(i, IrInst::ArrayAlloca { element_type: IrType::I64, count: 3, .. }))
            .count();
        assert_eq!(alloca_count, 1, "expected exactly one ArrayAlloca(I64, 3) for Numeric element type");
    }

    // lower_type must map SemanticType::Array(_) to IrType::Ptr.
    #[test]
    fn lower_type_array_maps_to_ptr() {
        let result = lower_type(&SemanticType::Array(4, Box::new(SemanticType::I64)));
        assert_eq!(result, Ok(IrType::Ptr));
    }

    // lower_type must reject SemanticType::Void: Void is not storable and must
    // not leak into params, block params, locals, or SSA values.  The only
    // legitimate Void→IR path is `lower_return_type`, which canonicalises Void
    // to `None` for `IrFunction::return_ty`.
    #[test]
    fn lower_type_rejects_void_in_storable_position() {
        let result = lower_type(&SemanticType::Void);
        assert!(
            matches!(result, Err(LoweringError::InternalInvariantViolation { .. })),
            "lower_type(Void) must error in storable position, got {:?}",
            result
        );
    }

    // lower_return_type must canonicalise SemanticType::Void to None.
    // Non-Void types delegate to lower_type and are wrapped in Some.
    #[test]
    fn lower_return_type_void_canonicalises_to_none() {
        assert_eq!(lower_return_type(&SemanticType::Void), Ok(None));
        assert_eq!(lower_return_type(&SemanticType::I64), Ok(Some(IrType::I64)));
        assert_eq!(lower_return_type(&SemanticType::Bool), Ok(Some(IrType::Bool)));
    }

    // A user-defined void-return function (SemanticType::Void return type)
    // must lower to an IrFunction with return_ty: None (canonicalised).
    #[test]
    fn lower_semantic_function_void_return_canonicalises_to_none() {
        use crate::frontend::semantic_types::{
            FunctionId, SemanticFunction, SemanticStmt, SemanticType,
        };
        // Build a minimal program: fn do_nothing() -> void {}
        let func = SemanticFunction {
            id: FunctionId(0),
            name: "do_nothing".to_string(),
            type_params: vec![],
            params: vec![],
            return_ty: Some(SemanticType::Void),
            body: vec![],
            ret_expr: None,
            is_test: false,
            pos: 0,
        };
        let program = crate::frontend::semantic_types::SemanticProgram {
            stmts: vec![SemanticStmt::FuncDef(func)],
            enums: vec![],
        };
        let module = lower_program(&program).expect("lowering must succeed");
        assert_eq!(module.functions.len(), 1);
        let ir_func = &module.functions[0];
        assert_eq!(ir_func.name, "do_nothing");
        // Void return must be canonicalised to None, not Some(IrType::Void).
        assert_eq!(
            ir_func.return_ty, None,
            "void-return function must have return_ty: None in IR"
        );
    }

    // Index lowering on an i64 array with an i64 literal index must emit:
    // ConstInt (stride), Binary (Mul for byte_offset), PtrAdd, Load.
    #[test]
    fn lowers_index_emits_ptr_add_and_load() {
        // fn read_elem(arr: [3: i64]) -> i64 { arr[1] }
        let arr_binding = BindingId(0);
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "read_elem",
                vec![typed_param(
                    arr_binding,
                    "arr",
                    SemanticType::Array(3, Box::new(SemanticType::I64)),
                )],
                Some(SemanticType::I64),
                vec![],
                Some(index_expr(
                    binding_ref(
                        arr_binding,
                        "arr",
                        SemanticType::Array(3, Box::new(SemanticType::I64)),
                    ),
                    int_expr(1, SemanticType::I64),
                    SemanticType::I64,
                )),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let func = module.functions.iter().find(|f| f.name == "read_elem").unwrap();
        let insts: Vec<&IrInst> = func.blocks.iter().flat_map(|b| b.insts.iter()).collect();

        // Must have a PtrAdd (runtime pointer arithmetic for the index).
        let has_ptr_add = insts.iter().any(|i| matches!(i, IrInst::PtrAdd { .. }));
        assert!(has_ptr_add, "expected PtrAdd for indexed element access");

        // Must have a Load for i64 (reading the element).
        let has_load = insts.iter().any(|i| matches!(i, IrInst::Load { ty: IrType::I64, .. }));
        assert!(has_load, "expected Load i64 for indexed element");

        // Must have a Binary Mul for byte_offset computation.
        let has_mul = insts
            .iter()
            .any(|i| matches!(i, IrInst::Binary { op: BinaryOp::Mul, ty: IrType::I64, .. }));
        assert!(has_mul, "expected Binary(Mul, I64) for byte offset computation");

        // Must NOT have a PtrOffset (Index uses PtrAdd, not the static variant).
        let has_ptr_offset = insts.iter().any(|i| matches!(i, IrInst::PtrOffset { .. }));
        assert!(!has_ptr_offset, "unexpected PtrOffset in Index lowering");
    }

    // Index lowering on a single-element array (stride = element size, index 0)
    // must still emit PtrAdd — stride computation is always emitted.
    #[test]
    fn lowers_index_zero_on_single_element_array() {
        let arr_binding = BindingId(0);
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "read_only",
                vec![typed_param(
                    arr_binding,
                    "arr",
                    SemanticType::Array(1, Box::new(SemanticType::I32)),
                )],
                Some(SemanticType::I32),
                vec![],
                Some(index_expr(
                    binding_ref(
                        arr_binding,
                        "arr",
                        SemanticType::Array(1, Box::new(SemanticType::I32)),
                    ),
                    int_expr(0, SemanticType::I64),
                    SemanticType::I32,
                )),
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let func = module.functions.iter().find(|f| f.name == "read_only").unwrap();
        let insts: Vec<&IrInst> = func.blocks.iter().flat_map(|b| b.insts.iter()).collect();

        assert!(
            insts.iter().any(|i| matches!(i, IrInst::PtrAdd { .. })),
            "expected PtrAdd even for index 0"
        );
        assert!(
            insts.iter().any(|i| matches!(i, IrInst::Load { ty: IrType::I32, .. })),
            "expected Load i32 for i32 array element"
        );
    }

    // ArrayLit rejects a zero-length array.
    #[test]
    fn rejects_array_lit_zero_length() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "arr",
                SemanticType::Array(0, Box::new(SemanticType::I64)),
                SemanticExpr {
                    ty: SemanticType::Array(0, Box::new(SemanticType::I64)),
                    kind: SemanticExprKind::ArrayLit { elements: vec![] },
                },
            )],
            enums: vec![],
        };
        assert!(lower_program(&program).is_err());
    }

    // Index lowering rejects a non-Array target type.
    #[test]
    fn rejects_index_on_non_array_target() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "v",
                SemanticType::I64,
                index_expr(
                    int_expr(42, SemanticType::I64),
                    int_expr(0, SemanticType::I64),
                    SemanticType::I64,
                ),
            )],
            enums: vec![],
        };
        assert!(lower_program(&program).is_err());
    }

    // ── array element write lowering tests ───────────────────────────────────

    // Helper: build a SemanticLValue::Index for writing to an array element.
    fn index_lvalue(
        arr_binding: BindingId,
        arr_name: &str,
        arr_ty: SemanticType,
        index: SemanticExpr,
        elem_ty: SemanticType,
    ) -> SemanticLValue {
        SemanticLValue::Index {
            target: Box::new(binding_ref(arr_binding, arr_name, arr_ty)),
            index: Box::new(index),
            elem_ty,
        }
    }

    // A simple array element write `arr:[1] = 99` must emit:
    // ConstInt (stride), Binary (Mul for byte_offset), PtrAdd, Store.
    // No Load should appear (this is a plain write, not read-modify-write).
    #[test]
    fn lowers_array_element_write_emits_ptr_add_and_store() {
        // fn write_elem(arr: [3: i64]) { arr:[1] = 99 }
        let arr_binding = BindingId(0);
        let arr_ty = SemanticType::Array(3, Box::new(SemanticType::I64));
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "write_elem",
                vec![typed_param(arr_binding, "arr", arr_ty.clone())],
                None,
                vec![SemanticStmt::Assign {
                    target: index_lvalue(
                        arr_binding,
                        "arr",
                        arr_ty,
                        int_expr(1, SemanticType::I64),
                        SemanticType::I64,
                    ),
                    expr: int_expr(99, SemanticType::I64),
                    pos_eq: 0,
                }],
                None,
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let func = module.functions.iter().find(|f| f.name == "write_elem").unwrap();
        let insts: Vec<&IrInst> = func.blocks.iter().flat_map(|b| b.insts.iter()).collect();

        // Must have a PtrAdd (runtime pointer arithmetic for the index).
        assert!(
            insts.iter().any(|i| matches!(i, IrInst::PtrAdd { .. })),
            "expected PtrAdd for array element write"
        );

        // Must have a Store for i64 (writing the element).
        assert!(
            insts.iter().any(|i| matches!(i, IrInst::Store { .. })),
            "expected Store for array element write"
        );

        // Must have a Binary Mul for byte_offset computation.
        assert!(
            insts
                .iter()
                .any(|i| matches!(i, IrInst::Binary { op: BinaryOp::Mul, ty: IrType::I64, .. })),
            "expected Binary(Mul, I64) for byte offset computation"
        );

        // Must NOT have a Load (no read-modify-write for plain assignment).
        assert!(
            !insts.iter().any(|i| matches!(i, IrInst::Load { .. })),
            "unexpected Load in plain array element write"
        );
    }

    // A compound-assign array element write `arr:[0] += 5` must emit:
    // PtrAdd, Load (read current), Binary (Mul + Add), Store (write back).
    #[test]
    fn lowers_array_element_compound_assign_emits_load_binary_store() {
        // fn compound_write(arr: [2: i64]) { arr:[0] += 5 }
        let arr_binding = BindingId(0);
        let arr_ty = SemanticType::Array(2, Box::new(SemanticType::I64));
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "compound_write",
                vec![typed_param(arr_binding, "arr", arr_ty.clone())],
                None,
                vec![SemanticStmt::CompoundAssign {
                    target: index_lvalue(
                        arr_binding,
                        "arr",
                        arr_ty,
                        int_expr(0, SemanticType::I64),
                        SemanticType::I64,
                    ),
                    op: crate::frontend::ast::Op::Plus,
                    operand: int_expr(5, SemanticType::I64),
                    pos: 0,
                }],
                None,
            )],
            enums: vec![],
        };

        let module = lower_and_validate(&program);
        let func = module.functions.iter().find(|f| f.name == "compound_write").unwrap();
        let insts: Vec<&IrInst> = func.blocks.iter().flat_map(|b| b.insts.iter()).collect();

        // Must have a PtrAdd for element address computation.
        assert!(
            insts.iter().any(|i| matches!(i, IrInst::PtrAdd { .. })),
            "expected PtrAdd for array element compound assign"
        );

        // Must have a Load to read the current element value.
        assert!(
            insts.iter().any(|i| matches!(i, IrInst::Load { ty: IrType::I64, .. })),
            "expected Load i64 to read current element value"
        );

        // Must have a Binary Add for the compound operation.
        assert!(
            insts
                .iter()
                .any(|i| matches!(i, IrInst::Binary { op: BinaryOp::Add, ty: IrType::I64, .. })),
            "expected Binary(Add, I64) for compound assign"
        );

        // Must have a Store to write back the result.
        assert!(
            insts.iter().any(|i| matches!(i, IrInst::Store { .. })),
            "expected Store to write back element value"
        );
    }

    // Array element write rejects a non-Array target type.
    #[test]
    fn rejects_array_element_write_on_non_array_target() {
        // Assign to SemanticLValue::Index where target is not an array type.
        let program = SemanticProgram {
            stmts: vec![semantic_function(
                "bad_write",
                vec![],
                None,
                vec![SemanticStmt::Assign {
                    target: SemanticLValue::Index {
                        target: Box::new(int_expr(42, SemanticType::I64)),
                        index: Box::new(int_expr(0, SemanticType::I64)),
                        elem_ty: SemanticType::I64,
                    },
                    expr: int_expr(99, SemanticType::I64),
                    pos_eq: 0,
                }],
                None,
            )],
            enums: vec![],
        };
        assert!(lower_program(&program).is_err());
    }

    // MethodCall with an empty struct_name (degenerate state — semantic phase
    // couldn't resolve the instance's struct type) lowers via the post-H2
    // synthetic-Call path: mangle_method("", "foo") = "$foo", which misses in
    // signature_table and surfaces as a named UnresolvedSemanticArtifact whose
    // artifact field carries the mangled callee. Regression-check that the
    // error remains structured and named, not a generic placeholder.
    #[test]
    fn method_call_produces_named_structured_error() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "v",
                SemanticType::I64,
                SemanticExpr {
                    ty: SemanticType::I64,
                    kind: SemanticExprKind::MethodCall {
                        instance: "obj".to_string(),
                        method: "foo".to_string(),
                        args: vec![],
                        instance_binding: BindingId(u32::MAX),
                        struct_name: String::new(),
                        pos: 0,
                    },
                },
            )],
            enums: vec![],
        };
        let err = lower_program(&program).unwrap_err();
        assert!(
            matches!(
                &err,
                LoweringError::UnresolvedSemanticArtifact { artifact }
                if artifact.contains("$foo")
            ),
            "expected UnresolvedSemanticArtifact carrying the mangled callee '$foo', got: {:?}",
            err
        );
    }

    // -----------------------------------------------------------------------
    // Phase 9 — Runtime intrinsics boundary audit (CX-35)
    //
    // Each builtin must produce UnsupportedSemanticConstruct (not the
    // generic UnresolvedSemanticArtifact that a signature_table miss gives).
    // -----------------------------------------------------------------------

    // Helper: build a top-level ExprStmt that calls a builtin with the given
    // args. `FunctionId(u32::MAX)` is the semantic-layer sentinel for builtins.
    fn builtin_stmt(name: &str, args: Vec<SemanticCallArg>) -> SemanticStmt {
        SemanticStmt::ExprStmt {
            expr: SemanticExpr {
                ty: SemanticType::Void,
                kind: SemanticExprKind::Call {
                    callee: name.to_string(),
                    function: FunctionId(u32::MAX),
                    args,
                },
            },
            pos: 0,
        }
    }

    fn assert_builtin_structured_error(name: &str) {
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(name, vec![])],
            enums: vec![],
        };
        let err = lower_program(&program).unwrap_err();
        assert!(
            matches!(
                &err,
                LoweringError::UnsupportedSemanticConstruct { construct }
                if construct.contains(name)
            ),
            "builtin '{}' should produce UnsupportedSemanticConstruct mentioning its name, got: {:?}",
            name, err
        );
    }

    #[test]
    fn rejects_user_function_named_cx_printn() {
        // lower_program_inner derives reserved C-ABI names from validate::runtime_intrinsic_names();
        // a user function named "cx_printn" must be rejected regardless of the hardcoded list.
        let program = SemanticProgram {
            stmts: vec![semantic_function("cx_printn", vec![], None, vec![], None)],
            enums: vec![],
        };
        let err = lower_program(&program).unwrap_err();
        assert!(
            matches!(
                &err,
                LoweringError::UnsupportedSemanticConstruct { construct }
                if construct.contains("cx_printn") && construct.contains("reserved")
            ),
            "expected reserved-name error for 'cx_printn', got: {:?}",
            err
        );
    }

    #[test]
    fn print_i64_lowers_to_cx_printn_call() {
        // print(42i64) must lower to IrInst::Call{callee:"cx_printn"} and pass validation.
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "print",
                vec![SemanticCallArg::Expr(int_expr(42, SemanticType::I64))],
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let cx_printn_calls: Vec<_> = main_fn
            .blocks
            .iter()
            .flat_map(|b| b.insts.iter())
            .filter(|inst| matches!(inst, IrInst::Call { callee, .. } if callee == "cx_printn"))
            .collect();
        assert_eq!(cx_printn_calls.len(), 1, "expected exactly one cx_printn call from print");
        match cx_printn_calls[0] {
            IrInst::Call { dst, callee, args, return_ty } => {
                assert!(dst.is_none(), "cx_printn is void — no destination");
                assert_eq!(callee, "cx_printn");
                assert_eq!(args.len(), 1, "cx_printn takes one argument");
                assert!(return_ty.is_none(), "cx_printn returns void");
            }
            _ => panic!("expected Call instruction"),
        }
    }

    #[test]
    fn println_i64_lowers_to_cx_printn_call() {
        // println(42i64) must lower to IrInst::Call{callee:"cx_printn"} and pass validation.
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "println",
                vec![SemanticCallArg::Expr(int_expr(42, SemanticType::I64))],
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let cx_printn_calls: Vec<_> = main_fn
            .blocks
            .iter()
            .flat_map(|b| b.insts.iter())
            .filter(|inst| matches!(inst, IrInst::Call { callee, .. } if callee == "cx_printn"))
            .collect();
        assert_eq!(cx_printn_calls.len(), 1, "expected exactly one cx_printn call from println");
        match cx_printn_calls[0] {
            IrInst::Call { dst, callee, args, return_ty } => {
                assert!(dst.is_none(), "cx_printn is void — no destination");
                assert_eq!(callee, "cx_printn");
                assert_eq!(args.len(), 1, "cx_printn takes one argument");
                assert!(return_ty.is_none(), "cx_printn returns void");
            }
            _ => panic!("expected Call instruction"),
        }
    }

    #[test]
    fn print_f64_arg_lowers_to_inline_plus_newline() {
        // D2.3d: print with an F64 argument now lowers to cx_print_f64_inline +
        // cx_print_newline (f64 was previously the still-unsupported set's first
        // member; I8/I16/I32 still sign-extend to I64; Bool/TBool route to
        // cx_print_bool).
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "print",
                vec![SemanticCallArg::Expr(float_expr(3.14))],
            )],
            enums: vec![],
        };
        let module = lower_program(&program).expect("F64 print should lower (D2.3d)");
        let callees: Vec<&str> = module
            .functions
            .iter()
            .flat_map(|f| f.blocks.iter())
            .flat_map(|b| b.insts.iter())
            .filter_map(|inst| match inst {
                IrInst::Call { callee, .. } => Some(callee.as_str()),
                _ => None,
            })
            .collect();
        assert!(
            callees.contains(&"cx_print_f64_inline"),
            "expected a cx_print_f64_inline call, got {:?}",
            callees
        );
        assert!(
            callees.contains(&"cx_print_newline"),
            "expected a trailing cx_print_newline call, got {:?}",
            callees
        );
    }

    #[test]
    fn printn_lowers_to_cx_printn_call() {
        // printn(42i64) must lower to IrInst::Call{callee:"cx_printn"} and pass validation.
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "printn",
                vec![SemanticCallArg::Expr(int_expr(42, SemanticType::I64))],
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        let cx_printn_calls: Vec<_> = main_fn
            .blocks
            .iter()
            .flat_map(|b| b.insts.iter())
            .filter(|inst| matches!(inst, IrInst::Call { callee, .. } if callee == "cx_printn"))
            .collect();
        assert_eq!(cx_printn_calls.len(), 1, "expected exactly one cx_printn call");
        match cx_printn_calls[0] {
            IrInst::Call { dst, callee, args, return_ty } => {
                assert!(dst.is_none(), "cx_printn is void — no destination");
                assert_eq!(callee, "cx_printn");
                assert_eq!(args.len(), 1, "cx_printn takes one argument");
                assert!(return_ty.is_none(), "cx_printn returns void");
            }
            _ => panic!("expected Call instruction"),
        }
    }

    #[test]
    fn printn_with_f64_arg_returns_error() {
        // printn with an F64 argument must return UnsupportedSemanticConstruct.
        // Post-widening contract: integers and Bool succeed via route_print_arg;
        // F64 is in the still-unsupported set (no cx_print_f64 intrinsic yet).
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "printn",
                vec![SemanticCallArg::Expr(float_expr(3.14))],
            )],
            enums: vec![],
        };
        let err = lower_program(&program).expect_err("lowering should fail for F64 printn arg");
        assert!(
            matches!(
                &err,
                LoweringError::UnsupportedSemanticConstruct { construct }
                if construct.contains("F64")
            ),
            "expected UnsupportedSemanticConstruct mentioning F64, got: {:?}",
            err
        );
    }

    // assert and assert_eq are now lowerable (Phase 9 sub-packet 3) so they no
    // longer trigger the is_cx_builtin gate.  The tests below verify that they
    // lower correctly to multi-block CFGs with a Trap terminator.

    #[test]
    fn assert_true_lowers_to_validated_multi_block_cfg() {
        // assert(true) → Branch on Bool 1 (pass) / Trap (fail, never taken)
        let program = SemanticProgram {
            stmts: vec![builtin_stmt("assert", vec![SemanticCallArg::Expr(bool_expr(true))])],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        // Synthetic main must have at least 3 blocks: decision, pass, trap.
        assert!(
            module.functions[0].blocks.len() >= 3,
            "expected at least 3 blocks, got {}",
            module.functions[0].blocks.len()
        );
        // At least one Trap terminator must exist.
        let has_trap = module.functions[0]
            .blocks
            .iter()
            .any(|b| matches!(b.term, IrTerminator::Trap));
        assert!(has_trap, "expected a Trap block in the CFG");
    }

    #[test]
    fn assert_integer_one_lowers_via_truthy_coercion() {
        // assert(1) → Compare(Ne, 1, 0) → Branch → Trap on false
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "assert",
                vec![SemanticCallArg::Expr(int_expr(1, SemanticType::I64))],
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        // Must contain a Compare(Ne) to coerce the integer to Bool.
        let has_ne_compare = module.functions[0].blocks.iter().any(|b| {
            b.insts.iter().any(|inst| {
                matches!(inst, IrInst::Compare { op: CompareOp::Ne, .. })
            })
        });
        assert!(has_ne_compare, "expected a Ne Compare for truthy-integer coercion");
        let has_trap = module.functions[0]
            .blocks
            .iter()
            .any(|b| matches!(b.term, IrTerminator::Trap));
        assert!(has_trap, "expected a Trap block in the CFG");
    }

    #[test]
    fn assert_eq_same_integers_lowers_to_validated_cfg() {
        // assert_eq(1, 1) → Compare(Eq) → Branch → Trap on false
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "assert_eq",
                vec![
                    SemanticCallArg::Expr(int_expr(1, SemanticType::I64)),
                    SemanticCallArg::Expr(int_expr(1, SemanticType::I64)),
                ],
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let has_eq_compare = module.functions[0].blocks.iter().any(|b| {
            b.insts.iter().any(|inst| {
                matches!(inst, IrInst::Compare { op: CompareOp::Eq, .. })
            })
        });
        assert!(has_eq_compare, "expected an Eq Compare for assert_eq");
        let has_trap = module.functions[0]
            .blocks
            .iter()
            .any(|b| matches!(b.term, IrTerminator::Trap));
        assert!(has_trap, "expected a Trap block in the CFG");
    }

    #[test]
    fn assert_eq_bool_true_true_lowers_to_validated_cfg() {
        // assert_eq(true, true) → Compare(Eq, Bool, Bool) → Branch → Trap on false
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "assert_eq",
                vec![
                    SemanticCallArg::Expr(bool_expr(true)),
                    SemanticCallArg::Expr(bool_expr(true)),
                ],
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let has_trap = module.functions[0]
            .blocks
            .iter()
            .any(|b| matches!(b.term, IrTerminator::Trap));
        assert!(has_trap, "expected a Trap block in the CFG");
    }

    #[test]
    fn assert_i128_nonzero_lowers_via_truthy_coercion() {
        // assert(1_i128) → Compare(Ne, 1_i128, 0_i128) → Branch → Trap on false
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "assert",
                vec![SemanticCallArg::Expr(int_expr(1, SemanticType::I128))],
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let has_ne_compare = module.functions[0].blocks.iter().any(|b| {
            b.insts.iter().any(|inst| {
                matches!(inst, IrInst::Compare { op: CompareOp::Ne, .. })
            })
        });
        assert!(has_ne_compare, "expected a Ne Compare for I128 truthy-integer coercion");
        let has_trap = module.functions[0]
            .blocks
            .iter()
            .any(|b| matches!(b.term, IrTerminator::Trap));
        assert!(has_trap, "expected a Trap block in the CFG");
    }

    #[test]
    fn assert_eq_same_i128_lowers_to_validated_cfg() {
        // assert_eq(1_i128, 1_i128) → Compare(Eq) → Branch → Trap on false
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "assert_eq",
                vec![
                    SemanticCallArg::Expr(int_expr(1, SemanticType::I128)),
                    SemanticCallArg::Expr(int_expr(1, SemanticType::I128)),
                ],
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let has_eq_compare = module.functions[0].blocks.iter().any(|b| {
            b.insts.iter().any(|inst| {
                matches!(inst, IrInst::Compare { op: CompareOp::Eq, .. })
            })
        });
        assert!(has_eq_compare, "expected an Eq Compare for I128 assert_eq");
        let has_trap = module.functions[0]
            .blocks
            .iter()
            .any(|b| matches!(b.term, IrTerminator::Trap));
        assert!(has_trap, "expected a Trap block in the CFG");
    }

    #[test]
    fn assert_with_wrong_arity_produces_invariant_violation() {
        // assert() with 0 args → InternalInvariantViolation (arity check)
        let program = SemanticProgram {
            stmts: vec![builtin_stmt("assert", vec![])],
            enums: vec![],
        };
        let err = lower_program(&program).unwrap_err();
        assert!(
            matches!(err, LoweringError::InternalInvariantViolation { .. }),
            "expected InternalInvariantViolation for wrong assert arity, got: {:?}",
            err
        );
    }

    #[test]
    fn assert_eq_with_wrong_arity_produces_invariant_violation() {
        // assert_eq() with 0 args → InternalInvariantViolation
        let program = SemanticProgram {
            stmts: vec![builtin_stmt("assert_eq", vec![])],
            enums: vec![],
        };
        let err = lower_program(&program).unwrap_err();
        assert!(
            matches!(err, LoweringError::InternalInvariantViolation { .. }),
            "expected InternalInvariantViolation for wrong assert_eq arity, got: {:?}",
            err
        );
    }

    #[test]
    fn builtin_read_produces_unsupported_construct_not_unresolved_artifact() {
        assert_builtin_structured_error("read");
    }

    #[test]
    fn assert_eq_with_numeric_binary_lhs_lowers_correctly() {
        // assert_eq(1 + 1, 2) — both args have SemanticType::Numeric.
        // Prior to CX-218 this returned UnsupportedSemanticType("Numeric")
        // because lower_binary called lower_type(Numeric) for arithmetic ops.
        // After the fix, Numeric resolves to I64 (on 64-bit targets) and the
        // entire assert_eq lowers to Compare(Eq) + Branch/Trap.
        let add_expr = SemanticExpr {
            ty: SemanticType::Numeric,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(int_expr(1, SemanticType::Numeric)),
                op: Op::Plus,
                pos: 0,
                rhs: Box::new(int_expr(1, SemanticType::Numeric)),
            },
        };
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "assert_eq",
                vec![
                    SemanticCallArg::Expr(add_expr),
                    SemanticCallArg::Expr(int_expr(2, SemanticType::Numeric)),
                ],
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let all_insts: Vec<_> = module.functions[0]
            .blocks
            .iter()
            .flat_map(|b| b.insts.iter())
            .collect();
        let has_add = all_insts
            .iter()
            .any(|i| matches!(i, IrInst::Binary { op: BinaryOp::Add, ty: IrType::I64, .. }));
        assert!(has_add, "expected Binary(Add, I64) for Numeric 1+1");
        let has_eq = all_insts
            .iter()
            .any(|i| matches!(i, IrInst::Compare { op: CompareOp::Eq, .. }));
        assert!(has_eq, "expected Compare(Eq) for assert_eq");
        let has_trap = module.functions[0]
            .blocks
            .iter()
            .any(|b| matches!(b.term, IrTerminator::Trap));
        assert!(has_trap, "expected Trap block for failing assert_eq path");
    }

    #[test]
    fn builtin_input_produces_unsupported_construct_not_unresolved_artifact() {
        assert_builtin_structured_error("input");
    }

    // ── TargetConfig tests ────────────────────────────────────────────────────

    #[test]
    fn target_config_host_pointer_bits_matches_usize() {
        let cfg = TargetConfig::host();
        assert_eq!(cfg.pointer_bits, usize::BITS);
    }

    #[test]
    fn target_config_64bit_numeric_literal_ir_type_is_i64() {
        let cfg = TargetConfig { pointer_bits: 64 };
        assert_eq!(cfg.numeric_literal_ir_type(), IrType::I64);
    }

    #[test]
    fn target_config_32bit_numeric_literal_ir_type_is_i32() {
        let cfg = TargetConfig { pointer_bits: 32 };
        assert_eq!(cfg.numeric_literal_ir_type(), IrType::I32);
    }

    #[test]
    fn target_config_64bit_numeric_literal_bounds_match_i64() {
        let cfg = TargetConfig { pointer_bits: 64 };
        assert_eq!(cfg.numeric_literal_min(), i64::MIN as i128);
        assert_eq!(cfg.numeric_literal_max(), i64::MAX as i128);
    }

    #[test]
    fn target_config_32bit_numeric_literal_bounds_match_i32() {
        let cfg = TargetConfig { pointer_bits: 32 };
        assert_eq!(cfg.numeric_literal_min(), i32::MIN as i128);
        assert_eq!(cfg.numeric_literal_max(), i32::MAX as i128);
    }

    #[test]
    fn numeric_literal_lowers_to_target_default_int_type() {
        // A Numeric-typed literal with an I64 binding should lower cleanly to
        // ConstInt(I64) on the host (64-bit) target.
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(1),
                "x",
                SemanticType::I64,
                int_expr(42, SemanticType::Numeric),
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let has_const_i64 = module.functions[0].blocks[0].insts.iter().any(|inst| {
            matches!(inst, IrInst::ConstInt { ty: IrType::I64, value: 42, .. })
        });
        assert!(has_const_i64, "expected ConstInt(I64, 42) from Numeric literal");
    }

    #[test]
    fn numeric_literal_exceeding_target_range_is_rejected() {
        // A Numeric-typed literal whose value exceeds i64::MAX should be
        // rejected on a 64-bit host target.
        let out_of_range: i128 = i64::MAX as i128 + 1;
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(1),
                "x",
                SemanticType::I128,
                int_expr(out_of_range, SemanticType::Numeric),
            )],
            enums: vec![],
        };
        let err = lower_program(&program).unwrap_err();
        assert!(
            matches!(err, LoweringError::UnsupportedSemanticConstruct { .. }),
            "expected UnsupportedSemanticConstruct for out-of-range Numeric literal, got: {:?}",
            err
        );
    }

    #[test]
    fn cast_from_numeric_source_uses_lowered_type() {
        // A Cast with from=Numeric should lower the literal directly at the
        // cast destination type (I32), emitting ConstInt(I32) without an
        // intermediate I64 → I32 cast instruction.
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(1),
                "x",
                SemanticType::I32,
                SemanticExpr {
                    ty: SemanticType::I32,
                    kind: SemanticExprKind::Cast {
                        expr: Box::new(int_expr(7, SemanticType::Numeric)),
                        from: SemanticType::Numeric,
                        to: SemanticType::I32,
                    },
                },
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let has_const_i32 = module.functions[0].blocks[0].insts.iter().any(|inst| {
            matches!(inst, IrInst::ConstInt { ty: IrType::I32, value: 7, .. })
        });
        assert!(has_const_i32, "expected ConstInt(I32, 7) — Numeric literal lowered at cast destination, no intermediate Cast needed");
        let has_spurious_cast = module.functions[0].blocks[0].insts.iter().any(|inst| {
            matches!(inst, IrInst::Cast { from: IrType::I64, to: IrType::I32, .. })
        });
        assert!(!has_spurious_cast, "unexpected I64→I32 Cast: Numeric literal should be emitted at I32 directly");
    }

    #[test]
    fn cast_from_numeric_out_of_i32_range_is_rejected() {
        // A Numeric literal that exceeds i32::MAX must be rejected when cast to
        // I32, even though it would pass the default I64 range check.
        let too_large: i128 = i32::MAX as i128 + 1;
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(1),
                "x",
                SemanticType::I32,
                SemanticExpr {
                    ty: SemanticType::I32,
                    kind: SemanticExprKind::Cast {
                        expr: Box::new(int_expr(too_large, SemanticType::Numeric)),
                        from: SemanticType::Numeric,
                        to: SemanticType::I32,
                    },
                },
            )],
            enums: vec![],
        };
        let err = lower_program(&program).unwrap_err();
        assert!(
            matches!(err, LoweringError::UnsupportedSemanticConstruct { .. }),
            "expected UnsupportedSemanticConstruct for out-of-range Numeric->I32 cast, got: {:?}",
            err
        );
    }

    #[test]
    fn cast_from_numeric_to_f64_uses_default_width_cast() {
        // A Numeric literal cast to F64 must NOT take the integer fast path.
        // Expected IR: ConstInt(I64, 42) followed by Cast(I64 → F64).
        // The fast path would wrongly emit ConstInt(F64, 42), which the IR
        // validator rejects because ConstInt requires an integer or bool type.
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(1),
                "x",
                SemanticType::F64,
                SemanticExpr {
                    ty: SemanticType::F64,
                    kind: SemanticExprKind::Cast {
                        expr: Box::new(int_expr(42, SemanticType::Numeric)),
                        from: SemanticType::Numeric,
                        to: SemanticType::F64,
                    },
                },
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let has_cast_i64_to_f64 = module.functions[0].blocks[0].insts.iter().any(|inst| {
            matches!(inst, IrInst::Cast { from: IrType::I64, to: IrType::F64, .. })
        });
        assert!(
            has_cast_i64_to_f64,
            "expected Cast(I64 → F64) for Numeric→F64 cast; integer fast path must not apply to float targets"
        );
    }

    fn logical_and_expr(lhs: SemanticExpr, rhs: SemanticExpr) -> SemanticExpr {
        SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(lhs),
                op: Op::And,
                pos: 0,
                rhs: Box::new(rhs),
            },
        }
    }

    fn logical_or_expr(lhs: SemanticExpr, rhs: SemanticExpr) -> SemanticExpr {
        SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(lhs),
                op: Op::Or,
                pos: 0,
                rhs: Box::new(rhs),
            },
        }
    }

    #[test]
    fn lowers_logical_and_to_short_circuit_cfg() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Bool,
                logical_and_expr(bool_expr(true), bool_expr(false)),
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        // Short-circuit lowering creates 4 blocks: entry/decision, rhs, sc, merge.
        assert!(
            main_fn.blocks.len() >= 4,
            "AND short-circuit must produce at least 4 blocks, got {}",
            main_fn.blocks.len()
        );
        // There must be at least one Branch terminator (the decision branch).
        let has_branch = main_fn.blocks.iter().any(|b| {
            matches!(b.term, IrTerminator::Branch { .. })
        });
        assert!(has_branch, "AND must emit a Branch terminator");
        // The short-circuit constant for AND is false (0).
        let has_false_const = main_fn.blocks.iter().flat_map(|b| b.insts.iter()).any(|inst| {
            matches!(inst, IrInst::ConstInt { ty: IrType::Bool, value: 0, .. })
        });
        assert!(has_false_const, "AND must emit ConstInt(Bool, 0) for the short-circuit path");
    }

    #[test]
    fn lowers_logical_or_to_short_circuit_cfg() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Bool,
                logical_or_expr(bool_expr(false), bool_expr(true)),
            )],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();
        assert!(
            main_fn.blocks.len() >= 4,
            "OR short-circuit must produce at least 4 blocks, got {}",
            main_fn.blocks.len()
        );
        let has_branch = main_fn.blocks.iter().any(|b| {
            matches!(b.term, IrTerminator::Branch { .. })
        });
        assert!(has_branch, "OR must emit a Branch terminator");
        // The short-circuit constant for OR is true (1).
        let has_true_const = main_fn.blocks.iter().flat_map(|b| b.insts.iter()).any(|inst| {
            matches!(inst, IrInst::ConstInt { ty: IrType::Bool, value: 1, .. })
        });
        assert!(has_true_const, "OR must emit ConstInt(Bool, 1) for the short-circuit path");
    }

    #[test]
    fn logical_and_result_is_bool() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Bool,
                logical_and_expr(bool_expr(true), bool_expr(true)),
            )],
            enums: vec![],
        };
        // IR validation (inside lower_and_validate) checks type consistency.
        // A successful call guarantees the result is Bool.
        let _ = lower_and_validate(&program);
    }

    #[test]
    fn logical_or_result_is_bool() {
        let program = SemanticProgram {
            stmts: vec![typed_assign(
                BindingId(0),
                "x",
                SemanticType::Bool,
                logical_or_expr(bool_expr(false), bool_expr(false)),
            )],
            enums: vec![],
        };
        let _ = lower_and_validate(&program);
    }

    #[test]
    fn nested_logical_and_or_lowers_without_error() {
        // (true && false) || true
        let inner = logical_and_expr(bool_expr(true), bool_expr(false));
        let outer = logical_or_expr(inner, bool_expr(true));
        let program = SemanticProgram {
            stmts: vec![typed_assign(BindingId(0), "x", SemanticType::Bool, outer)],
            enums: vec![],
        };
        let _ = lower_and_validate(&program);
    }

    #[test]
    fn logical_and_used_as_if_condition_lowers_correctly() {
        // if true && false { } else { }
        let program = SemanticProgram {
            stmts: vec![SemanticStmt::IfElse {
                condition: logical_and_expr(bool_expr(true), bool_expr(false)),
                then_body: vec![],
                else_ifs: vec![],
                else_body: Some(vec![]),
                pos: 0,
            }],
            enums: vec![],
        };
        let _ = lower_and_validate(&program);
    }

    #[test]
    fn logical_and_with_non_bool_result_type_is_rejected() {
        // A logical AND expression whose semantic result type is I64 (not Bool)
        // must be rejected before CFG lowering proceeds.
        let expr = SemanticExpr {
            ty: SemanticType::I64,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(bool_expr(true)),
                op: Op::And,
                pos: 0,
                rhs: Box::new(bool_expr(false)),
            },
        };
        let program = SemanticProgram {
            stmts: vec![typed_assign(BindingId(0), "x", SemanticType::I64, expr)],
            enums: vec![],
        };
        assert!(
            lower_program(&program).is_err(),
            "logical AND with non-Bool result type must be rejected by lower_logical"
        );
    }

    #[test]
    fn logical_or_with_non_bool_result_type_is_rejected() {
        let expr = SemanticExpr {
            ty: SemanticType::I64,
            kind: SemanticExprKind::Binary {
                lhs: Box::new(bool_expr(false)),
                op: Op::Or,
                pos: 0,
                rhs: Box::new(bool_expr(true)),
            },
        };
        let program = SemanticProgram {
            stmts: vec![typed_assign(BindingId(0), "x", SemanticType::I64, expr)],
            enums: vec![],
        };
        assert!(
            lower_program(&program).is_err(),
            "logical OR with non-Bool result type must be rejected by lower_logical"
        );
    }

    // CX-108: Observable side-effect proof.
    //
    // The Call instruction (representing an observable RHS side effect) must
    // appear only in the RHS block, never in the SC block.  This is the
    // IR-level equivalent of the rhs_trap() fixture in t141_logical_and_or_exit.cx
    // — it proves that short-circuit lowering truly isolates RHS evaluation to
    // the path where it is needed.
    //
    // CX-110 hardens these tests: instead of matching any IrInst::Call,
    // we match by callee name ("side_effect_fn") and assert that exactly one
    // such call exists across all blocks in main, and that block is rhs_id.

    #[test]
    fn and_short_circuit_rhs_call_is_in_rhs_block_only() {
        // x: bool = false && side_effect_fn()
        // AND: then=rhs (LHS true → evaluate RHS), else=sc (LHS false → false constant).
        // The Call must land in the rhs block and must be absent from the sc block.
        let call_rhs = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Call {
                callee: "side_effect_fn".to_string(),
                function: FunctionId(0),
                args: vec![],
            },
        };
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "side_effect_fn",
                    vec![],
                    Some(SemanticType::Bool),
                    vec![],
                    Some(bool_expr(true)),
                ),
                typed_assign(
                    BindingId(0),
                    "x",
                    SemanticType::Bool,
                    logical_and_expr(bool_expr(false), call_rhs),
                ),
            ],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();

        let branch_block = main_fn.blocks.iter()
            .find(|b| matches!(b.term, IrTerminator::Branch { .. }))
            .expect("AND must emit a Branch terminator");
        let (rhs_id, sc_id) = match &branch_block.term {
            IrTerminator::Branch { then_block, else_block, .. } => (*then_block, *else_block),
            _ => unreachable!(),
        };

        let sc_block = main_fn.blocks.iter().find(|b| b.id == sc_id).unwrap();

        let call_blocks: Vec<_> = main_fn.blocks.iter()
            .filter(|b| b.insts.iter().any(|i| {
                matches!(i, IrInst::Call { callee, .. } if callee == "side_effect_fn")
            }))
            .map(|b| b.id)
            .collect();
        assert_eq!(call_blocks.len(), 1, "expected exactly one side_effect_fn call in main");
        assert_eq!(call_blocks[0], rhs_id, "side_effect_fn call must be emitted only in RHS block");
        assert!(
            !sc_block.insts.iter().any(|i| {
                matches!(i, IrInst::Call { callee, .. } if callee == "side_effect_fn")
            }),
            "side_effect_fn call must NOT be present in SC block — AND short-circuit must suppress RHS evaluation"
        );
    }

    #[test]
    fn or_short_circuit_rhs_call_is_in_rhs_block_only() {
        // x: bool = true || side_effect_fn()
        // OR: then=sc (LHS true → true constant), else=rhs (LHS false → evaluate RHS).
        // The Call must land in the rhs block and must be absent from the sc block.
        let call_rhs = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Call {
                callee: "side_effect_fn".to_string(),
                function: FunctionId(0),
                args: vec![],
            },
        };
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "side_effect_fn",
                    vec![],
                    Some(SemanticType::Bool),
                    vec![],
                    Some(bool_expr(false)),
                ),
                typed_assign(
                    BindingId(0),
                    "x",
                    SemanticType::Bool,
                    logical_or_expr(bool_expr(true), call_rhs),
                ),
            ],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();

        let branch_block = main_fn.blocks.iter()
            .find(|b| matches!(b.term, IrTerminator::Branch { .. }))
            .expect("OR must emit a Branch terminator");
        let (sc_id, rhs_id) = match &branch_block.term {
            IrTerminator::Branch { then_block, else_block, .. } => (*then_block, *else_block),
            _ => unreachable!(),
        };

        let sc_block = main_fn.blocks.iter().find(|b| b.id == sc_id).unwrap();

        let call_blocks: Vec<_> = main_fn.blocks.iter()
            .filter(|b| b.insts.iter().any(|i| {
                matches!(i, IrInst::Call { callee, .. } if callee == "side_effect_fn")
            }))
            .map(|b| b.id)
            .collect();
        assert_eq!(call_blocks.len(), 1, "expected exactly one side_effect_fn call in main");
        assert_eq!(call_blocks[0], rhs_id, "side_effect_fn call must be emitted only in RHS block");
        assert!(
            !sc_block.insts.iter().any(|i| {
                matches!(i, IrInst::Call { callee, .. } if callee == "side_effect_fn")
            }),
            "side_effect_fn call must NOT be present in SC block — OR short-circuit must suppress RHS evaluation"
        );
    }

    // CX-109: Hardened RHS-block-only proof.
    //
    // CX-108 proved that a Call appearing as the RHS of a short-circuit
    // expression lands in the RHS block and not in the SC block.  These tests
    // strengthen that proof to cover all blocks: we scan every block in the
    // lowered function and assert that the Call appears in exactly one of them,
    // and that block is the RHS block.  This rules out leakage into the decision
    // block, the merge block, or any synthetic block the lowering might
    // introduce in the future.
    //
    // We additionally prove that the short-circuit constant (false for AND,
    // true for OR) is confined to the SC block and does not appear in any other
    // block, completing the dual-isolation guarantee.
    //
    // CX-110 hardens both groups:
    // — "confined" tests: match Call by callee ("side_effect_fn") instead of any Call.
    // — SC-constant tests: replace literal bool LHS with a non-literal call so
    //   the same ConstInt cannot leak into the decision block, then assert the
    //   constant appears in exactly one block (the SC block).

    #[test]
    fn and_rhs_call_confined_to_rhs_block_across_all_blocks() {
        // false && side_effect_fn()
        // Scans every block in main; the side_effect_fn Call must appear in
        // exactly one block and that block must be the RHS block (then-branch
        // of the AND Branch).
        let call_rhs = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Call {
                callee: "side_effect_fn".to_string(),
                function: FunctionId(0),
                args: vec![],
            },
        };
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "side_effect_fn",
                    vec![],
                    Some(SemanticType::Bool),
                    vec![],
                    Some(bool_expr(true)),
                ),
                typed_assign(
                    BindingId(0),
                    "x",
                    SemanticType::Bool,
                    logical_and_expr(bool_expr(false), call_rhs),
                ),
            ],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();

        let branch_block = main_fn.blocks.iter()
            .find(|b| matches!(b.term, IrTerminator::Branch { .. }))
            .expect("AND must emit a Branch terminator");
        // AND: then_block = rhs, else_block = sc.
        let rhs_id = match &branch_block.term {
            IrTerminator::Branch { then_block, .. } => *then_block,
            _ => unreachable!(),
        };

        let blocks_with_call: Vec<_> = main_fn.blocks.iter()
            .filter(|b| b.insts.iter().any(|i| {
                matches!(i, IrInst::Call { callee, .. } if callee == "side_effect_fn")
            }))
            .collect();

        assert_eq!(
            blocks_with_call.len(),
            1,
            "side_effect_fn Call must appear in exactly one block; got {} — decision/sc/merge block isolation violated",
            blocks_with_call.len()
        );
        assert_eq!(
            blocks_with_call[0].id,
            rhs_id,
            "The sole block containing a side_effect_fn Call must be the RHS block"
        );
    }

    #[test]
    fn or_rhs_call_confined_to_rhs_block_across_all_blocks() {
        // true || side_effect_fn()
        // Scans every block in main; the side_effect_fn Call must appear in
        // exactly one block and that block must be the RHS block (else-branch
        // of the OR Branch).
        let call_rhs = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Call {
                callee: "side_effect_fn".to_string(),
                function: FunctionId(0),
                args: vec![],
            },
        };
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "side_effect_fn",
                    vec![],
                    Some(SemanticType::Bool),
                    vec![],
                    Some(bool_expr(false)),
                ),
                typed_assign(
                    BindingId(0),
                    "x",
                    SemanticType::Bool,
                    logical_or_expr(bool_expr(true), call_rhs),
                ),
            ],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();

        let branch_block = main_fn.blocks.iter()
            .find(|b| matches!(b.term, IrTerminator::Branch { .. }))
            .expect("OR must emit a Branch terminator");
        // OR: then_block = sc, else_block = rhs.
        let rhs_id = match &branch_block.term {
            IrTerminator::Branch { else_block, .. } => *else_block,
            _ => unreachable!(),
        };

        let blocks_with_call: Vec<_> = main_fn.blocks.iter()
            .filter(|b| b.insts.iter().any(|i| {
                matches!(i, IrInst::Call { callee, .. } if callee == "side_effect_fn")
            }))
            .collect();

        assert_eq!(
            blocks_with_call.len(),
            1,
            "side_effect_fn Call must appear in exactly one block; got {} — decision/sc/merge block isolation violated",
            blocks_with_call.len()
        );
        assert_eq!(
            blocks_with_call[0].id,
            rhs_id,
            "The sole block containing a side_effect_fn Call must be the RHS block"
        );
    }

    #[test]
    fn and_sc_block_contains_exactly_false_constant() {
        // lhs_cond_fn() && side_effect_fn()
        // Uses a non-literal (call) LHS so ConstInt(Bool, 0) can only appear
        // in the SC block, never in the decision block.  Proves the false
        // short-circuit constant is exclusive to the SC block.
        let lhs_call = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Call {
                callee: "lhs_cond_fn".to_string(),
                function: FunctionId(0),
                args: vec![],
            },
        };
        let call_rhs = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Call {
                callee: "side_effect_fn".to_string(),
                function: FunctionId(0),
                args: vec![],
            },
        };
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "lhs_cond_fn",
                    vec![],
                    Some(SemanticType::Bool),
                    vec![],
                    Some(bool_expr(false)),
                ),
                semantic_function(
                    "side_effect_fn",
                    vec![],
                    Some(SemanticType::Bool),
                    vec![],
                    Some(bool_expr(true)),
                ),
                typed_assign(
                    BindingId(0),
                    "x",
                    SemanticType::Bool,
                    logical_and_expr(lhs_call, call_rhs),
                ),
            ],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();

        let branch_block = main_fn.blocks.iter()
            .find(|b| matches!(b.term, IrTerminator::Branch { .. }))
            .expect("AND must emit a Branch terminator");
        // AND: then_block = rhs, else_block = sc.
        let (rhs_id, sc_id) = match &branch_block.term {
            IrTerminator::Branch { then_block, else_block, .. } => (*then_block, *else_block),
            _ => unreachable!(),
        };
        let sc_block  = main_fn.blocks.iter().find(|b| b.id == sc_id).unwrap();
        let rhs_block = main_fn.blocks.iter().find(|b| b.id == rhs_id).unwrap();

        assert_eq!(
            sc_block.insts.len(),
            1,
            "AND SC block must contain exactly one instruction (the short-circuit constant), got {}",
            sc_block.insts.len()
        );
        assert!(
            matches!(sc_block.insts[0], IrInst::ConstInt { ty: IrType::Bool, value: 0, .. }),
            "AND SC block's sole instruction must be ConstInt(Bool, 0)"
        );
        assert!(
            !rhs_block.insts.iter().any(|i| matches!(i, IrInst::ConstInt { ty: IrType::Bool, value: 0, .. })),
            "ConstInt(Bool, 0) must not appear in the RHS block — false constant belongs only in SC block"
        );
        // Cross-block uniqueness: ConstInt(Bool, 0) must appear in exactly one
        // block (the SC block).  With a non-literal LHS call, the decision block
        // emits no ConstInt, so leakage would be caught here.
        let false_const_blocks: Vec<_> = main_fn.blocks.iter()
            .filter(|b| b.insts.iter().any(|i| matches!(i, IrInst::ConstInt { ty: IrType::Bool, value: 0, .. })))
            .collect();
        assert_eq!(
            false_const_blocks.len(),
            1,
            "ConstInt(Bool, 0) must appear in exactly one block (the SC block); got {}",
            false_const_blocks.len()
        );
        assert_eq!(
            false_const_blocks[0].id,
            sc_id,
            "The sole ConstInt(Bool, 0) must reside in the SC block"
        );
    }

    #[test]
    fn or_sc_block_contains_exactly_true_constant() {
        // lhs_cond_fn() || side_effect_fn()
        // Uses a non-literal (call) LHS so ConstInt(Bool, 1) can only appear
        // in the SC block, never in the decision block.  Proves the true
        // short-circuit constant is exclusive to the SC block.
        let lhs_call = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Call {
                callee: "lhs_cond_fn".to_string(),
                function: FunctionId(0),
                args: vec![],
            },
        };
        let call_rhs = SemanticExpr {
            ty: SemanticType::Bool,
            kind: SemanticExprKind::Call {
                callee: "side_effect_fn".to_string(),
                function: FunctionId(0),
                args: vec![],
            },
        };
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "lhs_cond_fn",
                    vec![],
                    Some(SemanticType::Bool),
                    vec![],
                    Some(bool_expr(true)),
                ),
                semantic_function(
                    "side_effect_fn",
                    vec![],
                    Some(SemanticType::Bool),
                    vec![],
                    Some(bool_expr(false)),
                ),
                typed_assign(
                    BindingId(0),
                    "x",
                    SemanticType::Bool,
                    logical_or_expr(lhs_call, call_rhs),
                ),
            ],
            enums: vec![],
        };
        let module = lower_and_validate(&program);
        let main_fn = module.functions.iter().find(|f| f.name == "main").unwrap();

        let branch_block = main_fn.blocks.iter()
            .find(|b| matches!(b.term, IrTerminator::Branch { .. }))
            .expect("OR must emit a Branch terminator");
        // OR: then_block = sc, else_block = rhs.
        let (sc_id, rhs_id) = match &branch_block.term {
            IrTerminator::Branch { then_block, else_block, .. } => (*then_block, *else_block),
            _ => unreachable!(),
        };
        let sc_block  = main_fn.blocks.iter().find(|b| b.id == sc_id).unwrap();
        let rhs_block = main_fn.blocks.iter().find(|b| b.id == rhs_id).unwrap();

        assert_eq!(
            sc_block.insts.len(),
            1,
            "OR SC block must contain exactly one instruction (the short-circuit constant), got {}",
            sc_block.insts.len()
        );
        assert!(
            matches!(sc_block.insts[0], IrInst::ConstInt { ty: IrType::Bool, value: 1, .. }),
            "OR SC block's sole instruction must be ConstInt(Bool, 1)"
        );
        assert!(
            !rhs_block.insts.iter().any(|i| matches!(i, IrInst::ConstInt { ty: IrType::Bool, value: 1, .. })),
            "ConstInt(Bool, 1) must not appear in the RHS block — true constant belongs only in SC block"
        );
        // Cross-block uniqueness: ConstInt(Bool, 1) must appear in exactly one
        // block (the SC block).  With a non-literal LHS call, the decision block
        // emits no ConstInt, so leakage would be caught here.
        let true_const_blocks: Vec<_> = main_fn.blocks.iter()
            .filter(|b| b.insts.iter().any(|i| matches!(i, IrInst::ConstInt { ty: IrType::Bool, value: 1, .. })))
            .collect();
        assert_eq!(
            true_const_blocks.len(),
            1,
            "ConstInt(Bool, 1) must appear in exactly one block (the SC block); got {}",
            true_const_blocks.len()
        );
        assert_eq!(
            true_const_blocks[0].id,
            sc_id,
            "The sole ConstInt(Bool, 1) must reside in the SC block"
        );
    }
}
