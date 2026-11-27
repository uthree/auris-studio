# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "soundfile==0.14.0"]
# ///
"""Compare two drum-kit workers on the held-out Salamander hits, development only.

Both workers play their factory settings; nothing is fitted here. Per family it prints the
paired mel change with its clustered bootstrap interval, and the envelope and 90%-energy-time
errors before and after, so a change to the kit is judged on hits no table was measured on.
"""

import argparse
import json
from pathlib import Path

import numpy as np
from fit_worker import Renderer
from physical_pack import FAMILIES, load, paired_summary, rows


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reference", type=Path)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("--split", default="validation", choices=("train", "validation"))
    parser.add_argument("--json", type=Path)
    args = parser.parse_args()
    manifest = json.loads((args.reference / "notes.json").read_text(encoding="utf-8"))
    before, after = Renderer(args.before.resolve()), Renderer(args.after.resolve())
    report = {}
    try:
        for family in FAMILIES:
            notes, reference = load(args.reference, manifest, family, args.split)
            a = rows(notes, reference, before.render("drums", notes, {}, 3, 3))
            b = rows(notes, reference, after.render("drums", notes, {}, 3, 3))
            summary = paired_summary(a, b)
            for name in ("attack", "body", "tail", "t90_error_seconds"):
                summary[f"{name}_before"] = float(np.mean([row[name] for row in a]))
                summary[f"{name}_after"] = float(np.mean([row[name] for row in b]))
            report[family] = summary
            low, high = summary["mean_delta_ci95"]
            print(f"{family:10s} mel {summary['before']:.3f} -> {summary['after']:.3f} "
                  f"[{low:+.3f}, {high:+.3f}]  envelope {summary['envelope_before_db']:5.1f} -> "
                  f"{summary['envelope_after_db']:5.1f} dB  t90 {summary['t90_error_seconds_before']:.3f} -> "
                  f"{summary['t90_error_seconds_after']:.3f} s  better {summary['improved']}/{summary['count']}",
                  flush=True)
    finally:
        before.close()
        after.close()
    if args.json:
        args.json.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
