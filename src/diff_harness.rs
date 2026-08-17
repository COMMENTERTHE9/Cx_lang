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
//! <name>.cx                   — Cx source program
//! <name>.cx.expected_output   — expected stdout (present only for output-verified pass tests)
//! <name>.cx.expected_fail     — expected-failure marker; empty, or carrying a
//!                               `#!` rejection-shape directive (see below)
//! <name>.cx.expected_exit     — a bare integer: the exact exit code both
//!                               backends must produce (exit() fixtures)
//! <name>.cx.jit_known_unsound — zero-byte marker; excludes the fixture from the
//!                               JIT parity gate because the JIT is known-unsound
//!                               on the construct it exercises (tracker #003).
//!                               Interpreter-side runs ignore this marker.
//! ```
//!
//! Sidecars resolve in priority order — `.expected_exit`, then `.expected_fail`,
//! then `.expected_output` — matching `run_matrix.sh`. A `.cx` file with no
//! companion is a "pass-any" test: the interpreter must exit 0, but its stdout
//! is not verified.
//!
//! # Rejection shapes
//!
//! `.expected_fail` used to assert only `exit_code != 0`, which could not tell a
//! diagnosed rejection from a crash — a JIT hard trap satisfied a fixture whose
//! interpreter emits a clean line-numbered message. It now records the shape of
//! each backend's refusal and asserts it on both sides:
//!
//! ```text
//! #! interp=diagnostic jit=trap
//! ```
//!
//! Lines starting with `#!` are directives; anything else is free-text prose and
//! is ignored, so the fixtures that already carried explanatory notes keep them.
//! An empty marker means `interp=diagnostic jit=diagnostic`.
//!
//! `jit=trap` is not a defect marker. The interpreter has fifteen distinct
//! runtime diagnostics; the JIT funnels all of them through one `cx_trap` exit
//! (126). Eighteen fixtures legitimately differ this way, and listing them is
//! `grep -l 'jit=trap' src/tests/verification_matrix/*.expected_fail`.
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
    LogicalOps,
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
            FeatureCategory::LogicalOps,
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
            FeatureCategory::LogicalOps     => "LogicalOps",
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
        | "t172_arith_t128_exit"
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
        | "t173_const_decl_exit"
        | "t174_block_scope_shadow_exit"
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
        | "t149_for_loop_exit"
        | "t150_for_in_func_exit"
            => FeatureCategory::ForLoop,

        // ── InfiniteLoop ──────────────────────────────────────────────────────
        "t25_loop_break"
        | "t106_loop_break_in_func"
        | "t134_loop_break_exit"
        | "t167_infinite_loop_counter_exit"
        | "t168_infinite_loop_countdown_exit"
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
        | "t159_direct_call_implicit_return_exit"
        | "t160_direct_call_explicit_return_exit"
        | "t161_direct_call_no_args_exit"
        | "t162_direct_call_chained_exit"
        | "t163_direct_call_forward_decl_exit"
        | "t164_direct_call_recursive_exit"
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
        | "t175_impl_basic_exit"
        | "t176_impl_return_exit"
        | "t177_multi_alias_impl_exit"
            => FeatureCategory::Struct,

        // ── Array ─────────────────────────────────────────────────────────────
        "t33_arrays"
        | "t112_array_of_result"
        | "t146_array_read_exit"
        | "t147_array_write_exit"
        | "t148_array_in_func_exit"
            => FeatureCategory::Array,

        // ── CompoundAssign ────────────────────────────────────────────────────
        "t26_compound_add_two"
        | "t41_compound_assign_dot"
        | "t128_struct_compound_assign_exit"
        | "t151_var_compound_assign_exit"
        | "t152_compound_assign_dotaccess_exit"
        | "t153_compound_assign_index_exit"
        | "t169_compound_assign_func_exit"
            => FeatureCategory::CompoundAssign,

        // ── Unary ─────────────────────────────────────────────────────────────
        "t96_overflow_t8_unary_neg"
        | "t165_unary_neg_int_exit"
        | "t166_unary_not_bool_exit"
            => FeatureCategory::Unary,

        // ── Cast ─────────────────────────────────────────────────────────────
        "t139_cast_t32_to_f64_exit"
        | "t140_cast_f64_truncate_exit"
        | "t157_cast_neg_t32_to_f64_exit"
        | "t158_cast_t64_to_f64_exit"
            => FeatureCategory::Cast,

        // ── FloatOps ──────────────────────────────────────────────────────────
        "t55_f64_basic"
        | "t135_float_arith_add_exit"
        | "t136_float_arith_sub_exit"
        | "t137_float_arith_mul_exit"
        | "t138_float_arith_div_exit"
        | "t155_float_arith_mod_exit"
        | "t156_float_neg_exit"
            => FeatureCategory::FloatOps,

        // ── BuiltinAssert ─────────────────────────────────────────────────────
        "t77_assert_basic"
        | "t78_assert_eq_strings"
        | "t79_assert_false_reject"
        | "t80_assert_eq_mismatch_reject"
        | "t170_assert_pass_exit"
        | "t171_assert_eq_pass_exit"
            => FeatureCategory::BuiltinAssert,

        // ── LogicalOps ────────────────────────────────────────────────────────
        "t141_logical_and_or_exit"
        | "t142_logical_while_and_nested_exit"
            => FeatureCategory::LogicalOps,

        // ── Other (everything not assigned to a named category) ───────────────
        _ => FeatureCategory::Other,
    }
}

/// Exit code produced by the Cx JIT binary when codegen encounters an
/// unsupported construct. Matches `JitExitCode::UNSUPPORTED_CONSTRUCT` in
/// `backend::cranelift::host_boundary`.
const JIT_SKIP_EXIT_CODE: i32 = 127;

/// Exit code produced when a JIT-compiled program traps at runtime. Matches
/// `JIT_TRAP_EXIT_CODE` / `JitExitCode::JIT_RUNTIME_FAILURE` in
/// `backend::cranelift::host_boundary`.
const JIT_TRAP_EXIT_CODE: i32 = 126;

/// Maximum time to wait for a single JIT subprocess before killing it.
#[cfg(feature = "jit")]
const JIT_TIMEOUT: Duration = Duration::from_secs(30);

// ── Fixture types ─────────────────────────────────────────────────────────────

/// How a backend refused a program.
///
/// A `Fail` fixture used to assert only `exit_code != 0`, which cannot tell
/// "both backends diagnosed it" from "one diagnosed it and the other crashed".
/// The audit found this is not a hypothetical: across all 137 `.expected_fail`
/// fixtures the interpreter's error KIND predicts the pair exactly — a
/// parse/resolve/semantic error gives (1, 1) because those are raised before
/// backend dispatch, while every *runtime* error gives (1, 126), the JIT having
/// exactly one trap channel against the interpreter's fifteen diagnostics.
///
/// So the pair is recorded rather than forbidden: 18 fixtures legitimately
/// differ, and asserting the recorded shape catches a change in EITHER
/// direction — a diagnosing fixture that starts trapping, or a trapping one
/// that stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectionShape {
    /// Refused with a rendered diagnostic — exit 1.
    Diagnostic,
    /// Refused by trapping — exit 126 (`cx_trap` /
    /// `JitExitCode::JIT_RUNTIME_FAILURE`). No interpreter path produces this
    /// today; the shape is per-backend so that if one ever does, the annotation
    /// can say so instead of the harness silently accepting it.
    Trap,
}

impl RejectionShape {
    /// The exit code this shape must produce.
    pub fn exit_code(self) -> i32 {
        match self {
            RejectionShape::Diagnostic => 1,
            RejectionShape::Trap => JIT_TRAP_EXIT_CODE,
        }
    }

    /// Parse one annotation value.
    fn parse(value: &str) -> Option<Self> {
        match value {
            "diagnostic" => Some(RejectionShape::Diagnostic),
            "trap" => Some(RejectionShape::Trap),
            _ => None,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            RejectionShape::Diagnostic => "diagnostic",
            RejectionShape::Trap => "trap",
        }
    }
}

/// What the interpreter is expected to do when given this fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TestExpectation {
    /// Test must exit 0 and produce stdout that matches the stored string
    /// exactly (after CRLF normalisation and trailing-whitespace trim).
    PassWithOutput(String),

    /// Test must exit 0. Stdout is not checked.
    PassAny,

    /// Test must be refused by both backends, each in the recorded shape
    /// (`.expected_fail` marker present). An empty marker means
    /// `interp=diagnostic jit=diagnostic`.
    Fail {
        interp: RejectionShape,
        jit: RejectionShape,
    },

    /// Test must exit with this exact code (`.expected_exit` sidecar), on both
    /// backends, optionally also matching stored stdout.
    ///
    /// Previously `.expected_exit` was read by `run_matrix.sh` only —
    /// `collect_matrix_tests` had no branch for it, so these fixtures were
    /// classified from their other sidecars. All five happen to also carry
    /// `.expected_fail`, so they were treated as "must exit non-zero" and their
    /// designed codes (3, 4, 5, 7, 9) went unasserted on both backends.
    ExitCode {
        code: i32,
        output: Option<String>,
    },
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

    /// When `true`, this fixture is excluded from the JIT parity gate because
    /// the JIT backend is *known* to be unsound on the construct it exercises
    /// (e.g. array out-of-bounds: the interpreter traps cleanly but the JIT has
    /// no bounds checking, so it returns garbage or segfaults — tracker #003).
    /// Marked by a zero-byte `<name>.cx.jit_known_unsound` sidecar. The parity
    /// harness counts these as SKIP, not PARITY_FAIL. Remove the sidecar (and
    /// this exclusion) once #003 makes the JIT trap OOB so both backends agree.
    /// Interpreter-side matrix runs ignore this flag entirely.
    pub jit_excluded: bool,
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
/// Parse the rejection-shape annotation out of a `.expected_fail` body.
///
/// **Syntax.** Lines beginning with `#!` are directives; every other line is
/// free-text prose and is ignored. A directive line carries whitespace-separated
/// `key=value` pairs, keys `interp` and `jit`, values `diagnostic` or `trap`:
///
/// ```text
/// #! interp=diagnostic jit=trap
/// D2.5c: reading .val on a dropped Handle must reject on both backends.
/// ```
///
/// The `#!` prefix is not invented here — `.cx` source already uses `#![imports]`
/// for its directive block, so it is the repo's own "this line is for the tooling"
/// marker. It is required because `.expected_fail` bodies were NOT reserved:
/// four fixtures already carry explanatory prose, and treating the whole body as
/// structured data would have broken them. Prose stays prose; directives opt in.
///
/// An empty marker — 133 of 137 fixtures, left untouched — means
/// `interp=diagnostic jit=diagnostic`, the (1, 1) majority.
///
/// # Errors
///
/// Returns the offending text for an unknown key, an unknown value, or a
/// malformed pair, so a typo in a sidecar fails loudly rather than silently
/// falling back to the default and re-opening the hole this closes.
fn parse_rejection_shapes(body: &str) -> Result<(RejectionShape, RejectionShape), String> {
    let mut interp = RejectionShape::Diagnostic;
    let mut jit = RejectionShape::Diagnostic;

    for line in body.lines() {
        let Some(directive) = line.trim().strip_prefix("#!") else {
            continue; // free-text prose
        };
        for pair in directive.split_whitespace() {
            let Some((key, value)) = pair.split_once('=') else {
                return Err(format!("malformed directive token {:?} (want key=value)", pair));
            };
            let shape = RejectionShape::parse(value).ok_or_else(|| {
                format!("unknown rejection shape {:?} (want 'diagnostic' or 'trap')", value)
            })?;
            match key {
                "interp" => interp = shape,
                "jit" => jit = shape,
                other => {
                    return Err(format!("unknown directive key {:?} (want 'interp' or 'jit')", other))
                }
            }
        }
    }

    Ok((interp, jit))
}

pub fn collect_matrix_tests() -> Vec<TestFixture> {
    let matrix_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/tests/verification_matrix");

    let mut paths: Vec<PathBuf> = fs::read_dir(&matrix_dir)
        .expect("src/tests/verification_matrix/ must exist and be readable")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            let s = name.to_string_lossy();
            // Accept only plain .cx files — exclude sidecars
            // (.expected_output / .expected_fail / .jit_known_unsound).
            if s.ends_with(".cx")
                && !s.ends_with(".expected_output")
                && !s.ends_with(".expected_fail")
                && !s.ends_with(".expected_exit")
                && !s.ends_with(".jit_known_unsound")
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
            let expected_exit_path = PathBuf::from(format!("{}.expected_exit", path_str));
            let jit_excluded_path = PathBuf::from(format!("{}.jit_known_unsound", path_str));

            let stored_output = || {
                let raw = fs::read_to_string(&expected_output_path)
                    .expect("failed to read .expected_output file");
                normalise(&raw)
            };

            // Priority order mirrors run_matrix.sh: an explicit exit-code
            // assertion is the most specific and wins, then the fail marker,
            // then stored output. `.expected_exit` is new here — the shell
            // runner has honoured it since the exit() slice, the Rust harness
            // never did.
            let expectation = if expected_exit_path.exists() {
                let raw = fs::read_to_string(&expected_exit_path)
                    .expect("failed to read .expected_exit file");
                let code = raw.trim().parse::<i32>().unwrap_or_else(|e| {
                    panic!("{:?}: .expected_exit must hold an integer ({})", path, e)
                });
                TestExpectation::ExitCode {
                    code,
                    output: expected_output_path.exists().then(stored_output),
                }
            } else if expected_fail_path.exists() {
                let body = fs::read_to_string(&expected_fail_path)
                    .expect("failed to read .expected_fail file");
                let (interp, jit) = parse_rejection_shapes(&body)
                    .unwrap_or_else(|e| panic!("{:?}: bad .expected_fail annotation — {}", path, e));
                TestExpectation::Fail { interp, jit }
            } else if expected_output_path.exists() {
                TestExpectation::PassWithOutput(stored_output())
            } else {
                TestExpectation::PassAny
            };

            let jit_excluded = jit_excluded_path.exists();

            TestFixture { name, path, expectation, jit_excluded }
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
/// 2. `<manifest_dir>/target/debug/Cx_0V[.exe]` — whatever the LAST `cargo build`
///    wrote there; feature set is NOT guaranteed. Callers running JIT must gate
///    on `assert_jit_capable`.
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

/// Minimal Cx program known to lower cleanly and exit 0 under the JIT
/// backend (mirrors fixture t159). Used only by [`assert_jit_capable`] to
/// detect a binary built WITHOUT `--features jit`. If the language changes
/// so this no longer lowers, the guard fails loud rather than silently
/// disabling itself.
#[cfg(feature = "jit")]
const JIT_CAPABILITY_PROBE_SRC: &str = "\
fnc: t64 add(a: t64, b: t64) {
    a + b
}
assert_eq(add(10, 20), 30)
";

/// Abort fast if `binary` was not built with `--features jit`.
///
/// `cx_binary_path()` resolves a shared on-disk path whose contents are
/// whatever the last `cargo build` wrote. A plain `cargo build` (no
/// features) leaves a non-JIT binary there; running it with
/// `--backend=cranelift` exits 1 with a fixed stderr, which the classifier
/// turns into a false PARITY_FAIL for every pass fixture. This converts that
/// silent multi-failure run into one loud, actionable panic before the loop.
#[cfg(feature = "jit")]
fn assert_jit_capable(binary: &Path) {
    use std::io::Read;

    let mut probe = std::env::temp_dir();
    probe.push(format!("cx_jit_probe_{}.cx", std::process::id()));
    std::fs::write(&probe, JIT_CAPABILITY_PROBE_SRC).unwrap_or_else(|e| {
        panic!("failed to write JIT capability probe to {:?}: {}", probe, e)
    });

    let mut child = Command::new(binary)
        .arg("--backend=cranelift")
        .arg(&probe)
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|e| {
            let _ = std::fs::remove_file(&probe);
            panic!(
                "failed to spawn binary {:?} for JIT capability probe: {}",
                binary, e
            )
        });

    let mut stderr_bytes = Vec::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_end(&mut stderr_bytes);
    }
    let status = child.wait().unwrap_or_else(|e| {
        let _ = std::fs::remove_file(&probe);
        panic!("failed to wait on JIT capability probe child: {}", e)
    });
    let _ = std::fs::remove_file(&probe);

    let stderr = String::from_utf8_lossy(&stderr_bytes);
    let code = status.code().unwrap_or(-1);

    // The non-JIT cranelift arm (src/backend/cranelift/mod.rs) emits exactly
    // this prefix. The JIT arm never does. ASCII-only substring on purpose.
    if stderr.contains("Cranelift backend requires the") {
        panic!(
            "\n==================================================================\n\
             JIT PARITY ABORTED — wrong binary.\n\
             {:?} was NOT built with `--features jit`.\n\
             stderr: {}\n\
             A plain `cargo build` overwrote target/debug/Cx_0V with a non-JIT\n\
             build. Rebuild and run together:\n\n    \
             cargo build --features jit && cargo test --features jit \
             jit_parity_by_feature -- --nocapture\n\n\
             Refusing to run the fixture matrix against a non-JIT binary (it\n\
             would report every pass fixture as a false PARITY_FAIL).\n\
             ==================================================================",
            binary,
            stderr.trim()
        );
    }

    // On a JIT binary this probe MUST exit 0. Anything else means the probe
    // program has rotted (or the env is broken) — fail loud so the guard can
    // never silently no-op and let a bad binary through.
    if code != 0 {
        panic!(
            "JIT capability probe unreliable: probe did not exit 0 on {:?} \
             (exit {}, stderr: {:?}). The probe must lower and run cleanly \
             under the JIT backend; update JIT_CAPABILITY_PROBE_SRC in \
             src/diff_harness.rs.",
            binary,
            code,
            stderr.trim()
        );
    }
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
            let _ = child.wait();
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
        let entry = map.entry(cat).or_insert((0, 0, 0));

        // Fixtures flagged `.jit_known_unsound` exercise a construct the JIT
        // handles unsoundly (array OOB — no bounds checking; tracker #003). The
        // interpreter traps cleanly but the JIT returns garbage or segfaults, so
        // running them here would produce a (correct-but-unactionable) PARITY_FAIL
        // that just re-reports #003 on every run. Count them as SKIP and don't
        // execute the JIT. Remove the sidecar once #003 lands and the backends
        // agree — they should then convert to PASS.
        if fixture.jit_excluded {
            eprintln!(
                "JIT-EXCLUDED (known-unsound, tracker #003): {} [{}]",
                fixture.name, cat
            );
            entry.1 += 1; // skip
            continue;
        }

        let outcome = run_jit_subprocess(binary, fixture);

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
                // Assert the RECORDED rejection shape, not merely "non-zero".
                // `exit != 0` let a JIT hard trap satisfy a fixture whose
                // interpreter emits a clean line-numbered diagnostic — the
                // blind spot every C5/C6-class divergence hid in. Both
                // directions are now caught: a `jit=diagnostic` fixture that
                // starts trapping, and a `jit=trap` one that stops.
                TestExpectation::Fail { jit, .. } => outcome.exit_code != jit.exit_code(),
                TestExpectation::ExitCode { code, output } => {
                    outcome.exit_code != *code
                        || output
                            .as_ref()
                            .is_some_and(|e| normalise(&outcome.stdout) != *e)
                }
                TestExpectation::PassAny => outcome.exit_code != 0,
                TestExpectation::PassWithOutput(expected) => {
                    outcome.exit_code != 0 || normalise(&outcome.stdout) != *expected
                }
            };
            if is_parity_fail {
                emit_parity_fail_diagnostic(cat, fixture, &outcome, binary);
                entry.2 += 1; // PARITY_FAIL
            } else {
                entry.0 += 1; // pass
            }
        }
    }

    map
}

// ── PARITY_FAIL diagnostic ────────────────────────────────────────────────────

/// Emit a diagnostic to stderr when `parity_by_feature` detects a PARITY_FAIL.
///
/// Prints the fixture name, feature category, expected vs actual outcome, and
/// a full IR dump produced by running `<binary> --backend=validate <fixture>`
/// as a subprocess. The IR dump subprocess is bounded by [`JIT_TIMEOUT`]; if
/// it times out the diagnostic says so and returns without hanging the test.
#[cfg(feature = "jit")]
fn emit_parity_fail_diagnostic(
    category: FeatureCategory,
    fixture: &TestFixture,
    outcome: &InterpOutcome,
    binary: &Path,
) {
    use std::io::Read;
    use wait_timeout::ChildExt;

    eprintln!("\nPARITY_FAIL: {} [{}]", fixture.name, category);

    let expected_desc = match &fixture.expectation {
        TestExpectation::Fail { interp, jit } => format!(
            "JIT rejection shape '{}' (exit {}); interpreter annotated '{}'",
            jit.as_str(),
            jit.exit_code(),
            interp.as_str()
        ),
        TestExpectation::ExitCode { code, output } => match output {
            Some(s) => format!(
                "exit {}, stdout = {:?}",
                code,
                s.lines().next().unwrap_or("(empty)")
            ),
            None => format!("exit {}", code),
        },
        TestExpectation::PassAny => "exit 0".to_string(),
        TestExpectation::PassWithOutput(s) => {
            format!(
                "exit 0, stdout = {:?}",
                s.lines().next().unwrap_or("(empty)")
            )
        }
    };
    eprintln!("  expected:  {}", expected_desc);
    eprintln!("  exit code: {}", outcome.exit_code);
    eprintln!(
        "  stdout:    {}",
        outcome.stdout.lines().next().unwrap_or("(empty)")
    );
    eprintln!(
        "  stderr:    {}",
        outcome.stderr.lines().next().unwrap_or("(empty)")
    );

    eprintln!("  IR dump:");
    let child = Command::new(binary)
        .arg("--backend=validate")
        .arg(&fixture.path)
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(c) => c,
        Err(e) => {
            eprintln!("    (IR dump failed: {})", e);
            return;
        }
    };

    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");

    match child.wait_timeout(JIT_TIMEOUT) {
        Ok(Some(_status)) => {
            let mut ir_bytes = Vec::new();
            let mut err_bytes = Vec::new();
            stdout_pipe.read_to_end(&mut ir_bytes).unwrap_or(0);
            stderr_pipe.read_to_end(&mut err_bytes).unwrap_or(0);
            let ir_text = String::from_utf8_lossy(&ir_bytes);
            let err_text = String::from_utf8_lossy(&err_bytes);
            if !ir_text.is_empty() {
                for line in ir_text.lines() {
                    eprintln!("    {}", line);
                }
            } else if !err_text.is_empty() {
                eprintln!(
                    "    (validation error: {})",
                    err_text.lines().next().unwrap_or("")
                );
            } else {
                eprintln!("    (no IR output)");
            }
        }
        Ok(None) => {
            let _ = child.kill();
            eprintln!("    (IR dump timed out after {}s)", JIT_TIMEOUT.as_secs());
        }
        Err(e) => {
            eprintln!("    (IR dump failed: {})", e);
        }
    }
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
            .filter(|f| matches!(f.expectation, TestExpectation::Fail { .. }))
            .count();
        let exit_code_count = fixtures
            .iter()
            .filter(|f| matches!(f.expectation, TestExpectation::ExitCode { .. }))
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
            fail_count + exit_code_count + pass_output_count + pass_any_count,
            "fixture counts must be exhaustive"
        );
    }

    /// The rejection-shape annotation parses, defaults, and rejects typos.
    #[test]
    fn rejection_shape_parsing() {
        use RejectionShape::{Diagnostic, Trap};

        // Empty marker — the 117-fixture majority — defaults to both diagnostic.
        assert_eq!(parse_rejection_shapes(""), Ok((Diagnostic, Diagnostic)));

        // Prose-only bodies (two fixtures predate the annotation) still default,
        // which is why prose had to stay legal rather than become a parse error.
        assert_eq!(
            parse_rejection_shapes("labeled-breaks (a): break 'nope has no enclosing loop.\n"),
            Ok((Diagnostic, Diagnostic))
        );

        // A directive, with prose kept alongside it.
        assert_eq!(
            parse_rejection_shapes("#! interp=diagnostic jit=trap\nexplanatory note\n"),
            Ok((Diagnostic, Trap))
        );
        assert_eq!(
            parse_rejection_shapes("#! jit=trap"),
            Ok((Diagnostic, Trap))
        );

        // A typo must fail loudly. Falling back to the default would silently
        // re-open the hole this annotation exists to close.
        assert!(parse_rejection_shapes("#! jit=crash").is_err());
        assert!(parse_rejection_shapes("#! backend=trap").is_err());
        assert!(parse_rejection_shapes("#! jittrap").is_err());
    }

    /// The shapes map to the exit codes the two backends actually use.
    #[test]
    fn rejection_shape_exit_codes() {
        assert_eq!(RejectionShape::Diagnostic.exit_code(), 1);
        assert_eq!(RejectionShape::Trap.exit_code(), JIT_TRAP_EXIT_CODE);
        assert_eq!(JIT_TRAP_EXIT_CODE, 126);
        assert_ne!(JIT_TRAP_EXIT_CODE, JIT_SKIP_EXIT_CODE);
    }

    /// The fixtures whose backends legitimately reject in different shapes must
    /// stay enumerable — making them visible as a list was half the point of
    /// recording the pair rather than accepting "non-zero" on both.
    #[test]
    fn differing_rejection_shapes_are_enumerable() {
        let differing: Vec<&str> = collect_matrix_tests()
            .iter()
            .filter(|f| {
                matches!(f.expectation, TestExpectation::Fail { interp, jit } if interp != jit)
            })
            .map(|f| Box::leak(f.name.clone().into_boxed_str()) as &str)
            .collect();

        assert!(
            !differing.is_empty(),
            "expected the known interp-diagnoses/JIT-traps set to be non-empty"
        );
        eprintln!(
            "fixtures whose backends reject in different shapes ({}):\n  {}",
            differing.len(),
            differing.join("\n  ")
        );
    }

    /// `.expected_exit` must be honoured by the Rust harness, not only by
    /// `run_matrix.sh`, and must win the sidecar priority order.
    #[test]
    fn expected_exit_is_honoured_and_takes_priority() {
        let fixtures = collect_matrix_tests();
        let exit_fixtures: Vec<_> = fixtures
            .iter()
            .filter(|f| matches!(f.expectation, TestExpectation::ExitCode { .. }))
            .collect();

        assert!(
            !exit_fixtures.is_empty(),
            "matrix must have at least one .expected_exit fixture"
        );

        for f in &exit_fixtures {
            let TestExpectation::ExitCode { code, .. } = &f.expectation else {
                unreachable!()
            };
            assert_ne!(*code, 0, "{}: an exit-code fixture asserts a real code", f.name);

            // Each also carries `.expected_fail`, which previously classified
            // them. Priority must put ExitCode first — the stronger assertion.
            let marker = PathBuf::from(format!("{}.expected_fail", f.path.to_string_lossy()));
            assert!(
                marker.exists(),
                "{}: expected the redundant .expected_fail companion to still be present",
                f.name
            );
        }
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
                // The interpreter half of the pair. Previously this asserted
                // only "did not exit 0", so the sidecar carried nothing about
                // the interpreter at all and a change in HOW it refuses went
                // unnoticed. Now the recorded shape is asserted on this side
                // too, which is what makes the annotation a pair rather than a
                // JIT-only note.
                TestExpectation::Fail { interp, .. } => {
                    if outcome.exit_code != interp.exit_code() {
                        failures.push(format!(
                            "FAIL [rejection shape]: {} — annotated interp={} (exit {}), got exit {}",
                            fixture.name,
                            interp.as_str(),
                            interp.exit_code(),
                            outcome.exit_code
                        ));
                    }
                }

                TestExpectation::ExitCode { code, output } => {
                    if outcome.exit_code != *code {
                        failures.push(format!(
                            "FAIL [exit code]: {} — expected exit {}, got {}",
                            fixture.name, code, outcome.exit_code
                        ));
                    } else if let Some(expected) = output {
                        let actual = normalise(&outcome.stdout);
                        if actual != *expected {
                            failures.push(format!(
                                "FAIL [output mismatch, exit {} ok]: {}\n  expected: {:?}\n  got:      {:?}",
                                code, fixture.name, expected, actual
                            ));
                        }
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

    // ── exit() builtin in --test mode ─────────────────────────────────────────

    /// Write `src` to a unique temp `.cx` file, run `<binary> --test <file>`,
    /// and return (stdout, exit_code). The temp file is removed before return.
    fn run_test_mode(src: &str, tag: &str) -> (String, i32) {
        let dir = std::env::temp_dir();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let file = dir.join(format!("cx_exit_testmode_{}_{}.cx", tag, nanos));
        std::fs::write(&file, src).expect("write temp test-mode fixture");

        let output = std::process::Command::new(cx_binary_path())
            .arg("--test")
            .arg(&file)
            .output()
            .expect("spawn --test subprocess");

        let _ = std::fs::remove_file(&file);

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let code = output.status.code().unwrap_or(-1);
        (stdout, code)
    }

    /// A `#[test]` function calling `exit(1)` must be recorded as a failure and
    /// must NOT terminate the runner — the second test must still execute.
    /// This is the structural property that stops `exit()` from silently
    /// disabling the rest of the test suite.
    #[test]
    fn test_mode_exit_nonzero_counts_as_fail_and_runner_continues() {
        let binary = cx_binary_path();
        if !binary.exists() {
            eprintln!("SKIP — binary not found at {:?}; build first.", binary);
            return;
        }

        let src = "\
#[test]
fnc: first() {
    exit(1)
}
#[test]
fnc: second() {
    assert_eq(1, 1)
}
";
        let (stdout, code) = run_test_mode(src, "fail");

        // The second test must have run — this is the anti-silent-disable check.
        assert!(
            stdout.contains("second"),
            "second test did not run — exit() killed the runner. stdout:\n{}",
            stdout
        );
        // first counts as a failure.
        assert!(
            stdout.contains("FAIL: first"),
            "exit(1) in a test should be recorded as FAIL. stdout:\n{}",
            stdout
        );
        // Overall accounting: one pass, one fail; runner exits non-zero.
        assert!(
            stdout.contains("1 passed, 1 failed"),
            "expected '1 passed, 1 failed'. stdout:\n{}",
            stdout
        );
        assert_ne!(code, 0, "runner must exit non-zero when a test failed");
    }

    /// A `#[test]` function calling `exit(0)` must be recorded as a pass and the
    /// runner must continue to the next test.
    #[test]
    fn test_mode_exit_zero_counts_as_pass_and_runner_continues() {
        let binary = cx_binary_path();
        if !binary.exists() {
            eprintln!("SKIP — binary not found at {:?}; build first.", binary);
            return;
        }

        let src = "\
#[test]
fnc: first() {
    exit(0)
}
#[test]
fnc: second() {
    assert_eq(2, 2)
}
";
        let (stdout, code) = run_test_mode(src, "pass");

        assert!(
            stdout.contains("second"),
            "second test did not run after exit(0). stdout:\n{}",
            stdout
        );
        assert!(
            stdout.contains("2 passed, 0 failed"),
            "expected '2 passed, 0 failed' (exit(0) = pass). stdout:\n{}",
            stdout
        );
        assert_eq!(code, 0, "runner must exit 0 when all tests passed");
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
            panic!(
                "\n==================================================================\n\
                 JIT PARITY ABORTED — binary not found.\n\
                 No Cx_0V binary at {:?}.\n\
                 The parity gate cannot pass without a JIT-built binary to test.\n\
                 Build and run together:\n\n    \
                 cargo build --features jit && cargo test --features jit \
                 jit_parity_by_feature -- --nocapture\n\n\
                 Refusing to report a green gate when no binary was exercised.\n\
                 ==================================================================",
                binary
            );
        }

        assert_jit_capable(&binary);

        let results = parity_by_feature(&binary);

        println!("\njit_parity_by_feature results:");
        println!("{:<20} {:>6} {:>6} {:>12}", "Feature", "PASS", "SKIP", "PARITY_FAIL");
        println!("{}", "-".repeat(48));
        for cat in FeatureCategory::all() {
            let (pass, skip, fail) = results[cat];
            println!("{:<20} {:>6} {:>6} {:>12}", cat, pass, skip, fail);
        }
        println!("{}", "-".repeat(48));

        // Single source of truth for all four totals — summed directly from
        // the same `results` accumulator the per-category table above uses.
        // The H5 finding in the 2026-05-19 re-audit identified that prior
        // reports (across four commit messages) gave 94/88 for PASS/SKIP when
        // the table summed to 99/83; that drift came from manual summation of
        // the printed table rather than from any accumulator inconsistency.
        // Emitting an authoritative line directly from `results` closes that
        // class of report-vs-reality drift.
        let total_pass: usize = results.values().map(|(p, _, _)| *p).sum();
        let total_skip: usize = results.values().map(|(_, s, _)| *s).sum();
        let total_fail: usize = results.values().map(|(_, _, f)| *f).sum();
        let total: usize = total_pass + total_skip + total_fail;

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
        eprintln!(
            "AUTHORITATIVE TOTALS: {} PASS / {} SKIP / {} PARITY_FAIL across {} fixtures",
            total_pass, total_skip, total_fail, total
        );
    }
}
