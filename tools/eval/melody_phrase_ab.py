# /// script
# requires-python = ">=3.11"
# dependencies = ["numpy<2", "soundfile>=0.12"]
# ///
"""Four explicit composer conditions over one frozen, editable backing.

Baseline projects/audio are copied exactly from a completed continuity corpus.
Pitch, rhythm and combined conditions must each supply a separate frozen composer
or a complete map of candidate projects. Whole melody arrays are transplanted;
notes are never zipped to manufacture an ablation. The pitch-writer condition
preserves every note field except pitch and documented same-pitch retrigger cuts;
rhythm-only preserves every note field except start/length. Violations are rejected
before rendering. Combined permits component interactions.
Observed timing, pitch-sequence and velocity changes are reported separately.

The renderer, font, first eight-bar 4/4 chorus, five-millisecond edge fades and
linear -23 LUFS gain are frozen. Learned evaluators consume each condition's
excerpts directory (CLAP: unchanged prompts and --segments 1). Phrase signatures
are exact written-score descriptions, not quality rewards or listening judgments.

    python tools/eval/melody_phrase_ab.py --baseline prior/after/manifest.json \
        --variants variants.json --out target/melody-phrase

Variant JSON schema: {"schema_version": 1, "variants": {"pitch": {
"controls": {"pitch": true, "rhythm": false}, "description": "Writer revision",
"cli": {"path": "/frozen/auris", "sha256": "..."}}, "rhythm": {...},
"combined": {...}}}. Instead of cli, use projects: {"rock-s201": artifact, ...}.
All artifact paths are absolute; all eleven prespecified cases are mandatory.
"""

from __future__ import annotations

import argparse
import copy
import math
import shutil
from itertools import combinations, pairwise
from pathlib import Path

from melody_ab import (
    clip_index,
    content_hash,
    melody_keys,
    reject_relative_assets,
    replace_melody,
    track_index,
    write_comparison,
)
from melody_continuity_ab import (
    CASES,
    MODEL_CONDITIONS,
    TARGET_LUFS,
    artifact,
    compose,
    create_output,
    environment,
    measure_audio,
    read,
    render,
    validate_baseline,
    verified,
)
from seed_diversity import save
from seed_metrics import BAR_TICKS, _lead_clip, _notes, _window

CONTROLS = {
    "baseline": {"pitch": False, "rhythm": False},
    "pitch": {"pitch": True, "rhythm": False},
    "rhythm": {"pitch": False, "rhythm": True},
    "combined": {"pitch": True, "rhythm": True},
}
ARTIFACT_FOLDERS = {
    "project": "projects",
    "wav": "audio",
    "raw_excerpt": "raw-excerpts",
    "excerpt": "excerpts",
}


def phrase_descriptives(project: dict) -> dict:
    """Describe nonoverlapping two/four-bar blocks, retaining rests and held notes."""
    _, start, end, _ = _window(project)
    _, clip = _lead_clip(project, start, end)
    notes = _notes(clip, start, end)
    result = {
        "scope": "First eight-bar 4/4 chorus; exact written ticks, no quantization",
        "ticks_per_quarter": 960,
        "note_count": len(notes),
        "notes_sha256": content_hash(notes),
        "blocks": {},
    }
    for bars in (2, 4):
        width = bars * BAR_TICKS
        blocks = []
        for first in range(0, 8 * BAR_TICKS, width):
            selected = [
                (at - first, length, pitch)
                for at, length, pitch, _ in notes
                if at < first + width and at + length > first
            ]
            pitches = [pitch for _, _, pitch in selected]
            blocks.append(
                {
                    "start_bar": first // BAR_TICKS + 1,
                    "attack_count": sum(at >= 0 for at, _, _ in selected),
                    "carry_in_count": sum(at < 0 for at, _, _ in selected),
                    "onset_duration": [[at, length] for at, length, _ in selected],
                    "relative_pitches": [p - pitches[0] for p in pitches],
                    "intervals": [b - a for a, b in pairwise(pitches)],
                    "first_pitch": pitches[0] if pitches else None,
                    "last_pitch": pitches[-1] if pitches else None,
                }
            )
        pairs = []
        for a, b in combinations(blocks, 2):
            nonempty = bool(a["onset_duration"] and b["onset_duration"])
            rhythm = a["onset_duration"] == b["onset_duration"]
            contour = a["relative_pitches"] == b["relative_pitches"]
            pairs.append(
                {
                    "start_bars": [a["start_bar"], b["start_bar"]],
                    "both_nonempty": nonempty,
                    "same_rhythm": nonempty and rhythm,
                    "same_relative_pitch_sequence": nonempty and contour,
                    "same_transposed_motif": nonempty and rhythm and contour,
                }
            )
        result["blocks"][str(bars)] = {"windows": blocks, "pairs": pairs}
    return result


def observed_changes(source: dict, candidate: dict) -> dict:
    """Compare whole sequences only; unequal counts never acquire note correspondence."""
    old_tracks, new_tracks = track_index(source), track_index(candidate)
    rows = []
    for identity, track in old_tracks.items():
        old, new = clip_index(track), clip_index(new_tracks[identity])
        for key in sorted(melody_keys(track)):
            before, after = (
                sorted(clips[key]["notes"], key=lambda n: (n["start"], n["pitch"]))
                for clips in (old, new)
            )
            rows.append(
                {
                    "track": identity[0],
                    "clip": list(key),
                    "baseline_note_count": len(before),
                    "candidate_note_count": len(after),
                    "note_count_delta": len(after) - len(before),
                    "same_onset_duration": [[n["start"], n["length"]] for n in before]
                    == [[n["start"], n["length"]] for n in after],
                    "same_pitch_sequence": [n["pitch"] for n in before]
                    == [n["pitch"] for n in after],
                    "same_velocity_sequence": [n["velocity"] for n in before]
                    == [n["velocity"] for n in after],
                }
            )
    return {
        "interpretation": "Whole ordered sequences; no note matching or causal quality claim",
        "clips": rows,
    }


def retrigger_gaps(notes: list[dict]) -> list[int | None]:
    """Mirror the composer's reverse same-pitch pass, including equal-start ties."""
    result = [None] * len(notes)
    next_start = {}
    for index in range(len(notes) - 1, -1, -1):
        note = notes[index]
        following = next_start.get(note["pitch"])
        if following is not None and following > note["start"]:
            result[index] = following - note["start"]
        # A duplicate attack at this tick shields the earlier duplicate from a
        # cut, just as parts::untangle does; do not skip equal starts.
        next_start[note["pitch"]] = note["start"]
    return result


def enforce_controls(name: str, source: dict, candidate: dict) -> list[dict]:
    """Reject undeclared changes; return explained pitch-dependent retrigger cuts."""
    allowed = {"pitch": {"pitch", "length"}, "rhythm": {"start", "length"}}
    if name not in allowed:
        return []
    exceptions = []
    old_tracks, new_tracks = track_index(source), track_index(candidate)
    for identity, track in old_tracks.items():
        old, new = clip_index(track), clip_index(new_tracks[identity])
        for key in sorted(melody_keys(track)):
            ordered = [
                sorted(clips[key]["notes"], key=lambda note: note["start"])
                for clips in (old, new)
            ]
            values = [
                [
                    {
                        field: value
                        for field, value in note.items()
                        if field not in allowed[name]
                    }
                    for note in notes
                ]
                for notes in ordered
            ]
            if values[0] != values[1]:
                raise ValueError(
                    f"Exact {name}-only control failed: {identity!r}, {key!r}"
                )
            if name == "pitch":
                gaps = [retrigger_gaps(notes) for notes in ordered]
                # Count and corresponding onsets/other fields have already been
                # established equal. This checks attribution, never synthesizes notes.
                for index, (before, after) in enumerate(zip(*ordered, strict=True)):
                    if before["length"] == after["length"]:
                        continue
                    requested = max(before["length"], after["length"])
                    expected = [
                        min(requested, side[index])
                        if side[index] is not None
                        else requested
                        for side in gaps
                    ]
                    if [before["length"], after["length"]] != expected:
                        raise ValueError(
                            f"Unexplained pitch-writer duration change: {identity!r}, {key!r}, note {index}"
                        )
                    exceptions.append(
                        {
                            "track": identity[0],
                            "clip": list(key),
                            "note_index": index,
                            "start_tick": before["start"],
                            "baseline_pitch": before["pitch"],
                            "candidate_pitch": after["pitch"],
                            "baseline_length": before["length"],
                            "candidate_length": after["length"],
                            "shared_requested_length": requested,
                            "baseline_retrigger_gap": gaps[0][index],
                            "candidate_retrigger_gap": gaps[1][index],
                        }
                    )
    return exceptions


def validate_reference(reference: dict, expected_cases=CASES) -> None:
    """Verify a complete current-continuity corpus and its frozen listening conditions."""
    if reference.get("stage") != "after":
        raise ValueError(
            "Reference must be the completed current-continuity after corpus"
        )
    validate_baseline({**reference, "stage": "before"}, expected_cases)
    if reference.get("model_conditions") != MODEL_CONDITIONS:
        raise ValueError("Reference model conditions changed")
    for name in ("cli", "candidate_cli", "ffmpeg", "soundfont"):
        verified(reference[name])
    for label, row in reference["files"].items():
        project = read(verified(row["project"]))
        reject_relative_assets(project)
        if (row["wav"]["sample_rate"], row["wav"]["channels"]) != (48000, 2):
            raise ValueError(f"Reference requires 48 kHz stereo audio: {label}")
        _, start, end, bpm = _window(project)
        excerpt = row["excerpt"]
        if (excerpt["start_tick"], excerpt["end_tick"], excerpt["bpm"]) != (
            start,
            end,
            bpm,
        ):
            raise ValueError(f"Reference excerpt bounds changed: {label}")
        level = excerpt["normalization"]
        if (
            level.get("target_lufs") != TARGET_LUFS
            or not math.isfinite(level["measured_output_lufs"])
            or abs(level["measured_output_lufs"] - TARGET_LUFS) > 0.15
            or not math.isfinite(level["output_true_peak_dbfs"])
            or level["output_true_peak_dbfs"] > -1
        ):
            raise ValueError(f"Reference loudness conditions changed: {label}")
        phrase_descriptives(project)


def validate_variants(spec: dict, reference: dict) -> None:
    """Require three explicit, complete variant sources before writing any output."""
    if spec.get("schema_version") != 1 or set(spec.get("variants", {})) != {
        "pitch",
        "rhythm",
        "combined",
    }:
        raise ValueError("Exactly pitch, rhythm and combined variants are required")
    for name, entry in spec["variants"].items():
        controls = entry.get("controls")
        if (
            controls != CONTROLS[name]
            or any(type(value) is not bool for value in controls.values())
            or not isinstance(entry.get("description"), str)
            or not entry["description"].strip()
        ):
            raise ValueError(f"Missing or inconsistent explicit controls: {name}")
        if ("cli" in entry) == ("projects" in entry):
            raise ValueError(f"Choose exactly one cli or projects source: {name}")
        for source in entry.get("sources", {}).values():
            if not Path(source["path"]).is_absolute():
                raise ValueError(f"Archived source path must be absolute: {name}")
            verified(source)
        if "cli" in entry:
            if not Path(entry["cli"]["path"]).is_absolute():
                raise ValueError(f"Variant artifact path must be absolute: {name}")
            verified(entry["cli"])
        else:
            if set(entry["projects"]) != set(reference["files"]):
                raise ValueError(
                    f"Variant projects differ from the prespecified cohort: {name}"
                )
            for label, value in entry["projects"].items():
                if not Path(value["path"]).is_absolute():
                    raise ValueError(
                        f"Variant artifact path must be absolute: {name}/{label}"
                    )
                source = read(verified(reference["files"][label]["project"]))
                candidate = read(verified(value))
                replace_melody(source, candidate)
                enforce_controls(name, source, candidate)
                phrase_descriptives(candidate)


def copy_artifact(value: dict, output: Path) -> dict:
    """Copy verified bytes to a new path, retaining measurement metadata."""
    source = verified(value)
    output.parent.mkdir(parents=True, exist_ok=True)
    with source.open("rb") as incoming, output.open("xb") as outgoing:
        shutil.copyfileobj(incoming, outgoing)
    result = {**copy.deepcopy(value), **artifact(output)}
    if result["sha256"] != value["sha256"]:
        raise ValueError(f"Copy changed: {source}")
    return result


def condition_manifest(reference: dict, name: str, reference_path: Path) -> dict:
    """Create a normal evaluator manifest for one explicit composer condition."""
    result = {
        key: copy.deepcopy(reference[key])
        for key in (
            "schema_version",
            "presets",
            "seeds",
            "cases",
            "assets",
            "soundfont",
            "ffmpeg",
            "model_conditions",
            "cli",
        )
    }
    result.update(
        {
            "condition": name,
            "controls": CONTROLS[name],
            "complete": False,
            "reference_manifest": artifact(reference_path),
            "files": {},
        }
    )
    return result


def prepare_experiment(
    baseline_path: Path, variants_path: Path, out: Path, expected_cases=CASES
) -> Path:
    """Produce all four conditions, failing closed on any omitted case or changed input."""
    baseline_path, variants_path, out = (
        path.resolve() for path in (baseline_path, variants_path, out)
    )
    reference, spec = read(baseline_path), read(variants_path)
    inputs = {"reference": artifact(baseline_path), "variants": artifact(variants_path)}
    validate_reference(reference, expected_cases)
    validate_variants(spec, reference)
    if out.exists():
        raise ValueError(f"Refusing to overwrite output: {out}")
    out.mkdir(parents=True)
    experiment = {
        "schema_version": 1,
        "complete": False,
        "inputs": inputs,
        "cases": reference["cases"],
        "design": "Explicit writer-component ablations; frozen baseline backing and renderer; complete melody arrays, no note correspondence inferred",
        "variants": {},
    }
    save(out / "manifest.json", experiment)
    env = environment(Path(reference["assets"]))
    renderer, ffmpeg = verified(reference["cli"]), verified(reference["ffmpeg"])
    for name in CONTROLS:
        folder = create_output(out / name)
        manifest = condition_manifest(reference, name, baseline_path)
        if name != "baseline":
            manifest["implementation"] = spec["variants"][name]
        else:
            manifest["composer_cli"] = reference["candidate_cli"]
        save(folder / "manifest.json", manifest)
        for case in reference["cases"]:
            label = f"{case['preset']}-s{case['seed']}"
            print(f"{name}: {label} ({case['cohort']})", flush=True)
            old = reference["files"][label]
            source = verified(old["project"])
            project = folder / "projects" / label / f"{label}.auris"
            if name == "baseline":
                row = copy.deepcopy(case)
                for field, destination in ARTIFACT_FOLDERS.items():
                    target = (
                        project
                        if field == "project"
                        else folder / destination / f"{label}.wav"
                    )
                    row[field] = copy_artifact(old[field], target)
                row["origin"] = {
                    "method": "Exact verified copy",
                    "project": old["project"],
                }
            else:
                entry = spec["variants"][name]
                if "cli" in entry:
                    candidate = compose(
                        verified(entry["cli"]),
                        folder,
                        case["preset"],
                        case["seed"],
                        env,
                        "candidates",
                    )
                else:
                    target = folder / "candidates" / label / f"{label}.auris"
                    candidate = Path(
                        copy_artifact(entry["projects"][label], target)["path"]
                    )
                source_score, candidate_score = read(source), read(candidate)
                replace_melody(source_score, candidate_score)
                articulation = enforce_controls(name, source_score, candidate_score)
                proof_path = write_comparison(source, candidate, project)
                proof = read(proof_path)
                wav = folder / "audio" / f"{label}.wav"
                render(
                    renderer, project, wav, folder / "logs" / f"{label}-render.log", env
                )
                row = {**case, **measure_audio(folder, label, project, wav, ffmpeg)}
                for field in ("frames", "sample_rate", "channels"):
                    if old["wav"][field] != row["wav"][field]:
                        raise ValueError(
                            f"Paired audio {field} changed: {name}/{label}"
                        )
                for field in (
                    "duration_seconds",
                    "start_tick",
                    "end_tick",
                    "start_frame",
                    "end_frame",
                    "bpm",
                ):
                    if old["excerpt"][field] != row["excerpt"][field]:
                        raise ValueError(
                            f"Paired excerpt {field} changed: {name}/{label}"
                        )
                row.update(
                    {
                        "candidate_project": artifact(candidate),
                        "transplant_manifest": artifact(proof_path),
                        "preserved_content_sha256": proof["preserved_source_sha256"],
                        "backing_notes_sha256": proof["backing_notes_sha256"],
                        "source_melody_sha256": proof["source_melody_sha256"],
                        "output_melody_sha256": proof["output_melody_sha256"],
                        "pitch_articulation_coupling": articulation,
                    }
                )
            saved = read(project)
            row["phrase"] = phrase_descriptives(saved)
            row["observed_changes"] = observed_changes(read(source), saved)
            manifest["files"][label] = row
            save(folder / "manifest.json", manifest)
        for value in inputs.values():
            verified(value)
        if name != "baseline" and "cli" in spec["variants"][name]:
            verified(spec["variants"][name]["cli"])
        verified(reference["cli"])
        manifest["complete"] = True
        save(folder / "manifest.json", manifest)
        experiment["variants"][name] = artifact(folder / "manifest.json")
        save(out / "manifest.json", experiment)
    # Recheck both the documents and every referenced input, including external
    # candidate projects, before declaring the experiment complete.
    for value in inputs.values():
        verified(value)
    validate_reference(reference, expected_cases)
    validate_variants(spec, reference)
    experiment["complete"] = True
    save(out / "manifest.json", experiment)
    return out / "manifest.json"


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", required=True, type=Path)
    parser.add_argument("--variants", required=True, type=Path)
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    print(prepare_experiment(args.baseline, args.variants, args.out))


if __name__ == "__main__":
    main()
