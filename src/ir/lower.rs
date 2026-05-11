#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt;

use crate::frontend::ast::Op;
use crate::frontend::semantic_types::{
    BindingId, SemanticCallArg, SemanticExpr, SemanticExprKind, SemanticLValue,
    SemanticParamKind, SemanticProgram, SemanticStmt, SemanticType, SemanticValue,
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
//   I/O (stdout)      | print, println    | blocked on frontend string ABI
//   I/O (stdout)      | printn            | LOWERABLE — see lower_printn_stmt
//   I/O (stdin)       | read, input       | blocked on Phase 8 str layout
//   Debug / assertion | assert, assert_eq | LOWERABLE — Phase 9 sub-packet 3
//
// Builtins that remain in `is_cx_builtin` produce a structured
// `UnsupportedSemanticConstruct` error instead of a misleading
// `UnresolvedSemanticArtifact` from a signature_table miss.
//
// Builtins that are handled before this gate (assert, assert_eq, printn)
// are intercepted in `lower_stmt` and never reach `is_cx_builtin`.
fn is_cx_builtin(name: &str) -> bool {
    matches!(name, "print" | "println" | "read" | "input")
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
                Some(ty) => match lower_type(ty) {
                    // IrType::Void in return position is canonicalised to None
                    // (the IR uses Option<IrType> where None = void).
                    Ok(IrType::Void) => None,
                    Ok(ir_ty) => Some(ir_ty),
                    Err(_) => continue,
                },
                None => None,
            };

            table.insert(function.name.clone(), FunctionSignature {
                param_types,
                return_ty,
            });
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
}

struct ActiveBlock {
    block: crate::ir::builder::IrBlockBuilder,
    bindings: BindingMap,
    terminated: bool,
    trace: bool,
}

#[derive(Clone, Debug)]
struct LoopContext {
    header_id: BlockId,
    exit_id: BlockId,
    ordered_bindings: Vec<BindingId>,
    exit_ordered_bindings: Vec<BindingId>,
}

struct FunctionLoweringSpec {
    name: String,
    return_ty: Option<IrType>,
    allow_return_stmt: bool,
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
    const RESERVED_RUNTIME_INTRINSICS: &[&str] = &["cx_printn"];

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
                if RESERVED_RUNTIME_INTRINSICS.contains(&function.name.as_str()) {
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
    };
    let mut ctx = LoweringCtx::new(signature_table.clone(), struct_table.clone(), trace, target);
    let entry = ctx.start_block(vec![], HashMap::new());
    let current = lower_stmt_sequence(
        stmts.iter().copied(),
        &mut ctx,
        Some(entry),
        &spec,
        None,
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
    let return_ty = {
        let raw = function.return_ty.as_ref().map(lower_type).transpose()?;
        // Canonicalise: IrType::Void in return position is equivalent to no return value.
        // IrFunction::return_ty uses Option<IrType> where None already encodes void.
        match raw {
            Some(IrType::Void) => None,
            other => other,
        }
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

    let spec = FunctionLoweringSpec {
        name: function.name.clone(),
        return_ty: return_ty.clone(),
        allow_return_stmt: true,
    };
    let entry = ctx.start_block(block_params, bindings);
    let current = lower_stmt_sequence(
        function.body.iter(),
        &mut ctx,
        Some(entry),
        &spec,
        None,
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
            finalize_active_block(
                &mut ctx,
                active,
                IrTerminator::Return {
                    value: Some(lowered.value),
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
    loop_ctx: Option<&LoopContext>,
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
    loop_ctx: Option<&LoopContext>,
) -> Result<Option<ActiveBlock>, LoweringError> {
    match stmt {
        SemanticStmt::Noop => Ok(Some(current)),
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
                    let target_ty = lower_type(ty)?;
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
            let lowered = lower_expr(expr, ctx, &mut current)?;
            let target_ty = lower_type(ty)?;
            ensure_type_match("typed assignment", target_ty.clone(), lowered.ty)?;
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
        SemanticStmt::ExprStmt { expr, .. } => {
            // Phase 9 sub-packets 2 and 3: assert/assert_eq and printn are now
            // fully lowerable.  Intercept them before the is_cx_builtin gate so
            // they are routed to the dedicated lowering functions rather than
            // returning a structured error.
            if let SemanticExprKind::Call { callee, args, .. } = &expr.kind {
                match callee.as_str() {
                    "assert" => return lower_assert_stmt(args, ctx, current),
                    "assert_eq" => return lower_assert_eq_stmt(args, ctx, current),
                    "printn" => return lower_printn_stmt(args, ctx, current),
                    _ => {}
                }
            }
            // Builtin intrinsics (print, etc.) are not in the signature_table.
            // Intercept them here so they produce a structured UnsupportedSemanticConstruct
            // rather than falling through to a misleading UnresolvedSemanticArtifact.
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
                    current.terminate(IrTerminator::Return {
                        value: Some(lowered.value),
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
            let target_ty = lower_type(ty)?;
            let current_val = current.bindings.get(binding).ok_or_else(|| {
                LoweringError::UnresolvedSemanticArtifact {
                    artifact: format!("compound assign binding {:?}", binding),
                }
            })?.clone();
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
        SemanticStmt::EnumDef { .. } => { unsupported!("EnumDef") },
SemanticStmt::Block { .. } => { unsupported!("Block") },
        SemanticStmt::WhileIn { .. } => { unsupported!("WhileIn") },
        SemanticStmt::While { cond, body, .. } => {
    return match lower_while(cond, body, ctx, current, spec, loop_ctx)? {
        Some(new_active) => Ok(Some(new_active)),
        None => Ok(None),
    };
},
        SemanticStmt::For { binding, start, end, inclusive, body, .. } => {
    return match lower_for(*binding, start, end, *inclusive, body, ctx, current, spec, loop_ctx)? {
        Some(new_active) => Ok(Some(new_active)),
        None => Ok(None),
    };
},
        SemanticStmt::Loop { body, .. } => {
    return match lower_loop(body, ctx, current, spec, loop_ctx)? {
        Some(new_active) => Ok(Some(new_active)),
        None => Ok(None),
    };
},
        SemanticStmt::Break { .. } => {
    let ctx_ref = loop_ctx.ok_or_else(|| LoweringError::UnsupportedSemanticConstruct {
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
        SemanticStmt::Continue { .. } => {
    let ctx_ref = loop_ctx.ok_or_else(|| LoweringError::UnsupportedSemanticConstruct {
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
        SemanticStmt::When { .. } => { unsupported!("When") },
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
        SemanticStmt::ImplBlock { .. } => { unsupported!("ImplBlock") },
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
    loop_ctx: Option<&LoopContext>,
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
    loop_ctx: Option<&LoopContext>,
) -> Result<Vec<ActiveBlock>, LoweringError> {
    let cond = lower_expr(condition, ctx, &mut decision_block)?;
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
    cond: &SemanticExpr,
    body: &[SemanticStmt],
    ctx: &mut LoweringCtx,
    current: ActiveBlock,
    spec: &FunctionLoweringSpec,
    _outer_loop_ctx: Option<&LoopContext>,
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
        header_id,
        exit_id,
        ordered_bindings: ordered_bindings.clone(),
        exit_ordered_bindings: ordered_bindings.clone(),
    };
    let body_result = lower_stmt_sequence(
        body.iter(),
        ctx,
        Some(body_block),
        spec,
        Some(&loop_context),
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
    body: &[SemanticStmt],
    ctx: &mut LoweringCtx,
    current: ActiveBlock,
    spec: &FunctionLoweringSpec,
    _outer_loop_ctx: Option<&LoopContext>,
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
        header_id,
        exit_id,
        ordered_bindings: ordered_bindings.clone(),
        exit_ordered_bindings: ordered_bindings.clone(),
    };

    let body_result = lower_stmt_sequence(
        body.iter(),
        ctx,
        Some(body_block),
        spec,
        Some(&loop_context),
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
    binding: BindingId,
    start: &SemanticExpr,
    end: &SemanticExpr,
    inclusive: bool,
    body: &[SemanticStmt],
    ctx: &mut LoweringCtx,
    current: ActiveBlock,
    spec: &FunctionLoweringSpec,
    _outer_loop_ctx: Option<&LoopContext>,
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

    let body_result = lower_stmt_sequence(
        body.iter(),
        ctx,
        Some(body_block),
        spec,
        Some(&loop_context),
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

fn lower_expr(
    expr: &SemanticExpr,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    match &expr.kind {
        SemanticExprKind::Value(value) => lower_value(value, &expr.ty, ctx, active),
        SemanticExprKind::VarRef { binding, name } => {
            let ty = lower_type(&expr.ty)?;
            let lowered = active.bindings.get(binding).cloned().ok_or_else(|| {
                LoweringError::InternalInvariantViolation {
                    detail: format!(
                        "binding '{name}' ({}) referenced before any SSA value was assigned",
                        binding.0
                    ),
                }
            })?;
            ensure_type_match("var ref", ty, lowered.ty.clone())?;
            Ok(lowered)
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
        SemanticExprKind::Call { callee, function: _, args } => {
            // Remaining builtin intrinsics (print, println, printn, read, input) are not
            // in the signature_table.  Intercept them before the lookup so the error is
            // structured and actionable rather than the generic UnresolvedSemanticArtifact
            // produced by a table miss.
            // Note: assert/assert_eq are handled at statement level and should not reach
            // lower_expr in well-formed programs.
            if is_cx_builtin(callee) {
                return Err(LoweringError::UnsupportedSemanticConstruct {
                    construct: format!(
                        "builtin '{}' is not yet lowerable to IR — codegen pending (Phase 9)",
                        callee
                    ),
                });
            }
            let (param_types, return_ty) = {
                let sig = ctx.signature_table.get(callee).ok_or_else(|| {
                    LoweringError::UnresolvedSemanticArtifact {
                        artifact: format!("function '{}'", callee),
                    }
                })?;
                (sig.param_types.clone(), sig.return_ty.clone())
            };

            let return_ty = return_ty.ok_or_else(|| {
                LoweringError::UnsupportedSemanticConstruct {
                    construct: format!(
                        "void function '{}' used in value position — void calls are only valid as statements",
                        callee
                    ),
                }
            })?;

            if args.len() != param_types.len() {
                return Err(LoweringError::InternalInvariantViolation {
                    detail: format!(
                        "call to '{}': expected {} arguments, got {}",
                        callee, param_types.len(), args.len()
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
        SemanticExprKind::HandleNew { .. } => { unsupported!("HandleNew") },
        SemanticExprKind::HandleVal { .. } => { unsupported!("HandleVal") },
        SemanticExprKind::HandleDrop { .. } => { unsupported!("HandleDrop") },
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
        SemanticExprKind::MethodCall { instance, method, .. } => {
            Err(LoweringError::UnsupportedSemanticConstruct {
                construct: format!("MethodCall '{}.{}'", instance, method),
            })
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

            for (field_idx, (canonical_name, _field_ty)) in layout_info.fields.iter().enumerate() {
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

                active.emit(IrInst::Store {
                    ptr: field_ptr,
                    value: lowered_field.value,
                })?;
            }

            Ok(LoweredValue { value: ptr, ty: IrType::Ptr })
        },
        SemanticExprKind::When { .. } => { unsupported!("WhenExpr") },
        SemanticExprKind::ResultOk { .. } => { unsupported!("ResultOk") },
        SemanticExprKind::ResultErr { .. } => { unsupported!("ResultErr") },
        SemanticExprKind::Try { .. } => { unsupported!("Try") },
    }
}

/// Returns the inclusive `[min, max]` range of values that fit in `ty` as a
/// signed integer, or `None` if `ty` is not an integer type.
fn ir_int_range(ty: &IrType) -> Option<(i128, i128)> {
    match ty {
        IrType::I8   => Some((i8::MIN  as i128, i8::MAX  as i128)),
        IrType::I16  => Some((i16::MIN as i128, i16::MAX as i128)),
        IrType::I32  => Some((i32::MIN as i128, i32::MAX as i128)),
        IrType::I64  => Some((i64::MIN as i128, i64::MAX as i128)),
        IrType::I128 => Some((i128::MIN, i128::MAX)),
        IrType::Bool => Some((0, 1)),
        _            => None,
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
        SemanticValue::Str(_) => Err(LoweringError::UnsupportedSemanticType {
            ty: "Str".to_string(),
        }),
        SemanticValue::Char(_) => Err(LoweringError::UnsupportedSemanticType {
            ty: "Char".to_string(),
        }),
        SemanticValue::EnumVariant { .. } => Err(LoweringError::UnsupportedSemanticType {
            ty: "Enum".to_string(),
        }),
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

    // Left operand lowered first — preserves left-to-right evaluation order.
    let lhs = lower_expr(lhs, ctx, active)?;
    // Right operand lowered second — all lhs side effects are already emitted.
    let rhs = lower_expr(rhs, ctx, active)?;
    let dst = ctx.fresh_value();

    match op {
        Op::Plus | Op::Minus | Op::Mul | Op::Div | Op::Mod => {
            let ty = lower_type(result_ty)?;
            ensure_type_match("binary lhs", ty.clone(), lhs.ty)?;
            ensure_type_match("binary rhs", ty.clone(), rhs.ty)?;
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

    // Numeric is a placeholder type used by the semantic layer for untyped
    // integer literals.  Map it to the target's default integer width (I64 on
    // 64-bit) so arrays of untyped literals (e.g. [10, 20, 30]) lower cleanly.
    let elem_ir_ty = if elem_sem_ty == &SemanticType::Numeric {
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

        active.emit(IrInst::Store {
            ptr: elem_ptr,
            value: lowered_elem.value,
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
        SemanticType::Handle(_) => { unsupported_type!("Handle") },
        SemanticType::StrRef => { unsupported_type!("StrRef") },
        SemanticType::Container => { unsupported_type!("Container") },
        SemanticType::Str => { unsupported_type!("Str") },
        SemanticType::Char => { unsupported_type!("Char") },
        SemanticType::Enum(_) => { unsupported_type!("Enum") },
        SemanticType::TypeParam(_) => { unsupported_type!("TypeParam") },
        SemanticType::Struct(_) => Ok(IrType::Ptr),
        SemanticType::Array(_, _) => Ok(IrType::Ptr),
        SemanticType::Result(_) => { unsupported_type!("Result") },
        // Void is a first-class IR type used to represent the absence of a return value.
        // Callers that use lower_type for return-type lowering must canonicalise
        // Some(IrType::Void) to None before placing it in IrFunction::return_ty.
        SemanticType::Void => Ok(IrType::Void),
    }
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

/// Lower `printn(n)` to `IrInst::Call { callee: "cx_printn", args: [i64_value] }`.
///
/// `printn` is a Cx builtin that prints an integer to stdout.  At the IR level
/// it lowers to a call to the `cx_printn` runtime intrinsic, which is pre-declared
/// in the JIT module as an imported C-ABI symbol.
///
/// Only `I64` arguments are accepted; other types produce a structured error.
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
    if arg.ty != IrType::I64 {
        return Err(LoweringError::UnsupportedSemanticConstruct {
            construct: format!(
                "printn argument must be I64, got {:?} — other types not yet supported",
                arg.ty
            ),
        });
    }
    current.emit(IrInst::Call {
        dst: None,
        callee: "cx_printn".to_string(),
        args: vec![arg.value],
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
    fn rejects_handle_type() {
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
            lower_program(&program).expect_err("lowering should reject unsupported type"),
            LoweringError::UnsupportedSemanticType {
                ty: "Handle".to_string()
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
                binding: BindingId(0),
                var: "i".to_string(),
                start: int_expr(0, SemanticType::I64),
                end: int_expr(10, SemanticType::I64),
                inclusive: false,
                body: vec![SemanticStmt::Break { pos: 0 }],
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
                binding: BindingId(0),
                var: "i".to_string(),
                start: int_expr(0, SemanticType::I64),
                end: int_expr(10, SemanticType::I64),
                inclusive: false,
                body: vec![SemanticStmt::Continue { pos: 0 }],
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

    // lower_type must map SemanticType::Array(_) to IrType::Ptr.
    #[test]
    fn lower_type_array_maps_to_ptr() {
        let result = lower_type(&SemanticType::Array(4, Box::new(SemanticType::I64)));
        assert_eq!(result, Ok(IrType::Ptr));
    }

    // lower_type must map SemanticType::Void to IrType::Void (not an error).
    // Callers (lower_semantic_function, build_signature_table) canonicalise
    // Some(IrType::Void) → None before it enters IrFunction::return_ty.
    #[test]
    fn lower_type_void_maps_to_irtype_void() {
        let result = lower_type(&SemanticType::Void);
        assert_eq!(result, Ok(IrType::Void));
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

    // MethodCall produces a named structured error that includes both the
    // instance name and the method name, rather than the generic "MethodCall"
    // placeholder produced by the unsupported! macro.
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
                LoweringError::UnsupportedSemanticConstruct { construct }
                if construct.contains("obj") && construct.contains("foo")
            ),
            "expected named error mentioning instance and method, got: {:?}",
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
    fn builtin_print_produces_unsupported_construct_not_unresolved_artifact() {
        assert_builtin_structured_error("print");
    }

    #[test]
    fn builtin_println_produces_unsupported_construct_not_unresolved_artifact() {
        assert_builtin_structured_error("println");
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
    fn printn_with_non_i64_arg_returns_error() {
        // printn with an I32 argument must return UnsupportedSemanticConstruct.
        let program = SemanticProgram {
            stmts: vec![builtin_stmt(
                "printn",
                vec![SemanticCallArg::Expr(int_expr(42, SemanticType::I32))],
            )],
            enums: vec![],
        };
        let err = lower_program(&program).expect_err("lowering should fail for non-I64 printn arg");
        assert!(
            matches!(
                &err,
                LoweringError::UnsupportedSemanticConstruct { construct }
                if construct.contains("I64")
            ),
            "expected UnsupportedSemanticConstruct mentioning I64, got: {:?}",
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
