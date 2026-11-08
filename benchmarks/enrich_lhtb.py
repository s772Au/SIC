"""Read-only public task metadata enrichment; never launches benchmark tasks."""

import concurrent.futures
import csv
import pathlib
import tomllib
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent
INVENTORY = HERE / "task-candidate-inventory.csv"


def fetch(row):
    if row["suite"] == "LHTB-2.0":
        segment = "tasks-2.0" if row["source"].split("/tree/main/")[-1].startswith("tasks-2.0/") else "tasks"
        url = f"https://raw.githubusercontent.com/zli12321/LHTB/main/{segment}/{row['task_id']}/task.toml"
    else:
        url = f"https://huggingface.co/datasets/UnipatAI/RoadmapBench/resolve/main/{row['task_id']}/task.toml"
    try:
        with urllib.request.urlopen(url, timeout=20) as response:
            item = tomllib.loads(response.read().decode("utf-8"))
        environment = item.get("environment", {})
        task = item.get("task", {})
        metadata = item.get("metadata", {})
        agent = item.get("agent", {})
        row.update(
            cpus=environment.get("cpus", ""),
            memory_mb=environment.get("memory_mb", ""),
            storage_mb=environment.get("storage_mb", ""),
            gpus=environment.get("gpus", ""),
            timeout_sec=agent.get("timeout_sec", ""),
            category=metadata.get("category", ""),
            difficulty=metadata.get("difficulty", ""),
            expert_time_min=metadata.get("expert_time_estimate_min", ""),
            title=task.get("description", metadata.get("description", "")),
        )
        row["resource"] = "public_task_toml"
    except Exception as exc:
        row["resource"] = f"metadata_fetch_failed:{type(exc).__name__}"
    return row


with INVENTORY.open(encoding="utf-8-sig", newline="") as handle:
    rows = list(csv.DictReader(handle))
with concurrent.futures.ThreadPoolExecutor(max_workers=8) as executor:
    enriched = list(executor.map(fetch, (row for row in rows if row["suite"] in {"LHTB-2.0", "RoadmapBench"})))
lookup = {(row["suite"], row["task_id"]): row for row in enriched}
rows = [lookup.get((row["suite"], row["task_id"]), row) for row in rows]
fields = list(dict.fromkeys(key for row in rows for key in row))
with INVENTORY.open("w", encoding="utf-8", newline="") as handle:
    writer = csv.DictWriter(handle, fieldnames=fields)
    writer.writeheader()
    writer.writerows(rows)
print(f"Task metadata read: {sum(row['resource'] == 'public_task_toml' for row in enriched)}/{len(enriched)}")
