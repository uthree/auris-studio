import copy
import json
from pathlib import Path

import melody_phrase_ab as ab
import numpy as np
import pytest
import soundfile as sf

CASES = (("pop-band", 102, "diagnostic"), ("rock", 201, "held-out"))


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


def project(seed=102):
    return {
        "format_version": 1,
        "sample_rate": 48000,
        "tempo_map": {"points": [{"tick": 0, "bpm": 960}]},
        "signatures": {
            "points": [{"tick": 0, "signature": {"numerator": 4, "denominator": 4}}]
        },
        "sections": {
            "points": [
                {"tick": 0, "label": "chorus"},
                {"tick": 30720, "label": "outro"},
            ]
        },
        "harmony": {"key": "C"},
        "tracks": [
            {
                "id": 1,
                "name": "Lead",
                "mixer": {"gain_db": -4},
                "kind": {
                    "type": "instrument",
                    "clips": [
                        {
                            "id": 2,
                            "name": "Chorus",
                            "start": 0,
                            "length": 30720,
                            "transforms": [{"type": "humanize", "seed": 123}],
                            "recipe": {
                                "preset": "lead",
                                "seed": seed,
                                "text_digest": 1,
                            },
                            "notes": [
                                {
                                    "start": bar * 3840 + at,
                                    "length": 480,
                                    "pitch": pitch,
                                    "velocity": 0.7,
                                }
                                for bar in range(8)
                                for at, pitch in ((0, 60), (960, 64), (1920, 62))
                            ],
                        }
                    ],
                },
            },
            {
                "id": 3,
                "name": "Chords",
                "kind": {
                    "type": "instrument",
                    "clips": [
                        {
                            "id": 4,
                            "name": "Chorus",
                            "start": 0,
                            "length": 30720,
                            "recipe": {"preset": "chords"},
                            "notes": [
                                {
                                    "start": 0,
                                    "length": 30720,
                                    "pitch": 48,
                                    "velocity": 0.8,
                                }
                            ],
                        }
                    ],
                },
            },
        ],
    }


def notes(value):
    return value["tracks"][0]["kind"]["clips"][0]["notes"]


def variant(source, name):
    result = copy.deepcopy(source)
    if name in ("pitch", "combined"):
        for note in notes(result):
            note["pitch"] += 3
    if name in ("rhythm", "combined"):
        for note in notes(result):
            note["start"] += 240
    if name == "combined":
        notes(result).append(
            {"start": 30000, "length": 480, "pitch": 67, "velocity": 0.5}
        )
    result["tracks"][0]["kind"]["clips"][0]["recipe"]["text_digest"] = 2
    # Fresh full compositions may change backing and performance; neither survives.
    result["tracks"][0]["mixer"] = {"gain_db": 20}
    result["tracks"][0]["kind"]["clips"][0]["transforms"] = []
    result["tracks"][1]["kind"]["clips"][0]["notes"] = []
    return result


def fixture(tmp_path, monkeypatch):
    reference = {
        "schema_version": 1,
        "stage": "after",
        "complete": True,
        "presets": ["pop-band", "rock"],
        "seeds": [102, 201],
        "cases": [dict(zip(("preset", "seed", "cohort"), case)) for case in CASES],
        "assets": str(tmp_path / "assets"),
        "model_conditions": ab.MODEL_CONDITIONS,
        "files": {},
    }
    for name in ("cli", "candidate_cli", "ffmpeg", "soundfont"):
        path = tmp_path / f"{name}.bin"
        path.write_bytes(name.encode())
        reference[name] = ab.artifact(path)
    wave = np.full((96000, 2), 0.1, dtype=np.float32)
    for preset, seed, cohort in CASES:
        label = f"{preset}-s{seed}"
        score = write(tmp_path / "reference" / label / f"{label}.auris", project(seed))
        wav = score.with_suffix(".wav")
        sf.write(wav, wave, 48000, subtype="FLOAT")
        reference["files"][label] = {
            "preset": preset,
            "seed": seed,
            "cohort": cohort,
            "project": ab.artifact(score),
            "wav": {
                **ab.artifact(wav),
                "frames": 96000,
                "sample_rate": 48000,
                "channels": 2,
            },
            "raw_excerpt": ab.artifact(wav),
            "excerpt": {
                **ab.artifact(wav),
                "duration_seconds": 2,
                "start_tick": 0,
                "end_tick": 30720,
                "start_frame": 0,
                "end_frame": 96000,
                "bpm": 960,
                "normalization": {
                    "target_lufs": -23,
                    "measured_output_lufs": -23,
                    "output_true_peak_dbfs": -10,
                },
            },
        }
    source_path = write(tmp_path / "reference.json", reference)
    spec = {"schema_version": 1, "variants": {}}
    for name in ("pitch", "rhythm", "combined"):
        projects = {}
        for label, row in reference["files"].items():
            candidate = write(
                tmp_path / name / label / f"{label}.auris",
                variant(ab.read(Path(row["project"]["path"])), name),
            )
            projects[label] = ab.artifact(candidate)
        spec["variants"][name] = {
            "controls": ab.CONTROLS[name],
            "description": f"Explicit {name} fixture",
            "projects": projects,
        }
    spec_path = write(tmp_path / "variants.json", spec)
    renders = []

    def fake_render(cli, score, wav, log, env):
        renders.append((cli, ab.read(score)))
        sf.write(wav, wave * 1.1, 48000, subtype="FLOAT")

    def fake_measure(out, label, score, wav, ffmpeg):
        row = copy.deepcopy(reference["files"][label])
        row["project"], row["wav"] = (
            ab.artifact(score),
            {**row["wav"], **ab.artifact(wav)},
        )
        for field, folder in (("raw_excerpt", "raw-excerpts"), ("excerpt", "excerpts")):
            target = out / folder / f"{label}.wav"
            target.write_bytes(wav.read_bytes())
            row[field].update(ab.artifact(target))
        return row

    monkeypatch.setattr(ab, "render", fake_render)
    monkeypatch.setattr(ab, "measure_audio", fake_measure)
    return source_path, spec_path, reference, spec, renders


def test_all_four_conditions_keep_exact_baseline_backing_audio_conditions_and_cohort(
    tmp_path, monkeypatch
):
    source, spec_path, reference, _, renders = fixture(tmp_path, monkeypatch)
    out = tmp_path / "output"
    result = ab.read(ab.prepare_experiment(source, spec_path, out, CASES))
    assert result["complete"] and list(result["variants"]) == list(ab.CONTROLS)
    assert len(renders) == 6
    assert {cli for cli, _ in renders} == {Path(reference["cli"]["path"])}
    for name in ab.CONTROLS:
        manifest = ab.read(ab.verified(result["variants"][name]))
        assert manifest["complete"] and manifest["cases"] == reference["cases"]
        assert set(manifest["files"]) == set(reference["files"])
        for label, row in manifest["files"].items():
            saved = ab.read(ab.verified(row["project"]))
            original = ab.read(Path(reference["files"][label]["project"]["path"]))
            assert saved["tracks"][1] == original["tracks"][1]
            assert saved["tracks"][0]["mixer"] == original["tracks"][0]["mixer"]
            assert (
                saved["tracks"][0]["kind"]["clips"][0]["transforms"]
                == original["tracks"][0]["kind"]["clips"][0]["transforms"]
            )
            assert row["excerpt"]["normalization"]["target_lufs"] == -23
            if name == "baseline":
                for field in ab.ARTIFACT_FOLDERS:
                    assert (
                        row[field]["sha256"]
                        == reference["files"][label][field]["sha256"]
                    )
            else:
                assert row["source_melody_sha256"] != row["output_melody_sha256"]
                assert len(notes(saved)) == len(notes(original)) + (name == "combined")
                assert row["observed_changes"]["clips"][0]["same_pitch_sequence"] == (
                    name == "rhythm"
                )


@pytest.mark.parametrize(
    "change",
    [
        "missing-case",
        "extra-project",
        "wrong-control",
        "integer-control",
        "bad-hash",
        "wrong-harmony",
        "pitch-timing",
        "rhythm-pitch",
    ],
)
def test_invalid_variant_is_rejected_before_output_or_render(
    tmp_path, monkeypatch, change
):
    source, spec_path, _, spec, renders = fixture(tmp_path, monkeypatch)
    entry = spec["variants"]["pitch"]
    if change == "missing-case":
        entry["projects"].pop("rock-s201")
    elif change == "extra-project":
        entry["projects"]["rock-s202"] = entry["projects"]["rock-s201"]
    elif change == "wrong-control":
        entry["controls"] = {"pitch": True, "rhythm": True}
    elif change == "integer-control":
        entry["controls"] = {"pitch": 1, "rhythm": 0}
    elif change == "bad-hash":
        entry["projects"]["rock-s201"]["sha256"] = "0" * 64
    else:
        if change == "rhythm-pitch":
            entry = spec["variants"]["rhythm"]
        path = Path(entry["projects"]["rock-s201"]["path"])
        score = ab.read(path)
        if change == "wrong-harmony":
            score["harmony"] = {"key": "D"}
        elif change == "pitch-timing":
            notes(score)[0]["length"] += 1
        else:
            notes(score)[0]["pitch"] += 1
        write(path, score)
        entry["projects"]["rock-s201"] = ab.artifact(path)
    write(spec_path, spec)
    out = tmp_path / "output"
    with pytest.raises(ValueError):
        ab.prepare_experiment(source, spec_path, out, CASES)
    assert not out.exists() and not renders


@pytest.mark.parametrize("field", ["cli", "candidate_cli", "soundfont", "ffmpeg"])
def test_changed_frozen_artifact_is_rejected_before_output(
    tmp_path, monkeypatch, field
):
    source, spec, reference, _, _ = fixture(tmp_path, monkeypatch)
    Path(reference[field]["path"]).write_bytes(b"changed")
    with pytest.raises(ValueError, match="Artifact changed"):
        ab.prepare_experiment(source, spec, tmp_path / "output", CASES)
    assert not (tmp_path / "output").exists()


def test_unequal_count_is_rejected_for_exact_axes_but_not_combined():
    source = project()
    candidate = copy.deepcopy(source)
    notes(candidate).append(
        {"start": 30000, "length": 480, "pitch": 67, "velocity": 0.7}
    )
    for name in ("pitch", "rhythm"):
        with pytest.raises(ValueError, match="Exact"):
            ab.enforce_controls(name, source, candidate)
    ab.enforce_controls("combined", source, candidate)
    observed = ab.observed_changes(source, candidate)["clips"][0]
    assert observed["note_count_delta"] == 1
    assert not observed["same_pitch_sequence"] and not observed["same_onset_duration"]


@pytest.mark.parametrize("name", ["pitch", "rhythm"])
def test_velocity_only_corruption_fails_each_exact_axis(name):
    source = project()
    candidate = copy.deepcopy(source)
    notes(candidate)[0]["velocity"] = 0.8
    with pytest.raises(ValueError, match="Exact"):
        ab.enforce_controls(name, source, candidate)


def test_phrase_signatures_retain_transposition_rests_duration_and_carry():
    source = project()
    original = copy.deepcopy(source)
    a = ab.phrase_descriptives(source)
    assert source == original
    two = a["blocks"]["2"]
    assert len(two["windows"]) == 4 and len(two["pairs"]) == 6
    assert all(pair["same_transposed_motif"] for pair in two["pairs"])
    shifted = copy.deepcopy(source)
    for note in notes(shifted):
        if note["start"] >= 7680:
            note["pitch"] += 5
    b = ab.phrase_descriptives(shifted)
    assert b["blocks"]["2"]["pairs"][0]["same_transposed_motif"]
    notes(shifted)[6]["start"] += 240
    notes(shifted)[6]["length"] -= 120
    c = ab.phrase_descriptives(shifted)
    assert not c["blocks"]["2"]["pairs"][0]["same_rhythm"]
    carried = project()
    notes(carried)[5]["length"] = 2500
    description = ab.phrase_descriptives(carried)["blocks"]["2"]["windows"][1]
    assert description["carry_in_count"] == 1
    assert description["onset_duration"][0] == [-1920, 2500]


def test_changed_inputs_during_generation_leave_experiment_incomplete(
    tmp_path, monkeypatch
):
    source, spec_path, _, _, renders = fixture(tmp_path, monkeypatch)
    original = ab.render

    def mutate_on_last_render(*args):
        original(*args)
        if len(renders) == 6:
            spec_path.write_text(spec_path.read_text() + "\n", encoding="utf-8")

    monkeypatch.setattr(ab, "render", mutate_on_last_render)
    out = tmp_path / "output"
    with pytest.raises(ValueError, match="Artifact changed"):
        ab.prepare_experiment(source, spec_path, out, CASES)
    assert not ab.read(out / "manifest.json")["complete"]


def test_frozen_composer_mode_uses_distinct_inputs_but_one_renderer(
    tmp_path, monkeypatch
):
    source, spec_path, reference, spec, renders = fixture(tmp_path, monkeypatch)
    composers = {}
    for name, entry in spec["variants"].items():
        cli = tmp_path / f"{name}.exe"
        cli.write_bytes(name.encode())
        composers[cli] = name
        entry.pop("projects")
        entry["cli"] = ab.artifact(cli)
    write(spec_path, spec)
    composed = []

    def fake_compose(cli, out, preset, seed, env, folder):
        composed.append(cli)
        label = f"{preset}-s{seed}"
        return write(
            out / folder / label / f"{label}.auris",
            variant(project(seed), composers[cli]),
        )

    monkeypatch.setattr(ab, "compose", fake_compose)
    ab.prepare_experiment(source, spec_path, tmp_path / "output", CASES)
    assert set(composed) == set(composers) and len(composed) == 6
    assert {cli for cli, _ in renders} == {Path(reference["cli"]["path"])}


def test_cli_candidate_failing_exact_controls_never_reaches_renderer(
    tmp_path, monkeypatch
):
    source, spec_path, _, spec, renders = fixture(tmp_path, monkeypatch)
    cli = tmp_path / "pitch.exe"
    cli.write_bytes(b"unexpectedly changes rhythm too")
    entry = spec["variants"]["pitch"]
    entry.pop("projects")
    entry["cli"] = ab.artifact(cli)
    write(spec_path, spec)

    def invalid_compose(cli, out, preset, seed, env, folder):
        label = f"{preset}-s{seed}"
        score = variant(project(seed), "pitch")
        notes(score)[0]["length"] += 120
        return write(out / folder / label / f"{label}.auris", score)

    monkeypatch.setattr(ab, "compose", invalid_compose)
    out = tmp_path / "output"
    with pytest.raises(ValueError, match="Unexplained pitch-writer duration"):
        ab.prepare_experiment(source, spec_path, out, CASES)
    assert not renders
    assert not ab.read(out / "manifest.json")["complete"]
    assert not ab.read(out / "pitch" / "manifest.json")["complete"]


def test_pitch_retrigger_articulation_is_attributed_without_modifying_notes():
    source = project()
    notes(source)[0]["length"] = 1100
    candidate = copy.deepcopy(source)
    notes(candidate)[0].update(pitch=64, length=960)
    snapshots = copy.deepcopy((source, candidate))
    cuts = ab.enforce_controls("pitch", source, candidate)
    assert (source, candidate) == snapshots
    assert len(cuts) == 1
    assert cuts[0]["baseline_length"] == 1100
    assert cuts[0]["candidate_length"] == cuts[0]["candidate_retrigger_gap"] == 960
    assert cuts[0]["shared_requested_length"] == 1100
    # Removing the retrigger restores the shared requested length, symmetrically.
    extensions = ab.enforce_controls("pitch", candidate, source)
    assert extensions[0]["baseline_length"] == 960
    assert extensions[0]["candidate_length"] == 1100


@pytest.mark.parametrize(
    "change", ["arbitrary-trim", "arbitrary-extension", "wrong-cut", "changed-onset"]
)
def test_pitch_articulation_exception_rejects_unexplained_changes(change):
    source = project()
    notes(source)[0]["length"] = 1100
    candidate = copy.deepcopy(source)
    if change == "arbitrary-trim":
        notes(candidate)[0]["length"] = 1000
    elif change == "arbitrary-extension":
        notes(candidate)[0]["length"] = 1200
    elif change == "wrong-cut":
        notes(candidate)[0].update(pitch=64, length=959)
    else:
        notes(candidate)[0].update(pitch=64, length=960, start=1)
    with pytest.raises(ValueError):
        ab.enforce_controls("pitch", source, candidate)


def test_retrigger_pass_preserves_equal_start_behavior_and_nearest_pitch():
    events = [
        {"pitch": 60, "start": 0},
        {"pitch": 60, "start": 0},
        {"pitch": 62, "start": 480},
        {"pitch": 60, "start": 960},
        {"pitch": 60, "start": 1920},
    ]
    assert ab.retrigger_gaps(events) == [None, 960, None, 960, None]


def test_changed_archived_writer_source_is_rejected_before_output(
    tmp_path, monkeypatch
):
    source, spec_path, _, spec, renders = fixture(tmp_path, monkeypatch)
    writer = tmp_path / "writer.rs"
    writer.write_bytes(b"frozen writer")
    spec["variants"]["pitch"]["sources"] = {"writer.rs": ab.artifact(writer)}
    write(spec_path, spec)
    writer.write_bytes(b"unexpected source change")
    out = tmp_path / "output"
    with pytest.raises(ValueError, match="Artifact changed"):
        ab.prepare_experiment(source, spec_path, out, CASES)
    assert not out.exists() and not renders
