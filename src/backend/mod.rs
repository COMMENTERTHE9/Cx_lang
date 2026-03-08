#![allow(dead_code)]

use crate::frontend::ast::Program;

pub mod cranelift;
pub mod llvm;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    Interpret,
    Cranelift,
    Llvm,
}

pub trait Backend {
    fn execute(&self, program: &Program) -> Result<(), String>;
}

pub fn parse_backend_flag(args: &[String]) -> BackendKind {
    for arg in args {
        if let Some(raw) = arg.strip_prefix("--backend=") {
            return match raw {
                "interp" => BackendKind::Interpret,
                "cranelift" => BackendKind::Cranelift,
                "llvm" => BackendKind::Llvm,
                _ => BackendKind::Interpret,
            };
        }
    }
    BackendKind::Interpret
}

pub fn lower_to_ir(
    program: &crate::frontend::semantic_types::SemanticProgram,
) -> crate::ir::IrModule {
    crate::ir::lower::lower_program(program)
}
