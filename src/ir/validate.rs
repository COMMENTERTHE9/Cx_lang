use std::collections::{HashMap, HashSet};

use crate::ir::instr::{IrInst, IrTerminator};
use crate::ir::types::{BlockId, IrBlock, IrFunction, IrModule, IrType, ValueId};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IrValidationError {
    EmptyFunctionName {
        function_index: usize,
    },
    EmptyFunctionBody {
        function: String,
    },
    DuplicateBlockId {
        function: String,
        block: BlockId,
    },
    MissingBlockTarget {
        function: String,
        from_block: BlockId,
        target: BlockId,
    },
    DuplicateBlockParam {
        function: String,
        block: BlockId,
        value: ValueId,
    },
    DuplicateValueDefinition {
        function: String,
        value: ValueId,
        context: String,
    },
    UndefinedValueUse {
        function: String,
        block: BlockId,
        value: ValueId,
        context: String,
    },
    InvalidEntryShape {
        function: String,
        detail: String,
    },
    InvalidTerminatorPlacement {
        function: String,
        block: BlockId,
        detail: String,
    },
    InvalidTypeUsage {
        function: String,
        block: BlockId,
        detail: String,
    },
    LoopVariableReassignment {
        function: String,
        block: BlockId,
        value: ValueId,
        context: String,
    },
    ReservedRuntimeIntrinsicName {
        function: String,
    },
}

struct ValidatorFunctionSig {
    param_count: usize,
    param_types: Vec<IrType>,
    has_return: bool,
}

/// Return the signatures of every known runtime intrinsic.
///
/// These are pre-seeded into the validator's `function_sigs` map so that
/// `IrInst::Call` to an intrinsic like `cx_printn` passes validation even
/// though it has no `IrFunction` entry in the module.
fn known_intrinsic_sigs() -> HashMap<String, ValidatorFunctionSig> {
    let mut sigs = HashMap::new();
    // cx_printn(n: I64) -> void — Phase 9 sub-packet 2 print-integer intrinsic.
    sigs.insert(
        "cx_printn".to_string(),
        ValidatorFunctionSig {
            param_count: 1,
            param_types: vec![IrType::I64],
            has_return: false,
        },
    );
    sigs
}

pub fn validate_module(module: &IrModule) -> Result<(), Vec<IrValidationError>> {
    let mut errors = Vec::new();

    // Seed known runtime intrinsic signatures before scanning user-defined functions.
    let mut function_sigs: HashMap<String, ValidatorFunctionSig> = known_intrinsic_sigs();
    for function in &module.functions {
        function_sigs.insert(function.name.clone(), ValidatorFunctionSig {
            param_count: function.params.len(),
            param_types: function.params.iter().map(|p| p.ty.clone()).collect(),
            has_return: function.blocks.iter().any(|b| {
                matches!(b.term, IrTerminator::Return { .. })
            }),
        });
    }

    for (function_index, function) in module.functions.iter().enumerate() {
        validate_function(function_index, function, &function_sigs, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn is_reserved_runtime_intrinsic(name: &str) -> bool {
    matches!(
        name,
        "print" | "println" | "printn" | "read" | "input"
            | "assert" | "assert_eq" | "is_known"
    )
}

fn validate_function(
    function_index: usize,
    function: &IrFunction,
    function_sigs: &HashMap<String, ValidatorFunctionSig>,
    errors: &mut Vec<IrValidationError>,
) {
    if function.name.is_empty() {
        errors.push(IrValidationError::EmptyFunctionName { function_index });
    }

    if is_reserved_runtime_intrinsic(&function.name) {
        errors.push(IrValidationError::ReservedRuntimeIntrinsicName {
            function: function.name.clone(),
        });
    }

    if function.blocks.is_empty() {
        errors.push(IrValidationError::EmptyFunctionBody {
            function: function.name.clone(),
        });
        return;
    }

    let mut block_ids = HashSet::new();
    let mut blocks_by_id = HashMap::new();
    for block in &function.blocks {
        if !block_ids.insert(block.id) {
            errors.push(IrValidationError::DuplicateBlockId {
                function: function.name.clone(),
                block: block.id,
            });
        }
        blocks_by_id.insert(block.id, block);
    }

    if is_synthetic_main(function) {
        validate_synthetic_main_shape(function, errors);
    }

    let mut defined_values = HashMap::<ValueId, IrType>::new();

    // Collect all values produced by SsaBind instructions across the whole
    // function.  These represent user-level assignments.  The terminator
    // validator uses this set to reject any Jump/Branch that forwards an
    // SsaBind-produced value into a `read_only` block parameter (which would
    // mean a for-loop variable was overwritten inside the loop body).
    let mut ssa_bind_values: HashSet<ValueId> = HashSet::new();
    for block in &function.blocks {
        for inst in &block.insts {
            if let IrInst::SsaBind { dst, .. } = inst {
                ssa_bind_values.insert(*dst);
            }
        }
    }

    for block in &function.blocks {
        let mut block_params = HashSet::new();
        for param in &block.params {
            if !block_params.insert(param.value) {
                errors.push(IrValidationError::DuplicateBlockParam {
                    function: function.name.clone(),
                    block: block.id,
                    value: param.value,
                });
            }
            define_value(
                function,
                block.id,
                param.value,
                param.ty.clone(),
                "block parameter",
                &mut defined_values,
                errors,
            );
        }

        for inst in &block.insts {
            validate_inst(function, block.id, inst, &mut defined_values, function_sigs, errors);
        }

        validate_terminator(
            function,
            block.id,
            &block.term,
            &defined_values,
            &blocks_by_id,
            &ssa_bind_values,
            errors,
        );
    }
}

fn validate_synthetic_main_shape(function: &IrFunction, errors: &mut Vec<IrValidationError>) {
    if function.params.len() != 0 || function.return_ty.is_some() {
        errors.push(IrValidationError::InvalidEntryShape {
            function: function.name.clone(),
            detail: "synthetic main must have no params and no return type".to_string(),
        });
    }

    let entry = &function.blocks[0];
    if !entry.params.is_empty() {
        errors.push(IrValidationError::InvalidEntryShape {
            function: function.name.clone(),
            detail: "synthetic main entry block must not have block params".to_string(),
        });
    }

    for block in &function.blocks {
        if matches!(block.term, IrTerminator::Return { value: Some(_) }) {
            errors.push(IrValidationError::InvalidEntryShape {
                function: function.name.clone(),
                detail: "synthetic main must not return a value".to_string(),
            });
        }
    }
}

fn is_synthetic_main(function: &IrFunction) -> bool {
    function.name == "main" && function.params.is_empty() && function.return_ty.is_none()
}

fn validate_inst(
    function: &IrFunction,
    block: BlockId,
    inst: &IrInst,
    defined_values: &mut HashMap<ValueId, IrType>,
    function_sigs: &HashMap<String, ValidatorFunctionSig>,
    errors: &mut Vec<IrValidationError>,
) {
    match inst {
        IrInst::ConstInt { dst, ty, .. } => {
            if !matches!(
                ty,
                IrType::I8 | IrType::I16 | IrType::I32 | IrType::I64 | IrType::I128 | IrType::Bool
            ) {
                errors.push(IrValidationError::InvalidTypeUsage {
                    function: function.name.clone(),
                    block,
                    detail: format!("ConstInt must use an integer or bool IR type, got {ty:?}"),
                });
            }
            define_value(
                function,
                block,
                *dst,
                ty.clone(),
                "ConstInt destination",
                defined_values,
                errors,
            );
        }
        IrInst::ConstFloat { dst, .. } => {
            define_value(
                function,
                block,
                *dst,
                IrType::F64,
                "ConstFloat destination",
                defined_values,
                errors,
            );
        }
        IrInst::SsaBind { dst, ty, src } => {
            let src_ty = require_value(
                function,
                block,
                *src,
                "SsaBind source",
                defined_values,
                errors,
            );
            if let Some(src_ty) = src_ty {
                if src_ty != *ty {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: format!(
                            "SsaBind type mismatch: source has {src_ty:?}, destination declares {ty:?}"
                        ),
                    });
                }
            }
            define_value(
                function,
                block,
                *dst,
                ty.clone(),
                "SsaBind destination",
                defined_values,
                errors,
            );
        }
        IrInst::Binary {
            dst, ty, lhs, rhs, ..
        } => {
            if !matches!(
                ty,
                IrType::I8 | IrType::I16 | IrType::I32 | IrType::I64 | IrType::I128 | IrType::F64
            ) {
                errors.push(IrValidationError::InvalidTypeUsage {
                    function: function.name.clone(),
                    block,
                    detail: format!("Binary result type must be arithmetic-capable, got {ty:?}"),
                });
            }

            let lhs_ty = require_value(function, block, *lhs, "Binary lhs", defined_values, errors);
            let rhs_ty = require_value(function, block, *rhs, "Binary rhs", defined_values, errors);
            for (side, operand_ty) in [("lhs", lhs_ty), ("rhs", rhs_ty)] {
                if let Some(operand_ty) = operand_ty {
                    if operand_ty != *ty {
                        errors.push(IrValidationError::InvalidTypeUsage {
                            function: function.name.clone(),
                            block,
                            detail: format!(
                                "Binary {side} type mismatch: operand has {operand_ty:?}, result declares {ty:?}"
                            ),
                        });
                    }
                }
            }

            define_value(
                function,
                block,
                *dst,
                ty.clone(),
                "Binary destination",
                defined_values,
                errors,
            );
        }
        IrInst::Compare { dst, lhs, rhs, .. } => {
            let lhs_ty =
                require_value(function, block, *lhs, "Compare lhs", defined_values, errors);
            let rhs_ty =
                require_value(function, block, *rhs, "Compare rhs", defined_values, errors);

            if let (Some(lhs_ty), Some(rhs_ty)) = (lhs_ty, rhs_ty) {
                if lhs_ty != rhs_ty {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: format!(
                            "Compare operands must have matching types, got {lhs_ty:?} and {rhs_ty:?}"
                        ),
                    });
                }
            }

            define_value(
                function,
                block,
                *dst,
                IrType::Bool,
                "Compare destination",
                defined_values,
                errors,
            );
        }
        IrInst::Call {
            dst,
            callee,
            args,
            return_ty,
        } => {
            for arg in args {
                require_value(function, block, *arg, "Call arg", defined_values, errors);
            }

            if let Some(sig) = function_sigs.get(callee) {
                if args.len() != sig.param_count {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: format!(
                            "Call to '{}': expected {} arguments, got {}",
                            callee, sig.param_count, args.len()
                        ),
                    });
                }

                for (i, arg) in args.iter().enumerate() {
                    if i < sig.param_types.len() {
                        if let Some(arg_ty) = defined_values.get(arg) {
                            if *arg_ty != sig.param_types[i] {
                                errors.push(IrValidationError::InvalidTypeUsage {
                                    function: function.name.clone(),
                                    block,
                                    detail: format!(
                                        "Call to '{}': argument {} has type {:?}, expected {:?}",
                                        callee, i, arg_ty, sig.param_types[i]
                                    ),
                                });
                            }
                        }
                    }
                }

                if !sig.has_return && (dst.is_some() || return_ty.is_some()) {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: format!(
                            "Call to '{}': callee returns void but call provides destination/return_ty",
                            callee
                        ),
                    });
                }
            } else {
                errors.push(IrValidationError::InvalidTypeUsage {
                    function: function.name.clone(),
                    block,
                    detail: format!("Call to undefined function '{}'", callee),
                });
            }

            if let Some(dst) = dst {
                let Some(return_ty) = return_ty else {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: "Call with dst must provide return_ty".to_string(),
                    });
                    return;
                };
                define_value(
                    function,
                    block,
                    *dst,
                    return_ty.clone(),
                    "Call destination",
                    defined_values,
                    errors,
                );
            }
        }
        IrInst::Cast {
            dst,
            from,
            to,
            value,
        } => {
            let value_ty = require_value(
                function,
                block,
                *value,
                "Cast value",
                defined_values,
                errors,
            );
            if let Some(value_ty) = value_ty {
                if value_ty != *from {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: format!(
                            "Cast source type mismatch: value has {value_ty:?}, cast declares {from:?}"
                        ),
                    });
                }
            }
            define_value(
                function,
                block,
                *dst,
                to.clone(),
                "Cast destination",
                defined_values,
                errors,
            );
        }
        IrInst::Alloca { dst, size, align } => {
            if *size == 0 {
                errors.push(IrValidationError::InvalidTypeUsage {
                    function: function.name.clone(),
                    block,
                    detail: "Alloca size must be > 0".to_string(),
                });
            }
            if *align == 0 || (align & (align - 1)) != 0 {
                errors.push(IrValidationError::InvalidTypeUsage {
                    function: function.name.clone(),
                    block,
                    detail: format!("Alloca align must be power of 2, got {}", align),
                });
            }
            define_value(function, block, *dst, IrType::Ptr, "Alloca destination", defined_values, errors);
        }
        IrInst::ArrayAlloca { dst, element_type, count } => {
            if *count == 0 {
                errors.push(IrValidationError::InvalidTypeUsage {
                    function: function.name.clone(),
                    block,
                    detail: "ArrayAlloca count must be > 0".to_string(),
                });
            }
            if matches!(element_type, IrType::Void) {
                errors.push(IrValidationError::InvalidTypeUsage {
                    function: function.name.clone(),
                    block,
                    detail: "ArrayAlloca element_type must be storable, got Void".to_string(),
                });
            }
            define_value(function, block, *dst, IrType::Ptr, "ArrayAlloca destination", defined_values, errors);
        }
        IrInst::PtrOffset { dst, base, .. } => {
            require_value(function, block, *base, "PtrOffset base", defined_values, errors);
            if let Some(base_ty) = defined_values.get(base) {
                if *base_ty != IrType::Ptr {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: format!("PtrOffset base must be Ptr, got {:?}", base_ty),
                    });
                }
            }
            define_value(function, block, *dst, IrType::Ptr, "PtrOffset destination", defined_values, errors);
        }
        IrInst::PtrAdd { dst, base, offset } => {
            require_value(function, block, *base, "PtrAdd base", defined_values, errors);
            require_value(function, block, *offset, "PtrAdd offset", defined_values, errors);
            if let Some(base_ty) = defined_values.get(base) {
                if *base_ty != IrType::Ptr {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: format!("PtrAdd base must be Ptr, got {:?}", base_ty),
                    });
                }
            }
            if let Some(offset_ty) = defined_values.get(offset) {
                if *offset_ty != IrType::I64 {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: format!("PtrAdd offset must be I64, got {:?}", offset_ty),
                    });
                }
            }
            define_value(function, block, *dst, IrType::Ptr, "PtrAdd destination", defined_values, errors);
        }
        IrInst::Load { dst, ty, ptr } => {
            require_value(function, block, *ptr, "Load ptr", defined_values, errors);
            if let Some(ptr_ty) = defined_values.get(ptr) {
                if *ptr_ty != IrType::Ptr {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: format!("Load ptr must be Ptr, got {:?}", ptr_ty),
                    });
                }
            }
            define_value(function, block, *dst, ty.clone(), "Load destination", defined_values, errors);
        }
        IrInst::Store { ptr, value } => {
            require_value(function, block, *ptr, "Store ptr", defined_values, errors);
            require_value(function, block, *value, "Store value", defined_values, errors);
            if let Some(ptr_ty) = defined_values.get(ptr) {
                if *ptr_ty != IrType::Ptr {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: format!("Store ptr must be Ptr, got {:?}", ptr_ty),
                    });
                }
            }
        }
    }
}

fn validate_terminator(
    function: &IrFunction,
    block: BlockId,
    term: &IrTerminator,
    defined_values: &HashMap<ValueId, IrType>,
    blocks_by_id: &HashMap<BlockId, &IrBlock>,
    ssa_bind_values: &HashSet<ValueId>,
    errors: &mut Vec<IrValidationError>,
) {
    match term {
        IrTerminator::Jump { target, args } => {
            require_target(function, block, *target, blocks_by_id, errors);
            for arg in args {
                require_value(function, block, *arg, "Jump arg", defined_values, errors);
            }
            validate_target_args(
                function,
                block,
                "Jump",
                *target,
                args,
                defined_values,
                blocks_by_id,
                ssa_bind_values,
                errors,
            );
        }
        IrTerminator::Branch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => {
            let cond_ty = require_value(
                function,
                block,
                *cond,
                "Branch cond",
                defined_values,
                errors,
            );
            if let Some(cond_ty) = cond_ty {
                if cond_ty != IrType::Bool {
                    errors.push(IrValidationError::InvalidTypeUsage {
                        function: function.name.clone(),
                        block,
                        detail: format!("Branch cond must be Bool, got {cond_ty:?}"),
                    });
                }
            }
            require_target(function, block, *then_block, blocks_by_id, errors);
            require_target(function, block, *else_block, blocks_by_id, errors);
            for arg in then_args {
                require_value(
                    function,
                    block,
                    *arg,
                    "Branch then_arg",
                    defined_values,
                    errors,
                );
            }
            for arg in else_args {
                require_value(
                    function,
                    block,
                    *arg,
                    "Branch else_arg",
                    defined_values,
                    errors,
                );
            }
            validate_target_args(
                function,
                block,
                "Branch then",
                *then_block,
                then_args,
                defined_values,
                blocks_by_id,
                ssa_bind_values,
                errors,
            );
            validate_target_args(
                function,
                block,
                "Branch else",
                *else_block,
                else_args,
                defined_values,
                blocks_by_id,
                ssa_bind_values,
                errors,
            );
        }
        IrTerminator::Return { value } => {
            if let Some(value) = value {
                require_value(
                    function,
                    block,
                    *value,
                    "Return value",
                    defined_values,
                    errors,
                );
            }
        }
        // Trap is an unconditional abort: no values consumed, no successor block.
        IrTerminator::Trap => {}
    }
}

fn define_value(
    function: &IrFunction,
    _block: BlockId,
    value: ValueId,
    ty: IrType,
    context: &str,
    defined_values: &mut HashMap<ValueId, IrType>,
    errors: &mut Vec<IrValidationError>,
) {
    if defined_values.insert(value, ty).is_some() {
        errors.push(IrValidationError::DuplicateValueDefinition {
            function: function.name.clone(),
            value,
            context: context.to_string(),
        });
    }
}

fn require_value(
    function: &IrFunction,
    block: BlockId,
    value: ValueId,
    context: &str,
    defined_values: &HashMap<ValueId, IrType>,
    errors: &mut Vec<IrValidationError>,
) -> Option<IrType> {
    let ty = defined_values.get(&value).cloned();
    if ty.is_none() {
        errors.push(IrValidationError::UndefinedValueUse {
            function: function.name.clone(),
            block,
            value,
            context: context.to_string(),
        });
    }
    ty
}

fn require_target(
    function: &IrFunction,
    from_block: BlockId,
    target: BlockId,
    blocks_by_id: &HashMap<BlockId, &IrBlock>,
    errors: &mut Vec<IrValidationError>,
) {
    if !blocks_by_id.contains_key(&target) {
        errors.push(IrValidationError::MissingBlockTarget {
            function: function.name.clone(),
            from_block,
            target,
        });
    }
}

fn validate_target_args(
    function: &IrFunction,
    block: BlockId,
    context: &str,
    target: BlockId,
    args: &[ValueId],
    defined_values: &HashMap<ValueId, IrType>,
    blocks_by_id: &HashMap<BlockId, &IrBlock>,
    ssa_bind_values: &HashSet<ValueId>,
    errors: &mut Vec<IrValidationError>,
) {
    let Some(target_block) = blocks_by_id.get(&target) else {
        return;
    };

    if args.len() != target_block.params.len() {
        errors.push(IrValidationError::InvalidTypeUsage {
            function: function.name.clone(),
            block,
            detail: format!(
                "{context} target block {:?} expects {} args, got {}",
                target,
                target_block.params.len(),
                args.len()
            ),
        });
        return;
    }

    for (arg, param) in args.iter().zip(&target_block.params) {
        if let Some(arg_ty) = defined_values.get(arg) {
            if *arg_ty != param.ty {
                errors.push(IrValidationError::InvalidTypeUsage {
                    function: function.name.clone(),
                    block,
                    detail: format!(
                        "{context} target block {:?} arg type mismatch: arg has {:?}, param expects {:?}",
                        target, arg_ty, param.ty
                    ),
                });
            }
        }

        // Loop variable read-only invariant: a `read_only` block parameter
        // may only receive values that were NOT produced by an SsaBind
        // instruction.  An SsaBind destination represents a user-level
        // assignment; if one reaches a read_only param it means the loop
        // variable was overwritten inside the body.
        if param.read_only && ssa_bind_values.contains(arg) {
            errors.push(IrValidationError::LoopVariableReassignment {
                function: function.name.clone(),
                block,
                value: *arg,
                context: format!(
                    "{context} passes SsaBind value {:?} into read-only loop counter param {:?} of block {:?}",
                    arg, param.value, target
                ),
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::instr::{BinaryOp, CompareOp};
    use crate::ir::types::{BlockParam, IrBlock, IrFunction, IrModule, IrParam};

    fn int_main(insts: Vec<IrInst>, term: IrTerminator) -> IrModule {
        IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts,
                    term,
                }],
            }],
        }
    }

    #[test]
    fn validates_empty_module() {
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![],
        };

        assert_eq!(validate_module(&module), Ok(()));
    }

    #[test]
    fn rejects_duplicate_block_id() {
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "f".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![
                    IrBlock {
                        id: BlockId(1),
                        params: vec![],
                        insts: vec![],
                        term: IrTerminator::Return { value: None },
                    },
                    IrBlock {
                        id: BlockId(1),
                        params: vec![],
                        insts: vec![],
                        term: IrTerminator::Return { value: None },
                    },
                ],
            }],
        };

        let errors =
            validate_module(&module).expect_err("validator should reject duplicate block ids");
        assert!(errors
            .iter()
            .any(|err| matches!(err, IrValidationError::DuplicateBlockId { .. })));
    }

    #[test]
    fn rejects_missing_branch_or_jump_target() {
        let module = int_main(
            vec![IrInst::ConstInt {
                dst: ValueId(0),
                ty: IrType::Bool,
                value: 1,
            }],
            IrTerminator::Jump {
                target: BlockId(9),
                args: vec![ValueId(0)],
            },
        );

        let errors =
            validate_module(&module).expect_err("validator should reject missing jump target");
        assert!(errors
            .iter()
            .any(|err| matches!(err, IrValidationError::MissingBlockTarget { .. })));
    }

    #[test]
    fn rejects_use_of_undefined_value_in_instruction_operand() {
        let module = int_main(
            vec![IrInst::Binary {
                dst: ValueId(1),
                op: BinaryOp::Add,
                ty: IrType::I64,
                lhs: ValueId(99),
                rhs: ValueId(100),
            }],
            IrTerminator::Return { value: None },
        );

        let errors = validate_module(&module)
            .expect_err("validator should reject undefined instruction operands");
        assert!(errors.iter().any(|err| matches!(err, IrValidationError::UndefinedValueUse { context, .. } if context == "Binary lhs")));
    }

    #[test]
    fn rejects_use_of_undefined_value_in_terminator() {
        let module = int_main(
            vec![],
            IrTerminator::Return {
                value: Some(ValueId(7)),
            },
        );

        let errors = validate_module(&module)
            .expect_err("validator should reject undefined terminator value");
        assert!(errors.iter().any(|err| matches!(err, IrValidationError::UndefinedValueUse { context, .. } if context == "Return value")));
    }

    #[test]
    fn rejects_duplicate_value_definition() {
        let module = int_main(
            vec![
                IrInst::ConstInt {
                    dst: ValueId(1),
                    ty: IrType::I64,
                    value: 1,
                },
                IrInst::ConstInt {
                    dst: ValueId(1),
                    ty: IrType::I64,
                    value: 2,
                },
            ],
            IrTerminator::Return { value: None },
        );

        let errors = validate_module(&module)
            .expect_err("validator should reject duplicate value definitions");
        assert!(errors
            .iter()
            .any(|err| matches!(err, IrValidationError::DuplicateValueDefinition { .. })));
    }

    #[test]
    fn rejects_invalid_const_int_type_usage() {
        let module = int_main(
            vec![IrInst::ConstInt {
                dst: ValueId(1),
                ty: IrType::F64,
                value: 1,
            }],
            IrTerminator::Return { value: None },
        );

        let errors = validate_module(&module)
            .expect_err("validator should reject invalid ConstInt type usage");
        assert!(errors
            .iter()
            .any(|err| matches!(err, IrValidationError::InvalidTypeUsage { detail, .. } if detail.contains("ConstInt"))));
    }

    #[test]
    fn rejects_malformed_synthetic_main_shape() {
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![IrInst::ConstInt {
                        dst: ValueId(0),
                        ty: IrType::I64,
                        value: 1,
                    }],
                    term: IrTerminator::Return {
                        value: Some(ValueId(0)),
                    },
                }],
            }],
        };

        let errors = validate_module(&module).expect_err("validator should reject malformed main");
        assert!(errors
            .iter()
            .any(|err| matches!(err, IrValidationError::InvalidEntryShape { .. })));
    }

    #[test]
    fn accepts_multi_block_synthetic_main_cfg() {
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![
                    IrBlock {
                        id: BlockId(0),
                        params: vec![],
                        insts: vec![
                            IrInst::ConstInt {
                                dst: ValueId(0),
                                ty: IrType::Bool,
                                value: 1,
                            },
                            IrInst::ConstInt {
                                dst: ValueId(1),
                                ty: IrType::I64,
                                value: 5,
                            },
                        ],
                        term: IrTerminator::Branch {
                            cond: ValueId(0),
                            then_block: BlockId(1),
                            then_args: vec![],
                            else_block: BlockId(2),
                            else_args: vec![],
                        },
                    },
                    IrBlock {
                        id: BlockId(1),
                        params: vec![],
                        insts: vec![],
                        term: IrTerminator::Jump {
                            target: BlockId(3),
                            args: vec![ValueId(1)],
                        },
                    },
                    IrBlock {
                        id: BlockId(2),
                        params: vec![],
                        insts: vec![],
                        term: IrTerminator::Jump {
                            target: BlockId(3),
                            args: vec![ValueId(1)],
                        },
                    },
                    IrBlock {
                        id: BlockId(3),
                        params: vec![BlockParam {
                            value: ValueId(2),
                            ty: IrType::I64,
                            read_only: false,
                        }],
                        insts: vec![],
                        term: IrTerminator::Return { value: None },
                    },
                ],
            }],
        };

        assert_eq!(validate_module(&module), Ok(()));
    }

    #[test]
    fn rejects_duplicate_block_param_value() {
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "f".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![
                        BlockParam {
                            value: ValueId(0),
                            ty: IrType::I64,
                            read_only: false,
                        },
                        BlockParam {
                            value: ValueId(0),
                            ty: IrType::I64,
                            read_only: false,
                        },
                    ],
                    insts: vec![],
                    term: IrTerminator::Return { value: None },
                }],
            }],
        };

        let errors =
            validate_module(&module).expect_err("validator should reject duplicate block params");
        assert!(errors
            .iter()
            .any(|err| matches!(err, IrValidationError::DuplicateBlockParam { .. })));
    }

    #[test]
    fn rejects_branch_condition_with_non_bool_type() {
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "f".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![
                    IrBlock {
                        id: BlockId(0),
                        params: vec![],
                        insts: vec![IrInst::ConstInt {
                            dst: ValueId(0),
                            ty: IrType::I64,
                            value: 1,
                        }],
                        term: IrTerminator::Branch {
                            cond: ValueId(0),
                            then_block: BlockId(1),
                            then_args: vec![],
                            else_block: BlockId(1),
                            else_args: vec![],
                        },
                    },
                    IrBlock {
                        id: BlockId(1),
                        params: vec![],
                        insts: vec![],
                        term: IrTerminator::Return { value: None },
                    },
                ],
            }],
        };

        let errors =
            validate_module(&module).expect_err("validator should reject non-bool branch cond");
        assert!(errors
            .iter()
            .any(|err| matches!(err, IrValidationError::InvalidTypeUsage { detail, .. } if detail.contains("Branch cond"))));
    }

    #[test]
    fn rejects_jump_arg_mismatch_for_block_params() {
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "f".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![
                    IrBlock {
                        id: BlockId(0),
                        params: vec![],
                        insts: vec![IrInst::ConstInt {
                            dst: ValueId(0),
                            ty: IrType::I64,
                            value: 1,
                        }],
                        term: IrTerminator::Jump {
                            target: BlockId(1),
                            args: vec![],
                        },
                    },
                    IrBlock {
                        id: BlockId(1),
                        params: vec![BlockParam {
                            value: ValueId(1),
                            ty: IrType::I64,
                            read_only: false,
                        }],
                        insts: vec![],
                        term: IrTerminator::Return { value: None },
                    },
                ],
            }],
        };

        let errors =
            validate_module(&module).expect_err("validator should reject mismatched jump args");
        assert!(errors
            .iter()
            .any(|err| matches!(err, IrValidationError::InvalidTypeUsage { detail, .. } if detail.contains("expects 1 args"))));
    }

    #[test]
    fn accepts_trap_terminator_in_unreachable_else_branch() {
        // Models an assertion: branch on bool, pass → Return, fail → Trap.
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "f".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![
                    IrBlock {
                        id: BlockId(0),
                        params: vec![],
                        insts: vec![IrInst::ConstInt {
                            dst: ValueId(0),
                            ty: IrType::Bool,
                            value: 1,
                        }],
                        term: IrTerminator::Branch {
                            cond: ValueId(0),
                            then_block: BlockId(1),
                            then_args: vec![],
                            else_block: BlockId(2),
                            else_args: vec![],
                        },
                    },
                    IrBlock {
                        id: BlockId(1),
                        params: vec![],
                        insts: vec![],
                        term: IrTerminator::Return { value: None },
                    },
                    IrBlock {
                        id: BlockId(2),
                        params: vec![],
                        insts: vec![],
                        term: IrTerminator::Trap,
                    },
                ],
            }],
        };

        assert_eq!(validate_module(&module), Ok(()));
    }

    #[test]
    fn accepts_trap_as_sole_terminator_in_single_block_function() {
        // A function that unconditionally traps is structurally valid IR.
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "always_fails".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![],
                    term: IrTerminator::Trap,
                }],
            }],
        };

        assert_eq!(validate_module(&module), Ok(()));
    }

    #[test]
    fn compare_is_treated_as_boolean_producing() {
        let module = int_main(
            vec![
                IrInst::ConstInt {
                    dst: ValueId(0),
                    ty: IrType::I64,
                    value: 1,
                },
                IrInst::ConstInt {
                    dst: ValueId(1),
                    ty: IrType::I64,
                    value: 2,
                },
                IrInst::Compare {
                    dst: ValueId(2),
                    op: CompareOp::Lt,
                    lhs: ValueId(0),
                    rhs: ValueId(1),
                },
                IrInst::SsaBind {
                    dst: ValueId(3),
                    ty: IrType::Bool,
                    src: ValueId(2),
                },
            ],
            IrTerminator::Return { value: None },
        );

        assert_eq!(validate_module(&module), Ok(()));
    }

    #[test]
    fn validates_normal_function_with_params_and_return_value() {
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "add1".to_string(),
                params: vec![IrParam {
                    name: "x".to_string(),
                    ty: IrType::I64,
                }],
                return_ty: Some(IrType::I64),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![BlockParam {
                        value: ValueId(0),
                        ty: IrType::I64,
                        read_only: false,
                    }],
                    insts: vec![
                        IrInst::ConstInt {
                            dst: ValueId(1),
                            ty: IrType::I64,
                            value: 1,
                        },
                        IrInst::Binary {
                            dst: ValueId(2),
                            op: BinaryOp::Add,
                            ty: IrType::I64,
                            lhs: ValueId(0),
                            rhs: ValueId(1),
                        },
                    ],
                    term: IrTerminator::Return {
                        value: Some(ValueId(2)),
                    },
                }],
            }],
        };

        assert_eq!(validate_module(&module), Ok(()));
    }

    #[test]
    fn real_main_with_params_is_not_forced_into_synthetic_main_shape() {
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![IrParam {
                    name: "argc".to_string(),
                    ty: IrType::I64,
                }],
                return_ty: Some(IrType::I64),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![BlockParam {
                        value: ValueId(0),
                        ty: IrType::I64,
                        read_only: false,
                    }],
                    insts: vec![],
                    term: IrTerminator::Return {
                        value: Some(ValueId(0)),
                    },
                }],
            }],
        };

        assert_eq!(validate_module(&module), Ok(()));
    }

    // ---------------------------------------------------------------------------
    // Loop variable read-only invariant tests
    // ---------------------------------------------------------------------------

    /// Builds a minimal for-loop IR that is CORRECT:
    ///
    ///   B0 (entry): v0=0, v1=9 → Jump B1[v0]
    ///   B1 [v2: I64 read_only] (header): cmp v2 < v1 → Branch(cmp, B2, B3)
    ///   B2 (body): Jump B4[v2]          ← passes original counter, no SsaBind
    ///   B3 (exit): Return
    ///   B4 [v3: I64 read_only] (inc): v4=1, v5=Add(v3,v4) → Jump B1[v5]
    fn valid_for_loop_module() -> IrModule {
        IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![
                    // B0: entry
                    IrBlock {
                        id: BlockId(0),
                        params: vec![],
                        insts: vec![
                            IrInst::ConstInt { dst: ValueId(0), ty: IrType::I64, value: 0 },
                            IrInst::ConstInt { dst: ValueId(1), ty: IrType::I64, value: 9 },
                        ],
                        term: IrTerminator::Jump { target: BlockId(1), args: vec![ValueId(0)] },
                    },
                    // B1: header — counter param is read_only
                    IrBlock {
                        id: BlockId(1),
                        params: vec![BlockParam { value: ValueId(2), ty: IrType::I64, read_only: true }],
                        insts: vec![IrInst::Compare {
                            dst: ValueId(10),
                            op: CompareOp::Lt,
                            lhs: ValueId(2),
                            rhs: ValueId(1),
                        }],
                        term: IrTerminator::Branch {
                            cond: ValueId(10),
                            then_block: BlockId(2),
                            then_args: vec![],
                            else_block: BlockId(3),
                            else_args: vec![],
                        },
                    },
                    // B2: body — correctly forwards the original counter (v2) to increment
                    IrBlock {
                        id: BlockId(2),
                        params: vec![],
                        insts: vec![],
                        term: IrTerminator::Jump { target: BlockId(4), args: vec![ValueId(2)] },
                    },
                    // B3: exit
                    IrBlock {
                        id: BlockId(3),
                        params: vec![],
                        insts: vec![],
                        term: IrTerminator::Return { value: None },
                    },
                    // B4: increment — counter param is also read_only
                    IrBlock {
                        id: BlockId(4),
                        params: vec![BlockParam { value: ValueId(3), ty: IrType::I64, read_only: true }],
                        insts: vec![
                            IrInst::ConstInt { dst: ValueId(4), ty: IrType::I64, value: 1 },
                            IrInst::Binary {
                                dst: ValueId(5),
                                op: BinaryOp::Add,
                                ty: IrType::I64,
                                lhs: ValueId(3),
                                rhs: ValueId(4),
                            },
                        ],
                        term: IrTerminator::Jump { target: BlockId(1), args: vec![ValueId(5)] },
                    },
                ],
            }],
        }
    }

    #[test]
    fn accepts_for_loop_without_counter_reassignment() {
        assert_eq!(validate_module(&valid_for_loop_module()), Ok(()));
    }

    #[test]
    fn rejects_for_loop_body_that_reassigns_counter_via_ssabind() {
        // Body block assigns a new value via SsaBind and forwards that to the
        // increment block's read_only counter param — must be rejected.
        //
        //   B2 (body):
        //     v6 = ConstInt(5)
        //     v7 = SsaBind(I64, v6)   ← user assignment: i = 5
        //     Jump B4 [v7]            ← passes SsaBind result to read_only param
        let mut module = valid_for_loop_module();
        let body = &mut module.functions[0].blocks[2]; // B2
        body.insts = vec![
            IrInst::ConstInt { dst: ValueId(6), ty: IrType::I64, value: 5 },
            IrInst::SsaBind { dst: ValueId(7), ty: IrType::I64, src: ValueId(6) },
        ];
        body.term = IrTerminator::Jump { target: BlockId(4), args: vec![ValueId(7)] };

        let errors = validate_module(&module)
            .expect_err("validator must reject loop body that overwrites the counter");
        assert!(
            errors.iter().any(|err| matches!(
                err,
                IrValidationError::LoopVariableReassignment { .. }
            )),
            "expected LoopVariableReassignment error, got: {:?}",
            errors
        );
    }

    // ---------------------------------------------------------------------------
    // Reserved runtime intrinsic name tests
    // ---------------------------------------------------------------------------

    fn single_block_function(name: &str) -> IrModule {
        IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: name.to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![],
                    term: IrTerminator::Return { value: None },
                }],
            }],
        }
    }

    #[test]
    fn rejects_function_named_print() {
        let errors = validate_module(&single_block_function("print"))
            .expect_err("validator should reject function named 'print'");
        assert!(errors.iter().any(|err| matches!(
            err,
            IrValidationError::ReservedRuntimeIntrinsicName { function, .. }
                if function == "print"
        )));
    }

    #[test]
    fn rejects_function_named_println() {
        let errors = validate_module(&single_block_function("println"))
            .expect_err("validator should reject function named 'println'");
        assert!(errors.iter().any(|err| matches!(
            err,
            IrValidationError::ReservedRuntimeIntrinsicName { function, .. }
                if function == "println"
        )));
    }

    #[test]
    fn rejects_function_named_printn() {
        let errors = validate_module(&single_block_function("printn"))
            .expect_err("validator should reject function named 'printn'");
        assert!(errors.iter().any(|err| matches!(
            err,
            IrValidationError::ReservedRuntimeIntrinsicName { function, .. }
                if function == "printn"
        )));
    }

    #[test]
    fn rejects_function_named_read() {
        let errors = validate_module(&single_block_function("read"))
            .expect_err("validator should reject function named 'read'");
        assert!(errors.iter().any(|err| matches!(
            err,
            IrValidationError::ReservedRuntimeIntrinsicName { function, .. }
                if function == "read"
        )));
    }

    #[test]
    fn rejects_function_named_input() {
        let errors = validate_module(&single_block_function("input"))
            .expect_err("validator should reject function named 'input'");
        assert!(errors.iter().any(|err| matches!(
            err,
            IrValidationError::ReservedRuntimeIntrinsicName { function, .. }
                if function == "input"
        )));
    }

    #[test]
    fn rejects_function_named_assert() {
        let errors = validate_module(&single_block_function("assert"))
            .expect_err("validator should reject function named 'assert'");
        assert!(errors.iter().any(|err| matches!(
            err,
            IrValidationError::ReservedRuntimeIntrinsicName { function, .. }
                if function == "assert"
        )));
    }

    #[test]
    fn rejects_function_named_assert_eq() {
        let errors = validate_module(&single_block_function("assert_eq"))
            .expect_err("validator should reject function named 'assert_eq'");
        assert!(errors.iter().any(|err| matches!(
            err,
            IrValidationError::ReservedRuntimeIntrinsicName { function, .. }
                if function == "assert_eq"
        )));
    }

    #[test]
    fn rejects_function_named_is_known() {
        let errors = validate_module(&single_block_function("is_known"))
            .expect_err("validator should reject function named 'is_known'");
        assert!(errors.iter().any(|err| matches!(
            err,
            IrValidationError::ReservedRuntimeIntrinsicName { function, .. }
                if function == "is_known"
        )));
    }

    #[test]
    fn accepts_function_with_non_reserved_name() {
        assert_eq!(validate_module(&single_block_function("my_func")), Ok(()));
    }

    #[test]
    fn rejects_compound_assign_equivalent_ssabind_reaching_read_only_param() {
        // Simulates:  i += 1  in the body — a compound assignment also
        // produces an SsaBind destination that reaches the read_only counter.
        //
        //   B2 (body):
        //     v6 = ConstInt(1)
        //     v7 = Binary Add (v2, v6)
        //     v8 = SsaBind(I64, v7)   ← compound assign result stored via SsaBind
        //     Jump B4 [v8]
        let mut module = valid_for_loop_module();
        let body = &mut module.functions[0].blocks[2];
        body.insts = vec![
            IrInst::ConstInt { dst: ValueId(6), ty: IrType::I64, value: 1 },
            IrInst::Binary {
                dst: ValueId(7),
                op: BinaryOp::Add,
                ty: IrType::I64,
                lhs: ValueId(2),
                rhs: ValueId(6),
            },
            IrInst::SsaBind { dst: ValueId(8), ty: IrType::I64, src: ValueId(7) },
        ];
        body.term = IrTerminator::Jump { target: BlockId(4), args: vec![ValueId(8)] };

        let errors = validate_module(&module)
            .expect_err("validator must reject compound-assign to loop counter");
        assert!(
            errors.iter().any(|err| matches!(
                err,
                IrValidationError::LoopVariableReassignment { .. }
            )),
            "expected LoopVariableReassignment error, got: {:?}",
            errors
        );
    }

    // ── Runtime intrinsic signature tests (CX-77) ────────────────────────────

    #[test]
    fn validator_accepts_cx_printn_call_with_i64_arg() {
        // A call to `cx_printn(i64)` must pass validation even though cx_printn
        // has no IrFunction entry in the module — it is seeded by known_intrinsic_sigs.
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::ConstInt { dst: ValueId(0), ty: IrType::I64, value: 42 },
                        IrInst::Call {
                            dst: None,
                            callee: "cx_printn".to_string(),
                            args: vec![ValueId(0)],
                            return_ty: None,
                        },
                    ],
                    term: IrTerminator::Return { value: None },
                }],
            }],
        };
        assert_eq!(validate_module(&module), Ok(()));
    }

    #[test]
    fn validator_rejects_cx_printn_call_with_wrong_arg_type() {
        // cx_printn expects I64; passing I32 must produce a type-mismatch error.
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::ConstInt { dst: ValueId(0), ty: IrType::I32, value: 42 },
                        IrInst::Call {
                            dst: None,
                            callee: "cx_printn".to_string(),
                            args: vec![ValueId(0)],
                            return_ty: None,
                        },
                    ],
                    term: IrTerminator::Return { value: None },
                }],
            }],
        };
        let errors = validate_module(&module)
            .expect_err("validator must reject wrong-type arg to cx_printn");
        assert!(
            errors.iter().any(|err| matches!(
                err,
                IrValidationError::InvalidTypeUsage { detail, .. }
                if detail.contains("Call to 'cx_printn': argument 0 has type")
                    && detail.contains("expected I64")
            )),
            "expected type-mismatch error for cx_printn, got: {:?}",
            errors
        );
    }

    #[test]
    fn validator_rejects_void_intrinsic_call_with_dst_or_return_ty() {
        // cx_printn is void; a Call that assigns its result to a dst or declares
        // a return_ty must be rejected.
        let module = IrModule {
            debug_name: "m".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::ConstInt { dst: ValueId(0), ty: IrType::I64, value: 42 },
                        IrInst::Call {
                            dst: Some(ValueId(1)),
                            callee: "cx_printn".to_string(),
                            args: vec![ValueId(0)],
                            return_ty: Some(IrType::I64),
                        },
                    ],
                    term: IrTerminator::Return { value: None },
                }],
            }],
        };
        let errors = validate_module(&module)
            .expect_err("validator must reject void intrinsic call with dst/return_ty");
        assert!(
            errors.iter().any(|err| matches!(
                err,
                IrValidationError::InvalidTypeUsage { detail, .. }
                if detail.contains("Call to 'cx_printn': callee returns void")
            )),
            "expected void-return shape error for cx_printn, got: {:?}",
            errors
        );
    }
}
