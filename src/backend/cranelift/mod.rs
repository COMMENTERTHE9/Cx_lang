use crate::backend::{Backend, BackendError};
use crate::ir::types::{IrBlock, IrFunction, IrModule};
// `IrType` is used only by `ir_type_to_cranelift`, which is `#[cfg(feature =
// "jit")]`; gating the import keeps a default (non-jit) build warning-free
// (tracker #042).
#[cfg(feature = "jit")]
use crate::ir::types::IrType;
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
/// All ten `IrType` variants are handled:
///
/// | IrType  | Cranelift type | Notes                                                      |
/// |---------|---------------|------------------------------------------------------------|
/// | I8      | I8            | direct mapping                                             |
/// | I16     | I16           | direct mapping                                             |
/// | I32     | I32           | direct mapping                                             |
/// | I64     | I64           | direct mapping                                             |
/// | I128    | I128          | direct mapping                                             |
/// | F64     | F64           | direct mapping                                             |
/// | Bool    | I8            | booleans are 0/1, no native bool in CL                     |
/// | TBool   | I8            | three-state (0/1/2) fits in i8                             |
/// | Ptr     | I64           | 64-bit pointer on all supported targets                    |
/// | Void    | —             | error: Cranelift has no void type; void functions use an   |
/// |         |               | empty return list in their signature (no ABI param at all) |
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
        IrType::Void  => {
            return Err(CraneliftLoweringError::InvalidIrType {
                ty: "Void (Cranelift has no void type; void-return functions use an empty \
                     return list — IrType::Void must never reach the signature builder)"
                    .to_string(),
            });
        }
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
        IrInst::ArrayAlloca { dst, element_type, count } => {
            Err(CraneliftLoweringError::UnsupportedInstruction {
                inst: "ArrayAlloca".to_string(),
                context: format!("dst={dst:?} element_type={element_type:?} count={count}"),
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

/// Returns `true` when `e` is a compile-or-link failure that warrants an IR
/// dump.  `RuntimePanic` means the IR compiled successfully and the failure
/// happened at execution time, so the dump would not add signal.
#[cfg(feature = "jit")]
fn should_dump_ir_on_jit_failure(e: &host_boundary::JitExecutionError) -> bool {
    !matches!(e, host_boundary::JitExecutionError::RuntimePanic { .. })
}

impl Backend for CraneliftBackend {
    fn execute(&self, module: &IrModule) -> Result<(), BackendError> {
        #[cfg(feature = "jit")]
        {
            use host_boundary::JitExecutionError;
            use jit::run_jit;
            match run_jit(module) {
                Ok(outcome) => {
                    if outcome.exit_code.is_success() {
                        Ok(())
                    } else {
                        // Propagate the Cx program's own exit code unchanged.
                        let code = outcome.exit_code.raw();
                        Err(BackendError {
                            message: format!("program exited with code {}", code),
                            exit_code: code,
                        })
                    }
                }
                Err(e) => {
                    let exit_code = match &e {
                        // 127 = JIT_SKIP_EXIT_CODE: the JIT pipeline cannot handle
                        // this program.  The differential harness counts this as SKIP.
                        JitExecutionError::UnsupportedConstruct { .. }
                        | JitExecutionError::CodegenFailure { .. }
                        | JitExecutionError::MainNotFound => 127,
                        // 126 = JIT_RUNTIME_FAILURE: program compiled but crashed.
                        JitExecutionError::RuntimePanic { .. } => 126,
                    };
                    if should_dump_ir_on_jit_failure(&e) {
                        eprintln!("--- cx jit: compile/link failed — IR dump ---");
                        eprint!("{}", crate::ir::printer::print_module(module));
                        eprintln!("--- end IR dump ---");
                    }
                    Err(BackendError {
                        message: e.to_string(),
                        exit_code,
                    })
                }
            }
        }
        #[cfg(not(feature = "jit"))]
        {
            let _ = module;
            Err(BackendError {
                message: "Cranelift backend requires the `jit` feature — rebuild with --features jit".to_string(),
                exit_code: 1,
            })
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "jit"))]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use crate::ir::instr::IrTerminator;
    use crate::ir::types::{BlockId, IrBlock, IrFunction, IrModule, IrType, ValueId};

    // ── BackendError exit-code propagation tests ─────────────────────────────

    #[test]
    fn cranelift_backend_unsupported_construct_exits_127() {
        // IrInst::ConstInt with I128 is explicitly unsupported in the JIT backend.
        // CraneliftBackend::execute must map UnsupportedConstruct → exit_code 127.
        use crate::ir::instr::IrInst;
        let module = IrModule {
            debug_name: "test_unsupported".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![IrInst::ConstInt {
                        dst: ValueId(0),
                        ty: IrType::I128,
                        value: 0,
                    }],
                    term: IrTerminator::Return { value: Some(ValueId(0)) },
                }],
            }],
        };
        let err = CraneliftBackend.execute(&module).unwrap_err();
        assert_eq!(
            err.exit_code, 127,
            "UnsupportedConstruct must map to exit code 127, got {}",
            err.exit_code
        );
    }

    #[test]
    fn cranelift_backend_nonzero_program_exit_propagates() {
        // A Cx program that returns 42 must produce BackendError { exit_code: 42 }.
        use crate::ir::instr::IrInst;
        let module = IrModule {
            debug_name: "test_exit42".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![IrInst::ConstInt {
                        dst: ValueId(0),
                        ty: IrType::I32,
                        value: 42,
                    }],
                    term: IrTerminator::Return { value: Some(ValueId(0)) },
                }],
            }],
        };
        let err = CraneliftBackend.execute(&module).unwrap_err();
        assert_eq!(
            err.exit_code, 42,
            "non-zero Cx program exit must propagate as exit_code, got {}",
            err.exit_code
        );
    }

    #[test]
    fn cranelift_backend_zero_exit_is_ok() {
        // A program returning 0 must produce Ok(()) — no error, no exit code.
        use crate::ir::instr::IrInst;
        let module = IrModule {
            debug_name: "test_exit0".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![IrInst::ConstInt {
                        dst: ValueId(0),
                        ty: IrType::I32,
                        value: 0,
                    }],
                    term: IrTerminator::Return { value: Some(ValueId(0)) },
                }],
            }],
        };
        assert!(
            CraneliftBackend.execute(&module).is_ok(),
            "program exiting 0 must produce Ok(())"
        );
    }

    // ── IR dump gating tests ─────────────────────────────────────────────────

    #[test]
    fn ir_dump_suppressed_for_runtime_panic() {
        use host_boundary::JitExecutionError;
        let err = JitExecutionError::RuntimePanic { detail: "crash".to_string() };
        assert!(
            !should_dump_ir_on_jit_failure(&err),
            "RuntimePanic must not trigger an IR dump"
        );
    }

    #[test]
    fn ir_dump_enabled_for_codegen_failure() {
        use host_boundary::JitExecutionError;
        let err = JitExecutionError::CodegenFailure { detail: "bad".to_string() };
        assert!(
            should_dump_ir_on_jit_failure(&err),
            "CodegenFailure must trigger an IR dump"
        );
    }

    #[test]
    fn ir_dump_enabled_for_unsupported_construct() {
        use host_boundary::JitExecutionError;
        let err = JitExecutionError::UnsupportedConstruct { construct: "X".to_string() };
        assert!(
            should_dump_ir_on_jit_failure(&err),
            "UnsupportedConstruct must trigger an IR dump"
        );
    }

    #[test]
    fn ir_dump_enabled_for_main_not_found() {
        use host_boundary::JitExecutionError;
        let err = JitExecutionError::MainNotFound;
        assert!(
            should_dump_ir_on_jit_failure(&err),
            "MainNotFound must trigger an IR dump"
        );
    }

    // ── Type mapping tests ───────────────────────────────────────────────────

    /// IrType::Void must produce an error from ir_type_to_cranelift, not a
    /// Cranelift type, because Cranelift has no void type.  Void-return
    /// functions skip the return-type ABI param entirely in build_cl_signature.
    #[test]
    fn ir_type_to_cranelift_void_returns_error() {
        let result = ir_type_to_cranelift(&IrType::Void);
        assert!(
            matches!(result, Err(CraneliftLoweringError::InvalidIrType { .. })),
            "expected InvalidIrType for Void, got {:?}",
            result
        );
    }

    /// All non-void scalar types must map successfully.
    #[test]
    fn ir_type_to_cranelift_scalar_types_succeed() {
        for ty in &[
            IrType::I8,
            IrType::I16,
            IrType::I32,
            IrType::I64,
            IrType::F64,
            IrType::Bool,
            IrType::TBool,
            IrType::Ptr,
        ] {
            assert!(
                ir_type_to_cranelift(ty).is_ok(),
                "expected Ok for {ty:?}, got Err"
            );
        }
    }
}
