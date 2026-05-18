#!/usr/bin/env python3
"""
Design Coverage Verification Script for Juncture

Mechanically verifies that Rust implementation matches design documents
by checking each checklist item against the source code.

Usage:
    python3 scripts/verify-design-coverage.py [--src-dir SRC] [--checklists-dir CHECKLISTS] [--json] [--summary-only] [--by-finding]

Exit codes:
    0 - All items verified (or no checklist files found)
    1 - Some items not found in source code
    2 - Script error (missing dirs, bad JSON, etc.)
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from dataclasses import dataclass, field
from pathlib import Path


# ── Output helpers (stdout/stderr writers) ──────────────────────────────

def emit(message: str) -> None:
    sys.stdout.write(message + "\n")


def emit_err(message: str) -> None:
    sys.stderr.write(message + "\n")


# ── Data model ──────────────────────────────────────────────────────────

@dataclass
class CheckItem:
    id: str
    category: str
    parent: str | None
    name: str
    required_methods: list[str]
    required_fields: list[str]
    required_variants: list[str]
    findings: list[str]
    description: str
    verification_pattern: str
    source_doc: str = ""
    module: str = ""
    found: bool = False
    details: list[str] = field(default_factory=list)


@dataclass
class CheckResult:
    total: int = 0
    found: int = 0
    missing: int = 0
    items: list[CheckItem] = field(default_factory=list)
    by_doc: dict[str, list[CheckItem]] = field(default_factory=dict)
    by_finding: dict[str, list[CheckItem]] = field(default_factory=dict)


# ── Source code scanning ────────────────────────────────────────────────

def collect_rust_sources(src_dir: Path) -> str:
    """Read all .rs files into one big string for regex searching."""
    parts: list[str] = []
    for rs_file in sorted(src_dir.rglob("*.rs")):
        try:
            content = rs_file.read_text(encoding="utf-8", errors="replace")
            rel = rs_file.relative_to(src_dir)
            for i, line in enumerate(content.splitlines(), 1):
                parts.append(f"{rel}:{i}: {line}")
        except OSError:
            continue
    return "\n".join(parts)


def verify_item(item: CheckItem, source_text: str) -> None:
    """Check a single item against the source code text."""
    pattern = item.verification_pattern.strip()
    if not pattern:
        item.found = False
        item.details.append("No verification pattern defined")
        return

    try:
        matches = re.findall(pattern, source_text, re.MULTILINE)
    except re.error as e:
        item.found = False
        item.details.append(f"Invalid regex: {e}")
        return

    if not matches:
        item.found = False
        item.details.append(f"Pattern not found: {pattern}")
        return

    item.found = True
    match_count = len(matches)
    item.details.append(f"Found {match_count} match(es) for main pattern")

    for method in item.required_methods:
        method_pattern = rf"fn\s+{re.escape(method)}\s*[<(]"
        method_matches = re.findall(method_pattern, source_text, re.MULTILINE)
        if method_matches:
            item.details.append(f"  method '{method}': found {len(method_matches)}")
        else:
            item.found = False
            item.details.append(f"  method '{method}': MISSING")

    for field_spec in item.required_fields:
        field_name = field_spec.split(":")[0].strip()
        field_pattern = rf"(?:pub\s+)?{re.escape(field_name)}\s*:"
        field_matches = re.findall(field_pattern, source_text, re.MULTILINE)
        if field_matches:
            item.details.append(f"  field '{field_name}': found {len(field_matches)}")
        else:
            item.found = False
            item.details.append(f"  field '{field_name}': MISSING")

    for variant in item.required_variants:
        variant_pattern = rf"\b{re.escape(variant)}\b"
        variant_matches = re.findall(variant_pattern, source_text, re.MULTILINE)
        if variant_matches:
            item.details.append(f"  variant '{variant}': found {len(variant_matches)}")
        else:
            item.found = False
            item.details.append(f"  variant '{variant}': MISSING")


# ── Checklist loading ───────────────────────────────────────────────────

def load_checklists(checklists_dir: Path) -> list[CheckItem]:
    """Load all checklist JSON files."""
    items: list[CheckItem] = []

    if not checklists_dir.exists():
        emit_err(f"ERROR: Checklists directory not found: {checklists_dir}")
        sys.exit(2)

    json_files = sorted(checklists_dir.glob("*.json"))
    if not json_files:
        emit_err(f"WARNING: No checklist files found in {checklists_dir}")
        return items

    for jf in json_files:
        try:
            data = json.loads(jf.read_text(encoding="utf-8"))
        except (json.JSONDecodeError, OSError) as e:
            emit_err(f"ERROR: Failed to load {jf}: {e}")
            sys.exit(2)

        source_doc = data.get("source_doc", jf.stem + ".md")
        module = data.get("module", "")

        for raw in data.get("items", []):
            item = CheckItem(
                id=raw.get("id", "UNKNOWN"),
                category=raw.get("category", "unknown"),
                parent=raw.get("parent"),
                name=raw.get("name", "UNKNOWN"),
                required_methods=raw.get("required_methods", []),
                required_fields=raw.get("required_fields", []),
                required_variants=raw.get("required_variants", []),
                findings=raw.get("findings", []),
                description=raw.get("description", ""),
                verification_pattern=raw.get("verification_pattern", ""),
                source_doc=source_doc,
                module=module,
            )
            items.append(item)

    return items


# ── Report formatting ───────────────────────────────────────────────────

def fmt_status(found: bool) -> str:
    return "PASS" if found else "FAIL"


def report_text(result: CheckResult, summary_only: bool) -> str:
    lines: list[str] = []
    pct = (result.found / result.total * 100) if result.total > 0 else 0.0

    lines.append("=" * 72)
    lines.append("JUNCTURE DESIGN COVERAGE REPORT")
    lines.append("=" * 72)
    lines.append(f"Total items: {result.total}")
    lines.append(f"Verified:    {result.found} ({pct:.1f}%)")
    lines.append(f"Missing:     {result.missing}")
    lines.append("=" * 72)

    if summary_only:
        return "\n".join(lines)

    for doc in sorted(result.by_doc.keys()):
        items = result.by_doc[doc]
        doc_found = sum(1 for i in items if i.found)
        doc_total = len(items)
        doc_pct = (doc_found / doc_total * 100) if doc_total > 0 else 0.0

        lines.append(f"\n--- {doc} ({doc_found}/{doc_total} = {doc_pct:.1f}%) ---")

        for item in items:
            status = fmt_status(item.found)
            lines.append(f"  [{status}] {item.id} {item.category:8s} {item.name}")
            if not item.found and item.details:
                for d in item.details:
                    if d.startswith("  "):
                        lines.append(f"         {d}")
                    else:
                        lines.append(f"         -> {d}")

    return "\n".join(lines)


def report_json(result: CheckResult) -> str:
    output = {
        "total": result.total,
        "found": result.found,
        "missing": result.missing,
        "coverage_pct": round(result.found / result.total * 100, 2) if result.total > 0 else 0.0,
        "items": [
            {
                "id": item.id,
                "category": item.category,
                "parent": item.parent,
                "name": item.name,
                "source_doc": item.source_doc,
                "findings": item.findings,
                "found": item.found,
                "details": item.details,
            }
            for item in result.items
        ],
    }
    return json.dumps(output, indent=2, ensure_ascii=False)


def report_by_finding(result: CheckResult) -> str:
    """Report grouped by finding ID for traceability."""
    lines: list[str] = []

    lines.append("=" * 72)
    lines.append("JUNCTURE FINDING VERIFICATION REPORT")
    lines.append("=" * 72)

    all_findings: dict[str, list[CheckItem]] = {}
    untracked: list[CheckItem] = []

    for item in result.items:
        if item.findings:
            for f in item.findings:
                all_findings.setdefault(f, []).append(item)
        else:
            untracked.append(item)

    for finding_id in sorted(all_findings.keys()):
        items = all_findings[finding_id]
        all_found = all(i.found for i in items)
        status = "PASS" if all_found else "FAIL"
        lines.append(f"\n[{status}] {finding_id}")
        for item in items:
            s = fmt_status(item.found)
            lines.append(f"  [{s}] {item.id} {item.name} ({item.category})")
            if not item.found:
                for d in item.details:
                    lines.append(f"       {d}")

    if untracked:
        lines.append(f"\n--- Items without finding IDs ({len(untracked)}) ---")
        for item in untracked:
            s = fmt_status(item.found)
            lines.append(f"  [{s}] {item.id} {item.name}")

    return "\n".join(lines)


# ── Main ────────────────────────────────────────────────────────────────

def main() -> int:
    parser = argparse.ArgumentParser(
        description="Verify Juncture Rust implementation against design checklists"
    )
    parser.add_argument(
        "--src-dir",
        type=Path,
        default=Path("src"),
        help="Rust source directory (default: src)",
    )
    parser.add_argument(
        "--checklists-dir",
        type=Path,
        default=Path("design/checklists"),
        help="Checklists directory (default: design/checklists)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Output JSON format",
    )
    parser.add_argument(
        "--summary-only",
        action="store_true",
        help="Only show summary statistics",
    )
    parser.add_argument(
        "--by-finding",
        action="store_true",
        help="Group report by finding ID for traceability",
    )
    args = parser.parse_args()

    project_root = Path(__file__).resolve().parent.parent
    src_dir = (project_root / args.src_dir).resolve()
    checklists_dir = (project_root / args.checklists_dir).resolve()

    items = load_checklists(checklists_dir)
    if not items:
        emit("No checklist items to verify. Nothing to do.")
        return 0

    if not src_dir.exists():
        emit_err(f"WARNING: Source directory not found: {src_dir}")
        emit_err("All items will be marked as missing (source not yet implemented).")
        source_text = ""
    else:
        rs_count = len(list(src_dir.rglob("*.rs")))
        emit_err(f"Scanning {rs_count} Rust source files in {src_dir}...")
        source_text = collect_rust_sources(src_dir)

    result = CheckResult()
    for item in items:
        if source_text:
            verify_item(item, source_text)
        else:
            item.found = False
            item.details.append("Source directory not found - not yet implemented")

        result.items.append(item)
        result.total += 1
        if item.found:
            result.found += 1
        else:
            result.missing += 1

        result.by_doc.setdefault(item.source_doc, []).append(item)
        for f in item.findings:
            result.by_finding.setdefault(f, []).append(item)

    if args.json:
        emit(report_json(result))
    elif args.by_finding:
        emit(report_by_finding(result))
    else:
        emit(report_text(result, args.summary_only))

    return 1 if result.missing > 0 and src_dir.exists() else 0


if __name__ == "__main__":
    sys.exit(main())
