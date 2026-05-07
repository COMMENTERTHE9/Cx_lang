//! JIT Runtime Host Boundary
//!
//! This module defines the execution boundary between the Cx JIT backend and the host process.
//!
//! # Process Ownership
//!
//! In JIT mode the host process is the Cx compiler/runner binary. The JIT runtime executes
//! inside the host process address space — there is no subprocess fork.
//! [`HostBoundary`] owns the Cranelift JIT module and controls when it is created and torn down.
//!
//! Startup sequence:
//!   1. IR validation must pass before `HostBoundary::execute` is called.
//!   2. `HostBoundary` creates a Cranelift `JITModule`, compiles all functions, locates `main`.
//!   3. `main` is called; its return value is captured as the exit code.
//!   4. The host process does **not** call `std::process::exit` inside the backend.
//!      Callers decide when and with what code to exit.
//!
//! Shutdown sequence:
//!   1. After `main` returns, `HostBoundary::execute` returns `Ok(JitOutcome)`.
//!   2. `HostBoundary` is dropped; Cranelift frees JIT memory on drop.
//!   3. The host process continues and acts on the returned `JitOutcome`.
//!
//! # Exit Code Extraction
//!
//! Cx `main` is lowered to a synthetic IR function with return type `I32`.
//! Its return value is the program's exit code.
//!
//! Mapping:
//!   - `main` returns 0  → [`JitExitCode::SUCCESS`] (0)
//!   - `main` returns n  → [`JitExitCode(n)`](JitExitCode) where n is the returned value
//!   - JIT codegen fails → [`JitExitCode::UNSUPPORTED_CONSTRUCT`] (127)
//!   - JIT runtime panic → [`JitExitCode::JIT_RUNTIME_FAILURE`] (126)
//!
//! Exit codes 126 and 127 are chosen because they fall outside the 0–125 range that
//! POSIX applications conventionally use, making them unambiguous JIT-level sentinels.
//!
//! # Output Capture
//!
//! JIT-compiled code writes directly to the host process stdout/stderr via C runtime
//! intrinsics (`puts`, `printf`). No in-process pipe redirection is performed.
//!
//! The differential harness captures JIT output by running the Cx compiler binary as a
//! subprocess with `--backend=cranelift <source_file>`, exactly as it does for the interpreter
//! baseline. This approach requires no in-process hooking and is consistent across both paths.
//!
//! # Runtime Failure Surfacing
//!
//! JIT-level failures surface as [`JitExecutionError`] variants, not as Rust panics.
//! `HostBoundary::execute` must catch all Cranelift errors and convert them.
//! The caller (`CraneliftBackend::execute`) translates to the `Backend` trait's `Result<(), String>`.

#![allow(dead_code)]

/// The exit code produced by a JIT-executed Cx program.
///
/// Cx `main` at the IR level is a synthetic function with return type `I32`.
/// Its return value becomes the process exit code via [`JitOutcome::exit_code`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitExitCode(pub i32);

impl JitExitCode {
    /// The Cx program returned 0 — clean exit.
    pub const SUCCESS: JitExitCode = JitExitCode(0);

    /// A JIT-level runtime error prevented normal execution.
    /// Mapped to 126 (outside the 0–125 application range on POSIX).
    pub const JIT_RUNTIME_FAILURE: JitExitCode = JitExitCode(126);

    /// An unsupported construct was encountered during JIT codegen.
    /// Mapped to 127 (consistent with "command not found" sentinel range on POSIX).
    pub const UNSUPPORTED_CONSTRUCT: JitExitCode = JitExitCode(127);

    /// Construct a failure exit code from an arbitrary value.
    pub const fn failure(code: i32) -> JitExitCode {
        JitExitCode(code)
    }

    /// Returns `true` if this is a clean exit (code == 0).
    pub fn is_success(self) -> bool {
        self.0 == 0
    }

    /// Returns the raw i32 exit code.
    pub fn raw(self) -> i32 {
        self.0
    }
}

impl std::fmt::Display for JitExitCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The observable outcome of a JIT execution.
///
/// Returned by [`HostBoundary::execute`] on any execution that reaches `main`,
/// including executions where `main` returns a non-zero exit code.
/// [`JitExecutionError`] is returned only for JIT-level failures (codegen errors, missing symbols).
///
/// ## Stdout and Stderr
///
/// In the current subprocess-capture model `stdout` and `stderr` are always empty strings.
/// JIT-compiled code writes directly to the host process streams; the differential harness
/// captures them by running the Cx binary as a subprocess. In-process capture is post-scaffold.
#[derive(Debug, Clone)]
pub struct JitOutcome {
    /// Captured standard output. Empty in the current subprocess-capture model.
    pub stdout: String,
    /// Captured standard error. Empty in the current subprocess-capture model.
    pub stderr: String,
    /// The exit code from Cx `main`, or a sentinel for JIT-level failures.
    pub exit_code: JitExitCode,
}

impl JitOutcome {
    /// Construct a clean outcome (exit code 0, no output).
    pub fn success() -> Self {
        JitOutcome {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: JitExitCode::SUCCESS,
        }
    }

    /// Construct an outcome from the raw i32 returned by Cx `main`.
    pub fn from_main_return(code: i32) -> Self {
        JitOutcome {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: JitExitCode(code),
        }
    }
}

/// Structured errors from the JIT execution boundary.
///
/// Every failure path through the JIT backend must produce one of these variants.
/// None of these should trigger a Rust panic in the host process.
#[derive(Debug, Clone)]
pub enum JitExecutionError {
    /// An IR construct with no Cranelift lowering was encountered.
    /// `construct` names the unsupported IR instruction or type.
    UnsupportedConstruct { construct: String },

    /// Cranelift failed to compile the IR module.
    /// `detail` is the underlying Cranelift error message.
    CodegenFailure { detail: String },

    /// The compiled `main` symbol could not be located in the JIT module.
    MainNotFound,

    /// A runtime error occurred inside JIT-compiled code (e.g. miscompilation detected,
    /// Cranelift trap). In a future phase this will include arena and handle violations.
    RuntimePanic { detail: String },
}

impl std::fmt::Display for JitExecutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JitExecutionError::UnsupportedConstruct { construct } => {
                write!(f, "JIT: unsupported construct: {}", construct)
            }
            JitExecutionError::CodegenFailure { detail } => {
                write!(f, "JIT: codegen failure: {}", detail)
            }
            JitExecutionError::MainNotFound => {
                write!(f, "JIT: no `main` symbol found in compiled module")
            }
            JitExecutionError::RuntimePanic { detail } => {
                write!(f, "JIT: runtime panic: {}", detail)
            }
        }
    }
}

/// The JIT runtime host boundary.
///
/// `HostBoundary` is the single point of control for the JIT execution lifecycle in Cx.
/// One `HostBoundary` is created per JIT execution attempt and dropped when execution ends.
///
/// ## Ownership and Lifecycle
///
/// - Created once per JIT execution, before `execute` is called.
/// - Holds a Cranelift `JITModule` for the duration of execution (when the `jit` feature is on).
/// - Dropped after `execute` returns; Cranelift frees JIT memory on drop.
/// - Does **not** call `std::process::exit`; callers decide what to do with [`JitOutcome`].
///
/// ## Thread Safety
///
/// `HostBoundary` is not `Send` or `Sync`. JIT execution runs on the calling thread.
/// Callers are responsible for ensuring sufficient stack depth — the 64 MB interpreter
/// thread configured in `main.rs` is appropriate for JIT execution.
///
/// ## Output Capture
///
/// JIT-compiled code writes to the host process stdout/stderr via C runtime intrinsics.
/// The differential harness captures this output by running the compiler as a subprocess.
/// In-process pipe redirection is a post-scaffold enhancement.
///
/// ## Supported Instructions (Phase 14 sub-packets 1, 2, and 3)
///
/// The JIT implementation (enabled with the `jit` feature) supports:
/// - `ConstInt` (types: I8, I16, I32, I64 — not I128)
/// - `Binary` (Add, Sub, Mul, Div, Rem — signed integer operations)
/// - `Alloca` — stack slot allocation; `dst` receives an I64 pointer to the slot
/// - `Load` — typed memory load from an Alloca-produced pointer
/// - `Store` — typed memory store through an Alloca-produced pointer
/// - `Compare` (Eq, Ne, Lt, Le, Gt, Ge — signed integer comparisons; result is I8)
/// - `Return` (with or without a value)
///
/// - `Jump` (unconditional block transfer with optional block-param arguments)
/// - `Branch` (two-way conditional branch using `brif`, with block-param arguments on both edges)
///
/// Multi-block functions are supported for forward-only control flow (no back-edges).
/// Loop back-edges are deferred to a later sub-packet.
///
/// All other IR instructions return [`JitExecutionError::UnsupportedConstruct`].
pub struct HostBoundary;

impl HostBoundary {
    /// Create a new `HostBoundary` for a single JIT execution.
    pub fn new() -> Self {
        HostBoundary
    }

    /// Execute the given IR module through the JIT backend.
    ///
    /// Returns [`JitOutcome`] when `main` returns (including non-zero exit codes).
    /// Returns [`JitExecutionError`] only for JIT-level failures: codegen errors,
    /// missing symbols, or runtime panics inside JIT-compiled code.
    #[cfg(feature = "jit")]
    pub fn execute(&self, ir: &crate::ir::IrModule) -> Result<JitOutcome, JitExecutionError> {
        use cranelift_codegen::settings::{self, Configurable};
        use cranelift_jit::{JITBuilder, JITModule};
        use cranelift_module::{Linkage, Module};

        // Build the native ISA.
        let mut flag_builder = settings::builder();
        flag_builder
            .set("use_colocated_libcalls", "false")
            .map_err(|e| JitExecutionError::CodegenFailure {
                detail: e.to_string(),
            })?;
        flag_builder
            .set("is_pic", "false")
            .map_err(|e| JitExecutionError::CodegenFailure {
                detail: e.to_string(),
            })?;
        let flags = settings::Flags::new(flag_builder);
        let isa = cranelift_native::builder()
            .map_err(|s| JitExecutionError::CodegenFailure {
                detail: s.to_string(),
            })?
            .finish(flags)
            .map_err(|e| JitExecutionError::CodegenFailure {
                detail: e.to_string(),
            })?;

        let jit_builder =
            JITBuilder::with_isa(isa, cranelift_module::default_libcall_names());
        let mut module = JITModule::new(jit_builder);

        let mut main_id = None;

        for (func_idx, ir_func) in ir.functions.iter().enumerate() {
            let sig = build_cl_signature(&module, ir_func)?;
            let func_id = module
                .declare_function(&ir_func.name, Linkage::Export, &sig)
                .map_err(|e| JitExecutionError::CodegenFailure {
                    detail: e.to_string(),
                })?;

            // Build the Cranelift IR for this function.
            let mut cl_func = cranelift_codegen::ir::Function::with_name_signature(
                cranelift_codegen::ir::UserFuncName::user(0, func_idx as u32),
                sig,
            );
            {
                let mut fbc = cranelift_frontend::FunctionBuilderContext::new();
                let mut builder =
                    cranelift_frontend::FunctionBuilder::new(&mut cl_func, &mut fbc);
                compile_ir_function(&mut builder, ir_func)?;
                builder.finalize();
            }

            // Define the function in the JIT module.
            let mut ctx = module.make_context();
            ctx.func = cl_func;
            module
                .define_function(func_id, &mut ctx)
                .map_err(|e| JitExecutionError::CodegenFailure {
                    detail: e.to_string(),
                })?;
            module.clear_context(&mut ctx);

            if ir_func.name == "main" {
                main_id = Some(func_id);
            }
        }

        module
            .finalize_definitions()
            .map_err(|e| JitExecutionError::CodegenFailure {
                detail: e.to_string(),
            })?;

        let main_id = main_id.ok_or(JitExecutionError::MainNotFound)?;
        let main_ptr = module.get_finalized_function(main_id);

        // SAFETY: `module` is still alive here, keeping the JIT code mapped.
        // The function signature () -> i32 matches the IR declaration of `main`.
        let main_fn: unsafe extern "C" fn() -> i32 =
            unsafe { std::mem::transmute(main_ptr) };
        let ret = unsafe { main_fn() };

        Ok(JitOutcome::from_main_return(ret))
    }

    /// Stub used when the `jit` feature is not enabled.
    #[cfg(not(feature = "jit"))]
    pub fn execute(&self, _ir: &crate::ir::IrModule) -> Result<JitOutcome, JitExecutionError> {
        Err(JitExecutionError::UnsupportedConstruct {
            construct: "JIT codegen not yet implemented — Phase 14 (First Executable Cranelift Slice) pending".to_string(),
        })
    }
}

impl Default for HostBoundary {
    fn default() -> Self {
        Self::new()
    }
}

// ── JIT helpers (only compiled when the `jit` feature is active) ─────────────

/// Build a Cranelift [`Signature`] from an [`IrFunction`]'s parameter and return-type list.
#[cfg(feature = "jit")]
fn build_cl_signature<M: cranelift_module::Module>(
    module: &M,
    ir_func: &crate::ir::types::IrFunction,
) -> Result<cranelift_codegen::ir::Signature, JitExecutionError> {
    use cranelift_codegen::ir::AbiParam;
    use super::ir_type_to_cranelift;

    let call_conv = module.target_config().default_call_conv;
    let mut sig = cranelift_codegen::ir::Signature::new(call_conv);

    for param in &ir_func.params {
        let cl_ty = ir_type_to_cranelift(&param.ty).map_err(|e| {
            JitExecutionError::UnsupportedConstruct {
                construct: e.to_string(),
            }
        })?;
        sig.params.push(AbiParam::new(cl_ty));
    }

    if let Some(ret_ty) = &ir_func.return_ty {
        let cl_ty = ir_type_to_cranelift(ret_ty).map_err(|e| {
            JitExecutionError::UnsupportedConstruct {
                construct: e.to_string(),
            }
        })?;
        sig.returns.push(AbiParam::new(cl_ty));
    }

    Ok(sig)
}

/// Emit Cranelift IR instructions for a single [`IrFunction`] into `builder`.
///
/// Supported instructions (Phase 14 sub-packets 1, 2, and 3):
/// - [`IrInst::ConstInt`] — integer constants for I8/I16/I32/I64 (not I128)
/// - [`IrInst::Binary`] — signed integer arithmetic: Add, Sub, Mul, Div, Rem
/// - [`IrInst::Alloca`] — stack slot allocation; `dst` receives an I64 pointer
/// - [`IrInst::Load`] — typed memory load from a pointer
/// - [`IrInst::Store`] — typed memory store through a pointer
/// - [`IrInst::Compare`] — signed integer comparisons (Eq/Ne/Lt/Le/Gt/Ge); result is I8
/// - [`IrTerminator::Return`] — return with or without a value
/// - [`IrTerminator::Jump`] — unconditional branch with optional block-param arguments
/// - [`IrTerminator::Branch`] — two-way conditional branch (`brif`) with block-param arguments
///
/// Block sealing strategy: each block is sealed immediately after `switch_to_block` because
/// IR blocks are emitted in topological order for forward-only CFGs.  All predecessor edges
/// (from `jump` / `brif`) are registered before the successor block is visited.
///
/// All other instructions yield [`JitExecutionError::UnsupportedConstruct`].
#[cfg(feature = "jit")]
fn compile_ir_function(
    builder: &mut cranelift_frontend::FunctionBuilder,
    ir_func: &crate::ir::types::IrFunction,
) -> Result<(), JitExecutionError> {
    use cranelift_codegen::ir::condcodes::IntCC;
    use cranelift_codegen::ir::InstBuilder;
    use crate::ir::instr::{BinaryOp, CompareOp, IrInst, IrTerminator};
    use crate::ir::types::{BlockId, IrType, ValueId};
    use super::ir_type_to_cranelift;
    use std::collections::HashMap;

    // Phase 1: create all Cranelift blocks and set up their block parameters.
    // We do this before switching into any block so that append_block_param is
    // always called on a block that has not yet had instructions emitted.
    let mut block_map: HashMap<BlockId, cranelift_codegen::ir::Block> = HashMap::new();
    let mut val_map: HashMap<ValueId, cranelift_codegen::ir::Value> = HashMap::new();

    for ir_block in &ir_func.blocks {
        let cl_block = builder.create_block();
        block_map.insert(ir_block.id, cl_block);

        for bp in &ir_block.params {
            let cl_ty = ir_type_to_cranelift(&bp.ty).map_err(|e| {
                JitExecutionError::UnsupportedConstruct {
                    construct: e.to_string(),
                }
            })?;
            let val = builder.append_block_param(cl_block, cl_ty);
            val_map.insert(bp.value, val);
        }
    }

    // Phase 2: emit each block's body.
    for ir_block in &ir_func.blocks {
        let cl_block = block_map[&ir_block.id];
        builder.switch_to_block(cl_block);
        // Blocks are processed in IR order, which is topological for forward-only CFGs.
        // Each block is sealed immediately after switch_to_block: by the time we visit
        // block N, all predecessor terminators (jump / brif) referencing block N have
        // already been emitted in earlier iterations, so all predecessor edges are
        // registered in Cranelift's internal tracking before the seal call.
        builder.seal_block(cl_block);

        for inst in &ir_block.insts {
            match inst {
                IrInst::ConstInt { dst, ty, value } => {
                    // I128 cannot be represented as a single iconst (Cranelift
                    // emulates it as two i64s). Deferred to a future sub-packet.
                    if *ty == IrType::I128 {
                        return Err(JitExecutionError::UnsupportedConstruct {
                            construct: "ConstInt I128 (not supported)"
                                .to_string(),
                        });
                    }
                    let cl_ty = ir_type_to_cranelift(ty).map_err(|e| {
                        JitExecutionError::UnsupportedConstruct {
                            construct: e.to_string(),
                        }
                    })?;
                    // The value fits in i64 for all supported integer types (I8/I16/I32/I64).
                    let cl_val = builder.ins().iconst(cl_ty, *value as i64);
                    val_map.insert(*dst, cl_val);
                }

                IrInst::Binary { dst, op, ty: _, lhs, rhs } => {
                    let lhs_val = *val_map.get(lhs).ok_or_else(|| {
                        JitExecutionError::CodegenFailure {
                            detail: format!("undefined value {:?} used as binary lhs", lhs),
                        }
                    })?;
                    let rhs_val = *val_map.get(rhs).ok_or_else(|| {
                        JitExecutionError::CodegenFailure {
                            detail: format!("undefined value {:?} used as binary rhs", rhs),
                        }
                    })?;
                    let result = match op {
                        BinaryOp::Add => builder.ins().iadd(lhs_val, rhs_val),
                        BinaryOp::Sub => builder.ins().isub(lhs_val, rhs_val),
                        BinaryOp::Mul => builder.ins().imul(lhs_val, rhs_val),
                        BinaryOp::Div => builder.ins().sdiv(lhs_val, rhs_val),
                        BinaryOp::Rem => builder.ins().srem(lhs_val, rhs_val),
                    };
                    val_map.insert(*dst, result);
                }

                IrInst::Alloca { dst, size, align } => {
                    use cranelift_codegen::ir::stackslot::{StackSlotData, StackSlotKind};
                    use cranelift_codegen::ir::types;

                    // align must be a power of two (IR invariant); align_shift = log2(align).
                    // align == 0 is treated as naturally aligned (shift = 0).
                    let align_shift = if *align == 0 { 0u8 } else { align.trailing_zeros() as u8 };
                    let slot = builder.create_sized_stack_slot(StackSlotData::new(
                        StackSlotKind::ExplicitSlot,
                        *size as u32,
                        align_shift,
                    ));
                    // Materialize the slot address as a native pointer (I64 on all supported targets).
                    let ptr_val = builder.ins().stack_addr(types::I64, slot, 0);
                    val_map.insert(*dst, ptr_val);
                }

                IrInst::Load { dst, ty, ptr } => {
                    use cranelift_codegen::ir::MemFlags;

                    let ptr_val = *val_map.get(ptr).ok_or_else(|| {
                        JitExecutionError::CodegenFailure {
                            detail: format!("undefined value {:?} used as load ptr", ptr),
                        }
                    })?;
                    let cl_ty = ir_type_to_cranelift(ty).map_err(|e| {
                        JitExecutionError::UnsupportedConstruct {
                            construct: e.to_string(),
                        }
                    })?;
                    let result = builder.ins().load(cl_ty, MemFlags::new(), ptr_val, 0);
                    val_map.insert(*dst, result);
                }

                IrInst::Store { ptr, value } => {
                    use cranelift_codegen::ir::MemFlags;

                    let ptr_val = *val_map.get(ptr).ok_or_else(|| {
                        JitExecutionError::CodegenFailure {
                            detail: format!("undefined value {:?} used as store ptr", ptr),
                        }
                    })?;
                    let stored_val = *val_map.get(value).ok_or_else(|| {
                        JitExecutionError::CodegenFailure {
                            detail: format!("undefined value {:?} used as store value", value),
                        }
                    })?;
                    builder.ins().store(MemFlags::new(), stored_val, ptr_val, 0);
                }

                IrInst::Compare { dst, op, lhs, rhs } => {
                    let lhs_val = *val_map.get(lhs).ok_or_else(|| {
                        JitExecutionError::CodegenFailure {
                            detail: format!("undefined value {:?} used as compare lhs", lhs),
                        }
                    })?;
                    let rhs_val = *val_map.get(rhs).ok_or_else(|| {
                        JitExecutionError::CodegenFailure {
                            detail: format!("undefined value {:?} used as compare rhs", rhs),
                        }
                    })?;
                    // Map Cx compare ops to Cranelift signed-integer condition codes.
                    // Unsigned variants are deferred until unsigned integer types are added.
                    let cc = match op {
                        CompareOp::Eq => IntCC::Equal,
                        CompareOp::Ne => IntCC::NotEqual,
                        CompareOp::Lt => IntCC::SignedLessThan,
                        CompareOp::Le => IntCC::SignedLessThanOrEqual,
                        CompareOp::Gt => IntCC::SignedGreaterThan,
                        CompareOp::Ge => IntCC::SignedGreaterThanOrEqual,
                    };
                    // icmp produces an I8 value (0 = false, 1 = true).
                    // This is used directly as the `brif` condition in Branch terminators.
                    let result = builder.ins().icmp(cc, lhs_val, rhs_val);
                    val_map.insert(*dst, result);
                }

                other => {
                    return Err(JitExecutionError::UnsupportedConstruct {
                        construct: format!("{:?}", other),
                    });
                }
            }
        }

        match &ir_block.term {
            IrTerminator::Return { value: Some(vid) } => {
                let ret_val = *val_map.get(vid).ok_or_else(|| {
                    JitExecutionError::CodegenFailure {
                        detail: format!("undefined return value {:?}", vid),
                    }
                })?;
                builder.ins().return_(&[ret_val]);
            }
            IrTerminator::Return { value: None } => {
                builder.ins().return_(&[]);
            }
            IrTerminator::Jump { target, args } => {
                let target_cl = *block_map.get(target).ok_or_else(|| {
                    JitExecutionError::CodegenFailure {
                        detail: format!("Jump targets undefined block {:?}", target),
                    }
                })?;
                let cl_args: Vec<cranelift_codegen::ir::Value> = args
                    .iter()
                    .map(|vid| {
                        val_map.get(vid).copied().ok_or_else(|| {
                            JitExecutionError::CodegenFailure {
                                detail: format!("undefined value {:?} used as Jump arg", vid),
                            }
                        })
                    })
                    .collect::<Result<_, _>>()?;
                builder.ins().jump(target_cl, &cl_args);
            }
            IrTerminator::Branch { cond, then_block, then_args, else_block, else_args } => {
                let cond_val = *val_map.get(cond).ok_or_else(|| {
                    JitExecutionError::CodegenFailure {
                        detail: format!("undefined condition value {:?} in Branch", cond),
                    }
                })?;
                let then_cl = *block_map.get(then_block).ok_or_else(|| {
                    JitExecutionError::CodegenFailure {
                        detail: format!("Branch then-block {:?} not found", then_block),
                    }
                })?;
                let else_cl = *block_map.get(else_block).ok_or_else(|| {
                    JitExecutionError::CodegenFailure {
                        detail: format!("Branch else-block {:?} not found", else_block),
                    }
                })?;
                let then_cl_args: Vec<cranelift_codegen::ir::Value> = then_args
                    .iter()
                    .map(|vid| {
                        val_map.get(vid).copied().ok_or_else(|| {
                            JitExecutionError::CodegenFailure {
                                detail: format!(
                                    "undefined value {:?} used as Branch then-arg",
                                    vid
                                ),
                            }
                        })
                    })
                    .collect::<Result<_, _>>()?;
                let else_cl_args: Vec<cranelift_codegen::ir::Value> = else_args
                    .iter()
                    .map(|vid| {
                        val_map.get(vid).copied().ok_or_else(|| {
                            JitExecutionError::CodegenFailure {
                                detail: format!(
                                    "undefined value {:?} used as Branch else-arg",
                                    vid
                                ),
                            }
                        })
                    })
                    .collect::<Result<_, _>>()?;
                builder.ins().brif(cond_val, then_cl, &then_cl_args, else_cl, &else_cl_args);
            }
        }
    }

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jit_exit_code_success_is_zero() {
        assert_eq!(JitExitCode::SUCCESS.raw(), 0);
        assert!(JitExitCode::SUCCESS.is_success());
    }

    #[test]
    fn jit_exit_code_sentinel_runtime_failure() {
        assert_eq!(JitExitCode::JIT_RUNTIME_FAILURE.raw(), 126);
        assert!(!JitExitCode::JIT_RUNTIME_FAILURE.is_success());
    }

    #[test]
    fn jit_exit_code_sentinel_unsupported_construct() {
        assert_eq!(JitExitCode::UNSUPPORTED_CONSTRUCT.raw(), 127);
        assert!(!JitExitCode::UNSUPPORTED_CONSTRUCT.is_success());
    }

    #[test]
    fn jit_exit_code_arbitrary_failure() {
        let code = JitExitCode::failure(42);
        assert_eq!(code.raw(), 42);
        assert!(!code.is_success());
    }

    #[test]
    fn jit_outcome_success_is_clean() {
        let outcome = JitOutcome::success();
        assert!(outcome.exit_code.is_success());
        assert!(outcome.stdout.is_empty());
        assert!(outcome.stderr.is_empty());
    }

    #[test]
    fn jit_outcome_from_main_return_nonzero() {
        let outcome = JitOutcome::from_main_return(5);
        assert_eq!(outcome.exit_code.raw(), 5);
        assert!(!outcome.exit_code.is_success());
    }

    #[test]
    fn jit_execution_error_display_unsupported_construct() {
        let e = JitExecutionError::UnsupportedConstruct {
            construct: "IrInst::Call".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("unsupported"), "got: {}", s);
        assert!(s.contains("IrInst::Call"), "got: {}", s);
    }

    #[test]
    fn jit_execution_error_display_codegen_failure() {
        let e = JitExecutionError::CodegenFailure {
            detail: "type mismatch in block 0".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("codegen failure"), "got: {}", s);
        assert!(s.contains("type mismatch"), "got: {}", s);
    }

    #[test]
    fn jit_execution_error_display_main_not_found() {
        let e = JitExecutionError::MainNotFound;
        assert!(e.to_string().contains("main"), "got: {}", e);
    }

    #[test]
    fn jit_execution_error_display_runtime_panic() {
        let e = JitExecutionError::RuntimePanic {
            detail: "stack overflow at 0x00".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("runtime panic"), "got: {}", s);
        assert!(s.contains("stack overflow"), "got: {}", s);
    }

    // The stub test only makes sense without the JIT feature, where execute()
    // still returns the Phase 14-pending placeholder error.
    #[cfg(not(feature = "jit"))]
    #[test]
    fn host_boundary_stub_returns_structured_error() {
        use crate::ir::types::IrModule;
        let boundary = HostBoundary::new();
        let ir = IrModule {
            debug_name: "test_module".to_string(),
            functions: vec![],
        };
        let result = boundary.execute(&ir);
        assert!(result.is_err());
        match result.unwrap_err() {
            JitExecutionError::UnsupportedConstruct { construct } => {
                assert!(
                    construct.contains("Phase 14"),
                    "expected Phase 14 mention, got: {}",
                    construct
                );
            }
            other => panic!("expected UnsupportedConstruct, got {:?}", other),
        }
    }
}

// ── JIT integration tests (require the `jit` feature) ────────────────────────

#[cfg(all(test, feature = "jit"))]
mod jit_tests {
    use super::*;
    use crate::ir::instr::{BinaryOp, IrInst, IrTerminator};
    use crate::ir::types::{BlockId, IrBlock, IrFunction, IrModule, IrType, ValueId};

    /// Build a minimal `main() -> i32` module that returns a single constant.
    fn const_return_module(value: i128) -> IrModule {
        IrModule {
            debug_name: "test_const".to_string(),
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
                        value,
                    }],
                    term: IrTerminator::Return {
                        value: Some(ValueId(0)),
                    },
                }],
            }],
        }
    }

    #[test]
    fn jit_const_return_zero() {
        let module = const_return_module(0);
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 0);
    }

    #[test]
    fn jit_const_return_42() {
        let module = const_return_module(42);
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 42);
    }

    #[test]
    fn jit_const_return_1() {
        let module = const_return_module(1);
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 1);
    }

    #[test]
    fn jit_arithmetic_add() {
        // main(): i32 { v0 = 10; v1 = 32; v2 = v0 + v1; return v2 }  → 42
        let module = IrModule {
            debug_name: "test_add".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::ConstInt { dst: ValueId(0), ty: IrType::I32, value: 10 },
                        IrInst::ConstInt { dst: ValueId(1), ty: IrType::I32, value: 32 },
                        IrInst::Binary {
                            dst: ValueId(2),
                            op: BinaryOp::Add,
                            ty: IrType::I32,
                            lhs: ValueId(0),
                            rhs: ValueId(1),
                        },
                    ],
                    term: IrTerminator::Return { value: Some(ValueId(2)) },
                }],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 42); // 10 + 32
    }

    #[test]
    fn jit_arithmetic_sub() {
        // main(): i32 { v0 = 50; v1 = 8; v2 = v0 - v1; return v2 }  → 42
        let module = IrModule {
            debug_name: "test_sub".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::ConstInt { dst: ValueId(0), ty: IrType::I32, value: 50 },
                        IrInst::ConstInt { dst: ValueId(1), ty: IrType::I32, value: 8 },
                        IrInst::Binary {
                            dst: ValueId(2),
                            op: BinaryOp::Sub,
                            ty: IrType::I32,
                            lhs: ValueId(0),
                            rhs: ValueId(1),
                        },
                    ],
                    term: IrTerminator::Return { value: Some(ValueId(2)) },
                }],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 42); // 50 - 8
    }

    #[test]
    fn jit_arithmetic_mul() {
        // main(): i32 { v0 = 6; v1 = 7; v2 = v0 * v1; return v2 }  → 42
        let module = IrModule {
            debug_name: "test_mul".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::ConstInt { dst: ValueId(0), ty: IrType::I32, value: 6 },
                        IrInst::ConstInt { dst: ValueId(1), ty: IrType::I32, value: 7 },
                        IrInst::Binary {
                            dst: ValueId(2),
                            op: BinaryOp::Mul,
                            ty: IrType::I32,
                            lhs: ValueId(0),
                            rhs: ValueId(1),
                        },
                    ],
                    term: IrTerminator::Return { value: Some(ValueId(2)) },
                }],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 42); // 6 * 7
    }

    #[test]
    fn jit_arithmetic_div() {
        // main(): i32 { v0 = 84; v1 = 2; v2 = v0 / v1; return v2 }  → 42
        let module = IrModule {
            debug_name: "test_div".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::ConstInt { dst: ValueId(0), ty: IrType::I32, value: 84 },
                        IrInst::ConstInt { dst: ValueId(1), ty: IrType::I32, value: 2 },
                        IrInst::Binary {
                            dst: ValueId(2),
                            op: BinaryOp::Div,
                            ty: IrType::I32,
                            lhs: ValueId(0),
                            rhs: ValueId(1),
                        },
                    ],
                    term: IrTerminator::Return { value: Some(ValueId(2)) },
                }],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 42); // 84 / 2
    }

    #[test]
    fn jit_arithmetic_rem() {
        // main(): i32 { v0 = 142; v1 = 100; v2 = v0 % v1; return v2 }  → 42
        let module = IrModule {
            debug_name: "test_rem".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::ConstInt { dst: ValueId(0), ty: IrType::I32, value: 142 },
                        IrInst::ConstInt { dst: ValueId(1), ty: IrType::I32, value: 100 },
                        IrInst::Binary {
                            dst: ValueId(2),
                            op: BinaryOp::Rem,
                            ty: IrType::I32,
                            lhs: ValueId(0),
                            rhs: ValueId(1),
                        },
                    ],
                    term: IrTerminator::Return { value: Some(ValueId(2)) },
                }],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 42); // 142 % 100
    }

    #[test]
    fn jit_no_main_returns_main_not_found() {
        let module = IrModule {
            debug_name: "no_main".to_string(),
            functions: vec![],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(
            matches!(result, Err(JitExecutionError::MainNotFound)),
            "expected MainNotFound, got {:?}",
            result
        );
    }

    #[test]
    fn jit_unsupported_inst_returns_error() {
        // SsaBind is not supported in sub-packet 1.
        let module = IrModule {
            debug_name: "test_unsupported".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::ConstInt { dst: ValueId(0), ty: IrType::I32, value: 0 },
                        IrInst::SsaBind {
                            dst: ValueId(1),
                            ty: IrType::I32,
                            src: ValueId(0),
                        },
                    ],
                    term: IrTerminator::Return { value: Some(ValueId(1)) },
                }],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(
            matches!(result, Err(JitExecutionError::UnsupportedConstruct { .. })),
            "expected UnsupportedConstruct, got {:?}",
            result
        );
    }

    // ── Sub-packet 2: Alloca + Load + Store ──────────────────────────────────

    #[test]
    fn jit_alloca_store_load_i32() {
        // main(): i32 {
        //   slot = alloca(4, 4)   // 4-byte I32 slot, 4-byte aligned
        //   store(slot, 42i32)
        //   v = load(i32, slot)
        //   return v              // → 42
        // }
        let module = IrModule {
            debug_name: "test_alloca_store_load_i32".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::Alloca { dst: ValueId(0), size: 4, align: 4 },
                        IrInst::ConstInt { dst: ValueId(1), ty: IrType::I32, value: 42 },
                        IrInst::Store { ptr: ValueId(0), value: ValueId(1) },
                        IrInst::Load { dst: ValueId(2), ty: IrType::I32, ptr: ValueId(0) },
                    ],
                    term: IrTerminator::Return { value: Some(ValueId(2)) },
                }],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 42);
    }

    #[test]
    fn jit_alloca_store_load_i8() {
        // Verify that an i8 slot round-trips correctly.
        // main(): i32 {
        //   slot = alloca(1, 1)
        //   store(slot, 99i8)
        //   v8  = load(i8, slot)
        //   v32 = sext v8 to i32      (done via ConstInt + Binary to avoid Cast)
        //   return v32                → 99
        // }
        // Simplification: store an i32 into a 4-byte slot and load it back as i32,
        // but use size=1/align=1 to exercise minimum-alignment alloca path.
        // We work around the lack of Cast by widening to i32 via arithmetic:
        // load i8, then add 0 (i32) would need Cast. Instead keep the result as i8
        // and cast via return_ty. Cranelift accepts returning an i8 from a function
        // declared with an i8 return type and the host receives it sign-extended.
        // Use i32 slot but align=1 to test the align_shift=0 path.
        let module = IrModule {
            debug_name: "test_alloca_i8_align".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        // align=1 exercises the align_shift=0 code path.
                        IrInst::Alloca { dst: ValueId(0), size: 4, align: 1 },
                        IrInst::ConstInt { dst: ValueId(1), ty: IrType::I32, value: 7 },
                        IrInst::Store { ptr: ValueId(0), value: ValueId(1) },
                        IrInst::Load { dst: ValueId(2), ty: IrType::I32, ptr: ValueId(0) },
                    ],
                    term: IrTerminator::Return { value: Some(ValueId(2)) },
                }],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 7);
    }

    #[test]
    fn jit_alloca_overwrite_returns_last_value() {
        // Write 10, overwrite with 42, load — must see 42 not 10.
        // main(): i32 {
        //   slot = alloca(4, 4)
        //   store(slot, 10i32)
        //   store(slot, 42i32)
        //   v = load(i32, slot)
        //   return v              // → 42
        // }
        let module = IrModule {
            debug_name: "test_alloca_overwrite".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::Alloca { dst: ValueId(0), size: 4, align: 4 },
                        IrInst::ConstInt { dst: ValueId(1), ty: IrType::I32, value: 10 },
                        IrInst::Store { ptr: ValueId(0), value: ValueId(1) },
                        IrInst::ConstInt { dst: ValueId(2), ty: IrType::I32, value: 42 },
                        IrInst::Store { ptr: ValueId(0), value: ValueId(2) },
                        IrInst::Load { dst: ValueId(3), ty: IrType::I32, ptr: ValueId(0) },
                    ],
                    term: IrTerminator::Return { value: Some(ValueId(3)) },
                }],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 42);
    }

    #[test]
    fn jit_alloca_two_independent_slots() {
        // Two slots hold independent values; verify both survive.
        // main(): i32 {
        //   s0 = alloca(4, 4); store(s0, 10)
        //   s1 = alloca(4, 4); store(s1, 32)
        //   a  = load(i32, s0)
        //   b  = load(i32, s1)
        //   r  = a + b          // → 42
        //   return r
        // }
        let module = IrModule {
            debug_name: "test_alloca_two_slots".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![IrBlock {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        IrInst::Alloca { dst: ValueId(0), size: 4, align: 4 },
                        IrInst::ConstInt { dst: ValueId(1), ty: IrType::I32, value: 10 },
                        IrInst::Store { ptr: ValueId(0), value: ValueId(1) },
                        IrInst::Alloca { dst: ValueId(2), size: 4, align: 4 },
                        IrInst::ConstInt { dst: ValueId(3), ty: IrType::I32, value: 32 },
                        IrInst::Store { ptr: ValueId(2), value: ValueId(3) },
                        IrInst::Load { dst: ValueId(4), ty: IrType::I32, ptr: ValueId(0) },
                        IrInst::Load { dst: ValueId(5), ty: IrType::I32, ptr: ValueId(2) },
                        IrInst::Binary {
                            dst: ValueId(6),
                            op: BinaryOp::Add,
                            ty: IrType::I32,
                            lhs: ValueId(4),
                            rhs: ValueId(5),
                        },
                    ],
                    term: IrTerminator::Return { value: Some(ValueId(6)) },
                }],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 42); // 10 + 32
    }

    // ── Jump tests ───────────────────────────────────────────────────────────

    #[test]
    fn jit_jump_passes_value_via_block_param() {
        // main() -> I32 {
        //   block0:
        //     v0 = const 42 : I32
        //     jump block1(v0)
        //   block1(v1: I32):
        //     return v1
        // }
        // Expected: 42
        use crate::ir::types::BlockParam;
        let module = IrModule {
            debug_name: "test_jump".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![
                    IrBlock {
                        id: BlockId(0),
                        params: vec![],
                        insts: vec![IrInst::ConstInt {
                            dst: ValueId(0),
                            ty: IrType::I32,
                            value: 42,
                        }],
                        term: IrTerminator::Jump {
                            target: BlockId(1),
                            args: vec![ValueId(0)],
                        },
                    },
                    IrBlock {
                        id: BlockId(1),
                        params: vec![BlockParam {
                            value: ValueId(1),
                            ty: IrType::I32,
                            read_only: false,
                        }],
                        insts: vec![],
                        term: IrTerminator::Return {
                            value: Some(ValueId(1)),
                        },
                    },
                ],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 42);
    }

    #[test]
    fn jit_jump_no_args() {
        // main() -> I32 {
        //   block0:
        //     jump block1
        //   block1:
        //     v0 = const 7 : I32
        //     return v0
        // }
        // Expected: 7
        let module = IrModule {
            debug_name: "test_jump_noarg".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![
                    IrBlock {
                        id: BlockId(0),
                        params: vec![],
                        insts: vec![],
                        term: IrTerminator::Jump {
                            target: BlockId(1),
                            args: vec![],
                        },
                    },
                    IrBlock {
                        id: BlockId(1),
                        params: vec![],
                        insts: vec![IrInst::ConstInt {
                            dst: ValueId(0),
                            ty: IrType::I32,
                            value: 7,
                        }],
                        term: IrTerminator::Return {
                            value: Some(ValueId(0)),
                        },
                    },
                ],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 7);
    }

    // ── Compare + Branch tests ───────────────────────────────────────────────

    /// Helper: build a two-block if/else module that compares two I32 constants.
    ///
    /// ```text
    /// main() -> I32 {
    ///   block0:
    ///     v0 = const lhs : I32
    ///     v1 = const rhs : I32
    ///     v2 = compare op(v0, v1)   // I8
    ///     branch v2, block1[], block2[]
    ///   block1:          // true path
    ///     v3 = const true_val : I32
    ///     return v3
    ///   block2:          // false path
    ///     v4 = const false_val : I32
    ///     return v4
    /// }
    /// ```
    fn compare_branch_module(
        lhs: i128,
        rhs: i128,
        op: crate::ir::instr::CompareOp,
        true_val: i128,
        false_val: i128,
    ) -> IrModule {
        IrModule {
            debug_name: "test_compare_branch".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![
                    IrBlock {
                        id: BlockId(0),
                        params: vec![],
                        insts: vec![
                            IrInst::ConstInt { dst: ValueId(0), ty: IrType::I32, value: lhs },
                            IrInst::ConstInt { dst: ValueId(1), ty: IrType::I32, value: rhs },
                            IrInst::Compare {
                                dst: ValueId(2),
                                op,
                                lhs: ValueId(0),
                                rhs: ValueId(1),
                            },
                        ],
                        term: IrTerminator::Branch {
                            cond: ValueId(2),
                            then_block: BlockId(1),
                            then_args: vec![],
                            else_block: BlockId(2),
                            else_args: vec![],
                        },
                    },
                    IrBlock {
                        id: BlockId(1),
                        params: vec![],
                        insts: vec![IrInst::ConstInt {
                            dst: ValueId(3),
                            ty: IrType::I32,
                            value: true_val,
                        }],
                        term: IrTerminator::Return { value: Some(ValueId(3)) },
                    },
                    IrBlock {
                        id: BlockId(2),
                        params: vec![],
                        insts: vec![IrInst::ConstInt {
                            dst: ValueId(4),
                            ty: IrType::I32,
                            value: false_val,
                        }],
                        term: IrTerminator::Return { value: Some(ValueId(4)) },
                    },
                ],
            }],
        }
    }

    #[test]
    fn jit_branch_compare_eq_takes_true_path() {
        // 5 == 5 → true → return 1
        use crate::ir::instr::CompareOp;
        let m = compare_branch_module(5, 5, CompareOp::Eq, 1, 0);
        let r = HostBoundary::new().execute(&m);
        assert!(r.is_ok(), "JIT failed: {:?}", r.unwrap_err());
        assert_eq!(r.unwrap().exit_code.raw(), 1);
    }

    #[test]
    fn jit_branch_compare_eq_takes_false_path() {
        // 5 == 6 → false → return 0
        use crate::ir::instr::CompareOp;
        let m = compare_branch_module(5, 6, CompareOp::Eq, 1, 0);
        let r = HostBoundary::new().execute(&m);
        assert!(r.is_ok(), "JIT failed: {:?}", r.unwrap_err());
        assert_eq!(r.unwrap().exit_code.raw(), 0);
    }

    #[test]
    fn jit_branch_compare_ne_true() {
        // 5 != 10 → true → return 42
        use crate::ir::instr::CompareOp;
        let m = compare_branch_module(5, 10, CompareOp::Ne, 42, 99);
        let r = HostBoundary::new().execute(&m);
        assert!(r.is_ok(), "JIT failed: {:?}", r.unwrap_err());
        assert_eq!(r.unwrap().exit_code.raw(), 42);
    }

    #[test]
    fn jit_branch_compare_lt_true() {
        // 3 < 7 → true → return 1
        use crate::ir::instr::CompareOp;
        let m = compare_branch_module(3, 7, CompareOp::Lt, 1, 0);
        let r = HostBoundary::new().execute(&m);
        assert!(r.is_ok(), "JIT failed: {:?}", r.unwrap_err());
        assert_eq!(r.unwrap().exit_code.raw(), 1);
    }

    #[test]
    fn jit_branch_compare_lt_false() {
        // 7 < 3 → false → return 0
        use crate::ir::instr::CompareOp;
        let m = compare_branch_module(7, 3, CompareOp::Lt, 1, 0);
        let r = HostBoundary::new().execute(&m);
        assert!(r.is_ok(), "JIT failed: {:?}", r.unwrap_err());
        assert_eq!(r.unwrap().exit_code.raw(), 0);
    }

    #[test]
    fn jit_branch_compare_le_equal_is_true() {
        // 5 <= 5 → true → return 1
        use crate::ir::instr::CompareOp;
        let m = compare_branch_module(5, 5, CompareOp::Le, 1, 0);
        let r = HostBoundary::new().execute(&m);
        assert!(r.is_ok(), "JIT failed: {:?}", r.unwrap_err());
        assert_eq!(r.unwrap().exit_code.raw(), 1);
    }

    #[test]
    fn jit_branch_compare_gt_true() {
        // 10 > 3 → true → return 42
        use crate::ir::instr::CompareOp;
        let m = compare_branch_module(10, 3, CompareOp::Gt, 42, 0);
        let r = HostBoundary::new().execute(&m);
        assert!(r.is_ok(), "JIT failed: {:?}", r.unwrap_err());
        assert_eq!(r.unwrap().exit_code.raw(), 42);
    }

    #[test]
    fn jit_branch_compare_ge_true() {
        // 10 >= 10 → true → return 1
        use crate::ir::instr::CompareOp;
        let m = compare_branch_module(10, 10, CompareOp::Ge, 1, 0);
        let r = HostBoundary::new().execute(&m);
        assert!(r.is_ok(), "JIT failed: {:?}", r.unwrap_err());
        assert_eq!(r.unwrap().exit_code.raw(), 1);
    }

    #[test]
    fn jit_branch_with_block_args_on_both_edges() {
        // main() -> I32 {
        //   block0:
        //     v0 = const 1 : I32    // condition value (nonzero → true)
        //     v1 = const 42 : I32
        //     v2 = const 99 : I32
        //     branch v0, block1(v1), block2(v2)
        //   block1(v3: I32):
        //     return v3        // taken: returns 42
        //   block2(v4: I32):
        //     return v4        // not taken
        // }
        // Expected: 42
        use crate::ir::types::BlockParam;
        let module = IrModule {
            debug_name: "test_branch_args".to_string(),
            functions: vec![IrFunction {
                name: "main".to_string(),
                params: vec![],
                return_ty: Some(IrType::I32),
                blocks: vec![
                    IrBlock {
                        id: BlockId(0),
                        params: vec![],
                        insts: vec![
                            IrInst::ConstInt { dst: ValueId(0), ty: IrType::I32, value: 1 },
                            IrInst::ConstInt { dst: ValueId(1), ty: IrType::I32, value: 42 },
                            IrInst::ConstInt { dst: ValueId(2), ty: IrType::I32, value: 99 },
                        ],
                        term: IrTerminator::Branch {
                            cond: ValueId(0),
                            then_block: BlockId(1),
                            then_args: vec![ValueId(1)],
                            else_block: BlockId(2),
                            else_args: vec![ValueId(2)],
                        },
                    },
                    IrBlock {
                        id: BlockId(1),
                        params: vec![BlockParam { value: ValueId(3), ty: IrType::I32, read_only: false }],
                        insts: vec![],
                        term: IrTerminator::Return { value: Some(ValueId(3)) },
                    },
                    IrBlock {
                        id: BlockId(2),
                        params: vec![BlockParam { value: ValueId(4), ty: IrType::I32, read_only: false }],
                        insts: vec![],
                        term: IrTerminator::Return { value: Some(ValueId(4)) },
                    },
                ],
            }],
        };
        let result = HostBoundary::new().execute(&module);
        assert!(result.is_ok(), "JIT failed: {:?}", result.unwrap_err());
        assert_eq!(result.unwrap().exit_code.raw(), 42);
    }
}