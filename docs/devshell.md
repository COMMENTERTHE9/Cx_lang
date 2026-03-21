# Dev Shell (MSVC Setup)

`scripts/devshell.ps1` loads the Visual Studio C++ build environment into your current PowerShell session.
It makes `cl`, `link`, and related MSVC tools available so Rust/Cargo builds are consistent.

## How to use

From the repo root, run:

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\devshell.ps1
```

Then in the same terminal, run your build commands:

```powershell
cargo build
```
