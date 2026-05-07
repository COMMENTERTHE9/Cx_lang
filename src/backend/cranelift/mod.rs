use crate::backend::Backend;
use crate::ir::types::{IrBlock, IrFunction, IrModule, IrType};
use crate::ir::instr::{IrInst, IrTerminator};

pub mod aot;
pub mod host_boundary;
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
        IrTerminator::Trap => {
            Err(CraneliftLoweringError::UnsupportedTerminator {
                term: "Trap".to_string(),
                context: "assertion-failure abort".to_string(),
            })
        }
    }
}

// ── Backend impl ─────────────────────────────────────────────────────────────

impl Backend for CraneliftBackend {
    fn execute(&self, module: &IrModule) -> Result<(), String> {
        #[cfg(feature = "jit")]
        {
            use jit::run_jit;
            match run_jit(module) {
                Ok(outcome) => {
                    if outcome.exit_code.is_success() {
                        Ok(())
                    } else {
                        Err(format!(
                            "JIT: program exited with code {}",
                            outcome.exit_code
                        ))
                    }
                }
                Err(e) => Err(e.to_string()),
            }
        }
        #[cfg(not(feature = "jit"))]
        {
            let _ = module;
            Err("Cranelift backend requires the `jit` feature — rebuild with --features jit".to_string())
        }
    }
}
