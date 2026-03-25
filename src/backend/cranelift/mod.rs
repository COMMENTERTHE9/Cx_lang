use crate::backend::Backend;
use crate::ir::IrModule;

pub mod aot;
pub mod jit;

pub struct CraneliftBackend;

impl Backend for CraneliftBackend {
    fn execute(&self, _module: &IrModule) -> Result<(), String> {
        Err("Cranelift backend not implemented yet; use --backend=interp".to_string())
    }
}
