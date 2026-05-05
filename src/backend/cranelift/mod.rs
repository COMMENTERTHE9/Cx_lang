use crate::backend::Backend;
use crate::ir::IrModule;

pub mod aot;
pub mod host_boundary;
pub mod jit;

pub struct CraneliftBackend;

impl Backend for CraneliftBackend {
    fn execute(&self, module: &IrModule) -> Result<(), String> {
        #[cfg(feature = "jit")]
        {
            use jit::run_jit;
            match run_jit(module) {
                Ok(outcome) => {
                    if outcome.exit_code.is_success() {
                        Ok(())
                    } else {
                        Err(format!(
                            "JIT: program exited with code {}",
                            outcome.exit_code
                        ))
                    }
                }
                Err(e) => Err(e.to_string()),
            }
        }
        #[cfg(not(feature = "jit"))]
        {
            let _ = module;
            Err("Cranelift backend requires the `jit` feature — rebuild with --features jit".to_string())
        }
    }
}
