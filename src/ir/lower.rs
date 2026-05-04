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

struct LoweringCtx {
    builder: IrBuilder,
    finished_blocks: Vec<IrBlock>,
    signature_table: HashMap<String, FunctionSignature>,
    struct_table: HashMap<String, StructLayoutInfo>,
    trace: bool,
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
    ) -> Self {
        Self {
            builder: IrBuilder::new(),
            finished_blocks: Vec::new(),
            signature_table,
            struct_table,
            trace,
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

    for stmt in &program.stmts {
        match stmt {
            SemanticStmt::FuncDef(function) => {
                if function.name == "main" {
                    has_real_main = true;
                }
                module.functions.push(lower_semantic_function(function, &signature_table, &struct_table, trace)?);
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
            .push(lower_top_level_main(&top_level_stmts, &signature_table, &struct_table, trace)?);
    }

    Ok(module)
}

fn lower_top_level_main(stmts: &[&SemanticStmt], signature_table: &HashMap<String, FunctionSignature>, struct_table: &HashMap<String, StructLayoutInfo>, trace: bool) -> Result<IrFunction, LoweringError> {
    let spec = FunctionLoweringSpec {
        name: "main".to_string(),
        return_ty: None,
        allow_return_stmt: false,
    };
    let mut ctx = LoweringCtx::new(signature_table.clone(), struct_table.clone(), trace);
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
) -> Result<IrFunction, LoweringError> {
    let mut ir_params = Vec::with_capacity(function.params.len());
    let mut block_params = Vec::with_capacity(function.params.len());
    let mut bindings = HashMap::new();
    let return_ty = function.return_ty.as_ref().map(lower_type).transpose()?;

    let mut ctx = LoweringCtx::new(signature_table.clone(), struct_table.clone(), trace);
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
    header_params.push(BlockParam { value: counter_param, ty: start_val.ty.clone() });
    entry_args.push(start_val.value);

    for b in &ordered_bindings {
        let val = incoming.get(b).unwrap();
        let pv = ctx.fresh_value();
        header_params.push(BlockParam { value: pv, ty: val.ty.clone() });
        header_bindings.insert(*b, LoweredValue { value: pv, ty: val.ty.clone() });
        entry_args.push(val.value);
    }

    let mut header = ctx.start_block(header_params, header_bindings.clone());
    let header_id = header.id();

    current.terminate(IrTerminator::Jump { target: header_id, args: entry_args })?;
    ctx.seal_block(current)?;

    // Increment block: counter + bindings as params, increments counter, jumps to header
    let inc_counter_param = ctx.fresh_value();
    let mut inc_params = vec![BlockParam { value: inc_counter_param, ty: start_val.ty.clone() }];
    let mut inc_bindings = HashMap::new();
    for b in &ordered_bindings {
        let val = incoming.get(b).unwrap();
        let pv = ctx.fresh_value();
        inc_params.push(BlockParam { value: pv, ty: val.ty.clone() });
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
        exit_params.push(BlockParam { value: pv, ty: val.ty.clone() });
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
            let lowered = lower_expr(expr, ctx, active)?;
            let from_ty = lower_type(from)?;
            let to_ty = lower_type(to)?;
            ensure_type_match("cast source", from_ty.clone(), lowered.ty)?;
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
        SemanticExprKind::Range { .. } => { unsupported!("Range") },
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
        SemanticExprKind::MethodCall { .. } => { unsupported!("MethodCall") },
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

fn lower_value(
    value: &SemanticValue,
    semantic_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let ty = lower_type(semantic_ty)?;
    let dst = ctx.fresh_value();

    match value {
        SemanticValue::Num(n) => {
            let value =
                i128::try_from(*n).map_err(|_| LoweringError::InternalInvariantViolation {
                    detail: format!("integer literal {n} exceeds i128 IR constant range"),
                })?;
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

fn lower_binary(
    lhs: &SemanticExpr,
    op: Op,
    rhs: &SemanticExpr,
    result_ty: &SemanticType,
    ctx: &mut LoweringCtx,
    active: &mut ActiveBlock,
) -> Result<LoweredValue, LoweringError> {
    let lhs = lower_expr(lhs, ctx, active)?;
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
        Op::And => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Binary::And".to_string(),
        }),
        Op::Or => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "Binary::Or".to_string(),
        }),
    }
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

    let elem_ir_ty = lower_type(elem_sem_ty)?;
    let layout = compute_array_layout(&elem_ir_ty, count);

    // 1. Alloca: reserve stack space for the entire array.
    let ptr = ctx.fresh_value();
    active.emit(IrInst::Alloca {
        dst: ptr,
        size: layout.total_size,
        align: layout.alignment,
    })?;

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

    let elem_ir_ty = lower_type(declared_elem_sem_ty)?;
    let layout = compute_array_layout(&elem_ir_ty, count);

    // Verify the outer expression type is consistent with the element type.
    let outer_ir_ty = lower_type(elem_sem_ty)?;
    if outer_ir_ty != elem_ir_ty {
        return Err(LoweringError::InternalInvariantViolation {
            detail: format!(
                "Index expression type {:?} does not match array element type {:?}",
                outer_ir_ty, elem_ir_ty
            ),
        });
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
        SemanticType::Void => { unsupported_type!("Void") },
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

    // ArrayLit lowering: a three-element i64 array must emit exactly one Alloca
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

        // One Alloca for the array storage (3 * 8 = 24 bytes, align 8).
        let alloca_count = insts
            .iter()
            .filter(|i| matches!(i, IrInst::Alloca { size: 24, align: 8, .. }))
            .count();
        assert_eq!(alloca_count, 1, "expected exactly one Alloca(24, 8)");

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
}
