"""Offline request and wall-time envelope for the prepared benchmark packages.

This is a capacity model, not a gateway measurement or an experiment.
"""

import argparse
import json
import math
from pathlib import Path


ROOT = Path(__file__).resolve().parent


def estimate(mix, arms, max_in_flight, compressor_seconds, answer_seconds, judge_seconds):
    catalog = json.loads((ROOT / "test-catalog.json").read_text(encoding="utf-8"))
    datasets = {item["id"]: item for item in catalog["datasets"]}
    totals = {"compressor": 0, "answer": 0, "judge": 0, "histories": 0, "questions": 0}
    critical_seconds = 0
    for selected in mix:
        dataset = datasets[selected["id"]]
        histories = selected["histories"]
        questions = selected["questions"]
        if not 1 <= histories <= dataset["historyCount"]:
            raise ValueError(f"invalid histories for {selected['id']}")
        if not 1 <= questions <= min(dataset.get("questionCount", dataset["caseCount"]), histories * dataset["questionsPerHistoryMax"]):
            raise ValueError(f"invalid questions for {selected['id']}")
        rates = dataset["requestEstimate"]
        totals["histories"] += histories * arms
        totals["questions"] += questions * arms
        totals["compressor"] += histories * rates["compressorCallsPerHistory"] * arms
        totals["answer"] += questions * rates["answerCallsPerQuestion"] * arms
        totals["judge"] += questions * rates["judgeCallsPerQuestion"] * arms
        critical_seconds = max(
            critical_seconds,
            rates["compressorCallsPerHistory"] * compressor_seconds
            + rates["answerCallsPerQuestion"] * answer_seconds
            + rates["judgeCallsPerQuestion"] * judge_seconds,
        )
    service_seconds = (totals["compressor"] * compressor_seconds + totals["answer"] * answer_seconds + totals["judge"] * judge_seconds)
    capacity_bound = service_seconds / max_in_flight
    ingestion_bound = totals["compressor"] * compressor_seconds / min(max_in_flight, totals["histories"])
    lower = max(critical_seconds, capacity_bound, ingestion_bound)
    # Phase-by-phase schedule is a coarse upper planning envelope at the assumed latencies.
    upper = (math.ceil(totals["histories"] / max_in_flight) * max((datasets[s["id"]]["requestEstimate"]["compressorCallsPerHistory"] for s in mix), default=0) * compressor_seconds
             + totals["answer"] * answer_seconds / max_in_flight
             + totals["judge"] * judge_seconds / max_in_flight)
    return {
        "arms": arms,
        "maxInFlightAssumption": max_in_flight,
        "latencyAssumptionSeconds": {"compressor": compressor_seconds, "answer": answer_seconds, "judge": judge_seconds},
        "requests": totals,
        "estimatedRequestRateAtFullCapacityRps": round(max_in_flight / compressor_seconds, 3),
        "minimumWallMinutesAtAssumedLatency": round(lower / 60, 1),
        "upperPhasePlanningMinutesAtAssumedLatency": round(max(lower, upper) / 60, 1),
        "notes": "Latency assumptions are unmeasured. 429, retries, source ingest, network, gateway adaptation, model output and LAFS scoring can extend wall time.",
    }


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--mix", type=Path, required=True)
    parser.add_argument("--arms", type=int, default=2)
    parser.add_argument("--max-in-flight", type=int, default=32)
    args = parser.parse_args()
    if args.arms < 1 or args.max_in_flight < 1:
        parser.error("arms and max-in-flight must be positive")
    mix = json.loads(args.mix.read_text(encoding="utf-8"))["mix"]
    for latency in (20, 60, 120):
        print(json.dumps(estimate(mix, args.arms, args.max_in_flight, latency, 10, 5), ensure_ascii=False))


if __name__ == "__main__":
    main()
