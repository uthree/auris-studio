# /// script
# requires-python = ">=3.10"
# dependencies = []
# ///
"""Describe where a lead pauses, without scoring long notes or rests as defects.

    uv run tools/eval/melody_continuity.py --manifest before.json --json pauses.json
    uv run tools/eval/melody_continuity.py --project song.auris --json pauses.json

The first eight-bar 4/4 lead chorus and nearest-sixteenth timing come from
seed_metrics. Beats in individual records start at zero: musical beat three is
2.0. Phrase-ending bars are an explicit analysis assumption (4 and 8 by default),
not information recovered from saved phrase plans. Non-ending and ending bars
are reported separately. There is no aggregate quality score or penalty.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from collections import Counter
from itertools import pairwise
from pathlib import Path

from seed_metrics import (
    BAR_TICKS,
    CHORUS_BARS,
    InvalidProjectError,
)
from seed_metrics import (
    analyze_project as analyze_seed_project,
)

BEAT_TICKS = BAR_TICKS // 4
EARLY_IOI_TICKS = 2 * BEAT_TICKS
LONG_HOLD_TICKS = 3 * BEAT_TICKS // 2


def _sounding_ticks(rhythm: list[list[int]], left: int, right: int) -> int:
    """Measure interval-union coverage; overlapping held notes cannot double count."""
    total, covered_until = 0, left
    for at, duration in rhythm:
        a, b = max(left, at), min(right, at + duration)
        if b > max(a, covered_until):
            total += b - max(a, covered_until)
            covered_until = b
    return total


def _summary(bars: list[dict]) -> dict:
    return {
        "bar_count": len(bars),
        "note_count": sum(bar["note_count"] for bar in bars),
        "empty_attack_bar_count": sum(bar["note_count"] == 0 for bar in bars),
        "early_stationary_bar_count": sum(bar["early_stationary_span"] for bar in bars),
        "beat_three_attack_bar_count": sum(bar["attack_on_beat_three"] for bar in bars),
        "maximum_attack_free_span_beats": max(
            (bar["maximum_attack_free_span_beats"] for bar in bars), default=None
        ),
        "maximum_complete_in_bar_ioi_beats": max(
            (
                bar["maximum_complete_in_bar_ioi_beats"]
                for bar in bars
                if bar["maximum_complete_in_bar_ioi_beats"] is not None
            ),
            default=None,
        ),
        "early_long_hold_count": sum(bar["early_long_hold_count"] for bar in bars),
        "late_long_hold_count": sum(bar["late_long_hold_count"] for bar in bars),
    }


def analyze_project(project: dict, phrase_end_bars: tuple[int, ...] = (4, 8)) -> dict:
    """Describe normalized attack gaps, with explicit one-based phrase endings.

    An early stationary span is a complete inter-onset interval of at least two
    beats, starting inside the first half of a bar and strictly crossing beat
    three. An attack exactly on beat three prevents such a span. Leading/trailing
    silence without two observed attacks is not called an IOI. Empty internal
    bars and held notes across bars remain visible; an empty first bar, collapsed
    onsets, or unsupported score geometry is rejected by seed_metrics.
    """
    if any(
        not isinstance(bar, int) or isinstance(bar, bool) or not 1 <= bar <= CHORUS_BARS
        for bar in phrase_end_bars
    ) or len(set(phrase_end_bars)) != len(phrase_end_bars):
        raise InvalidProjectError(
            "phrase endings must be unique bar numbers from 1 to 8"
        )
    source = analyze_seed_project(project)
    rhythm = source["chorus"]["onset_duration"]
    onsets = [at for at, _ in rhythm]
    end = BAR_TICKS * CHORUS_BARS
    bars = []
    for index in range(CHORUS_BARS):
        left, right = index * BAR_TICKS, (index + 1) * BAR_TICKS
        middle = left + 2 * BEAT_TICKS
        local = [(at, duration) for at, duration in rhythm if left <= at < right]
        attacks = [at for at, _ in local]
        edges = sorted({left, right, *attacks})
        complete = [b - a for a, b in pairwise(attacks)]
        span = None
        if middle not in onsets:
            previous = max((at for at in onsets if at < middle), default=None)
            following = next((at for at in onsets if at > middle), None)
            gap_start = previous if previous is not None else 0
            gap_end = following if following is not None else end
            clipped_start, clipped_end = max(left, gap_start), min(right, gap_end)
            sounding = _sounding_ticks(rhythm, clipped_start, clipped_end)
            span = {
                "previous_attack_beat": (
                    (previous - left) / BEAT_TICKS if previous is not None else None
                ),
                "next_attack_beat": (
                    (following - left) / BEAT_TICKS if following is not None else None
                ),
                "ioi_beats": (
                    (following - previous) / BEAT_TICKS
                    if previous is not None and following is not None
                    else None
                ),
                "span_in_bar_beats": (clipped_end - clipped_start) / BEAT_TICKS,
                "sounding_in_bar_beats": sounding / BEAT_TICKS,
                "rest_in_bar_beats": (clipped_end - clipped_start - sounding)
                / BEAT_TICKS,
                "crosses_bar_boundary": gap_start < left or gap_end > right,
            }
        early = bool(
            span is not None
            and span["ioi_beats"] is not None
            and span["previous_attack_beat"] is not None
            and 0 <= span["previous_attack_beat"] < 2
            and span["ioi_beats"] >= EARLY_IOI_TICKS / BEAT_TICKS
        )
        bars.append(
            {
                "bar": index + 1,
                "phrase_ending": index + 1 in phrase_end_bars,
                "note_count": len(local),
                "attack_on_beat_three": middle in onsets,
                "beat_three_span": span,
                "early_stationary_span": early,
                "maximum_attack_free_span_beats": max(b - a for a, b in pairwise(edges))
                / BEAT_TICKS,
                "maximum_complete_in_bar_ioi_beats": (
                    max(complete) / BEAT_TICKS if complete else None
                ),
                "sounding_beats": _sounding_ticks(rhythm, left, right) / BEAT_TICKS,
                "early_long_hold_count": sum(
                    duration >= LONG_HOLD_TICKS and at < middle
                    for at, duration in local
                ),
                "late_long_hold_count": sum(
                    duration >= LONG_HOLD_TICKS and at >= middle
                    for at, duration in local
                ),
            }
        )
    signatures = []
    for bar in bars:
        span = bar["beat_three_span"]
        signatures.append(
            (
                span["previous_attack_beat"],
                span["next_attack_beat"],
                span["sounding_in_bar_beats"],
                span["rest_in_bar_beats"],
            )
            if bar["early_stationary_span"] and not bar["phrase_ending"]
            else None
        )
    longest, run, previous = 0, 0, None
    for signature in signatures:
        run = (run + 1 if signature == previous else 1) if signature is not None else 0
        longest = max(longest, run)
        previous = signature
    counts = Counter(signature for signature in signatures if signature is not None)
    return {
        "schema_version": 1,
        "interpretation": "Location and recurrence of pauses, not musical quality; longer is not inherently worse.",
        "region": source["region"],
        "hashes": source["hashes"],
        "quantization": source["quantization"],
        "definitions": {
            "beat_origin": "zero; musical beat three is 2.0",
            "phrase_end_bars": sorted(phrase_end_bars),
            "phrase_end_source": "explicit analysis assumption, not inferred score metadata",
            "early_stationary_minimum_ioi_beats": EARLY_IOI_TICKS / BEAT_TICKS,
            "long_hold_minimum_beats": LONG_HOLD_TICKS / BEAT_TICKS,
            "long_hold_position": "note onset before beat 2.0 versus at/after 2.0; full written duration",
            "boundary_policy": "Clip attack-free coverage to each bar; retain complete observed IOI across bars; no invented boundary attacks.",
            "performance": "Written score only; pitch curves and performance transforms are not measured.",
        },
        "note_count": source["note_count"],
        "consecutive_same_pitch_attack_count": sum(
            interval == 0 for interval in source["chorus"]["intervals"]
        ),
        "nonending_summary": _summary(
            [bar for bar in bars if not bar["phrase_ending"]]
        ),
        "ending_summary": _summary([bar for bar in bars if bar["phrase_ending"]]),
        "early_stationary_recurrence": {
            "longest_consecutive_nonending_bar_run": longest,
            "adjacent_identical_nonending_pair_count": sum(
                a is not None and a == b for a, b in pairwise(signatures)
            ),
            "most_common_pattern_bar_count": max(counts.values(), default=0),
            "patterns": [
                {"signature": list(signature), "bar_count": count}
                for signature, count in sorted(counts.items())
            ],
        },
        "bars": bars,
    }


def analyze_manifest(path: Path, phrase_end_bars: tuple[int, ...] = (4, 8)) -> dict:
    """Verify project artifact hashes and measure a generation manifest without changing it."""
    path = Path(path).resolve()
    data = path.read_bytes()
    manifest = json.loads(data)
    files = manifest.get("files") if isinstance(manifest, dict) else None
    if not isinstance(files, dict) or not files:
        raise InvalidProjectError(
            "manifest requires nonempty files with project artifacts"
        )
    output = {}
    for label, row in files.items():
        artifact = row.get("project") if isinstance(row, dict) else None
        if (
            not isinstance(artifact, dict)
            or not isinstance(artifact.get("path"), str)
            or not artifact["path"]
            or not isinstance(artifact.get("sha256"), str)
            or not re.fullmatch(r"[0-9a-fA-F]{64}", artifact["sha256"])
        ):
            raise InvalidProjectError(f"malformed project artifact for {label!r}")
        source_path = Path(artifact["path"])
        if not source_path.is_absolute():
            source_path = path.parent / source_path
        raw = source_path.read_bytes()
        if hashlib.sha256(raw).hexdigest() != artifact["sha256"].lower():
            raise InvalidProjectError(f"project SHA256 mismatch for {label!r}")
        output[label] = {
            "preset": row.get("preset"),
            "seed": row.get("seed"),
            "project_sha256": hashlib.sha256(raw).hexdigest(),
            "continuity": analyze_project(json.loads(raw), phrase_end_bars),
        }
    return {
        "schema_version": 1,
        "source_manifest_sha256": hashlib.sha256(data).hexdigest(),
        "files": output,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    inputs = parser.add_mutually_exclusive_group(required=True)
    inputs.add_argument("--manifest", type=Path)
    inputs.add_argument("--project", type=Path, nargs="+")
    parser.add_argument("--phrase-end-bars", type=int, nargs="*", default=[4, 8])
    parser.add_argument("--json", type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.json.exists():
            raise InvalidProjectError("output must be a new file")
        endings = tuple(args.phrase_end_bars)
        if args.manifest:
            result = analyze_manifest(args.manifest, endings)
        else:
            files = {}
            for path in args.project:
                key = str(path.resolve())
                if key in files:
                    raise InvalidProjectError("duplicate project path")
                raw = path.read_bytes()
                files[key] = {
                    "project_sha256": hashlib.sha256(raw).hexdigest(),
                    "continuity": analyze_project(json.loads(raw), endings),
                }
            result = {"schema_version": 1, "files": files}
        args.json.parent.mkdir(parents=True, exist_ok=True)
        with args.json.open("x", encoding="utf8") as stream:
            stream.write(json.dumps(result, indent=2, allow_nan=False) + "\n")
    except (OSError, ValueError) as error:
        parser.error(str(error))
    print(f"Measured {len(result['files'])} projects; wrote {args.json}")


if __name__ == "__main__":
    main()
