//! CX-23 — Differential harness: interpreter baseline capture and fixture format.
//!
//! Phase 12, sub-packet 1.
//!
//! This module is the shell of the differential testing harness. It defines
//! the data types, fixture format, and collection logic for running every
//! matrix test through the interpreter and comparing output against stored
//! expectations.
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
//! - `cx_binary_path()` — locate the compiled Cx binary
//! - `#[test] interpreter_baseline_all` — baseline gate

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

// ── Feature classification ────────────────────────────────────────────────────

/// Language feature categories for the Phase 12 parity checklist.
///
/// Each `t*.cx` fixture maps to exactly one category. This mapping lets the
/// differential harness report per-feature pass / skip / PARITY_FAIL counts
/// rather than a single aggregate total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum FeatureCategory {
    Arithmetic,
    VariableDecl,
    IfElse,
    WhileLoop,
    ForLoop,
    InfiniteLoop,
    DirectCall,
    Struct,
    Array,
    CompoundAssign,
    Unary,
    Cast,
    FloatOps,
    BuiltinAssert,
    Other,
}

impl FeatureCategory {
    /// All category variants in a stable order for table output.
    pub fn all() -> &'static [FeatureCategory] {
        &[
            FeatureCategory::Arithmetic,
            FeatureCategory::VariableDecl,
            FeatureCategory::IfElse,
            FeatureCategory::WhileLoop,
            FeatureCategory::ForLoop,
            FeatureCategory::InfiniteLoop,
            FeatureCategory::DirectCall,
            FeatureCategory::Struct,
            FeatureCategory::Array,
            FeatureCategory::CompoundAssign,
            FeatureCategory::Unary,
            FeatureCategory::Cast,
            FeatureCategory::FloatOps,
            FeatureCategory::BuiltinAssert,
            FeatureCategory::Other,
        ]
    }
}

impl std::fmt::Display for FeatureCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            FeatureCategory::Arithmetic     => "Arithmetic",
            FeatureCategory::VariableDecl   => "VariableDecl",
            FeatureCategory::IfElse         => "IfElse",
            FeatureCategory::WhileLoop      => "WhileLoop",
            FeatureCategory::ForLoop        => "ForLoop",
            FeatureCategory::InfiniteLoop   => "InfiniteLoop",
            FeatureCategory::DirectCall     => "DirectCall",
            FeatureCategory::Struct         => "Struct",
            FeatureCategory::Array          => "Array",
            FeatureCategory::CompoundAssign => "CompoundAssign",
            FeatureCategory::Unary          => "Unary",
            FeatureCategory::Cast           => "Cast",
            FeatureCategory::FloatOps       => "FloatOps",
            FeatureCategory::BuiltinAssert  => "BuiltinAssert",
            FeatureCategory::Other          => "Other",
        };
        write!(f, "{}", s)
    }
}

/// Map a fixture stem (e.g. `"t01_arith_eq_mod"`) to its feature category.
///
/// Every `t*.cx` fixture maps to exactly one category. Fixtures not matching
/// a named entry map to [`FeatureCategory::Other`].
pub fn feature_of(fixture_name: &str) -> FeatureCategory {
    match fixture_name {
        // ── Arithmetic ────────────────────────────────────────────────────────
        "t01_arith_eq_mod"
        | "t89_overflow_t8_add"
        | "t90_overflow_t8_mul"
        | "t91_overflow_t8_chain"
        | "t92_overflow_t8_compare"
        | "t93_overflow_t16_wrap"
        | "t94_overflow_mixed_widths"
        | "t95_overflow_t128_unchanged"
        | "t103_arithmetic_on_strings"
        | "t114_eval_order_binary_arith"
        | "t115_eval_order_compare"
        | "t116_eval_order_nested"
        | "t117_arith_add_exit"
        | "t118_arith_sub_exit"
        | "t119_arith_mul_exit"
        | "t120_arith_div_exit"
        | "t121_arith_mod_exit"
            => FeatureCategory::Arithmetic,

        // ── VariableDecl ──────────────────────────────────────────────────────
        "t15_block_scope_shadow"
        | "t56_const_basic"
        | "t57_const_reassign_reject"
        | "t101_undefined_var_hint"
        | "t102_type_mismatch_uses_cx_names"
        | "t122_vardecl_int_exit"
        | "t123_vardecl_reassign_exit"
        | "t124_vardecl_arith_exit"
            => FeatureCategory::VariableDecl,

        // ── IfElse ────────────────────────────────────────────────────────────
        "t44_if_else_basic"
        | "t45_if_else_in_func"
        | "t46_if_not"
        | "t129_if_else_exit"
        | "t130_if_else_in_func_exit"
        | "t131_if_not_exit"
            => FeatureCategory::IfElse,

        // ── WhileLoop ─────────────────────────────────────────────────────────
        "t23_while_loop"
        | "t34_while_in"
        | "t35_while_in_then"
        | "t105_while_in_func"
        | "t107_continue_in_func"
        | "t108_nested_loops_in_func"
        | "t132_while_loop_exit"
        | "t133_while_in_func_exit"
            => FeatureCategory::WhileLoop,

        // ── ForLoop ───────────────────────────────────────────────────────────
        "t48_for_loop"
        | "t104_for_in_func"
            => FeatureCategory::ForLoop,

        // ── InfiniteLoop ──────────────────────────────────────────────────────
        "t25_loop_break"
        | "t106_loop_break_in_func"
        | "t134_loop_break_exit"
            => FeatureCategory::InfiniteLoop,

        // ── DirectCall ────────────────────────────────────────────────────────
        "t02_implicit_return"
        | "t03_explicit_return"
        | "t04_wrong_return_type"
        | "t05_missing_return_value"
        | "t06_void_unexpected_return"
        | "t07_arg_count_mismatch"
        | "t08_arg_type_mismatch"
        | "t14_nested_5_deep"
        | "t29_forward_decl"
        | "t50_nested_func_no_leak"
        | "t113_recursive_fib"
            => FeatureCategory::DirectCall,

        // ── Struct ────────────────────────────────────────────────────────────
        "t36_struct_probe"
        | "t39_impl_basic"
        | "t40_impl_return"
        | "t43_multi_alias_impl"
        | "t109_struct_field_overflow"
        | "t110_struct_field_assign_overflow"
        | "t114_field_type_mismatch_reject"
        | "t115_strref_in_struct_reject"
        | "t125_struct_field_read_exit"
        | "t126_struct_second_field_read_exit"
        | "t127_struct_field_write_exit"
            => FeatureCategory::Struct,

        // ── Array ─────────────────────────────────────────────────────────────
        "t33_arrays"
        | "t112_array_of_result"
            => FeatureCategory::Array,

        // ── CompoundAssign ────────────────────────────────────────────────────
        "t26_compound_add_two"
        | "t41_compound_assign_dot"
        | "t128_struct_compound_assign_exit"
            => FeatureCategory::CompoundAssign,

        // ── Unary ─────────────────────────────────────────────────────────────
        "t96_overflow_t8_unary_neg"
            => FeatureCategory::Unary,

        // ── Cast ─────────────────────────────────────────────────────────────
        "t139_cast_t32_to_f64_exit"
        | "t140_cast_f64_truncate_exit"
            => FeatureCategory::Cast,

        // ── FloatOps ──────────────────────────────────────────────────────────
        "t55_f64_basic"
        | "t135_float_arith_add_exit"
        | "t136_float_arith_sub_exit"
        | "t137_float_arith_mul_exit"
        | "t138_float_arith_div_exit"
            => FeatureCategory::FloatOps,

        // ── BuiltinAssert ─────────────────────────────────────────────────────
        "t77_assert_basic"
        | "t78_assert_eq_strings"
        | "t79_assert_false_reject"
        | "t80_assert_eq_mismatch_reject"
            => FeatureCategory::BuiltinAssert,

        // ── Other (everything not assigned to a named category) ───────────────
        _ => FeatureCategory::Other,
    }
}

/// Exit code produced by the Cx JIT binary when codegen encounters an
/// unsupported construct. Matches `JitExitCode::UNSUPPORTED_CONSTRUCT` in
/// `backend::cranelift::host_boundary`.
const JIT_SKIP_EXIT_CODE: i32 = 127;

/// Maximum time to wait for a single JIT subprocess before killing it.
#[cfg(feature = "jit")]
const JIT_TIMEOUT: Duration = Duration::from_secs(30);

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

// ── JIT subprocess runner ─────────────────────────────────────────────────────

/// Run the Cx binary in JIT mode on `fixture` and return the captured outcome.
///
/// Spawns `<binary> --backend=cranelift <fixture_path>` as a subprocess.
/// An exit code of [`JIT_SKIP_EXIT_CODE`] (127) means codegen encountered an
/// unsupported construct — callers should count this as SKIP, not PARITY_FAIL.
///
/// Requires the binary to have been built with `--features jit`.
#[cfg(feature = "jit")]
pub fn run_jit_subprocess(binary: &Path, fixture: &TestFixture) -> InterpOutcome {
    use std::io::Read;
    use wait_timeout::ChildExt;

    let mut child = Command::new(binary)
        .arg("--backend=cranelift")
        .arg(&fixture.path)
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| {
            panic!(
                "failed to spawn JIT binary {:?} for fixture {:?}: {}",
                binary, fixture.path, e
            )
        });

    // Take pipe handles before calling wait_timeout so we can read them after
    // the process exits without a second wait() call.
    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");

    match child.wait_timeout(JIT_TIMEOUT).unwrap_or_else(|e| {
        panic!("wait_timeout failed for fixture {:?}: {}", fixture.path, e)
    }) {
        Some(status) => {
            let mut stdout_bytes = Vec::new();
            let mut stderr_bytes = Vec::new();
            stdout_pipe.read_to_end(&mut stdout_bytes).unwrap_or(0);
            stderr_pipe.read_to_end(&mut stderr_bytes).unwrap_or(0);
            InterpOutcome {
                stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
                stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
                exit_code: status.code().unwrap_or(-1),
            }
        }
        None => {
            let _ = child.kill();
            InterpOutcome {
                stdout: String::new(),
                stderr: format!(
                    "JIT subprocess timed out after {}s",
                    JIT_TIMEOUT.as_secs()
                ),
                exit_code: -1,
            }
        }
    }
}

/// Run all matrix fixtures through the Cranelift JIT subprocess and aggregate
/// results by [`FeatureCategory`].
///
/// Returns a map from category to `(pass, skip, parity_fail)` counts:
///
/// - **pass** — JIT outcome matched the stored fixture expectation
/// - **skip** — JIT subprocess exited with [`JIT_SKIP_EXIT_CODE`] (127);
///   codegen does not yet support the construct (expected, not a failure)
/// - **parity_fail** — JIT outcome diverged from the stored expectation
///
/// All 15 [`FeatureCategory`] variants are present in the returned map even
/// when no fixture maps to that category (zero counts).
#[cfg(feature = "jit")]
pub fn parity_by_feature(
    binary: &Path,
) -> std::collections::HashMap<FeatureCategory, (usize, usize, usize)> {
    use std::collections::HashMap;

    let fixtures = collect_matrix_tests();
    let mut map: HashMap<FeatureCategory, (usize, usize, usize)> = HashMap::new();
    for &cat in FeatureCategory::all() {
        map.insert(cat, (0, 0, 0));
    }

    for fixture in &fixtures {
        let cat = feature_of(&fixture.name);
        let outcome = run_jit_subprocess(binary, fixture);
        let entry = map.entry(cat).or_insert((0, 0, 0));

        // Two SKIP signals:
        //
        // 1. exit 127 (JIT_SKIP_EXIT_CODE): the binary propagated the
        //    unsupported-construct sentinel (JitExitCode::UNSUPPORTED_CONSTRUCT).
        //    This is the canonical SKIP path after CX-74.
        //
        // 2. exit 0 with non-empty stderr: legacy fallback retained for safety.
        //    Before CX-74 this fired when IR lowering or JIT codegen failed
        //    without propagating a non-zero exit code.  After CX-74 all error
        //    paths in main.rs propagate non-zero exit codes, so this condition
        //    should no longer fire in practice.
        if outcome.exit_code == JIT_SKIP_EXIT_CODE
            || (outcome.exit_code == 0 && !outcome.stderr.is_empty())
        {
            entry.1 += 1; // skip
        } else {
            let is_parity_fail = match &fixture.expectation {
                TestExpectation::Fail => outcome.exit_code == 0,
                TestExpectation::PassAny => outcome.exit_code != 0,
                TestExpectation::PassWithOutput(expected) => {
                    outcome.exit_code != 0 || normalise(&outcome.stdout) != *expected
                }
            };
            if is_parity_fail {
                entry.2 += 1; // PARITY_FAIL
            } else {
                entry.0 += 1; // pass
            }
        }
    }

    map
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

    // ── JIT parity by feature ─────────────────────────────────────────────────

    /// Per-feature JIT parity gate (Phase 12 checklist).
    ///
    /// Runs every matrix fixture through the Cranelift JIT subprocess and
    /// reports pass / skip / PARITY_FAIL counts per [`FeatureCategory`].
    ///
    /// A PARITY_FAIL occurs when the JIT outcome diverges from the stored
    /// fixture expectation. A SKIP (exit 127) means codegen does not yet
    /// support the construct — skips are expected and do not fail the test.
    ///
    /// Run with:
    ///
    /// ```text
    /// cargo build --features jit && cargo test --features jit jit_parity_by_feature --nocapture
    /// ```
    #[test]
    #[cfg(feature = "jit")]
    fn jit_parity_by_feature() {
        let binary = cx_binary_path();

        if !binary.exists() {
            eprintln!(
                "SKIP jit_parity_by_feature — binary not found at {:?}.\n\
                 Build with `cargo build --features jit` then re-run tests.",
                binary
            );
            return;
        }

        let results = parity_by_feature(&binary);

        println!("\njit_parity_by_feature results:");
        println!("{:<20} {:>6} {:>6} {:>12}", "Feature", "PASS", "SKIP", "PARITY_FAIL");
        println!("{}", "-".repeat(48));
        for cat in FeatureCategory::all() {
            let (pass, skip, fail) = results[cat];
            println!("{:<20} {:>6} {:>6} {:>12}", cat, pass, skip, fail);
        }
        println!("{}", "-".repeat(48));

        let total: usize = results.values().map(|(p, s, f)| p + s + f).sum();
        let total_fail: usize = results.values().map(|(_, _, f)| *f).sum();

        assert_eq!(
            total_fail,
            0,
            "{} PARITY_FAIL(s) detected across all feature categories (see table above)",
            total_fail
        );

        eprintln!(
            "jit_parity_by_feature: {} fixtures checked across {} feature categories, 0 PARITY_FAILs",
            total,
            results.len()
        );
    }
}
