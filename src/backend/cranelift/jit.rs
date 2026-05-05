#![allow(dead_code)]
#![cfg(feature = "jit")]

pub use super::host_boundary::{HostBoundary, JitExecutionError, JitOutcome};

/// Run the Cx IR module through the Cranelift JIT backend.
///
/// Delegates to [`HostBoundary::execute`], which owns the JIT lifecycle.
/// See [`host_boundary`](super::host_boundary) for the full execution contract.
pub fn run_jit(ir: &crate::ir::IrModule) -> Result<JitOutcome, JitExecutionError> {
    HostBoundary::new().execute(ir)
}
