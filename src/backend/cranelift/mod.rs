use crate::backend::Backend;
use crate::ir::types::{IrBlock, IrFunction, IrModule, IrType};
use crate::ir::instr::{IrInst, IrTerminator};

pub mod aot;
pub mod jit;

pub struct CraneliftBackend;

// ── Structured error type ────────────────────────────────────────────────────

/// Errors produced by the Cranelift lowering skeleton.
///
/// Every variant carries enough context to identify the exact construct that
/// could not be lowered, without requiring the caller to re-inspect the IR.
#[derive(Debug, Clone)]
pub enum CraneliftLoweringError {
    /// An `IrType` has no direct Cranelift equivalent and cannot be lowered.
    InvalidIrType { ty: String },
    /// An IR instruction is structurally valid but not yet implemented by the
    /// Cranelift backend.
    UnsupportedInstruction { inst: String, context: String },
    /// An IR terminator is structurally valid but not yet implemented by the
    /// Cranelift backend.
    UnsupportedTerminator { term: String, context: String },
    /// A function-level failure wrapping a lower-level error with function
    /// name context.
    FunctionLoweringFailed { function: String, reason: String },
}

impl std::fmt::Display for CraneliftLoweringError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CraneliftLoweringError::InvalidIrType { ty } => {
                write!(f, "IrType '{ty}' has no Cranelift equivalent")
            }
            CraneliftLoweringError::UnsupportedInstruction { inst, context } => {
                write!(f, "unsupported instruction '{inst}': {context}")
            }
            CraneliftLoweringError::UnsupportedTerminator { term, context } => {
                write!(f, "unsupported terminator '{term}': {context}")
            }
            CraneliftLoweringError::FunctionLoweringFailed { function, reason } => {
                write!(f, "failed to lower function '{function}': {reason}")
            }
        }
    }
}

// ── IrType → Cranelift type mapping ─────────────────────────────────────────

/// Map an [`IrType`] to its Cranelift [`cranelift_codegen::ir::types::Type`]
/// equivalent.
///
/// All nine `IrType` variants are handled:
///
/// | IrType  | Cranelift type | Notes                                    |
/// |---------|---------------|------------------------------------------|
/// | I8      | I8            | direct mapping                           |
/// | I16     | I16           | direct mapping                           |
/// | I32     | I32           | direct mapping                           |
/// | I64     | I64           | direct mapping                           |
/// | I128    | I128          | direct mapping                           |
/// | F64     | F64           | direct mapping                           |
/// | Bool    | I8            | booleans are 0/1, no native bool in CL   |
/// | TBool   | I8            | three-state (0/1/2) fits in i8           |
/// | Ptr     | I64           | 64-bit pointer on all supported targets  |
#[cfg(feature = "jit")]
pub fn ir_type_to_cranelift(
    ty: &IrType,
) -> Result<cranelift_codegen::ir::types::Type, CraneliftLoweringError> {
    use cranelift_codegen::ir::types;
    Ok(match ty {
        IrType::I8    => types::I8,
        IrType::I16   => types::I16,
        IrType::I32   => types::I32,
        IrType::I64   => types::I64,
        IrType::I128  => types::I128,
        IrType::F64   => types::F64,
        IrType::Bool  => types::I8,
        IrType::TBool => types::I8,
        IrType::Ptr   => types::I64,
    })
}

// ── Module traversal skeleton ────────────────────────────────────────────────

/// Walk every function in `module` through the lowering skeleton.
///
/// Returns the first [`CraneliftLoweringError`] encountered, with the
/// function name prepended as [`CraneliftLoweringError::FunctionLoweringFailed`].
/// Returns `Ok(())` only when the module contains no functions.
pub fn lower_module(module: &IrModule) -> Result<(), CraneliftLoweringError> {
    for function in &module.functions {
        lower_function(function).map_err(|e| CraneliftLoweringError::FunctionLoweringFailed {
            function: function.name.clone(),
            reason: e.to_string(),
        })?;
    }
    Ok(())
}

fn lower_function(function: &IrFunction) -> Result<(), CraneliftLoweringError> {
    for block in &function.blocks {
        lower_block(block)?;
    }
    Ok(())
}

fn lower_block(block: &IrBlock) -> Result<(), CraneliftLoweringError> {
    for inst in &block.insts {
        lower_instruction(inst)?;
    }
    lower_terminator(&block.term)
}

fn lower_instruction(inst: &IrInst) -> Result<(), CraneliftLoweringError> {
    match inst {
        IrInst::ConstInt { dst, ty, value } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "ConstInt".to_string(),
                context: format!("dst={dst:?} ty={ty:?} value={value}"),
            })
        }
        IrInst::ConstFloat { dst, value } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "ConstFloat".to_string(),
                context: format!("dst={dst:?} value={value}"),
            })
        }
        IrInst::SsaBind { dst, ty, src } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "SsaBind".to_string(),
                context: format!("dst={dst:?} ty={ty:?} src={src:?}"),
            })
        }
        IrInst::Binary { dst, op, ty, lhs, rhs } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "Binary".to_string(),
                context: format!("dst={dst:?} op={op:?} ty={ty:?} lhs={lhs:?} rhs={rhs:?}"),
            })
        }
        IrInst::Compare { dst, op, lhs, rhs } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "Compare".to_string(),
                context: format!("dst={dst:?} op={op:?} lhs={lhs:?} rhs={rhs:?}"),
            })
        }
        IrInst::Call { dst, callee, args, return_ty } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "Call".to_string(),
                context: format!(
                    "callee={callee} args={args:?} dst={dst:?} return_ty={return_ty:?}"
                ),
            })
        }
        IrInst::Cast { dst, from, to, value } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "Cast".to_string(),
                context: format!("dst={dst:?} from={from:?} to={to:?} value={value:?}"),
            })
        }
        IrInst::Alloca { dst, size, align } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "Alloca".to_string(),
                context: format!("dst={dst:?} size={size} align={align}"),
            })
        }
        IrInst::PtrOffset { dst, base, offset } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "PtrOffset".to_string(),
                context: format!("dst={dst:?} base={base:?} offset={offset}"),
            })
        }
        IrInst::PtrAdd { dst, base, offset } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "PtrAdd".to_string(),
                context: format!("dst={dst:?} base={base:?} offset={offset:?}"),
            })
        }
        IrInst::Load { dst, ty, ptr } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "Load".to_string(),
                context: format!("dst={dst:?} ty={ty:?} ptr={ptr:?}"),
            })
        }
        IrInst::Store { ptr, value } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "Store".to_string(),
                context: format!("ptr={ptr:?} value={value:?}"),
            })
        }
    }
}

fn lower_terminator(term: &IrTerminator) -> Result<(), CraneliftLoweringError> {
    match term {
        IrTerminator::Jump { target, args } => {
            Err(CraneliftLoweringError::UnsupportedTerminator {
                term: "Jump".to_string(),
                context: format!("target={target:?} args={args:?}"),
            })
        }
        IrTerminator::Branch {
            cond,
            then_block,
            then_args,
            else_block,
            else_args,
        } => Err(CraneliftLoweringError::UnsupportedTerminator {
            term: "Branch".to_string(),
            context: format!(
                "cond={cond:?} then={then_block:?} then_args={then_args:?} \
                 else={else_block:?} else_args={else_args:?}"
            ),
        }),
        IrTerminator::Return { value } => {
            Err(CraneliftLoweringError::UnsupportedTerminator {
                term: "Return".to_string(),
                context: format!("value={value:?}"),
            })
        }
    }
}

// ── Backend impl ─────────────────────────────────────────────────────────────

impl Backend for CraneliftBackend {
    fn execute(&self, module: &IrModule) -> Result<(), String> {
        lower_module(module).map_err(|e| e.to_string())
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::types::{BlockId, IrBlock, IrFunction, IrModule, IrType, ValueId};
    use crate::ir::instr::{IrInst, IrTerminator};

    fn one_function_module(name: &str, term: IrTerminator) -> IrModule {
        IrModule {
            debug_name: "test".to_string(),
            functions: vec![IrFunction {
                name: name.to_string(),
                params: vec![],
                return_ty: None,
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![],
                    term,
                }],
            }],
        }
    }

    // ── lower_module ─────────────────────────────────────────────────────────

    #[test]
    fn lower_module_empty_module_succeeds() {
        let module = IrModule {
            debug_name: "empty".to_string(),
            functions: vec![],
        };
        assert!(lower_module(&module).is_ok());
    }

    #[test]
    fn lower_module_wraps_inner_error_with_function_name() {
        let module = one_function_module("my_fn", IrTerminator::Return { value: None });
        let err = lower_module(&module).unwrap_err();
        assert!(
            matches!(
                &err,
                CraneliftLoweringError::FunctionLoweringFailed { function, .. }
                if function == "my_fn"
            ),
            "expected FunctionLoweringFailed for 'my_fn', got: {err:?}"
        );
    }

    // ── lower_terminator ─────────────────────────────────────────────────────

    #[test]
    fn terminator_return_produces_named_unsupported_error() {
        let term = IrTerminator::Return { value: None };
        let err = lower_terminator(&term).unwrap_err();
        assert!(
            matches!(&err, CraneliftLoweringError::UnsupportedTerminator { term, .. } if term == "Return"),
            "expected UnsupportedTerminator 'Return', got: {err:?}"
        );
    }

    #[test]
    fn terminator_jump_produces_named_unsupported_error() {
        let term = IrTerminator::Jump { target: BlockId(1), args: vec![] };
        let err = lower_terminator(&term).unwrap_err();
        assert!(
            matches!(&err, CraneliftLoweringError::UnsupportedTerminator { term, .. } if term == "Jump"),
            "expected UnsupportedTerminator 'Jump', got: {err:?}"
        );
    }

    #[test]
    fn terminator_branch_produces_named_unsupported_error() {
        let term = IrTerminator::Branch {
            cond: ValueId(0),
            then_block: BlockId(1),
            then_args: vec![],
            else_block: BlockId(2),
            else_args: vec![],
        };
        let err = lower_terminator(&term).unwrap_err();
        assert!(
            matches!(&err, CraneliftLoweringError::UnsupportedTerminator { term, .. } if term == "Branch"),
            "expected UnsupportedTerminator 'Branch', got: {err:?}"
        );
    }

    // ── lower_instruction ────────────────────────────────────────────────────

    #[test]
    fn instruction_const_int_produces_named_error() {
        let inst = IrInst::ConstInt { dst: ValueId(0), ty: IrType::I32, value: 42 };
        let err = lower_instruction(&inst).unwrap_err();
        assert!(
            matches!(&err, CraneliftLoweringError::UnsupportedInstruction { inst, .. } if inst == "ConstInt"),
            "expected UnsupportedInstruction 'ConstInt', got: {err:?}"
        );
    }

    #[test]
    fn instruction_const_float_produces_named_error() {
        let inst = IrInst::ConstFloat { dst: ValueId(1), value: 3.14 };
        let err = lower_instruction(&inst).unwrap_err();
        assert!(
            matches!(&err, CraneliftLoweringError::UnsupportedInstruction { inst, .. } if inst == "ConstFloat"),
            "expected UnsupportedInstruction 'ConstFloat', got: {err:?}"
        );
    }

    #[test]
    fn instruction_ssa_bind_produces_named_error() {
        let inst = IrInst::SsaBind { dst: ValueId(2), ty: IrType::I64, src: ValueId(1) };
        let err = lower_instruction(&inst).unwrap_err();
        assert!(
            matches!(&err, CraneliftLoweringError::UnsupportedInstruction { inst, .. } if inst == "SsaBind"),
            "expected UnsupportedInstruction 'SsaBind', got: {err:?}"
        );
    }

    #[test]
    fn instruction_load_produces_named_error() {
        let inst = IrInst::Load { dst: ValueId(3), ty: IrType::I64, ptr: ValueId(0) };
        let err = lower_instruction(&inst).unwrap_err();
        assert!(
            matches!(&err, CraneliftLoweringError::UnsupportedInstruction { inst, .. } if inst == "Load"),
            "expected UnsupportedInstruction 'Load', got: {err:?}"
        );
    }

    #[test]
    fn instruction_store_produces_named_error() {
        let inst = IrInst::Store { ptr: ValueId(0), value: ValueId(1) };
        let err = lower_instruction(&inst).unwrap_err();
        assert!(
            matches!(&err, CraneliftLoweringError::UnsupportedInstruction { inst, .. } if inst == "Store"),
            "expected UnsupportedInstruction 'Store', got: {err:?}"
        );
    }

    #[test]
    fn instruction_alloca_produces_named_error() {
        let inst = IrInst::Alloca { dst: ValueId(0), size: 8, align: 8 };
        let err = lower_instruction(&inst).unwrap_err();
        assert!(
            matches!(&err, CraneliftLoweringError::UnsupportedInstruction { inst, .. } if inst == "Alloca"),
            "expected UnsupportedInstruction 'Alloca', got: {err:?}"
        );
    }

    #[test]
    fn instruction_ptr_offset_produces_named_error() {
        let inst = IrInst::PtrOffset { dst: ValueId(1), base: ValueId(0), offset: 8 };
        let err = lower_instruction(&inst).unwrap_err();
        assert!(
            matches!(&err, CraneliftLoweringError::UnsupportedInstruction { inst, .. } if inst == "PtrOffset"),
            "expected UnsupportedInstruction 'PtrOffset', got: {err:?}"
        );
    }

    #[test]
    fn instruction_ptr_add_produces_named_error() {
        let inst = IrInst::PtrAdd { dst: ValueId(2), base: ValueId(0), offset: ValueId(1) };
        let err = lower_instruction(&inst).unwrap_err();
        assert!(
            matches!(&err, CraneliftLoweringError::UnsupportedInstruction { inst, .. } if inst == "PtrAdd"),
            "expected UnsupportedInstruction 'PtrAdd', got: {err:?}"
        );
    }

    // ── Display ──────────────────────────────────────────────────────────────

    #[test]
    fn error_display_contains_instruction_name() {
        let err = CraneliftLoweringError::UnsupportedInstruction {
            inst: "Load".to_string(),
            context: "dst=ValueId(1) ty=I64 ptr=ValueId(0)".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Load"), "display should mention 'Load': {msg}");
    }

    #[test]
    fn error_display_contains_terminator_name() {
        let err = CraneliftLoweringError::UnsupportedTerminator {
            term: "Return".to_string(),
            context: "value=None".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Return"), "display should mention 'Return': {msg}");
    }

    #[test]
    fn error_display_function_failed_contains_name() {
        let err = CraneliftLoweringError::FunctionLoweringFailed {
            function: "compute".to_string(),
            reason: "unsupported terminator 'Return': value=None".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("compute"), "display should mention 'compute': {msg}");
    }

    // ── IrType → Cranelift type mapping ─────────────────────────────────────

    #[cfg(feature = "jit")]
    #[test]
    fn ir_type_to_cranelift_maps_all_scalar_integer_types() {
        use cranelift_codegen::ir::types;
        assert_eq!(ir_type_to_cranelift(&IrType::I8).unwrap(), types::I8);
        assert_eq!(ir_type_to_cranelift(&IrType::I16).unwrap(), types::I16);
        assert_eq!(ir_type_to_cranelift(&IrType::I32).unwrap(), types::I32);
        assert_eq!(ir_type_to_cranelift(&IrType::I64).unwrap(), types::I64);
        assert_eq!(ir_type_to_cranelift(&IrType::I128).unwrap(), types::I128);
    }

    #[cfg(feature = "jit")]
    #[test]
    fn ir_type_to_cranelift_maps_f64() {
        use cranelift_codegen::ir::types;
        assert_eq!(ir_type_to_cranelift(&IrType::F64).unwrap(), types::F64);
    }

    #[cfg(feature = "jit")]
    #[test]
    fn ir_type_to_cranelift_bool_and_tbool_map_to_i8() {
        use cranelift_codegen::ir::types;
        assert_eq!(ir_type_to_cranelift(&IrType::Bool).unwrap(), types::I8);
        assert_eq!(ir_type_to_cranelift(&IrType::TBool).unwrap(), types::I8);
    }

    #[cfg(feature = "jit")]
    #[test]
    fn ir_type_to_cranelift_ptr_maps_to_i64() {
        use cranelift_codegen::ir::types;
        assert_eq!(ir_type_to_cranelift(&IrType::Ptr).unwrap(), types::I64);
    }
}
