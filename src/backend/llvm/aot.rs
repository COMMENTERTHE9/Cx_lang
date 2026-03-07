#![allow(dead_code)]

pub fn emit_object(_ir: &crate::ir::IrModule) -> Result<(), String> {
    // TODO: Implement LLVM AOT object emission from lowered IR.
    Err("LLVM AOT backend not implemented yet".into())
}
