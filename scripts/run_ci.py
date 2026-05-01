#!/usr/bin/env python3
"""
CI runner for agent-teams-v2.

Pipeline (fail-fast):
  1. Golden Rules (file size, secrets, console.log, doc freshness, invariant coverage)
  2. cargo fmt --check
  3. cargo clippy --all-targets -- -D warnings
  4. cargo test (workspace default features + team-mode-web feature)

Usage:
  python scripts/run_ci.py
  python scripts/run_ci.py --skip-test       # skip cargo test (faster local check)
  python scripts/run_ci.py --skip-clippy     # skip clippy

Exit code:
  0 -- all PASS
  1 -- any FAIL
"""
import argparse
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SRC_DIRS = [str(ROOT / "src"), str(ROOT / "web"), str(ROOT / "plugin")]
DOCS_DIR = str(ROOT / ".plans" / "agent-teams-v2" / "docs")


def banner(title):
    print()
    print("=" * 60)
    print(f"  {title}")
    print("=" * 60)


def run(cmd, label, allow_fail=False):
    """Run cmd. Return True on success."""
    banner(label)
    print(f"$ {' '.join(cmd)}")
    try:
        result = subprocess.run(cmd, cwd=ROOT)
    except FileNotFoundError:
        print(f"[FAIL] command not found: {cmd[0]}")
        return allow_fail
    if result.returncode == 0:
        print(f"[PASS] {label}")
        return True
    print(f"[FAIL] {label} (exit {result.returncode})")
    return False


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--skip-test", action="store_true")
    parser.add_argument("--skip-clippy", action="store_true")
    parser.add_argument("--skip-fmt", action="store_true")
    parser.add_argument("--skip-golden", action="store_true")
    args = parser.parse_args()

    failures = []

    # Step 1: Golden Rules
    if not args.skip_golden:
        banner("Step 1: Golden Rules")
        sys.path.insert(0, str(ROOT / "scripts"))
        try:
            from golden_rules import check_all
            fails, _warns, _infos = check_all(SRC_DIRS, docs_dir=DOCS_DIR)
            if fails > 0:
                failures.append(f"Golden Rules ({fails} FAIL)")
        except Exception as e:
            print(f"[FAIL] golden_rules failed to run: {e}")
            failures.append("Golden Rules (exception)")

    # Step 2: cargo fmt
    if not args.skip_fmt:
        if not run(["cargo", "fmt", "--check"], "Step 2: cargo fmt --check"):
            failures.append("cargo fmt")

    # Step 3: cargo clippy
    if not args.skip_clippy:
        if not run(
            ["cargo", "clippy", "--all-targets", "--", "-D", "warnings"],
            "Step 3: cargo clippy -D warnings",
        ):
            failures.append("cargo clippy")

    # Step 4: cargo test
    if not args.skip_test:
        if not run(["cargo", "test", "--workspace"], "Step 4: cargo test --workspace"):
            failures.append("cargo test")
        if not run(
            ["cargo", "test", "--workspace", "--features", "team-mode-web"],
            "Step 4b: cargo test --workspace --features team-mode-web",
        ):
            failures.append("cargo test --features team-mode-web")

    # Summary
    banner("CI Summary")
    if not failures:
        print("[OK] All checks passed.")
        sys.exit(0)
    print("[FAIL] The following steps failed:")
    for f in failures:
        print(f"  - {f}")
    print()
    print("Fix before sending to @reviewer.")
    sys.exit(1)


if __name__ == "__main__":
    main()
