#![allow(dead_code)]
#![cfg(feature = "jit")]

pub fn emit_object(_ir: &crate::ir::IrModule) -> Result<(), String> {
    // TODO: Implement Cranelift AOT object emission from lowered IR.
    Err("Cranelift AOT backend not implemented yet".into())
}
