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
}

pub fn validate_module(module: &IrModule) -> Result<(), Vec<IrValidationError>> {
    let mut errors = Vec::new();

    for (function_index, function) in module.functions.iter().enumerate() {
        validate_function(function_index, function, &mut errors);
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_function(
    function_index: usize,
    function: &IrFunction,
    errors: &mut Vec<IrValidationError>,
) {
    if function.name.is_empty() {
        errors.push(IrValidationError::EmptyFunctionName { function_index });
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
            validate_inst(function, block.id, inst, &mut defined_values, errors);
        }

        validate_terminator(
            function,
            block.id,
            &block.term,
            &defined_values,
            &blocks_by_id,
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
            args,
            return_ty,
            ..
        } => {
            for arg in args {
                require_value(function, block, *arg, "Call arg", defined_values, errors);
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
    }
}

fn validate_terminator(
    function: &IrFunction,
    block: BlockId,
    term: &IrTerminator,
    defined_values: &HashMap<ValueId, IrType>,
    blocks_by_id: &HashMap<BlockId, &IrBlock>,
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
                        },
                        BlockParam {
                            value: ValueId(0),
                            ty: IrType::I64,
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
}
