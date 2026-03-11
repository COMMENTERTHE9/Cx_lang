use crate::backend::Backend;
use crate::frontend::ast::Program;

pub mod aot;

pub struct LlvmBackend;

impl Backend for LlvmBackend {
    fn execute(&self, _program: &Program) -> Result<(), String> {
        Err("LLVM backend not implemented yet; use --backend=interp".to_string())
    }
}
