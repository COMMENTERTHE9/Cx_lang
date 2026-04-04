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
use crate::ir::types::{BlockId, BlockParam, IrBlock, IrFunction, IrModule, IrParam, IrType, ValueId};

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

struct LoweringCtx {
    builder: IrBuilder,
    finished_blocks: Vec<IrBlock>,
    signature_table: HashMap<String, FunctionSignature>,
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
}

struct FunctionLoweringSpec {
    name: String,
    return_ty: Option<IrType>,
    allow_return_stmt: bool,
}

impl LoweringCtx {
    fn new(signature_table: HashMap<String, FunctionSignature>, trace: bool) -> Self {
        Self {
            builder: IrBuilder::new(),
            finished_blocks: Vec::new(),
            signature_table,
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

    for stmt in &program.stmts {
        match stmt {
            SemanticStmt::FuncDef(function) => {
                if function.name == "main" {
                    has_real_main = true;
                }
                module.functions.push(lower_semantic_function(function, &signature_table, trace)?);
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
            .push(lower_top_level_main(&top_level_stmts, &signature_table, trace)?);
    }

    Ok(module)
}

fn lower_top_level_main(stmts: &[&SemanticStmt], signature_table: &HashMap<String, FunctionSignature>, trace: bool) -> Result<IrFunction, LoweringError> {
    let spec = FunctionLoweringSpec {
        name: "main".to_string(),
        return_ty: None,
        allow_return_stmt: false,
    };
    let mut ctx = LoweringCtx::new(signature_table.clone(), trace);
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
    trace: bool,
) -> Result<IrFunction, LoweringError> {
    let mut ir_params = Vec::with_capacity(function.params.len());
    let mut block_params = Vec::with_capacity(function.params.len());
    let mut bindings = HashMap::new();
    let return_ty = function.return_ty.as_ref().map(lower_type).transpose()?;

    let mut ctx = LoweringCtx::new(signature_table.clone(), trace);
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
                SemanticLValue::DotAccess { .. } => {
                    Err(LoweringError::UnsupportedSemanticConstruct {
                        construct: "Assign::DotAccess".to_string(),
                    })
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
        SemanticStmt::CompoundAssign { .. } => Err(LoweringError::UnresolvedSemanticArtifact {
            artifact: "CompoundAssign".to_string(),
        }),
        SemanticStmt::FuncDef(_) => Err(LoweringError::UnsupportedSemanticConstruct {
            construct: "nested FuncDef".to_string(),
        }),
        SemanticStmt::EnumDef { .. } => { unsupported!("EnumDef") },
SemanticStmt::Block { .. } => { unsupported!("Block") },
        SemanticStmt::WhileIn { .. } => { unsupported!("WhileIn") },
        SemanticStmt::While { cond, body, .. } => {
    return match lower_while(cond, body, ctx, current, spec)? {
        Some(new_active) => Ok(Some(new_active)),
        None => Ok(None),
    };
},
        SemanticStmt::For { .. } => { unsupported!("For") },
        SemanticStmt::Loop { .. } => { unsupported!("Loop") },
        SemanticStmt::Break { .. } => { unsupported!("Break") },
        SemanticStmt::Continue { .. } => { unsupported!("Continue") },
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
        ),
        SemanticStmt::StructDef { .. } => { unsupported!("StructDef") },
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
        if let Some(active) = lower_stmt_sequence(then_body.iter(), ctx, Some(then_active), spec, None)? {
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
        if let Some(active) = lower_stmt_sequence(then_body.iter(), ctx, Some(then_active), spec, None)? {
            fallthroughs.push(active);
        }
        let else_result = if let Some(else_body) = else_body {
            lower_stmt_sequence(else_body.iter(), ctx, Some(else_active), spec, None)?
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

    let exit_block = ctx.start_block(vec![], header_bindings.clone());
    let exit_id = exit_block.id();

    header.terminate(IrTerminator::Branch {
        cond: cond_val.value,
        then_block: body_id,
        then_args: vec![],
        else_block: exit_id,
        else_args: vec![],
    })?;
    ctx.seal_block(header)?;

    let body_result = lower_stmt_sequence(
        body.iter(),
        ctx,
        Some(body_block),
        spec,
        None,
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
                    construct: "void function call — IrType::Void pending".to_string(),
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
        SemanticExprKind::DotAccess { .. } => { unsupported!("DotAccess") },
        SemanticExprKind::HandleNew { .. } => { unsupported!("HandleNew") },
        SemanticExprKind::HandleVal { .. } => { unsupported!("HandleVal") },
        SemanticExprKind::HandleDrop { .. } => { unsupported!("HandleDrop") },
        SemanticExprKind::Range { .. } => { unsupported!("Range") },
        SemanticExprKind::Unary { .. } => { unsupported!("Unary") },
        SemanticExprKind::ArrayLit { .. } => { unsupported!("ArrayLit") },
        SemanticExprKind::Index { .. } => { unsupported!("Index") },
        SemanticExprKind::MethodCall { .. } => { unsupported!("MethodCall") },
        SemanticExprKind::StructInstance { .. } => { unsupported!("StructInstance") },
        SemanticExprKind::When { .. } => { unsupported!("WhenExpr") },
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
        SemanticType::Struct(_) => { unsupported_type!("Struct") },
        SemanticType::Array(_, _) => { unsupported_type!("Array") },
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
                artifact: "CompoundAssign".to_string()
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
    fn rejects_void_call() {
        let program = SemanticProgram {
            stmts: vec![
                semantic_function(
                    "do_nothing",
                    vec![],
                    None,
                    vec![],
                    None,
                ),
                SemanticStmt::ExprStmt {
                    expr: SemanticExpr {
                        ty: SemanticType::I64,
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

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticConstruct {
                construct: "void function call — IrType::Void pending".to_string(),
            }
        );
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
            stmts: vec![SemanticStmt::Loop {
                body: vec![],
                pos: 0,
            }],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticConstruct {
                construct: "Loop".to_string()
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
                vec![SemanticStmt::Loop {
                    body: vec![],
                    pos: 0,
                }],
                None,
            )],
            enums: vec![],
        };

        assert_eq!(
            lower_program(&program).expect_err("lowering should fail"),
            LoweringError::UnsupportedSemanticConstruct {
                construct: "Loop".to_string()
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
                vec![SemanticStmt::Loop {
                    body: vec![],
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
                construct: "Loop".to_string()
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
                artifact: "CompoundAssign".to_string()
            }
        );
    }
}
