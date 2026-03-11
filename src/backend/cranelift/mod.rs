use crate::backend::Backend;
use crate::frontend::ast::Program;

pub mod aot;
pub mod jit;

pub struct CraneliftBackend;

impl Backend for CraneliftBackend {
    fn execute(&self, _program: &Program) -> Result<(), String> {
        Err("Cranelift backend not implemented yet; use --backend=interp".to_string())
    }
}
