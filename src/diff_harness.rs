//! CX-23 / CX-31 — Differential harness: interpreter baseline and JIT comparison.
//!
//! Phase 12, sub-packet 1 (interpreter baseline) + CX-31 (JIT differential).
//!
//! This module defines the data types, fixture format, and collection logic for
//! running every matrix test through the interpreter and comparing output against
//! stored expectations.  CX-31 extends this with a JIT execution path that runs
//! the same binary with `--backend=cranelift` and compares the process exit code
//! against the interpreter baseline for the arithmetic subset.
//!
//! # Fixture format
//!
//! Each test lives in `src/tests/verification_matrix/` as a triple:
//!
//! ```text
//! <name>.cx                  — Cx source program
//! <name>.cx.expected_output  — expected stdout (present only for output-verified pass tests)
//! <name>.cx.expected_fail    — zero-byte marker (present only for expected-failure tests)
//! ```
//!
//! A `.cx` file with neither companion file is a "pass-any" test: the interpreter
//! must exit 0, but its stdout is not verified.
//!
//! # Arithmetic subset (JIT differential fixtures)
//!
//! Fixtures whose names begin with `jit_arith_` are dedicated to the JIT
//! differential gate.  They contain only top-level arithmetic expression
//! statements with no function calls or I/O, so the JIT can compile and execute
//! them end-to-end.  The interpreter processes them in the same way (no output,
//! exit 0), making the comparison unambiguous: both backends must exit 0.
//!
//! # Comparison semantics
//!
//! Stored expected-output files may use CRLF or LF line endings (the files were
//! created on Windows and may have CRLF). The interpreter subprocess also produces
//! CRLF on Windows. Both sides are normalised to LF and right-trimmed before
//! comparison — matching the behaviour of the bash `$()` command substitution used
//! in `run_matrix.sh`.
//!
//! # Sub-packet deliverables
//!
//! - `TestExpectation` — what a fixture expects from the interpreter
//! - `TestFixture` — one matrix test entry
//! - `InterpOutcome` — result of a single interpreter run
//! - `collect_matrix_tests()` — enumerate all fixtures from the matrix directory
//! - `run_interpreter()` — capture one interpreter run via subprocess
//! - `run_jit_subprocess()` — capture one JIT run via subprocess (`--backend=cranelift`)
//! - `cx_binary_path()` — locate the compiled Cx binary
//! - `#[test] interpreter_baseline_all` — interpreter baseline gate
//! - `#[test] jit_differential_arithmetic` — JIT vs interpreter gate for arithmetic subset

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// ── Fixture types ─────────────────────────────────────────────────────────────

/// What the interpreter is expected to do when given this fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestExpectation {
    /// Test must exit 0 and produce stdout that matches the stored string
    /// exactly (after CRLF normalisation and trailing-whitespace trim).
    PassWithOutput(String),

    /// Test must exit 0. Stdout is not checked.
    PassAny,

    /// Test must exit non-zero (`.expected_fail` marker present).
    Fail,
}

/// One entry in the verification matrix.
#[derive(Debug, Clone)]
pub struct TestFixture {
    /// Short name derived from the filename stem, e.g. `"t01_arith_eq_mod"`.
    pub name: String,

    /// Absolute path to the `.cx` source file.
    pub path: PathBuf,

    /// What the interpreter is expected to produce for this fixture.
    pub expectation: TestExpectation,
}

// ── Interpreter run result ────────────────────────────────────────────────────

/// Result of running the interpreter on a single fixture.
#[derive(Debug, Clone)]
pub struct InterpOutcome {
    /// Captured stdout, as raw bytes decoded to UTF-8 (lossy).
    pub stdout: String,

    /// Captured stderr, as raw bytes decoded to UTF-8 (lossy).
    pub stderr: String,

    /// Process exit code. 0 means success. -1 means the OS gave no code.
    pub exit_code: i32,
}

impl InterpOutcome {
    /// Returns `true` if the process exited with code 0.
    pub fn passed(&self) -> bool {
        self.exit_code == 0
    }
}

// ── Collection ────────────────────────────────────────────────────────────────

/// Normalise line endings to LF and trim trailing whitespace.
///
/// This mirrors the bash `$()` command substitution which strips trailing
/// newlines and works correctly regardless of whether the source used CRLF or LF.
fn normalise(s: &str) -> String {
    s.replace("\r\n", "\n").replace('\r', "\n").trim_end().to_string()
}

/// Enumerate all `.cx` fixtures in the verification matrix directory.
///
/// Returns fixtures sorted by filename so that the order is deterministic
/// across runs and platforms.
///
/// # Panics
///
/// Panics if the `src/tests/verification_matrix/` directory cannot be read.
pub fn collect_matrix_tests() -> Vec<TestFixture> {
    let matrix_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/verification_matrix");

    let mut paths: Vec<PathBuf> = fs::read_dir(&matrix_dir)
        .expect("src/tests/verification_matrix/ must exist and be readable")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let s = name.to_string_lossy();
            // Accept only plain .cx files — exclude .expected_output / .expected_fail.
            if s.ends_with(".cx")
                && !s.ends_with(".expected_output")
                && !s.ends_with(".expected_fail")
            {
                Some(entry.path())
            } else {
                None
            }
        })
        .collect();

    paths.sort();

    paths
        .into_iter()
        .map(|path| {
            let name = path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();

            let path_str = path.to_string_lossy();
            let expected_output_path = PathBuf::from(format!("{}.expected_output", path_str));
            let expected_fail_path = PathBuf::from(format!("{}.expected_fail", path_str));

            let expectation = if expected_fail_path.exists() {
                TestExpectation::Fail
            } else if expected_output_path.exists() {
                let raw = fs::read_to_string(&expected_output_path)
                    .expect("failed to read .expected_output file");
                TestExpectation::PassWithOutput(normalise(&raw))
            } else {
                TestExpectation::PassAny
            };

            TestFixture { name, path, expectation }
        })
        .collect()
}

// ── Subprocess runner ─────────────────────────────────────────────────────────

/// Run the interpreter on `fixture` and return the captured outcome.
///
/// `binary` must point to the compiled `Cx_0V` executable.
///
/// # Panics
///
/// Panics if the subprocess cannot be spawned (e.g. binary path is wrong
/// or the OS refuses to exec). This is a hard failure — the harness cannot
/// proceed without a working interpreter binary.
pub fn run_interpreter(binary: &Path, fixture: &TestFixture) -> InterpOutcome {
    let output = Command::new(binary)
        .arg(&fixture.path)
        // Disable colour output so stderr is plain text.
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "failed to spawn interpreter binary {:?} for fixture {:?}: {}",
                binary, fixture.path, e
            )
        });

    InterpOutcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    }
}

/// Run the Cranelift JIT backend on `fixture` and return the captured outcome.
///
/// Invokes the compiled `Cx_0V` binary with `--backend=cranelift <fixture_path>`.
/// The binary prints the lowered IR module to stdout (a debugging aid) and then
/// executes it through the JIT.  On success the process exits 0; on any JIT or
/// lowering failure it exits 1 (see `main.rs`).
///
/// Because the IR text is printed to stdout unconditionally, callers should
/// **not** compare stdout when using JIT fixtures — compare exit codes only.
///
/// # Panics
///
/// Panics if the subprocess cannot be spawned.
pub fn run_jit_subprocess(binary: &Path, fixture: &TestFixture) -> InterpOutcome {
    let output = Command::new(binary)
        .arg("--backend=cranelift")
        .arg(&fixture.path)
        .env("NO_COLOR", "1")
        .output()
        .unwrap_or_else(|e| {
            panic!(
                "failed to spawn JIT binary {:?} for fixture {:?}: {}",
                binary, fixture.path, e
            )
        });

    InterpOutcome {
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        exit_code: output.status.code().unwrap_or(-1),
    }
}

// ── Binary location ───────────────────────────────────────────────────────────

/// Return the path to the compiled `Cx_0V` binary.
///
/// Resolution order:
/// 1. `CARGO_BIN_EXE_Cx_0V` environment variable (set by cargo for integration
///    tests — not available for inline `#[test]` functions).
/// 2. `<manifest_dir>/target/debug/Cx_0V[.exe]` — the default debug build
///    produced by `cargo build --features jit`.
pub fn cx_binary_path() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_Cx_0V") {
        return PathBuf::from(p);
    }

    let exe = if cfg!(windows) { "Cx_0V.exe" } else { "Cx_0V" };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join("debug")
        .join(exe)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Fixture collection ────────────────────────────────────────────────────

    /// Enumeration must return at least one fixture and every fixture must have
    /// a `.cx`-extension path.
    #[test]
    fn collects_matrix_tests_non_empty() {
        let fixtures = collect_matrix_tests();
        assert!(
            !fixtures.is_empty(),
            "collect_matrix_tests() returned no fixtures — verification_matrix must not be empty"
        );
        for f in &fixtures {
            assert_eq!(
                f.path.extension().and_then(|e| e.to_str()),
                Some("cx"),
                "fixture path must end in .cx: {:?}",
                f.path
            );
        }
    }

    /// The fixture set must contain both expected-pass and expected-fail entries,
    /// and the totals must be internally consistent.
    #[test]
    fn fixture_expectations_cover_pass_and_fail() {
        let fixtures = collect_matrix_tests();
        let total = fixtures.len();

        let fail_count = fixtures
            .iter()
            .filter(|f| f.expectation == TestExpectation::Fail)
            .count();
        let pass_output_count = fixtures
            .iter()
            .filter(|f| matches!(f.expectation, TestExpectation::PassWithOutput(_)))
            .count();
        let pass_any_count = fixtures
            .iter()
            .filter(|f| f.expectation == TestExpectation::PassAny)
            .count();

        assert!(fail_count > 0, "matrix must have at least one expected-fail test");
        assert!(
            pass_output_count + pass_any_count > 0,
            "matrix must have at least one passing test"
        );
        assert_eq!(
            total,
            fail_count + pass_output_count + pass_any_count,
            "fixture counts must be exhaustive"
        );
    }

    /// Every PassWithOutput expectation must be a non-empty normalised string
    /// (the expected output file had content).
    #[test]
    fn pass_with_output_expectations_are_non_empty() {
        let fixtures = collect_matrix_tests();
        for f in &fixtures {
            if let TestExpectation::PassWithOutput(ref expected) = f.expectation {
                assert!(
                    !expected.is_empty(),
                    "PassWithOutput expectation must not be empty for fixture: {}",
                    f.name
                );
            }
        }
    }

    // ── Interpreter baseline ──────────────────────────────────────────────────

    /// Interpreter baseline gate.
    ///
    /// Runs every matrix fixture through the interpreter subprocess and checks
    /// that each outcome matches its stored expectation:
    ///
    /// - `Fail`              → interpreter must exit non-zero
    /// - `PassAny`           → interpreter must exit 0
    /// - `PassWithOutput(s)` → interpreter must exit 0 and stdout (normalised)
    ///                         must equal `s`
    ///
    /// Requires the `Cx_0V` binary to be present at `target/debug/Cx_0V[.exe]`.
    /// If the binary is absent the test is skipped with a diagnostic message.
    ///
    /// Run with:
    ///
    /// ```text
    /// cargo build --features jit && cargo test --features jit
    /// ```
    #[test]
    fn interpreter_baseline_all() {
        let binary = cx_binary_path();

        if !binary.exists() {
            eprintln!(
                "SKIP interpreter_baseline_all — binary not found at {:?}.\n\
                 Build with `cargo build --features jit` then re-run tests.",
                binary
            );
            return;
        }

        let fixtures = collect_matrix_tests();
        let mut failures: Vec<String> = Vec::new();

        for fixture in &fixtures {
            let outcome = run_interpreter(&binary, fixture);

            match &fixture.expectation {
                TestExpectation::Fail => {
                    if outcome.passed() {
                        failures.push(format!(
                            "FAIL [should-fail but exited 0]: {}",
                            fixture.name
                        ));
                    }
                }

                TestExpectation::PassAny => {
                    if !outcome.passed() {
                        failures.push(format!(
                            "FAIL [expected-pass, exit {}]: {}\n  stderr: {}",
                            outcome.exit_code,
                            fixture.name,
                            outcome.stderr.lines().next().unwrap_or("(no stderr)")
                        ));
                    }
                }

                TestExpectation::PassWithOutput(expected) => {
                    if !outcome.passed() {
                        failures.push(format!(
                            "FAIL [expected-pass, exit {}]: {}\n  stderr: {}",
                            outcome.exit_code,
                            fixture.name,
                            outcome.stderr.lines().next().unwrap_or("(no stderr)")
                        ));
                    } else {
                        let actual = normalise(&outcome.stdout);
                        if actual != *expected {
                            failures.push(format!(
                                "FAIL [output mismatch]: {}\n  expected: {:?}\n  got:      {:?}",
                                fixture.name, expected, actual
                            ));
                        }
                    }
                }
            }
        }

        if !failures.is_empty() {
            panic!(
                "\n{} interpreter baseline failure(s) out of {} total:\n\n{}\n",
                failures.len(),
                fixtures.len(),
                failures.join("\n\n")
            );
        }

        eprintln!(
            "interpreter_baseline_all: {}/{} fixtures passed",
            fixtures.len(),
            fixtures.len()
        );
    }

    // ── JIT differential gate ─────────────────────────────────────────────────

    /// JIT differential gate for the arithmetic subset.
    ///
    /// Collects all fixtures whose names begin with `jit_arith_` and runs each
    /// through **both** the interpreter and the Cranelift JIT backend (via
    /// `--backend=cranelift`).  Both must exit 0 for the test to pass.
    ///
    /// The arithmetic-subset fixtures contain only top-level expression statements
    /// (no function calls, no I/O), so both the interpreter and JIT can execute
    /// them without producing output — making the comparison unambiguous.
    ///
    /// The test is skipped (with a diagnostic message) if the `Cx_0V` binary is
    /// not present.  Build it first:
    ///
    /// ```text
    /// cargo build --features jit && cargo test --features jit
    /// ```
    ///
    /// If the JIT exits non-zero for a fixture, the test fails and reports
    /// the fixture name and any stderr output from the JIT process.
    #[test]
    fn jit_differential_arithmetic() {
        let binary = cx_binary_path();

        if !binary.exists() {
            eprintln!(
                "SKIP jit_differential_arithmetic — binary not found at {:?}.\n\
                 Build with `cargo build --features jit` then re-run tests.",
                binary
            );
            return;
        }

        // Only exercise the dedicated arithmetic-subset fixtures.
        let fixtures: Vec<TestFixture> = collect_matrix_tests()
            .into_iter()
            .filter(|f| f.name.starts_with("jit_arith_"))
            .collect();

        if fixtures.is_empty() {
            panic!(
                "jit_differential_arithmetic: no jit_arith_* fixtures found in \
                 verification_matrix/ — add at least one jit_arith_*.cx fixture"
            );
        }

        let mut failures: Vec<String> = Vec::new();

        for fixture in &fixtures {
            // Step 1: interpreter must accept the program.
            let interp = run_interpreter(&binary, fixture);
            if !interp.passed() {
                failures.push(format!(
                    "FAIL [interpreter rejected fixture, exit {}]: {}\n  stderr: {}",
                    interp.exit_code,
                    fixture.name,
                    interp.stderr.lines().next().unwrap_or("(no stderr)")
                ));
                // Don't bother running JIT if interpreter already failed.
                continue;
            }

            // Step 2: JIT must also accept the program (exit 0).
            let jit = run_jit_subprocess(&binary, fixture);
            if !jit.passed() {
                failures.push(format!(
                    "FAIL [JIT rejected fixture, exit {}]: {}\n  stderr: {}",
                    jit.exit_code,
                    fixture.name,
                    jit.stderr.lines().next().unwrap_or("(no stderr)")
                ));
            }
        }

        if !failures.is_empty() {
            panic!(
                "\n{} JIT differential failure(s) out of {} arithmetic fixture(s):\n\n{}\n",
                failures.len(),
                fixtures.len(),
                failures.join("\n\n")
            );
        }

        eprintln!(
            "jit_differential_arithmetic: {}/{} fixtures passed (interpreter + JIT)",
            fixtures.len(),
            fixtures.len()
        );
    }
}
