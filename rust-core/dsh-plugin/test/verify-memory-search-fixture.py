"""Verify a synthetic DSH session without printing its messages or credentials."""

import json
import sqlite3
import sys

db, marker = sys.argv[1:]
connection = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
rows = connection.execute(
    "SELECT state, progress FROM dsh_bridge_turns ORDER BY rowid"
).fetchall()
turns = [(state, json.loads(progress)) for state, progress in rows]
first_cb = next((p.get("published_cb") for _, p in turns if p.get("published_cb")), None)
verified = 0
verified_after_first = 0
for turn_index, (_, progress) in enumerate(turns):
    for worker_round in progress.get("worker_rounds", []):
        calls = {
            call.get("id")
            for call in worker_round.get("tool_calls", [])
            if call.get("function", {}).get("name") == "sic_memory_search"
        }
        for result in worker_round.get("tool_results", []):
            if result.get("tool_call_id") not in calls:
                continue
            envelope = result.get("result", {})
            if not envelope.get("ok"):
                continue
            for hit in envelope.get("result", {}).get("hits", []):
                if hit.get("cb_id") != first_cb or marker not in hit.get("excerpt", ""):
                    continue
                def load(kind, identity):
                    row = connection.execute(
                        "SELECT data FROM objects WHERE kind=? AND id=?", (kind, identity)
                    ).fetchone()
                    return json.loads(row[0]) if row else None

                cb = load("cb", hit["cb_id"])
                wb = load("wb", hit["wb_id"])
                source = load("source", hit["source_id"])
                index = next(
                    (entry for entry in (cb or {}).get("index", [])
                     if entry.get("unit_id") == hit.get("materialized_unit_id")),
                    None,
                )
                if (
                    cb and wb and source and index
                    and cb.get("wb_id") == hit["wb_id"]
                    and hit["source_id"] in cb.get("source_ids", [])
                    and hit.get("operation_indexes") == index.get("operation_indexes", [])[:32]
                    and hit.get("operation_index_count") == len(index.get("operation_indexes", []))
                    and any(
                        unit.get("unit_id") == hit.get("unit_id")
                        and marker in unit.get("text", "")
                        for unit in source.get("units", [])
                    )
                ):
                    verified += 1
                    if turn_index > 0:
                        verified_after_first += 1

if len(turns) < 2 or turns[-1][0] != "delivered" or not verified_after_first:
    print(json.dumps({"verified": False, "turns": len(turns)}))
    sys.exit(1)
print(json.dumps({"verified": True, "turns": len(turns), "matching_tool_results": verified,
                  "verified_after_resume": verified_after_first}))
