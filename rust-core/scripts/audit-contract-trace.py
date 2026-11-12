#!/usr/bin/env python3
"""Audit contract -> matrix -> draft/manuscript -> status traceability.

This is a read-only consistency check. It never upgrades a status claim into
implementation evidence. It proves only that:

* every F01-F73 has one matrix entry and is named by the first cell of a
  live CONTRACT-STATUS row (mentions in evidence/remaining are references only);
* every D/P section reference in the matrix resolves to a real heading in the
  frozen draft or manuscript (so the contract still points at the paper);
* every test/source path cited by CONTRACT-STATUS.md or LEAN-ACCEPTANCE.md exists
  inside rust-core/ (a dangling citation is a documentation error, not a pass);
* the optional public paper copy still matches the manuscript hash.

The JSON report also summarises the live table per feature (claim, dedicated or
grouped row, remaining text) and buckets ids by declared status. The summary is
a navigation aid for gap targeting; it never upgrades a claim into evidence.

Exit 0 when no errors; warnings do not fail the run.
"""

import hashlib
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CONTRACT = ROOT / "paper/review/sic_implementation_contract.md"
MATRIX = ROOT / "paper/review/sic_feature_acceptance_matrix.json"
STATUS = ROOT / "rust-core/CONTRACT-STATUS.md"
LEAN = ROOT / "rust-core/LEAN-ACCEPTANCE.md"
PUBLIC_PAPER = ROOT / "paper/sic_paper_zh_v1.md"
SOURCES = {
    "D": ROOT / "paper/draft/sic_draft_working.md",
    "P": ROOT / "paper/review/sic_manuscript_review_zh.md",
}
EXPECTED = [f"F{i:02d}" for i in range(1, 74)]
REQUIRED_FEATURE_FIELDS = (
    "id", "title", "inputs", "operation", "outputs", "normal_steps",
    "pass_conditions", "negative_cases", "required_evidence", "sources",
)
HEADING = re.compile(r"^(#{1,6})\s+(\d+(?:\.\d+)*)\.?\s*(.*)$")
# A matrix source may cite one section, a list ("§2.1、§2.3、§6") or a range
# ("§3.1–3.4") with an author label that summarises several headings. Only the
# numbers are a structural claim; the label is prose and may paraphrased.
SECTION_REF = re.compile(
    r"§\s*(\d+(?:\.\d+)*)"
    r"(?:\s*[–—~～-]\s*§?\s*(\d+(?:\.\d+)*))?"
)
ID_RANGE = re.compile(r"F(\d{2})\s*[–—\-~～至]\s*F?(\d{2})")
# The migration baseline tables below this heading cite Python entry points and
# old test names as location clues. They are not local Rust files and must not
# be required to exist; only the live status section is audited for citations.
BASELINE_HEADING = "## 双链与编辑交付"
CITED = re.compile(
    r"(?:^|[\s`(])((?:tests|src|scripts|config|dsh-plugin)/[A-Za-z0-9_./-]+"
    r"\.(?:rs|md|json|py|ps1|mjs|js))"
)


def normalize(text):
    return re.sub(r"\s+", "", text)


def headings(path):
    index = {}
    for number, line in enumerate(path.read_text(encoding="utf-8-sig").splitlines(), 1):
        match = HEADING.match(line)
        if match:
            index.setdefault(match.group(2), []).append((normalize(match.group(3)), line))
    return index


def reference_label(reference):
    """The prose after the first § number, used only for drift warnings."""
    match = SECTION_REF.search(reference)
    if not match:
        return ""
    label = reference[match.end():]
    label = re.split(r"[、，,；;]", label, maxsplit=1)[0]
    return normalize(label)


def resolve_reference(index, reference):
    matches = list(SECTION_REF.finditer(reference))
    if not matches:
        return [], "reference has no § section number"
    numbers = []
    for match in matches:
        numbers.append(match.group(1))
        if match.group(2):
            numbers.append(match.group(2))
    missing = [number for number in numbers if number not in index]
    if missing:
        return [], f"no heading(s) {missing} in the source"
    return numbers, None


def label_overlap(index, number, label):
    """True when an author label shares any character with the heading text."""
    candidates = index.get(number) or []
    if not candidates or not label:
        return True
    title = candidates[0][0]
    return bool(set(label) & set(title))


def ids_in(line):
    ids = set(re.findall(r"F\d{2}", line))
    for start, end in ID_RANGE.findall(line):
        first, last = int(start), int(end)
        if first <= last:
            ids.update(f"F{i:02d}" for i in range(first, last + 1))
    return ids


CELL_SPLIT = re.compile(r"(?<!\\)\|")
STATUS_BUCKETS = (
    ("unimplemented", ("未实现", "待实现")),
    ("partial", ("部分", "初步", "未验收", "未完成")),
    ("implemented", ("已接入", "已修复", "已实现", "已完成")),
)


def live_status_table(text):
    """Parse the live status table into ids, row count and per-feature rows.

    A row may cover a range or a group. The per-feature view keeps every row so
    a dedicated row (smaller id cell) can be preferred over a grouped one.
    """
    lines = text.splitlines()
    try:
        start = next(i for i, line in enumerate(lines) if line.startswith("| 契约项 | 状态 |"))
    except StopIteration:
        return set(), 0, {}
    ids, rows, by_feature = set(), 0, {}
    for line in lines[start:]:
        if not line.startswith("|"):
            break
        rows += 1
        cells = [cell.strip() for cell in CELL_SPLIT.split(line)]
        if len(cells) < 5:
            continue
        row_ids = ids_in(cells[1])
        # Only the first cell claims coverage; ids mentioned in evidence or
        # remaining text are references, not a live row for that feature.
        ids.update(row_ids)
        for feature_id in sorted(row_ids):
            by_feature.setdefault(feature_id, []).append(
                {
                    "ids": cells[1],
                    "claim": cells[2],
                    "evidence": cells[3],
                    "remaining": cells[4],
                    "width": len(row_ids),
                }
            )
    return ids, rows, by_feature


def status_summary(by_feature):
    summary, buckets = {}, {name: [] for name, _ in STATUS_BUCKETS}
    for feature_id, rows in by_feature.items():
        best = min(rows, key=lambda row: (row["width"], len(row["claim"])))
        summary[feature_id] = {
            "claim": best["claim"][:160],
            "dedicated": best["width"] == 1,
            "remaining": best["remaining"][:240],
        }
        bucket = "other"
        for name, needles in STATUS_BUCKETS:
            if any(needle in best["claim"] for needle in needles):
                bucket = name
                break
        buckets.setdefault(bucket, []).append(feature_id)
    return summary, {name: sorted(ids) for name, ids in buckets.items()}


def main():
    errors, warnings = [], []
    contract = CONTRACT.read_text(encoding="utf-8-sig")
    matrix = json.loads(MATRIX.read_text(encoding="utf-8-sig"))
    status = STATUS.read_text(encoding="utf-8-sig")
    lean = LEAN.read_text(encoding="utf-8-sig")

    features = matrix.get("features", [])
    by_id = {}
    for feature in features:
        feature_id = feature.get("id")
        if feature_id in by_id:
            errors.append(f"duplicate matrix feature {feature_id}")
        by_id[feature_id] = feature

    # 1. Feature coverage across contract, matrix and groups.
    contract_ids = set(re.findall(r"^### (F\d+) ", contract, re.MULTILINE))
    group_ids = [fid for group in matrix.get("groups", []) for fid in group.get("features", [])]
    for label, ids in (
        ("matrix", [f.get("id") for f in features]),
        ("contract", sorted(contract_ids)),
        ("groups", group_ids),
    ):
        if sorted(ids) != EXPECTED:
            missing = sorted(set(EXPECTED) - set(ids))
            extra = sorted(set(ids) - set(EXPECTED))
            errors.append(f"{label} ids incomplete; missing={missing} extra={extra}")

    # 2. Matrix entries are complete enough to audit.
    for feature in features:
        for field in REQUIRED_FEATURE_FIELDS:
            value = feature.get(field)
            if value is None or (isinstance(value, (list, str)) and len(value) == 0):
                errors.append(f"{feature.get('id')}: empty matrix field {field}")
        for source in feature.get("sources", []):
            if not source.get("source") or not source.get("section"):
                errors.append(f"{feature.get('id')}: source without id/section")

    # 3. D/P section references resolve in the frozen documents.
    source_ids = {source.get("id") for source in matrix.get("sources", [])}
    resolved = 0
    drift = {}
    indexes = {sid: headings(path) for sid, path in SOURCES.items() if path.is_file()}
    for sid, path in SOURCES.items():
        if not path.is_file():
            errors.append(f"missing frozen source {sid}: {path.relative_to(ROOT)}")
            continue
        expected_hash = next((s.get("sha256") for s in matrix.get("sources", []) if s.get("id") == sid), None)
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if expected_hash != digest or digest not in contract:
            errors.append(f"frozen source {sid} hash mismatch or not pinned by the contract")
    for feature in features:
        for source in feature.get("sources", []):
            sid, reference = source.get("source"), source.get("section", "")
            if sid not in source_ids:
                errors.append(f"{feature.get('id')}: unknown source id {sid}")
                continue
            if sid not in indexes:
                continue  # O/A01/U-* have no frozen local file by design
            numbers, problem = resolve_reference(indexes[sid], reference)
            if problem:
                errors.append(f"{feature.get('id')}: {sid} {reference} -> {problem}")
                continue
            resolved += len(numbers)
            label = reference_label(reference)
            if label and not label_overlap(indexes[sid], numbers[0], label):
                actual = indexes[sid][numbers[0]][0][1]
                drift.setdefault(
                    (sid, numbers[0], label, actual),
                    []).append(feature.get("id"))
    for (sid, number, label, actual), feature_ids in sorted(drift.items()):
        warnings.append(
            f"{sid} §{number}: matrix label '{label}' shares no text with "
            f"'{actual}' (used by {', '.join(sorted(set(feature_ids)))})"
        )

    # 4. Every feature has a live status row, and cited files exist.
    live_ids, live_rows, live_rows_by_feature = live_status_table(status)
    missing_status = sorted(set(EXPECTED) - live_ids)
    if missing_status:
        errors.append(f"CONTRACT-STATUS live table misses ids: {missing_status}")
    all_status_ids = set(re.findall(r"F\d{2}", status))
    if all_status_ids - set(EXPECTED):
        warnings.append(f"CONTRACT-STATUS mentions unexpected ids: {sorted(all_status_ids - set(EXPECTED))}")

    live_text = status.split(BASELINE_HEADING, 1)[0]
    cited = set(CITED.findall(live_text)) | set(CITED.findall(lean))
    missing_files = []
    for relative in sorted(cited):
        base = ROOT / "rust-core"
        if not (base / relative).is_file():
            missing_files.append(relative)
    if missing_files:
        errors.append(f"cited files do not exist under rust-core/: {missing_files}")

    # 5. The public paper copy, when present, is the manuscript.
    if PUBLIC_PAPER.is_file():
        manuscript_hash = next(
            (s.get("sha256") for s in matrix.get("sources", []) if s.get("id") == "P"), None)
        public_hash = hashlib.sha256(PUBLIC_PAPER.read_bytes()).hexdigest()
        if manuscript_hash and public_hash != manuscript_hash:
            warnings.append("paper/sic_paper_zh_v1.md differs from the frozen manuscript hash")

    feature_summary, status_buckets = status_summary(live_rows_by_feature)
    report = {
        "ok": not errors,
        "features": len(features),
        "live_status_rows": live_rows,
        "live_status_ids": len(live_ids),
        "section_refs_resolved": resolved,
        "cited_files": len(cited),
        "status_buckets": status_buckets,
        "grouped_only_ids": sorted(
            feature_id
            for feature_id, item in feature_summary.items()
            if not item["dedicated"]
        ),
        "feature_summary": {key: feature_summary[key] for key in sorted(feature_summary)},
        "errors": errors,
        "warnings": warnings,
    }
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    sys.exit(main())
