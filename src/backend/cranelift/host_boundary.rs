//! JIT Runtime Host Boundary
//!
//! This module defines the execution boundary between the Cx JIT backend and the host process.
//! It is a scaffold: the types and contract are defined here; Cranelift compilation is wired in
//! Phase 14 (First Executable Cranelift Slice).
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
//! In-process capture (e.g. redirecting stdout via `dup2` before calling into JIT code) is a
//! post-scaffold enhancement. It is not required for the differential harness to work.
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
/// In the current scaffold `stdout` and `stderr` are always empty strings.
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
/// - Will hold a Cranelift `JITModule` in Phase 14 (First Executable Cranelift Slice).
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
    ///
    /// In the current scaffold this always returns
    /// `Err(JitExecutionError::UnsupportedConstruct)` indicating Phase 14 is pending.
    /// Phase 14 will replace this stub with real Cranelift JIT compilation.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::types::IrModule;

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

    #[test]
    fn host_boundary_stub_returns_structured_error() {
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
