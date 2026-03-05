use crate::frontend::ast::Program;
use crate::backend::Backend;

pub mod jit;
pub mod aot;

pub struct CraneliftBackend;

impl Backend for CraneliftBackend {
    fn execute(&self, _program: &Program) -> Result<(), String> {
        Err("Cranelift backend not implemented yet; use --backend=interp".to_string())
    }
}
