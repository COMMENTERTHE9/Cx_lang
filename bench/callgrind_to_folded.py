#!/usr/bin/env python3
"""Render a flat self-cost flamegraph from `callgrind_annotate` output.

No `perf` is available in the audit container (and perf_event_paranoid=2
blocks it), so profiles are taken with `valgrind --tool=callgrind`. Callgrind
records a call *graph* with costs, not sampled stacks; reconstructing stacks
from it is ambiguous under recursion. We instead let `callgrind_annotate`
compute the authoritative per-function *self* cost (Ir = instructions
retired, --inclusive=no) and reshape that into a flat folded file.

The resulting flamegraph is an honest per-function self-cost chart, not a
sampled stack profile — read each column's width as "share of instructions
executed inside this function".

Usage:
    callgrind_annotate --inclusive=no --threshold=100 cg.out \
        | python3 callgrind_to_folded.py \
        | inferno-flamegraph --countname 'instructions (Ir)' > out.svg
"""
import sys, re

def shorten(fn):
    fn = fn.split(":", 1)[-1] if ":" in fn else fn   # drop file: prefix
    fn = fn.replace("Cx_0V::runtime::runtime::", "").replace("Cx_0V::", "")
    fn = re.sub(r"\s*\[.*?\]", "", fn).strip()
    fn = fn.lstrip("?:").strip()
    return fn or "?"

def main():
    # callgrind_annotate self-cost lines look like:
    #   "   164,701,807 (14.62%)  file.rs:func [obj]"
    pat = re.compile(r"^\s*([\d,]+)\s*(?:\([^)]*\))?\s+(\S.*)$")
    for line in sys.stdin:
        if "PROGRAM TOTALS" in line or "Ir " in line and "file:function" in line:
            continue
        m = pat.match(line)
        if not m:
            continue
        ir = int(m.group(1).replace(",", ""))
        name = shorten(m.group(2))
        if ir > 0 and name not in ("?",):
            print(f"all;{name} {ir}")

if __name__ == "__main__":
    main()
