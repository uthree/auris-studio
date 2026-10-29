"""Validate paired seven-genre composition suites and SongEval reports."""

from __future__ import annotations

import argparse
import hashlib
import json
import math
from collections import Counter
from pathlib import Path
from typing import Any

AXES = ("Coherence", "Musicality", "Memorability", "Clarity", "Naturalness")
AUX_AXES = AXES[:-1]
GENRES = (
    "pop",
    "city_pop",
    "lofi",
    "neo_soul",
    "dance_electronic",
    "anime_song",
    "game_bgm",
)
ACCEPTANCE = {
    "selection_policy_version": "overall-mean-priority-v2",
    "overall_mean_delta_at_least": 0.03,
    "holdout_mean_delta_positive": True,
    "combined_mean_delta_at_least": 0.03,
}


def digest(path: Path) -> str:
    """Return the SHA-256 digest of a file."""
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def _read(path: Path) -> Any:
    return json.loads(path.read_text(encoding="utf-8"))


def _is_link(path: Path) -> bool:
    """Return whether *path* is a symlink or Windows junction."""
    return path.is_symlink() or getattr(path, "is_junction", lambda: False)()


def _trial_root(root: Path, identifier: str) -> Path:
    """Resolve a safe trial directory below a suite root."""
    if not isinstance(identifier, str):
        raise TypeError("trial ID must be a string")
    relative = Path(identifier)
    if (
        not identifier
        or relative.is_absolute()
        or ".." in relative.parts
        or len(relative.parts) != 1
        or any(character in identifier for character in "/\\:")
    ):
        raise ValueError(f"unsafe trial ID/path: {identifier!r}")
    raw = root / relative
    if _is_link(raw):
        raise ValueError(f"unsafe trial path: {identifier}")
    resolved_root = root.resolve(strict=True)
    # Failed process startup may leave no trial directory at all.
    resolved = raw.resolve()
    if resolved_root not in resolved.parents:
        raise ValueError(f"trial path escapes suite root: {identifier}")
    return resolved


def _artifact_path(root: Path, identifier: str, filename: str) -> Path:
    """Resolve one native delivery artifact without following a link."""
    trial = _trial_root(root, identifier)
    raw = trial / filename
    if _is_link(raw):
        raise ValueError(f"unsafe artifact path: {identifier}/{filename}")
    resolved = raw.resolve()
    if resolved.parent != trial:
        raise ValueError(f"artifact path escapes trial: {identifier}/{filename}")
    return resolved


def scoring_manifest(suites: list[Path]) -> dict[str, Any]:
    """Build SongEval inputs from retained WAV hashes, listing unavailable audio."""
    inputs, missing, sources = [], [], {}
    seen: set[str] = set()
    for root in suites:
        root = root.resolve()
        suite_file = root / "suite.json"
        suite = _read(suite_file)
        sources[str(suite_file)] = digest(suite_file)
        for trial in suite["trials"]:
            identifier = trial["id"]
            if identifier in seen:
                raise ValueError(f"duplicate trial ID: {identifier}")
            seen.add(identifier)
            audio = _artifact_path(root, identifier, "result.wav")
            expected = trial.get("artifacts", {}).get("result.wav")
            if expected is None:
                missing.append({"id": identifier, "reason": "no_retained_audio"})
                continue
            if not audio.is_file() or digest(audio) != expected:
                raise ValueError(f"retained audio is missing or changed: {identifier}")
            inputs.append({"id": identifier, "path": str(audio)})
    return {"inputs": inputs, "missing_audio": missing, "source_suites": sources}


def validate_manifest(path: Path) -> dict[str, Any]:
    """Validate the fixed two-split, seven-genre benchmark manifest."""
    value = _read(path)
    if value.get("schema_version") != 1 or tuple(value.get("genres", ())) != GENRES:
        raise ValueError("manifest must contain the seven versioned genres")
    if tuple(value.get("axes", ())) != (
        "request_fit",
        "melody",
        "form",
        "ensemble",
        "sound",
    ):
        raise ValueError(
            "manifest axes must contain the four composition axes and sound"
        )
    if not value.get("conditions") or not value.get("delivery_prompt"):
        raise ValueError("manifest needs conditions and delivery_prompt")
    cases = value.get("cases")
    if not isinstance(cases, list) or len(cases) != 14:
        raise ValueError("genre benchmark needs exactly fourteen cases")
    seen: set[str] = set()
    counts: dict[tuple[str, str], int] = {}
    sections = [
        {"name": "A", "start_bar": 1, "end_bar": 8},
        {"name": "B", "start_bar": 9, "end_bar": 16},
    ]
    for case in cases:
        if not isinstance(case, dict):
            raise TypeError("each case must be an object")
        identifier, genre, split = case.get("id"), case.get("genre"), case.get("split")
        if not isinstance(identifier, str) or not identifier or identifier in seen:
            raise ValueError("case IDs must be unique nonempty strings")
        if genre not in GENRES or split not in {"development", "holdout"}:
            raise ValueError("invalid genre or split")
        if case.get("fixture") != "blank" or case.get("bars") != 16:
            raise ValueError("all cases must be sixteen-bar blank fixtures")
        prompts = case.get("prompts")
        if (
            not isinstance(prompts, list)
            or not prompts
            or not all(isinstance(p, str) and p.strip() for p in prompts)
        ):
            raise ValueError("each case needs nonempty prompts")
        if not isinstance(case.get("key"), str) or not isinstance(
            case.get("tempo"), int
        ):
            raise TypeError("cases need a string key and integer tempo")
        if case.get("sections") != sections:
            raise ValueError("sections must cover bars 1-8 and 9-16")
        seen.add(identifier)
        counts[(genre, split)] = counts.get((genre, split), 0) + 1
    if any(
        counts.get((genre, split), 0) != 1
        for genre in GENRES
        for split in ("development", "holdout")
    ):
        raise ValueError("each genre needs one development and one holdout case")
    return value


def _selected_cases(suite: dict[str, Any], manifest: dict[str, Any]) -> list[str]:
    selected = suite.get("pilot_selection", {}).get(
        "cases", suite.get("selected_cases")
    )
    if selected is None:
        selected = [
            case["id"]
            for case in manifest["cases"]
            if case["split"] == suite.get("split")
        ]
    if (
        not isinstance(selected, list)
        or not selected
        or not all(isinstance(case, str) for case in selected)
    ):
        raise ValueError("suite selected_cases must be a nonempty list of case IDs")
    if len(set(selected)) != len(selected):
        raise ValueError("suite selected_cases contains duplicates")
    known = {case["id"] for case in manifest["cases"]}
    if not set(selected) <= known:
        raise ValueError("suite selected_cases contains an unknown case")
    return selected


def _suite(path: Path, manifest: dict[str, Any]) -> dict[str, Any]:
    value = _read(path / "suite.json")
    required = (
        "manifest_sha256",
        "model",
        "backend",
        "split",
        "repetitions",
        "runtime",
        "trials",
    )
    if any(key not in value for key in required):
        raise ValueError(f"suite missing required fields: {required}")
    if value["split"] not in {"development", "holdout"}:
        raise ValueError("suite split must be development or holdout")
    if type(value["repetitions"]) is not int or not 1 <= value["repetitions"] <= 20:
        raise ValueError("repetitions must be 1..20")
    if not isinstance(value["runtime"], dict) or not value["runtime"]:
        raise ValueError("suite runtime metadata must be a nonempty object")
    selected = _selected_cases(value, manifest)
    expected_cases = {
        case["id"] for case in manifest["cases"] if case["split"] == value["split"]
    }
    if set(selected) != expected_cases:
        raise ValueError("suite must contain the complete split cohort")
    trials = value["trials"]
    if not isinstance(trials, list):
        raise TypeError("suite trials must be a list")
    expected_ids = {
        f"{case}-{repetition:02}"
        for case in selected
        for repetition in range(1, value["repetitions"] + 1)
    }
    actual_ids = [trial.get("id") for trial in trials if isinstance(trial, dict)]
    if (
        len(actual_ids) != len(trials)
        or len(set(actual_ids)) != len(actual_ids)
        or set(actual_ids) != expected_ids
    ):
        raise ValueError("incomplete, duplicate or extra trial cohort")
    value["selected_cases"] = selected
    value["_root"] = path
    return value


def _combine(paths: list[Path], manifest: dict[str, Any]) -> dict[str, Any]:
    """Load complete splits; development alone can screen a candidate."""
    if not 1 <= len(paths) <= 2:
        raise ValueError("provide one or two complete split suites")
    values = [_suite(path, manifest) for path in paths]
    if len({value["split"] for value in values}) != len(values):
        raise ValueError("provide each split only once")
    fields = (
        "manifest_sha256",
        "model",
        "backend",
        "repetitions",
        "prompt_profile",
        "context_tokens",
        "candidate_tools_enabled",
        "source_commit",
    )
    for field in fields:
        if any(value.get(field) is None for value in values):
            raise ValueError(f"suite missing condition field: {field}")
        if any(values[0][field] != value[field] for value in values[1:]):
            raise ValueError(f"suite conditions differ: {field}")
    for field in ("runtime", "prompt_condition_sha256", "source_diff"):
        if any(values[0].get(field) != value.get(field) for value in values[1:]):
            raise ValueError(f"treatment changed between splits: {field}")
    merged = dict(values[0])
    merged["splits"] = [value["split"] for value in values]
    merged["_roots"] = {value["split"]: value["_root"] for value in values}
    merged["runtimes"] = {value["split"]: value.get("runtime") for value in values}
    merged["trials"] = [trial for value in values for trial in value["trials"]]
    merged["selected_cases"] = [
        case for value in values for case in value["selected_cases"]
    ]
    if len({trial["id"] for trial in merged["trials"]}) != len(merged["trials"]):
        raise ValueError("duplicate trial IDs across split suites")
    return merged


def _scores(path: Path) -> dict[str, dict[str, Any]]:
    value = _read(path)
    rows = value.get("rows") if isinstance(value, dict) else value
    if not isinstance(rows, list):
        raise TypeError("SongEval report must contain rows")
    result: dict[str, dict[str, Any]] = {}
    seen: set[str] = set()
    runtime = value.get("runtime") if isinstance(value, dict) else None
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get("id"), str):
            raise TypeError("every SongEval row needs a trial ID")
        identifier, scores = row["id"], row.get("scores")
        if identifier in seen:
            raise ValueError(f"duplicate score ID: {identifier}")
        seen.add(identifier)
        if row.get("status") != "scored":
            continue
        if (
            not isinstance(identifier, str)
            or not isinstance(scores, dict)
            or set(scores) != set(AXES)
        ):
            raise ValueError("each scored row needs an ID and all five SongEval axes")
        primary = {}
        for axis in AXES:
            number = scores[axis]
            if (
                isinstance(number, bool)
                or not isinstance(number, (int, float))
                or not math.isfinite(number)
                or not 1 <= number <= 5
            ):
                raise ValueError("SongEval scores must be finite values in 1..5")
            primary[axis] = float(number)
        auxiliary = {axis: primary[axis] for axis in AUX_AXES}
        result[identifier] = {
            "scores": primary,
            "aux_scores": auxiliary,
            "sha256": row.get("sha256"),
            "runtime": row.get("runtime", runtime),
            "diagnostics": row.get("diagnostics"),
        }
    return result


def delivery_issues(
    trial: dict[str, Any], score: dict[str, Any] | None, case: dict[str, Any]
) -> list[str]:
    """Require a complete, audible, unclipped sixteen-bar rendered delivery."""
    issues = []
    if trial.get("transport_pass") is not True:
        issues.append("agent_transport_failed")
    if trial.get("delivery_present") is not True:
        issues.append("delivery_missing")
    if score is None:
        issues.append("songeval_failed_or_missing")
        return issues
    encoded = trial.get("encoded")
    diagnostic = score.get("diagnostics")
    if not isinstance(encoded, dict) or not isinstance(diagnostic, dict):
        return issues + ["audio_measurements_missing"]
    expected_sha = trial.get("artifacts", {}).get("result.wav")
    if encoded.get("sha256") != expected_sha:
        issues.append("encoded_artifact_hash_mismatch")
    expected_seconds = case["bars"] * 4 * 60 / case["tempo"]
    for measurement in (encoded, diagnostic):
        seconds, rate = measurement.get("seconds"), measurement.get("sample_rate")
        if (
            not isinstance(seconds, (int, float))
            or not math.isfinite(seconds)
            or not isinstance(rate, int)
            or rate <= 0
            or abs(seconds - expected_seconds) > 1 / rate + 1e-6
        ):
            issues.append("render_duration_mismatch")
            break
    if diagnostic.get("admissible") is not True:
        issues.append("audio_silent_or_invalid")
    if encoded.get("full_scale_samples") != 0:
        issues.append("encoded_clipping")
    for field in ("peak_db", "true_peak_db"):
        value = encoded.get(field)
        if not isinstance(value, (int, float)) or not math.isfinite(value) or value > 0:
            issues.append(f"invalid_{field}")
    return issues


def _aggregate(
    rows: list[dict[str, Any]], key: str = "scores", axes: tuple[str, ...] = AXES
) -> dict[str, Any]:
    if not rows:
        return {
            "count": 0,
            "mean_delta": None,
            "axis_delta": {axis: None for axis in axes},
        }
    axis_delta = {
        axis: sum(row["delta"][key][axis] for row in rows) / len(rows) for axis in axes
    }
    return {
        "count": len(rows),
        "before_mean": round(
            sum(
                sum(row["before"][key][axis] for axis in axes) / len(axes)
                for row in rows
            )
            / len(rows),
            6,
        ),
        "after_mean": round(
            sum(
                sum(row["after"][key][axis] for axis in axes) / len(axes)
                for row in rows
            )
            / len(rows),
            6,
        ),
        "mean_delta": round(sum(axis_delta.values()) / len(axes), 6),
        "axis_delta": {axis: round(value, 6) for axis, value in axis_delta.items()},
    }


def trial_efficiency(root: Path, trial: dict[str, Any]) -> dict[str, Any]:
    """Read verified transcript evidence, keeping generated work separate from notes sent."""
    path = root / trial["id"] / "transcript.json"
    expected = trial.get("artifacts", {}).get("transcript.json")
    if expected is None:
        return {"available": False}
    if not path.is_file() or digest(path) != expected:
        raise ValueError(f"transcript changed or missing: {trial['id']}")
    entries = _read(path)
    calls = [entry for entry in entries if entry.get("kind") == "tool"]
    generators = {"accompaniment", "generate_phrase"}
    counts = Counter(entry["name"] for entry in calls)
    return {
        "available": True,
        "tool_calls": len(calls),
        "calls_by_name": dict(sorted(counts.items())),
        "successful_generation_calls": sum(
            entry.get("ok") is True and entry["name"] in generators for entry in calls
        ),
        "failed_generation_calls": sum(
            entry.get("ok") is False and entry["name"] in generators for entry in calls
        ),
        "edit_notes_calls": counts["edit_notes"],
        "elapsed_seconds": trial.get("elapsed_seconds"),
    }


def compare(
    before_suite: Path | list[Path],
    after_suite: Path | list[Path],
    before_scores: Path,
    after_scores: Path,
    manifest: Path,
) -> dict[str, Any]:
    """Compare complete development and holdout cohorts under frozen conditions."""
    definition = validate_manifest(manifest)
    before = _combine(
        [before_suite] if isinstance(before_suite, Path) else before_suite, definition
    )
    after = _combine(
        [after_suite] if isinstance(after_suite, Path) else after_suite, definition
    )
    if before["manifest_sha256"] != digest(manifest) or after[
        "manifest_sha256"
    ] != digest(manifest):
        raise ValueError("suite manifest hash differs from benchmark manifest")
    condition_fields = (
        "manifest_sha256",
        "model",
        "backend",
        "repetitions",
        "prompt_profile",
        "context_tokens",
        "candidate_tools_enabled",
    )
    for field in condition_fields:
        if before[field] != after[field]:
            raise ValueError(f"before/after suite conditions differ: {field}")
    before_trials = {row["id"]: row for row in before["trials"]}
    after_trials = {row["id"]: row for row in after["trials"]}
    if set(before_trials) != set(after_trials):
        raise ValueError("before/after trial cohorts differ")
    genres = {
        case["id"]: (case["genre"], case["split"]) for case in definition["cases"]
    }
    cases_by_id = {case["id"]: case for case in definition["cases"]}
    before_report, after_report = _scores(before_scores), _scores(after_scores)
    rows: list[dict[str, Any]] = []
    runtime_fingerprint: str | None = None
    for identifier in sorted(before_trials):
        case_id = identifier.rsplit("-", 1)[0]
        if case_id not in genres:
            raise ValueError(f"trial is not in benchmark manifest: {identifier}")
        genre, split = genres[case_id]
        left, right = before_report.get(identifier), after_report.get(identifier)
        for side, trial, score, suite in (
            ("before", before_trials[identifier], left, before),
            ("after", after_trials[identifier], right, after),
        ):
            successful = (
                trial.get("transport_pass") is True
                and trial.get("delivery_present") is True
            )
            artifacts = trial.get("artifacts", {})
            expected_hash = artifacts.get("result.wav")
            expected_midi_hash = artifacts.get("result.mid")
            if not expected_hash and not successful and score is None:
                continue
            if not isinstance(expected_hash, str) or not expected_hash:
                raise ValueError(f"{side} trial lacks result.wav SHA-256: {identifier}")
            if successful and (
                not isinstance(expected_midi_hash, str) or not expected_midi_hash
            ):
                raise ValueError(f"{side} trial lacks result.mid SHA-256: {identifier}")
            rendered = _artifact_path(suite["_roots"][split], identifier, "result.wav")
            if not rendered.is_file() or digest(rendered) != expected_hash:
                raise ValueError(
                    f"{side} trial artifact is missing or changed: {identifier}"
                )
            if successful:
                midi = _artifact_path(suite["_roots"][split], identifier, "result.mid")
                if not midi.is_file() or digest(midi) != expected_midi_hash:
                    raise ValueError(
                        f"{side} trial MIDI artifact is missing or changed: {identifier}"
                    )
            if score is not None:
                if score["sha256"] != expected_hash:
                    raise ValueError(
                        f"{side} SongEval hash differs from trial artifact: {identifier}"
                    )
                if score["runtime"] is None:
                    raise ValueError("SongEval report lacks runtime conditions")
                fingerprint = json.dumps(score["runtime"], sort_keys=True)
                if runtime_fingerprint is None:
                    runtime_fingerprint = fingerprint
                elif fingerprint != runtime_fingerprint:
                    raise ValueError("SongEval runtime conditions differ")
        issues = {
            "before": delivery_issues(
                before_trials[identifier], left, cases_by_id[case_id]
            ),
            "after": delivery_issues(
                after_trials[identifier], right, cases_by_id[case_id]
            ),
        }
        valid = not issues["before"] and not issues["after"]
        rows.append(
            {
                "trial": identifier,
                "case": case_id,
                "genre": genre,
                "split": split,
                "status": "paired" if valid else "failed_or_missing",
                "issues": issues,
                "elapsed_seconds": {
                    "before": before_trials[identifier].get("elapsed_seconds"),
                    "after": after_trials[identifier].get("elapsed_seconds"),
                },
                "tool_errors": {
                    "before": before_trials[identifier].get("tool_errors"),
                    "after": after_trials[identifier].get("tool_errors"),
                },
                "efficiency": {
                    "before": trial_efficiency(
                        before["_roots"][split], before_trials[identifier]
                    ),
                    "after": trial_efficiency(
                        after["_roots"][split], after_trials[identifier]
                    ),
                },
                "before": left,
                "after": right,
                "delta": (
                    {
                        "scores": {
                            axis: right["scores"][axis] - left["scores"][axis]
                            for axis in AXES
                        },
                        "aux_scores": (
                            {
                                axis: right["aux_scores"][axis]
                                - left["aux_scores"][axis]
                                for axis in AUX_AXES
                            }
                            if left["aux_scores"] is not None
                            and right["aux_scores"] is not None
                            else None
                        ),
                    }
                    if valid
                    else None
                ),
            }
        )
    valid_rows = [row for row in rows if row["status"] == "paired"]
    genre_reports = {
        genre: _aggregate([row for row in valid_rows if row["genre"] == genre])
        for genre in GENRES
    }
    overall = _aggregate(valid_rows)
    development_rows = [row for row in valid_rows if row["split"] == "development"]
    development = _aggregate(development_rows)
    development_genres = {
        genre: _aggregate([row for row in development_rows if row["genre"] == genre])
        for genre in GENRES
    }
    holdout = _aggregate([row for row in valid_rows if row["split"] == "holdout"])
    auxiliary = _aggregate(
        [row for row in valid_rows if row["delta"]["aux_scores"] is not None],
        "aux_scores",
        AUX_AXES,
    )
    improved = sum(
        report["mean_delta"] is not None and report["mean_delta"] > 0
        for report in genre_reports.values()
    )
    acceptance = {
        "all_pairs_valid": len(valid_rows) == len(rows),
        "complete_development_and_holdout": set(before["splits"])
        == {"development", "holdout"},
        "candidate_eligible": False,
        "overall_mean_delta": overall["mean_delta"],
        "worst_genre_delta": min(
            (
                report["mean_delta"]
                for report in genre_reports.values()
                if report["mean_delta"] is not None
            ),
            default=None,
        ),
        "improved_genres": improved,
        "holdout_mean_delta": holdout["mean_delta"],
        "accepted": False,
    }
    acceptance["candidate_eligible"] = bool(
        acceptance["all_pairs_valid"]
        and "development" in before["splits"]
        and development["mean_delta"] is not None
        and development["mean_delta"] >= ACCEPTANCE["overall_mean_delta_at_least"]
    )
    acceptance["accepted"] = bool(
        acceptance["candidate_eligible"]
        and acceptance["complete_development_and_holdout"]
        and acceptance["holdout_mean_delta"] is not None
        and acceptance["holdout_mean_delta"] > 0
        and acceptance["overall_mean_delta"] is not None
        and acceptance["overall_mean_delta"]
        >= ACCEPTANCE["combined_mean_delta_at_least"]
    )
    return {
        "schema_version": 1,
        "axes": list(AXES),
        "supplementary_axes": list(AUX_AXES),
        "naturalness_note": "Naturalness is retained for SongEval parity; instrumental cases do not establish vocal naturalness.",
        "acceptance_rule": ACCEPTANCE,
        "selection_policy_version": ACCEPTANCE["selection_policy_version"],
        "conditions": {key: before[key] for key in condition_fields},
        "splits": before["splits"],
        "agent_runtimes": {"before": before["runtimes"], "after": after["runtimes"]},
        "agent_source_commits": {
            "before": before["source_commit"],
            "after": after["source_commit"],
        },
        "report_hashes": {
            "manifest": digest(manifest),
            "before_scores": digest(before_scores),
            "after_scores": digest(after_scores),
        },
        "rows": rows,
        "genres": genre_reports,
        "overall": overall,
        "development": development,
        "development_genres": development_genres,
        "holdout": holdout,
        "supplementary": auxiliary,
        "acceptance": acceptance,
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    scorer = commands.add_parser(
        "inputs", help="Build a hash-verified SongEval manifest"
    )
    scorer.add_argument("--suite", type=Path, action="append", required=True)
    scorer.add_argument("--output", type=Path, required=True)
    comparison = commands.add_parser("compare", help="Compare complete genre cohorts")
    comparison.add_argument("--before-suite", type=Path, action="append", required=True)
    comparison.add_argument("--after-suite", type=Path, action="append", required=True)
    comparison.add_argument("--before-scores", type=Path, required=True)
    comparison.add_argument("--after-scores", type=Path, required=True)
    comparison.add_argument(
        "--manifest",
        type=Path,
        default=Path(__file__).with_name("references") / "genre-benchmark-v1.json",
    )
    comparison.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    report = (
        scoring_manifest(args.suite)
        if args.command == "inputs"
        else compare(
            args.before_suite,
            args.after_suite,
            args.before_scores,
            args.after_scores,
            args.manifest,
        )
    )
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


if __name__ == "__main__":
    main()
