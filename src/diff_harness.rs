//! CX-23 / CX-51 — Differential harness: interpreter baseline and JIT parity matrix.
//!
//! Phase 12, sub-packets 1 (CX-23) and 4 (CX-51).
//!
//! This module is the differential testing harness. It defines the data types,
//! fixture format, and collection logic for running every matrix test through both
//! the interpreter and the Cranelift JIT backend, then comparing outcomes.
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
//! A `.cx` file with neither companion file is a "pass-any" test: the backend
//! must exit 0, but its stdout is not verified.
//!
//! # Comparison semantics
//!
//! Stored expected-output files may use CRLF or LF line endings (the files were
//! created on Windows and may have CRLF). The interpreter subprocess also produces
//! CRLF on Windows. Both sides are normalised to LF and right-trimmed before
//! comparison — matching the behaviour of the bash `$()` command substitution used
//! in `run_matrix.sh`.
//!
//! # JIT parity baseline (Phase 12 sub-packet 4)
//!
//! The JIT backend (Cranelift, `--backend=cranelift`) is invoked as a subprocess
//! for each matrix fixture, exactly as the interpreter is. JIT outcomes are
//! classified as:
//!
//! - **PASS** — JIT exit code and stdout match the fixture expectation.
//! - **SKIP** — JIT exited 127 (`UNSUPPORTED_CONSTRUCT`): the fixture exercises a
//!   construct not yet implemented by the JIT (Phase 14 sub-packets 1–3 cover
//!   constants, integer arithmetic, memory, and forward-only control flow). These
//!   are not counted as failures; they define the current JIT coverage frontier.
//! - **PARITY_FAIL** — JIT disagrees with the interpreter in a way that cannot be
//!   explained by an unsupported construct:
//!   - JIT exits 0 on a `Fail` fixture (JIT incorrectly accepts a failing program).
//!   - JIT exits non-zero (and not 127) on a `Pass` fixture.
//!   - JIT exits 0 on a `PassWithOutput` fixture but stdout does not match.
//!
//! The `jit_differential_all` test gate fails only on `PARITY_FAIL` results. A run
//! where all fixtures are either PASS or SKIP is considered green. The eprintln!
//! summary at the end of the test records the exact counts and constitutes the
//! documented parity baseline for this phase.
//!
//! # JIT exit code sentinels
//!
//! | Code | Meaning                                                   |
//! |------|-----------------------------------------------------------|
//! | 0    | Success — program ran to completion                       |
//! | 1–125| Program-level non-zero return                             |
//! | 126  | `JIT_RUNTIME_FAILURE` — JIT internal error at runtime     |
//! | 127  | `UNSUPPORTED_CONSTRUCT` — codegen skipped (SKIP category) |
//!
//! # Sub-packet deliverables
//!
//! - `TestExpectation` — what a fixture expects from the backend
//! - `TestFixture` — one matrix test entry
//! - `InterpOutcome` — result of a single backend run (reused for both paths)
//! - `collect_matrix_tests()` — enumerate all fixtures from the matrix directory
//! - `run_interpreter()` — capture one interpreter run via subprocess
//! - `run_jit()` — capture one JIT run via subprocess (`--backend=cranelift`)
//! - `cx_binary_path()` — locate the compiled Cx binary
//! - `#[test] interpreter_baseline_all` — interpreter baseline gate
//! - `#[test] jit_differential_all` — JIT parity gate

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

// ── JIT sentinel exit codes ───────────────────────────────────────────────────

/// JIT exit code: unsupported IR construct encountered during codegen.
///
/// The JIT backend emits this when it encounters an instruction or terminator
/// that Phase 14 sub-packets 1–3 do not yet implement. Fixtures that produce
/// this exit code are classified as SKIP in the differential harness.
const JIT_EXIT_UNSUPPORTED: i32 = 127;

// ── JIT subprocess runner ─────────────────────────────────────────────────────

/// Run the Cranelift JIT backend on `fixture` and return the captured outcome.
///
/// Identical to [`run_interpreter`] except `--backend=cranelift` is prepended
/// to the argument list, routing execution through the JIT path.
///
/// The binary must have been compiled with `--features jit`. If the JIT
/// encounters an unsupported construct it exits with code 127 (`UNSUPPORTED_CONSTRUCT`);
/// the differential harness classifies that as SKIP rather than PARITY_FAIL.
///
/// # Panics
///
/// Panics if the subprocess cannot be spawned (e.g. binary path is wrong).
pub fn run_jit(binary: &Path, fixture: &TestFixture) -> InterpOutcome {
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

    // ── JIT differential ──────────────────────────────────────────────────────

    /// JIT differential gate — Phase 12, sub-packet 4 (CX-51).
    ///
    /// Runs every verification-matrix fixture through the Cranelift JIT backend
    /// (`--backend=cranelift`) and classifies each result against the fixture's
    /// stored expectation:
    ///
    /// - **PASS** — JIT outcome matches expectation (correct exit code; stdout
    ///   matches for `PassWithOutput` fixtures).
    /// - **SKIP** — JIT exited 127 (`UNSUPPORTED_CONSTRUCT`): the fixture uses a
    ///   construct beyond the current Phase 14 JIT scope. Skips are not failures.
    /// - **PARITY_FAIL** — JIT disagrees with expectation in a way not attributable
    ///   to an unsupported construct.
    ///
    /// The test panics only if there are `PARITY_FAIL` results. A run that
    /// produces only PASS and SKIP results is considered green. The eprintln!
    /// summary constitutes the Phase 12 / Phase 14 parity baseline record.
    ///
    /// Requires the `Cx_0V` binary compiled with `--features jit`. Skips
    /// with a diagnostic message if the binary is absent.
    ///
    /// Run with:
    ///
    /// ```text
    /// cargo build --features jit && cargo test --features jit
    /// ```
    #[test]
    fn jit_differential_all() {
        let binary = cx_binary_path();

        if !binary.exists() {
            eprintln!(
                "SKIP jit_differential_all — binary not found at {:?}.\n\
                 Build with `cargo build --features jit` then re-run tests.",
                binary
            );
            return;
        }

        let fixtures = collect_matrix_tests();
        let mut parity_failures: Vec<String> = Vec::new();
        let mut pass_count: usize = 0;
        let mut skip_count: usize = 0;

        for fixture in &fixtures {
            let outcome = run_jit(&binary, fixture);

            // JIT exit 127 = unsupported construct — skip, not a failure.
            if outcome.exit_code == JIT_EXIT_UNSUPPORTED {
                skip_count += 1;
                continue;
            }

            match &fixture.expectation {
                TestExpectation::Fail => {
                    // Expected non-zero exit. Any non-zero (except 127, handled above)
                    // counts as agreement.
                    if outcome.passed() {
                        parity_failures.push(format!(
                            "PARITY_FAIL [should-fail but JIT exited 0]: {}",
                            fixture.name
                        ));
                    } else {
                        pass_count += 1;
                    }
                }

                TestExpectation::PassAny => {
                    if !outcome.passed() {
                        parity_failures.push(format!(
                            "PARITY_FAIL [expected-pass, JIT exit {}]: {}\n  stderr: {}",
                            outcome.exit_code,
                            fixture.name,
                            outcome.stderr.lines().next().unwrap_or("(no stderr)")
                        ));
                    } else {
                        pass_count += 1;
                    }
                }

                TestExpectation::PassWithOutput(expected) => {
                    if !outcome.passed() {
                        parity_failures.push(format!(
                            "PARITY_FAIL [expected-pass, JIT exit {}]: {}\n  stderr: {}",
                            outcome.exit_code,
                            fixture.name,
                            outcome.stderr.lines().next().unwrap_or("(no stderr)")
                        ));
                    } else {
                        let actual = normalise(&outcome.stdout);
                        if actual != *expected {
                            parity_failures.push(format!(
                                "PARITY_FAIL [output mismatch]: {}\n  expected: {:?}\n  got:      {:?}",
                                fixture.name, expected, actual
                            ));
                        } else {
                            pass_count += 1;
                        }
                    }
                }
            }
        }

        let total = fixtures.len();
        eprintln!(
            "jit_differential_all: {} pass, {} skip (unsupported), {} parity_fail — {} total",
            pass_count,
            skip_count,
            parity_failures.len(),
            total
        );

        if !parity_failures.is_empty() {
            panic!(
                "\n{} JIT parity failure(s) out of {} total ({} skipped):\n\n{}\n",
                parity_failures.len(),
                total,
                skip_count,
                parity_failures.join("\n\n")
            );
        }
    }
}
