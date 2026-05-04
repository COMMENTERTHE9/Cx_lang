#!/bin/bash
# run_all.sh -- Run every Cx example and report pass/fail
# Usage: bash examples/run_all.sh
# Works on Linux, macOS, and Windows Git Bash.

# Resolve cargo — support CARGO env override, PATH, and default install location
if [ -n "$CARGO" ]; then
    CARGO_CMD="$CARGO"
elif command -v cargo >/dev/null 2>&1; then
    CARGO_CMD="cargo"
elif [ -x "$HOME/.cargo/bin/cargo" ]; then
    CARGO_CMD="$HOME/.cargo/bin/cargo"
else
    echo "ERROR: cargo not found. Set CARGO env var or add cargo to PATH."
    exit 1
fi

EXAMPLES_DIR="examples"
PASS=0
FAIL=0
ERRORS=()

for cx_file in "$EXAMPLES_DIR"/*.cx; do
    name=$(basename "$cx_file")
    output=$($CARGO_CMD run --quiet -- "$cx_file" 2>&1)
    exit_code=$?

    if [ $exit_code -eq 0 ]; then
        echo "PASS -- $name"
        PASS=$((PASS + 1))
    else
        echo "FAIL -- $name"
        echo "  Output: $output"
        FAIL=$((FAIL + 1))
        ERRORS+=("$name")
    fi
done

echo ""
echo "Results: $PASS PASS, $FAIL FAIL out of $((PASS + FAIL)) total"

if [ ${#ERRORS[@]} -gt 0 ]; then
    echo ""
    echo "Failed examples:"
    for e in "${ERRORS[@]}"; do
        echo "  - $e"
    done
    exit 1
fi

exit 0
