#!/usr/bin/env python3
"""
Doc-Sync Verification Script for Juncture

Mechanically verifies that the `doc/` tutorials match the current HEAD API:
fallible provider constructors, public-item references, en/zh structural parity,
valid feature flags, getting-started commands, and new-feature coverage.

Bound to `.planning/2026-07-30-doc-tutorials-sync/doc-tutorials-sync.spec.md`. Pure text/structural checks --
the failure path *is* the doc drift itself.

Usage:
    python3 scripts/verify-doc-sync.py

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
DOC = REPO / "doc"

FEATURE_VOCAB = [
    "anthropic",
    "openai",
    "ollama",
    "sqlite",
    "postgres",
    "otel",
    "store",
    "wasm",
    "test-util",
]

CURATED_ITEMS = [
    "MessagesState",
    "create_react_agent",
    "create_agent",
    "StateGraph",
    "Message",
    "human",
    "ChatOpenAI",
    "ChatModel",
    "SubagentTool",
    "MemorySaver",
    "ToolNode",
    "RemoteGraph",
    "GraphOutput",
    "Command",
    "entrypoint",
    "task",
]

DOC_FILES = [
    "doc/README.md",
    "doc/en/getting-started.md",
    "doc/en/core-concepts.md",
    "doc/en/advanced-features.md",
    "doc/en/examples-guide.md",
    "doc/zh/getting-started.md",
    "doc/zh/core-concepts.md",
    "doc/zh/advanced-features.md",
    "doc/zh/examples-guide.md",
]


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8")


def collect(root: Path, ext: str) -> list[Path]:
    return sorted(p for p in root.rglob(f"*.{ext}") if p.is_file())


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


def token_in_pub_lines(name: str, pub_lines: list[str]) -> bool:
    for line in pub_lines:
        for tok in re.split(r"[^A-Za-z0-9_]+", line):
            if tok == name:
                return True
    return False


def pub_lines(files: list[Path]) -> list[str]:
    out: list[str] = []
    for f in files:
        for line in read(f).splitlines():
            stripped = line.lstrip()
            if stripped.startswith("pub ") or stripped.startswith("pub("):
                out.append(line)
    return out


def mentions_feature(text: str, feature: str) -> bool:
    if "-" in feature:
        return feature in text
    return feature in re.split(r"[^A-Za-z0-9_]+", text)


def check_provider_new() -> tuple[bool, str]:
    patterns = ["ChatOpenAI::new(", "ChatAnthropic::new(", "ChatOllama::new("]
    offenders: list[str] = []
    for path in collect(DOC, "md"):
        for line in read(path).splitlines():
            for pat in patterns:
                idx = line.find(pat)
                if idx == -1:
                    continue
                after = line[idx + len(pat):]
                q = after.find("?")
                w = after.find(".with_")
                propagated = q != -1 or "-> Result" in line or ".expect" in line
                if not propagated:
                    offenders.append(f"{path}: not propagated: {line.strip()}")
                elif q != -1 and w != -1 and w < q:
                    offenders.append(f"{path}: bare new().with_ chain: {line.strip()}")
    if offenders:
        return False, "; ".join(offenders)
    return True, "all provider `new()` calls propagated with `?`"


def check_referenced_items() -> tuple[bool, str]:
    rs_files: list[Path] = []
    for crate in ["juncture", "juncture-core", "juncture-derive",
                  "juncture-checkpoint", "juncture-tracing", "juncture-store"]:
        rs_files += collect(REPO / "crates" / crate / "src", "rs")
    all_pub = pub_lines(rs_files)
    facade_pub = pub_lines([REPO / "crates" / "juncture" / "src" / "lib.rs"])

    missing_curated = [n for n in CURATED_ITEMS if not token_in_pub_lines(n, all_pub)]

    segments: set[str] = set()
    for path in collect(DOC, "md"):
        segments.update(m.group(1) for m in re.finditer(r"juncture::([A-Za-z_][A-Za-z0-9_]*)", read(path)))
    missing_seg = [s for s in sorted(segments) if not token_in_pub_lines(s, facade_pub)]

    if missing_curated or missing_seg:
        parts = []
        if missing_curated:
            parts.append(f"not public: {missing_curated}")
        if missing_seg:
            parts.append(f"facade segs not public: {missing_seg}")
        return False, "; ".join(parts)
    return True, f"{len(CURATED_ITEMS)} curated items + {len(segments)} juncture:: segments all public"


def check_parity() -> tuple[bool, str]:
    en = {p.name for p in (DOC / "en").glob("*.md")}
    zh = {p.name for p in (DOC / "zh").glob("*.md")}
    if en != zh:
        return False, f"file sets differ: en-only={sorted(en - zh)}, zh-only={sorted(zh - en)}"
    mismatches: list[str] = []
    for name in sorted(en):
        en_text = read(DOC / "en" / name)
        zh_text = read(DOC / "zh" / name)
        en_h2 = sum(1 for l in en_text.splitlines() if l.startswith("## "))
        zh_h2 = sum(1 for l in zh_text.splitlines() if l.startswith("## "))
        en_rust = sum(1 for l in en_text.splitlines() if l.startswith("```rust"))
        zh_rust = sum(1 for l in zh_text.splitlines() if l.startswith("```rust"))
        if en_h2 != zh_h2:
            mismatches.append(f"{name}: ## {en_h2}!={zh_h2}")
        if en_rust != zh_rust:
            mismatches.append(f"{name}: ```rust {en_rust}!={zh_rust}")
    if mismatches:
        return False, "; ".join(mismatches)
    return True, f"{len(en)} en/zh file pairs structurally equal (## + ```rust)"


def check_features() -> tuple[bool, str]:
    valid: set[str] = set()
    for crate in ["juncture", "juncture-core", "juncture-derive",
                  "juncture-checkpoint", "juncture-tracing", "juncture-store"]:
        cargo = REPO / "crates" / crate / "Cargo.toml"
        if cargo.is_file():
            valid |= cargo_feature_keys(read(cargo))
    invalid: list[str] = []
    for path in collect(DOC, "md"):
        text = read(path)
        for feature in FEATURE_VOCAB:
            if mentions_feature(text, feature) and feature not in valid:
                invalid.append(f"{path}: `{feature}`")
    if invalid:
        return False, f"features not in any Cargo.toml: {invalid}"
    return True, "all doc-mentioned features are valid Cargo.toml keys"


def check_getting_started() -> tuple[bool, str]:
    missing: list[str] = []
    for lang in ["en", "zh"]:
        text = read(DOC / lang / "getting-started.md")
        for needle in ["cargo run -p juncture-simple-example", "OPENAI_API_KEY", ".env"]:
            if needle not in text:
                missing.append(f"{lang}: `{needle}`")
    if missing:
        return False, "; ".join(missing)
    return True, "getting-started (en+zh) has package + env commands"


def check_advanced_features() -> tuple[bool, str]:
    missing: list[str] = []
    for lang in ["en", "zh"]:
        text = read(DOC / lang / "advanced-features.md")
        if "#[entrypoint]" not in text and "#[task]" not in text:
            missing.append(f"{lang}: macros")
        if "RemoteGraph" not in text:
            missing.append(f"{lang}: RemoteGraph")
    if missing:
        return False, "; ".join(missing)
    return True, "advanced-features (en+zh) covers #[task]/#[entrypoint] + RemoteGraph"


def check_files_present() -> tuple[bool, str]:
    missing = [r for r in DOC_FILES if not (REPO / r).is_file()]
    if missing:
        return False, f"missing: {missing}"
    return True, f"all {len(DOC_FILES)} doc files present"


CHECKS = [
    ("doc provider new() uses Result", check_provider_new),
    ("doc referenced items exist", check_referenced_items),
    ("doc en/zh parity", check_parity),
    ("doc features match Cargo.toml", check_features),
    ("getting-started commands current", check_getting_started),
    ("advanced-features covers new macros + RemoteGraph", check_advanced_features),
    ("doc files present", check_files_present),
]


def main() -> int:
    emit = lambda m: sys.stdout.write(m + "\n")
    emit("JUNCTURE DOC-SYNC VERIFICATION")
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
