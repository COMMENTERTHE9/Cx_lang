use crate::backend::{Backend, BackendError};
use crate::ir::IrModule;

pub mod aot;

pub struct LlvmBackend;

impl Backend for LlvmBackend {
    fn execute(&self, _module: &IrModule) -> Result<(), BackendError> {
        Err(BackendError {
            message: "LLVM backend not implemented yet; use --backend=interp".to_string(),
            exit_code: 1,
        })
    }
}
