#!/usr/bin/env python3
"""
Golden Rules -- universal code health checks for CCteam-creator projects.

Pre-installed by CCteam-creator skill. Copied to <project>/scripts/ during
Step 3.6 (Harness Setup). Called by run_ci.py as part of the CI pipeline.

Usage:
    # Standalone
    python golden_rules.py src/backend src/frontend

    # From run_ci.py
    from golden_rules import check_all
    result = check_all(["src/backend", "src/frontend"], docs_dir=".plans/<project>/docs")

Error messages follow agent-readable format:
    [TAG] <what's wrong>
      File: <path:line>
      FIX: <exactly how to fix it>
"""
import re
import subprocess
import sys
from dataclasses import dataclass, field
from pathlib import Path

# ---------------------------------------------------------------------------
# Result collector (no global mutable state)
# ---------------------------------------------------------------------------


@dataclass
class CheckResult:
    fails: int = 0
    warns: int = 0
    infos: int = 0

    def fail(self, tag, msg, fix):
        self.fails += 1
        print(f"  [FAIL] [{tag}] {msg}")
        print(f"    FIX: {fix}\n")

    def warn(self, tag, msg, fix):
        self.warns += 1
        print(f"  [WARN] [{tag}] {msg}")
        print(f"    FIX: {fix}\n")

    def info(self, tag, msg, fix):
        self.infos += 1
        print(f"  [INFO] [{tag}] {msg}")
        print(f"    FIX: {fix}\n")


# ---------------------------------------------------------------------------
# Shared helpers
# ---------------------------------------------------------------------------
CODE_EXTENSIONS = {
    ".py", ".ts", ".tsx", ".js", ".mjs", ".jsx", ".vue", ".svelte",
    ".go", ".rs", ".java", ".kt", ".rb", ".php",
}
WEB_FILE_SIZE_EXTENSIONS = {".js", ".mjs"}

EXCLUDE_DIRS = {
    "node_modules", ".git", "__pycache__", ".venv", "venv",
    "dist", "build", ".next", ".nuxt", "coverage", ".plans",
}


def _iter_code_files(src_dirs):
    """Yield Path objects for code files in src_dirs, skipping excluded dirs and minified files."""
    for src_dir in src_dirs:
        root = Path(src_dir)
        if not root.exists():
            continue
        for f in root.rglob("*"):
            if not f.is_file():
                continue
            if f.suffix not in CODE_EXTENSIONS:
                continue
            if any(part in EXCLUDE_DIRS for part in f.parts):
                continue
            # Skip minified files (e.g., foo.min.js)
            if ".min." in f.name:
                continue
            yield f


def _iter_web_file_size_files(project_root=None):
    """Yield web/**/*.js and web/**/*.mjs for GR-1 even when web/ is not in src_dirs."""
    root = Path(project_root) if project_root else Path.cwd()
    web_root = root / "web"
    if not web_root.exists():
        return
    for f in web_root.rglob("*"):
        if not f.is_file():
            continue
        if f.suffix not in WEB_FILE_SIZE_EXTENSIONS:
            continue
        if any(part in EXCLUDE_DIRS for part in f.parts):
            continue
        if ".min." in f.name:
            continue
        yield f


def _project_root():
    """Return the current project root for repo-specific guardrails."""
    return Path.cwd()


def _rel(path, root=None):
    root = root or _project_root()
    try:
        return path.relative_to(root).as_posix()
    except ValueError:
        return path.as_posix()


def _line_number_for_text(path, needle):
    try:
        for i, line in enumerate(path.read_text(encoding="utf-8", errors="ignore").splitlines(), 1):
            if needle in line:
                return i
    except Exception:
        return 1
    return 1


def _latest_adr_number(project_root):
    decisions = project_root / ".plans" / "agent-teams-v2" / "decisions.md"
    if not decisions.exists():
        return None
    try:
        content = decisions.read_text(encoding="utf-8", errors="ignore")
    except Exception:
        return None
    nums = [int(match) for match in re.findall(r"\bADR-(\d{3})\b", content)]
    return max(nums) if nums else None


# ---------------------------------------------------------------------------
# GR-1: File Size
# ---------------------------------------------------------------------------
def check_file_size(src_dirs, result, warn_limit=800, fail_limit=1200):
    """Files over warn_limit lines get WARN; over fail_limit get FAIL."""
    print("[GR-1] File Size Check")
    found = False
    files = {}
    for f in _iter_code_files(src_dirs):
        try:
            files[f.resolve()] = f
        except Exception:
            files[f] = f
    for f in _iter_web_file_size_files():
        try:
            files[f.resolve()] = f
        except Exception:
            files[f] = f
    for f in files.values():
        try:
            lines = len(f.read_text(encoding="utf-8", errors="ignore").splitlines())
        except Exception:
            continue
        if lines > fail_limit:
            result.fail("GR-FILE-SIZE", f"{f} -- {lines} lines (limit: {fail_limit})",
                        "Split into smaller modules. Extract helper functions or classes.")
            found = True
        elif lines > warn_limit:
            result.warn("GR-FILE-SIZE", f"{f} -- {lines} lines (limit: {warn_limit})",
                        "Consider splitting. Files over 800 lines are hard for agents to navigate.")
            found = True
    if not found:
        print("  [OK] All files within size limits.\n")


# ---------------------------------------------------------------------------
# GR-2: Hardcoded Secrets
# ---------------------------------------------------------------------------
SECRET_PATTERNS = [
    (r"""['"]sk-[a-zA-Z0-9]{20,}['"]""", "Possible OpenAI/Stripe API key"),
    (r"""['"]ghp_[a-zA-Z0-9]{30,}['"]""", "Possible GitHub personal access token"),
    (r"""['"]AKIA[A-Z0-9]{16}['"]""", "Possible AWS access key"),
    (r"""(?i)(password|secret|api_key|apikey|token)\s*[:=]\s*['"][^'"]{8,}['"]""",
     "Possible hardcoded secret"),
]

# Lines containing these markers are likely examples/placeholders, not real secrets
EXAMPLE_MARKERS = ("example", "placeholder", "your_key_here", "xxx", "changeme", "<your")


def check_secrets(src_dirs, result):
    """Scan for hardcoded secrets using regex patterns."""
    print("[GR-2] Hardcoded Secrets Check")
    found = False
    for f in _iter_code_files(src_dirs):
        try:
            content = f.read_text(encoding="utf-8", errors="ignore")
        except Exception:
            continue
        for i, line in enumerate(content.splitlines(), 1):
            stripped = line.strip()
            # Skip lines that are clearly examples/placeholders
            if any(marker in stripped.lower() for marker in EXAMPLE_MARKERS):
                continue
            for pattern, desc in SECRET_PATTERNS:
                if re.search(pattern, line):
                    result.fail("GR-SECRET", f"{f}:{i} -- {desc}",
                                "Move to environment variable. Never commit secrets to code.")
                    found = True
                    break  # one match per line is enough
    if not found:
        print("  [OK] No hardcoded secrets detected.\n")


# ---------------------------------------------------------------------------
# GR-3: No console.log in Production Code
# ---------------------------------------------------------------------------
CONSOLE_PATTERN = re.compile(r"\bconsole\.(log|debug|info|warn|error)\b")
TEST_DIR_NAMES = {"test", "tests", "__tests__", "spec", "scripts", "e2e", "cypress"}


def check_console_log(src_dirs, result):
    """Detect console.log in production code (not test files)."""
    print("[GR-3] Console Log Check")
    found = False
    for f in _iter_code_files(src_dirs):
        if f.suffix not in {".ts", ".tsx", ".js", ".jsx", ".vue", ".svelte"}:
            continue
        if any(part in TEST_DIR_NAMES for part in f.parts):
            continue
        try:
            content = f.read_text(encoding="utf-8", errors="ignore")
        except Exception:
            continue
        for i, line in enumerate(content.splitlines(), 1):
            if CONSOLE_PATTERN.search(line):
                stripped = line.strip()
                if stripped.startswith("//"):
                    continue
                result.warn("GR-CONSOLE", f"{f}:{i} -- {stripped[:80]}",
                            "Remove console.log from production code. Use a structured logger instead.")
                found = True
    if not found:
        print("  [OK] No console.log in production code.\n")


# ---------------------------------------------------------------------------
# GR-4: Doc Freshness (requires git)
# ---------------------------------------------------------------------------
def check_doc_freshness(docs_dir, src_dirs, result, stale_commit_threshold=10):
    """Compare docs/ last-modified commit vs source code commits.

    If source has N+ commits since docs were last touched, emit WARN.
    Requires git. Silently skips if git is not available or docs_dir missing.
    """
    print("[GR-4] Doc Freshness Check")
    docs_path = Path(docs_dir)
    if not docs_path.exists():
        print("  [SKIP] docs/ directory not found. Skipping freshness check.\n")
        return

    doc_files = {
        "api-contracts.md": "API contract",
        "architecture.md": "architecture",
        "invariants.md": "invariants",
    }

    found = False
    for doc_name, label in doc_files.items():
        doc_file = docs_path / doc_name
        if not doc_file.exists():
            continue
        try:
            last_doc_commit = subprocess.run(
                ["git", "log", "-1", "--format=%H", "--", str(doc_file)],
                capture_output=True, text=True, timeout=10
            ).stdout.strip()

            if not last_doc_commit:
                continue

            src_commits = 0
            for src_dir in src_dirs:
                if not Path(src_dir).exists():
                    continue
                count_result = subprocess.run(
                    ["git", "rev-list", "--count", f"{last_doc_commit}..HEAD", "--", src_dir],
                    capture_output=True, text=True, timeout=10
                )
                count = count_result.stdout.strip()
                if count.isdigit():
                    src_commits += int(count)

            if src_commits >= stale_commit_threshold:
                quoted_dirs = " ".join(f'"{d}"' for d in src_dirs)
                result.warn(
                    "GR-DOC-STALE",
                    f"{doc_file} -- {src_commits} source commits since last doc update",
                    f"Review and update {label} docs. Run: git log --oneline {last_doc_commit}..HEAD -- {quoted_dirs}")
                found = True
        except Exception:
            continue

    if not found:
        print("  [OK] All docs appear fresh.\n")


# ---------------------------------------------------------------------------
# GR-5: Invariant Coverage
# ---------------------------------------------------------------------------
def check_invariant_coverage(docs_dir, result):
    """Scan invariants.md for items marked 'no test' and report them."""
    print("[GR-5] Invariant Coverage Check")
    inv_file = Path(docs_dir) / "invariants.md"
    if not inv_file.exists():
        print("  [SKIP] docs/invariants.md not found. Skipping.\n")
        return

    try:
        content = inv_file.read_text(encoding="utf-8", errors="ignore")
    except Exception:
        print("  [SKIP] Could not read invariants.md.\n")
        return

    no_test_count = 0
    for i, line in enumerate(content.splitlines(), 1):
        if re.search(r"(?i)status:\s*no\s*test", line):
            result.info(
                "GR-INV-NO-TEST",
                f"docs/invariants.md:{i} -- Invariant without automated test: {line.strip()[:80]}",
                "Write an automated test for this invariant. Untested invariants rely on human memory.")
            no_test_count += 1

    if no_test_count == 0:
        print("  [OK] All invariants have test coverage (or no invariants defined).\n")
    else:
        print(f"  {no_test_count} invariant(s) without automated tests.\n")


# ---------------------------------------------------------------------------
# GR-6: CLAUDE.md / AGENTS.md Sync
# ---------------------------------------------------------------------------
def check_claude_agents_sync(result, project_root=None):
    """Ensure lead and worker instruction files stay byte-identical."""
    print("[GR-6] CLAUDE.md / AGENTS.md Sync Check")
    root = Path(project_root) if project_root else Path.cwd()
    claude_file = root / "CLAUDE.md"
    agents_file = root / "AGENTS.md"

    if not claude_file.exists() and not agents_file.exists():
        print("  [SKIP] CLAUDE.md and AGENTS.md not found. Skipping sync check.\n")
        return

    fix = "copy CLAUDE.md to AGENTS.md after intentionally updating the shared instructions."
    if not claude_file.exists() or not agents_file.exists():
        result.fail(
            "GR-INSTRUCTIONS-SYNC",
            "[CHECK] CLAUDE.md and AGENTS.md content drift + FIX: copy CLAUDE.md to AGENTS.md -- one file is missing",
            fix,
        )
        return

    try:
        claude_bytes = claude_file.read_bytes()
        agents_bytes = agents_file.read_bytes()
    except Exception as exc:
        result.fail(
            "GR-INSTRUCTIONS-SYNC",
            f"[CHECK] CLAUDE.md and AGENTS.md content drift + FIX: copy CLAUDE.md to AGENTS.md -- read failed: {exc}",
            fix,
        )
        return

    if claude_bytes != agents_bytes:
        result.fail(
            "GR-INSTRUCTIONS-SYNC",
            "[CHECK] CLAUDE.md and AGENTS.md content drift + FIX: copy CLAUDE.md to AGENTS.md",
            fix,
        )
        return

    print("  [OK] CLAUDE.md and AGENTS.md content match.\n")


# ---------------------------------------------------------------------------
# GR-7: Project docs/index.md latest ADR freshness
# ---------------------------------------------------------------------------
def check_docs_index_latest_adr(result, project_root=None):
    """Ensure docs/index.md mentions the latest ADR in decisions.md."""
    print("[GR-7] docs/index.md Latest ADR Check")
    root = Path(project_root) if project_root else _project_root()
    latest = _latest_adr_number(root)
    if latest is None:
        print("  [SKIP] No ADR numbers found in decisions.md. Skipping.\n")
        return

    adr = f"ADR-{latest:03d}"
    index_file = root / ".plans" / "agent-teams-v2" / "docs" / "index.md"
    if not index_file.exists():
        result.fail(
            "GR-DOCS-INDEX-ADR",
            f"[CHECK] docs index must mention latest ADR {adr} + File: {_rel(index_file, root)}:1",
            f"Add {adr} to .plans/agent-teams-v2/docs/index.md so navigation reflects the active ADR log.",
        )
        return

    try:
        content = index_file.read_text(encoding="utf-8", errors="ignore")
    except Exception as exc:
        result.fail(
            "GR-DOCS-INDEX-ADR",
            f"[CHECK] docs index must mention latest ADR {adr} + File: {_rel(index_file, root)}:1 -- read failed: {exc}",
            f"Make .plans/agent-teams-v2/docs/index.md readable and include {adr}.",
        )
        return

    if adr not in content:
        result.fail(
            "GR-DOCS-INDEX-ADR",
            f"[CHECK] docs index must mention latest ADR {adr} + File: {_rel(index_file, root)}:1",
            f"Add a current-decision note for {adr} to .plans/agent-teams-v2/docs/index.md.",
        )
        return

    print(f"  [OK] docs/index.md mentions {adr}.\n")


# ---------------------------------------------------------------------------
# GR-8: Main task_plan.md header latest ADR freshness
# ---------------------------------------------------------------------------
def check_main_task_plan_latest_adr(result, project_root=None):
    """Ensure main plan Status and Updated header lines mention the latest ADR."""
    print("[GR-8] Main task_plan.md Latest ADR Check")
    root = Path(project_root) if project_root else _project_root()
    latest = _latest_adr_number(root)
    if latest is None:
        print("  [SKIP] No ADR numbers found in decisions.md. Skipping.\n")
        return

    adr = f"ADR-{latest:03d}"
    plan_file = root / ".plans" / "agent-teams-v2" / "task_plan.md"
    if not plan_file.exists():
        result.fail(
            "GR-MAIN-PLAN-ADR",
            f"[CHECK] main task_plan header must mention latest ADR {adr} + File: {_rel(plan_file, root)}:1",
            f"Create .plans/agent-teams-v2/task_plan.md with Status and Updated lines mentioning {adr}.",
        )
        return

    try:
        lines = plan_file.read_text(encoding="utf-8", errors="ignore").splitlines()
    except Exception as exc:
        result.fail(
            "GR-MAIN-PLAN-ADR",
            f"[CHECK] main task_plan header must mention latest ADR {adr} + File: {_rel(plan_file, root)}:1 -- read failed: {exc}",
            f"Make .plans/agent-teams-v2/task_plan.md readable and mention {adr} in Status / Updated.",
        )
        return

    required_prefixes = ("> Status:", "> Updated:")
    found = False
    for prefix in required_prefixes:
        matching = [(idx, line) for idx, line in enumerate(lines, 1) if line.startswith(prefix)]
        if not matching:
            result.fail(
                "GR-MAIN-PLAN-ADR",
                f"[CHECK] main task_plan header must contain {prefix} with latest ADR {adr} + File: {_rel(plan_file, root)}:1",
                f"Add a {prefix} header line mentioning {adr}.",
            )
            found = True
            continue
        line_no, line = matching[0]
        if adr not in line:
            result.fail(
                "GR-MAIN-PLAN-ADR",
                f"[CHECK] main task_plan {prefix} line must mention latest ADR {adr} + File: {_rel(plan_file, root)}:{line_no}",
                f"Refresh the {prefix} line to mention {adr}.",
            )
            found = True

    if not found:
        print(f"  [OK] main task_plan Status / Updated mention {adr}.\n")


# ---------------------------------------------------------------------------
# GR-9: README default path must stay on HTTP service
# ---------------------------------------------------------------------------
README_STDIO_DEFAULT_PATTERNS = (
    "spawns team_mode_mcp via stdio",
    "cargo build --release --bin team_mode_mcp",
    "target/release/team_mode_mcp",
    '"command": "team_mode_mcp"',
    "team_mode_daemon(.exe) exist",
)


def _line_is_legacy_context(line):
    lowered = line.lower()
    return "fallback" in lowered or "legacy rollback" in lowered or "not default" in lowered


def check_readme_http_default_path(result, project_root=None):
    """Keep public README default install docs on the HTTP service path."""
    print("[GR-9] README HTTP Default Path Check")
    root = Path(project_root) if project_root else _project_root()
    readmes = [root / "README.md", root / "README.zh-CN.md"]
    required = ("team_mode_service", "http://127.0.0.1:8786/mcp", "team-mode-service.ps1 start")
    found_issue = False

    for readme in readmes:
        if not readme.exists():
            result.fail(
                "GR-README-HTTP-DEFAULT",
                f"[CHECK] README default run path must use HTTP service + File: {_rel(readme, root)}:1",
                "Restore README.md and README.zh-CN.md with team_mode_service, HTTP /mcp URL, and service start command.",
            )
            found_issue = True
            continue
        try:
            lines = readme.read_text(encoding="utf-8", errors="ignore").splitlines()
        except Exception as exc:
            result.fail(
                "GR-README-HTTP-DEFAULT",
                f"[CHECK] README default run path must use HTTP service + File: {_rel(readme, root)}:1 -- read failed: {exc}",
                "Make the README readable and restore the HTTP service default path.",
            )
            found_issue = True
            continue
        content = "\n".join(lines)
        for token in required:
            if token not in content:
                result.fail(
                    "GR-README-HTTP-DEFAULT",
                    f"[CHECK] README default run path missing `{token}` + File: {_rel(readme, root)}:1",
                    "Document team_mode_service, http://127.0.0.1:8786/mcp, and scripts/team-mode-service.ps1 start as the default path.",
                )
                found_issue = True
        for i, line in enumerate(lines, 1):
            lowered = line.lower()
            if any(pattern.lower() in lowered for pattern in README_STDIO_DEFAULT_PATTERNS):
                if not _line_is_legacy_context(line):
                    result.fail(
                        "GR-README-HTTP-DEFAULT",
                        f"[CHECK] README default run path must not revert to stdio team_mode_mcp + File: {_rel(readme, root)}:{i}",
                        "Move this stdio path under an explicit legacy rollback / fallback note, or replace it with team_mode_service HTTP setup.",
                    )
                    found_issue = True

    if not found_issue:
        print("  [OK] READMEs keep HTTP service as the default path.\n")


# ---------------------------------------------------------------------------
# GR-10: Scratch artifacts must not be tracked or unignored
# ---------------------------------------------------------------------------
SCRATCH_ARTIFACT_PATTERNS = (
    re.compile(r"(^|/)\.async-wake-probe\.log$"),
    re.compile(r"(^|/)\.mid-turn-probe\.log$"),
    re.compile(r"(^|/)\.mcp-launcher\.log$"),
    re.compile(r"(^|/)dashboard.*\.png$", re.IGNORECASE),
    re.compile(r"(^|/)responsive.*\.png$", re.IGNORECASE),
    re.compile(r"(^|/)team-mode.*\.png$", re.IGNORECASE),
    re.compile(r"(^|/)备忘\.txt$"),
    re.compile(r"(^|/)%SystemDrive%(/|$)", re.IGNORECASE),
    re.compile(r"(^|/)\.agent-teams-mcp(/|$)"),
)


def _git_list_repo_visible_files(root):
    try:
        proc = subprocess.run(
            ["git", "ls-files", "--cached", "--others", "--exclude-standard"],
            cwd=root,
            capture_output=True,
            text=True,
            timeout=10,
        )
    except Exception:
        return None
    if proc.returncode != 0:
        return None
    return [line.strip().replace("\\", "/") for line in proc.stdout.splitlines() if line.strip()]


def check_no_scratch_artifacts(result, project_root=None):
    """Reject probe logs, ad-hoc screenshots, and runtime scratch that Git would include."""
    print("[GR-10] Scratch Artifact Check")
    root = Path(project_root) if project_root else _project_root()
    files = _git_list_repo_visible_files(root)
    if files is None:
        print("  [SKIP] git ls-files unavailable. Skipping scratch artifact check.\n")
        return

    found = False
    for rel_path in files:
        if any(pattern.search(rel_path) for pattern in SCRATCH_ARTIFACT_PATTERNS):
            line = _line_number_for_text(root / rel_path, "")
            result.fail(
                "GR-SCRATCH-ARTIFACT",
                f"[CHECK] probe logs, screenshots, and runtime scratch must not be committed + File: {rel_path}:{line}",
                "Remove the artifact from the working tree or add a precise .gitignore rule before committing.",
            )
            found = True
    if not found:
        print("  [OK] No tracked or unignored scratch artifacts detected.\n")


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------
def check_all(src_dirs, docs_dir=None):
    """Run all golden rule checks. Returns (fail_count, warn_count, info_count)."""
    result = CheckResult()

    print("=" * 60)
    print("Golden Rules Check")
    print("=" * 60 + "\n")

    check_file_size(src_dirs, result)
    check_secrets(src_dirs, result)
    check_console_log(src_dirs, result)

    if docs_dir:
        check_doc_freshness(docs_dir, src_dirs, result)
        check_invariant_coverage(docs_dir, result)
    check_claude_agents_sync(result)
    check_docs_index_latest_adr(result)
    check_main_task_plan_latest_adr(result)
    check_readme_http_default_path(result)
    check_no_scratch_artifacts(result)

    print("=" * 60)
    print(f"Golden Rules Summary: {result.fails} FAIL, {result.warns} WARN, {result.infos} INFO")
    if result.fails > 0:
        print("Result: FAILED -- fix FAIL items before proceeding.")
    elif result.warns > 0:
        print("Result: PASSED with warnings -- review WARN items.")
    else:
        print("Result: PASSED")
    print("=" * 60)

    return result.fails, result.warns, result.infos


# ---------------------------------------------------------------------------
# CLI entry point
# ---------------------------------------------------------------------------
if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("Usage: python golden_rules.py <src_dir1> [src_dir2] ... [--docs <docs_dir>]")
        print("Example: python golden_rules.py src/ --docs .plans/myproject/docs")
        sys.exit(2)

    args = sys.argv[1:]
    docs = None
    src = []
    i = 0
    while i < len(args):
        if args[i] == "--docs" and i + 1 < len(args):
            docs = args[i + 1]
            i += 2
        else:
            src.append(args[i])
            i += 1

    fails, warns, infos = check_all(src, docs_dir=docs)
    sys.exit(1 if fails > 0 else 0)
