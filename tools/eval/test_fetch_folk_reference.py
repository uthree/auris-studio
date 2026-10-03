"""Numeric checks for the folk-reference preparation protocol."""

import hashlib
import json
import sys
from itertools import pairwise
from pathlib import Path

import numpy as np
import pytest
import soundfile as sf

sys.path.insert(0, str(Path(__file__).parent))
from fetch_folk_reference import (
    RATE,
    WHISTLE_PITCHES,
    expected_frequency,
    prepare_note,
    prepare_steady_note,
    stable_segments,
    tuning_cents,
)
from folk_copy import load as load_folk
from folk_copy import metric_rows
from folk_copy import render as render_folk
from folk_tuning import measure


def tone(pitch: int, seconds: float, rate: int = 48_000) -> np.ndarray:
    time = np.arange(round(seconds * rate)) / rate
    return .25 * np.sin(2 * np.pi * expected_frequency(pitch) * time)


def test_stable_segments_follows_an_ascending_scale_without_overlap():
    gap = np.zeros(round(.25 * 48_000))
    source = np.concatenate([gap] + [part for pitch in WHISTLE_PITCHES for part in (tone(pitch, .4), gap)])
    spans = stable_segments(source, 48_000, WHISTLE_PITCHES)
    assert len(spans) == len(WHISTLE_PITCHES)
    assert all(a[1] <= b[0] for a, b in pairwise(spans))
    assert all(abs(tuning_cents(source[round(a * 48_000) : round(b * 48_000)], 48_000, pitch)) < 8
               for pitch, (a, b) in zip(WHISTLE_PITCHES, spans, strict=True))


def test_preparation_is_mono_24khz_three_seconds_with_onset_preroll():
    source = tone(69, 1.0)
    prepared = prepare_note(source, .2, .8, 48_000)
    assert prepared.dtype == np.dtype("<f4")
    assert prepared.shape == (3 * RATE,)
    assert np.sqrt(np.mean(prepared[-RATE // 4 :] ** 2)) < .001


def test_frozen_cohort_has_no_pitch_leakage_between_splits():
    manifest_path = Path("target/folk-physical/reference/notes.json")
    if not manifest_path.exists():
        manifest_path = Path(__file__).parent / "references" / "folk-notes.json"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    by_model = {}
    for note in manifest["notes"]:
        by_model.setdefault(note["model"], {}).setdefault(note["pitch"], set()).add(note["split"])
    assert all(len(splits) == 1 for pitches in by_model.values() for splits in pitches.values())
    assert {note["pitch"] for note in manifest["notes"] if note["model"] == "hammered_dulcimer"} == {
        60, 62, 64, 66, 67, 69, 71, 72, 73, 74
    }


def test_whistle_steady_metric_ignores_gain_but_not_timbre():
    note = {"id": "w", "pitch": 74, "split": "train", "hold_seconds": .6}
    reference = np.zeros((1, 72_000), dtype=np.float32)
    reference[0, : round(.6 * RATE)] = tone(74, .6, RATE)
    candidate = np.zeros_like(reference)
    candidate[0, round(.2 * RATE) : round(.8 * RATE)] = 3 * reference[0, : round(.6 * RATE)]
    same = metric_rows("tin_whistle", [note], reference, candidate)[0]
    assert same["mel"] < 1e-5
    candidate[0, round(.2 * RATE) : round(.8 * RATE)] = tone(76, .6, RATE)
    different = metric_rows("tin_whistle", [note], reference, candidate)[0]
    assert different["mel"] > same["mel"] + .01


def test_steady_excerpt_does_not_include_the_next_scale_pitch():
    first = tone(74, .5)
    second = tone(76, .5)
    source = np.concatenate([first, second])
    excerpt = prepare_steady_note(source, 0, .5, RATE * 2)
    # The sample rate argument above deliberately exercises the generic path;
    # the excerpt must end before the neighboring tone despite padding.
    assert np.max(np.abs(excerpt[round(.27 * RATE) :])) < 1e-6


def test_render_passes_each_reference_hold_to_the_worker():
    class FakeRenderer:
        def __init__(self):
            self.holds = []

        def render(self, _model, _notes, _params, _seconds, hold, rate):
            self.holds.append(hold)
            assert _notes[0]["hold"] == hold
            return np.zeros((1, round(3 * rate)), dtype=np.float32)

    fake = FakeRenderer()
    notes = [{"id": "a", "hold_seconds": .31}, {"id": "b", "hold_seconds": .57}]
    render_folk(fake, {"seconds": 3, "rate": RATE}, notes, {}, "tin_whistle", RATE)
    assert fake.holds == [.56, .82]


def test_load_rejects_prepared_hash_corruption_and_pitch_split_leakage(tmp_path):
    (tmp_path / "sources").mkdir()
    source = b"frozen source bytes"
    (tmp_path / "sources/source.bin").write_bytes(source)
    sf.write(tmp_path / "note.wav", tone(74, 3, RATE), RATE, subtype="FLOAT")
    prepared = (tmp_path / "note.wav").read_bytes()
    note = {"id": "w", "pitch": 74, "model": "tin_whistle", "split": "train",
            "source": "source.bin", "path": "note.wav", "hold_seconds": .6,
            "source_sha256": hashlib.sha256(source).hexdigest(),
            "prepared_sha256": hashlib.sha256(prepared).hexdigest()}
    mini = {"notes": [note], "rate": RATE, "seconds": 3}
    (tmp_path / "notes.json").write_text(json.dumps(mini), encoding="utf-8")
    (tmp_path / note["path"]).write_bytes((tmp_path / note["path"]).read_bytes() + b"corrupt")
    with pytest.raises(ValueError, match="prepared reference hash"):
        load_folk(tmp_path, "tin_whistle")
    (tmp_path / note["path"]).write_bytes(prepared)
    (tmp_path / f"sources/{note['source']}").write_bytes(
        (tmp_path / f"sources/{note['source']}").read_bytes() + b"corrupt"
    )
    mini["notes"] = [note]
    (tmp_path / "notes.json").write_text(json.dumps(mini), encoding="utf-8")
    with pytest.raises(ValueError, match="source reference hash"):
        load_folk(tmp_path, "tin_whistle")
    note2 = dict(note, id="duplicate-pitch", split="validation")
    mini["notes"] = [note, note2]
    (tmp_path / f"sources/{note['source']}").write_bytes(source)
    (tmp_path / note["path"]).write_bytes(prepared)
    (tmp_path / "notes.json").write_text(json.dumps(mini), encoding="utf-8")
    with pytest.raises(ValueError, match="pitch leakage"):
        load_folk(tmp_path, "tin_whistle")


def test_tuning_uses_harmonics_when_the_fundamental_is_weak():
    time = np.arange(RATE) / RATE
    expected = expected_frequency(60)
    audio = sum(amplitude * np.sin(2 * np.pi * expected * partial * time)
                for partial, amplitude in ((1, .005), (2, .2), (3, .15), (4, .1)))
    assert abs(tuning_cents(audio, RATE, 60)) < .2


def test_global_tuning_guard_detects_a_louder_upper_mode():
    time = np.arange(round(1.2 * RATE)) / RATE
    frequency = expected_frequency(74) * 2 ** (7 / 1200)
    fundamental = .05 * np.sin(2 * np.pi * frequency * time)
    overtone = .2 * np.sin(2 * np.pi * frequency * 3 * time)
    measured = measure(fundamental + overtone, RATE, 74)
    assert abs(measured["cents"] - 7) < 1
    assert measured["fundamental_peak_fraction"] < .3
    assert abs(measured["strongest_hz"] - 3 * frequency) < 2
