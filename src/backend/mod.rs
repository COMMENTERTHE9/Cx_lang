#![allow(dead_code)]

pub mod cranelift;
pub mod llvm;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Interpret,
    Cranelift,
    Llvm,
    Validate,
}

/// Error returned by [`Backend::execute`].
///
/// Carries both a human-readable message and the process exit code the CLI
/// should propagate.  The caller (main.rs) is responsible for calling
/// `std::process::exit(exit_code)` — backends never call exit directly.
///
/// | Scenario                                                   | exit_code |
/// |------------------------------------------------------------|-----------|
/// | Cx program ran and returned N                              | N         |
/// | JIT can't handle it (unsupported construct, codegen gap,   |           |
/// |   missing main, or IR lowering failure)                    | 127       |
/// | JIT runtime panic / trap                                   | 126       |
/// | LLVM or other non-JIT backend failure                      | 1         |
#[derive(Debug, Clone)]
pub struct BackendError {
    pub message: String,
    pub exit_code: i32,
}

pub trait Backend {
    fn execute(&self, module: &crate::ir::IrModule) -> Result<(), BackendError>;
}

pub fn parse_backend_flag(args: &[String]) -> BackendKind {
    for arg in args {
        if let Some(raw) = arg.strip_prefix("--backend=") {
            return match raw {
                "interp" => BackendKind::Interpret,
                "cranelift" => BackendKind::Cranelift,
                "llvm" => BackendKind::Llvm,
                "validate" => BackendKind::Validate,
                _ => BackendKind::Interpret,
            };
        }
    }
    BackendKind::Interpret
}

pub fn lower_to_ir(
    program: &crate::frontend::semantic_types::SemanticProgram,
) -> Result<crate::ir::IrModule, crate::ir::lower::LoweringError> {
    crate::ir::lower::lower_program(program)
}
