#!/usr/bin/env python3
"""FakApp runtime code must be panic-free.

A watchdog that dies takes its silence with it: no unwrap/expect on Option or
Result, no panic!/unreachable!/todo! outside test modules. `unwrap_or*` is
fine — it is total and cannot panic. Test modules are exempt.
"""
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parents[1]

FORBIDDEN = re.compile(
    r"\.(unwrap|expect)\(|\bpanic!\s*\(|\bunreachable!\s*\(|\btodo!\s*\(|\bunimplemented!\s*\("
)
# .unwrap_or(, .unwrap_or_else(, .unwrap_or_default() never match the pattern
# above because of the word boundary + open-paren requirement.

failures = []
for path in sorted((ROOT / "src").rglob("*.rs")):
    runtime = path.read_text(encoding="utf-8").split("#[cfg(test)]", 1)[0]
    for number, line in enumerate(runtime.splitlines(), start=1):
        if line.lstrip().startswith("//"):
            continue
        if FORBIDDEN.search(line):
            failures.append(f"{path.relative_to(ROOT)}:{number}: {line.strip()}")

if failures:
    print("RUNTIME_PANICS=FAIL — a watchdog must not die quietly:")
    for failure in failures:
        print(f"  {failure}")
    sys.exit(1)

print("RUNTIME_PANICS=PASS unwrap/expect/panic-free on all runtime paths")
