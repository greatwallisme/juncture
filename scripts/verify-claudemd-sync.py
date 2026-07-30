#!/usr/bin/env python3
"""
CLAUDE.md-Sync Verification Script for Juncture

Mechanically verifies that the 11 CLAUDE.md docs match the current HEAD API:
feature gates, build/test commands, fallible provider constructors, module
references, the NonZeroUsize cache signature, and file presence.

Bound to `.planning/2026-07-30-claudemd-sync/claudemd-sync.spec.md`. Pure text/structural checks -- the
failure path *is* the doc drift itself.

Usage:
    python3 scripts/verify-claudemd-sync.py

Exit codes:
    0 - All invariants hold
    1 - At least one invariant is violated (docs are out of sync)
    2 - Script error (files missing, etc.)
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

CRATES = ["juncture", "juncture-core", "juncture-derive",
          "juncture-checkpoint", "juncture-tracing", "juncture-store"]

CLAUDEMD_FILES = [
    "CLAUDE.md",
    "crates/juncture/CLAUDE.md",
    "crates/juncture-core/CLAUDE.md",
    "crates/juncture-derive/CLAUDE.md",
    "crates/juncture-checkpoint/CLAUDE.md",
    "crates/juncture-tracing/CLAUDE.md",
    "crates/juncture-store/CLAUDE.md",
    "benchmarks/CLAUDE.md",
    "examples/CLAUDE.md",
    "examples/deep-research/CLAUDE.md",
    ".claude/CLAUDE.md",
]

PROVIDER_PATTERNS = ["ChatOpenAI::new(", "ChatAnthropic::new(", "ChatOllama::new("]


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def extract_section(text: str, header: str) -> str:
    out: list[str] = []
    cap = False
    for line in text.splitlines():
        if line.startswith("## "):
            if cap:
                break
            cap = line == header
        elif cap:
            out.append(line)
    return "\n".join(out)


def extract_toml_table(text: str, table: str) -> str:
    """Return the body of a TOML `[table]` (lines until the next table header)."""
    header = f"[{table}]"
    out: list[str] = []
    cap = False
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("[") and not stripped.startswith("[["):
            if cap:
                break
            cap = stripped == header
        elif cap:
            out.append(line)
    return "\n".join(out)


def _lint_group_is_warn(block: str, group: str) -> bool:
    """True if `group` is enabled at warn level, in either table form
    (`group = { level = "warn", ... }`) or string form (`group = "warn"`)."""
    if re.search(rf'^{group}\s*=\s*\{{[^}}]*"warn"', block, re.M):
        return True
    return bool(re.search(rf'^{group}\s*=\s*"warn"', block, re.M))


def cargo_feature_keys(cargo_text: str) -> set[str]:
    keys: set[str] = set()
    in_features = False
    for line in cargo_text.splitlines():
        stripped = line.lstrip()
        if stripped.startswith("["):
            in_features = stripped == "[features]"
            continue
        if in_features and "=" in stripped:
            key = stripped.split("=", 1)[0].strip()
            if key and key != "default":
                keys.add(key)
    return keys


def doc_feature_keys(features_section: str) -> set[str]:
    keys: set[str] = set()
    for line in features_section.splitlines():
        m = re.match(r"- `([a-z0-9-]+)`", line.lstrip())
        if m:
            keys.add(m.group(1))
    return keys


def check_features_match_cargo() -> tuple[bool, str]:
    cargo = read(REPO / "crates" / "juncture" / "Cargo.toml")
    expected = cargo_feature_keys(cargo)
    md = read(REPO / "crates" / "juncture" / "CLAUDE.md")
    documented = doc_feature_keys(extract_section(md, "## Features"))
    if "test-util" not in documented:
        return False, f"facade Features missing default-off `test-util` (got {sorted(documented)})"
    if expected != documented:
        return False, f"Features mismatch: cargo={sorted(expected)} doc={sorted(documented)}"
    return True, f"facade Features == Cargo.toml [{len(expected)} keys incl test-util]"


def check_tracing_test_util() -> tuple[bool, str]:
    md = read(REPO / "crates" / "juncture-tracing" / "CLAUDE.md")
    section = extract_section(md, "## Features")
    if "test-util" not in section:
        return False, "tracing Features missing `test-util`"
    if "TestMetricsCollector" not in section:
        return False, "tracing Features missing `TestMetricsCollector` test-util gating"
    return True, "tracing Features documents test-util + TestMetricsCollector gating"


def check_build_commands() -> tuple[bool, str]:
    md = read(REPO / "CLAUDE.md")
    required = [
        "cargo build --workspace --all-features",
        "cargo test --workspace --all-features",
        "cargo clippy --workspace --all-targets --all-features -- -D warnings",
    ]
    missing = [c for c in required if c not in md]
    if missing:
        return False, f"root CLAUDE.md missing commands: {missing}"
    return True, "root CLAUDE.md Build & Test has build/test(--all-features)/clippy(-D warnings)"


def check_provider_new_result() -> tuple[bool, str]:
    md = read(REPO / "crates" / "juncture" / "CLAUDE.md")
    offenders: list[str] = []
    for line in md.splitlines():
        for pat in PROVIDER_PATTERNS:
            idx = line.find(pat)
            if idx == -1:
                continue
            if "-> Result" in line:
                continue
            after = line[idx + len(pat):]
            q = after.find("?")
            w = after.find(".with_")
            if q == -1:
                offenders.append(f"not propagated: {line.strip()}")
            elif w != -1 and w < q:
                offenders.append(f"bare new().with_ chain: {line.strip()}")
    if offenders:
        return False, "; ".join(offenders)
    return True, "facade CLAUDE.md provider constructors use fallible `new()?`"


def check_module_refs() -> tuple[bool, str]:
    problems: list[str] = []
    for crate in CRATES:
        md_path = REPO / "crates" / crate / "CLAUDE.md"
        if not md_path.is_file():
            continue
        md = read(md_path)
        refs = set(re.findall(r"`([A-Za-z0-9_/-]+\.rs)`", md))
        crate_dir = REPO / "crates" / crate
        existing = {p.name for p in crate_dir.rglob("*.rs") if p.is_file()}
        for ref in refs:
            base = ref.rsplit("/", 1)[-1]
            if base not in existing:
                problems.append(f"{crate}/CLAUDE.md: `{ref}` (no {base} under crates/{crate})")
    core_md = read(REPO / "crates" / "juncture-core" / "CLAUDE.md")
    if "remote.rs" not in core_md:
        problems.append("juncture-core/CLAUDE.md does not list `remote.rs` (RemoteGraph)")
    if problems:
        return False, "; ".join(problems)
    return True, "all crate CLAUDE.md .rs module refs resolve; core lists remote.rs"


def check_cache_nonzero() -> tuple[bool, str]:
    md = read(REPO / "crates" / "juncture-checkpoint" / "CLAUDE.md")
    if "MemoryCache::new" not in md:
        return False, "checkpoint CLAUDE.md missing `MemoryCache::new`"
    if "NonZeroUsize" not in md:
        return False, "checkpoint CLAUDE.md missing `NonZeroUsize` capacity contract"
    return True, "checkpoint CLAUDE.md documents MemoryCache::new(NonZeroUsize)"


def check_files_present() -> tuple[bool, str]:
    missing = [r for r in CLAUDEMD_FILES if not (REPO / r).is_file()]
    if missing:
        return False, f"missing: {missing}"
    return True, f"all {len(CLAUDEMD_FILES)} CLAUDE.md files present"


def check_clippy_lint_config() -> tuple[bool, str]:
    """The docs claim clippy enables the `all`/`pedantic`/`nursery`/`cargo`
    groups wholesale plus a *curated subset* of `restriction` lints (not the
    whole restriction group). Assert Cargo.toml matches that posture and that
    CLAUDE.md points at the lint table as source of truth -- this is the exact
    drift that previously had the doc claiming wholesale `restriction`."""
    cargo = read(REPO / "Cargo.toml")
    block = extract_toml_table(cargo, "workspace.lints.clippy")
    if not block:
        return False, "root Cargo.toml missing [workspace.lints.clippy]"
    missing = [g for g in ("all", "pedantic", "nursery", "cargo")
               if not _lint_group_is_warn(block, g)]
    if missing:
        return False, f"[workspace.lints.clippy] missing warn-level groups: {missing}"
    if re.search(r'^restriction\s*=\s*\{', block, re.M) or re.search(r'^restriction\s*=\s*"warn"', block, re.M):
        return False, "[workspace.lints.clippy] enables `restriction` wholesale; docs claim only a curated subset"
    individual = re.findall(r'^([a-z0-9_]+)\s*=\s*"warn"', block, re.M)
    if len(individual) < 5:
        return False, f"[workspace.lints.clippy] has too few individual warn lints: {individual}"
    if "[workspace.lints.clippy]" not in read(REPO / "CLAUDE.md"):
        return False, "root CLAUDE.md lint line must reference `[workspace.lints.clippy]` as source of truth"
    return True, (f"clippy groups all/pedantic/nursery/cargo + {len(individual)} curated "
                  f"restriction lints; CLAUDE.md points to Cargo.toml")


CHECKS = [
    ("facade Features match Cargo.toml", check_features_match_cargo),
    ("tracing test-util documented", check_tracing_test_util),
    ("root build/test commands current", check_build_commands),
    ("provider new() is Result", check_provider_new_result),
    ("crate module refs resolve", check_module_refs),
    ("checkpoint MemoryCache NonZeroUsize", check_cache_nonzero),
    ("clippy lint config matches docs", check_clippy_lint_config),
    ("CLAUDE.md files present", check_files_present),
]


def main() -> int:
    emit = lambda m: sys.stdout.write(m + "\n")
    emit("JUNCTURE CLAUDEMD-SYNC VERIFICATION")
    emit("=" * 60)
    failures = 0
    for name, fn in CHECKS:
        try:
            ok, detail = fn()
        except Exception as exc:  # noqa: BLE001 -- surface any check error as a failure
            ok, detail = False, f"check error: {exc}"
        marker = "PASS" if ok else "FAIL"
        emit(f"[{marker}] {name}: {detail}")
        if not ok:
            failures += 1
    emit("=" * 60)
    emit(f"Total: {len(CHECKS)}, Passed: {len(CHECKS) - failures}, Failed: {failures}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
