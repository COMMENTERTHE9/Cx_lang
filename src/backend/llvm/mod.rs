use crate::backend::Backend;
use crate::ir::IrModule;

pub mod aot;

pub struct LlvmBackend;

impl Backend for LlvmBackend {
    fn execute(&self, _module: &IrModule) -> Result<(), String> {
        Err("LLVM backend not implemented yet; use --backend=interp".to_string())
    }
}
