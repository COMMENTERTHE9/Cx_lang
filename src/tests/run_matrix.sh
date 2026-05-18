#!/bin/bash

MATRIX_DIR="src/tests/verification_matrix"
PASS=0
FAIL=0
ERRORS=()

for test_file in "$MATRIX_DIR"/t*.cx; do
    test_name=$(basename "$test_file")
    expected_fail=false

    if [ -f "${test_file}.expected_fail" ]; then
        expected_fail=true
    fi

    output=$(cargo run --quiet -- "$test_file" 2>&1)
    exit_code=$?

    if $expected_fail; then
        if [ $exit_code -ne 0 ]; then
            echo "PASS (expected fail) — $test_name"
            PASS=$((PASS + 1))
        else
            echo "FAIL (should have errored) — $test_name"
            FAIL=$((FAIL + 1))
            ERRORS+=("$test_name")
        fi
    else
        if [ $exit_code -eq 0 ]; then
            if [ -f "${test_file}.expected_output" ]; then
                expected=$(cat "${test_file}.expected_output")
                actual=$(cargo run --quiet -- "$test_file" 2>/dev/null)
                if [ "$actual" = "$expected" ]; then
                    echo "PASS (output verified) — $test_name"
                    PASS=$((PASS + 1))
                else
                    echo "FAIL (output mismatch) — $test_name"
                    echo "  Expected: $expected"
                    echo "  Got:      $actual"
                    FAIL=$((FAIL + 1))
                    ERRORS+=("$test_name")
                fi
            else
                echo "PASS — $test_name"
                PASS=$((PASS + 1))
            fi
        else
            echo "FAIL — $test_name"
            echo "  Output: $output"
            FAIL=$((FAIL + 1))
            ERRORS+=("$test_name")
        fi
    fi
done

echo ""
echo "Results: $PASS PASS, $FAIL FAIL out of $((PASS + FAIL)) total"

if [ ${#ERRORS[@]} -gt 0 ]; then
    echo ""
    echo "Failed tests:"
    for e in "${ERRORS[@]}"; do
        echo "  - $e"
    done
    exit 1
fi

exit 0
