use crate::frontend::ast::Program;
use crate::backend::Backend;

pub mod aot;

pub struct LlvmBackend;

impl Backend for LlvmBackend {
    fn execute(&self, _program: &Program) -> Result<(), String> {
        Err("LLVM backend not implemented yet; use --backend=interp".to_string())
    }
}
