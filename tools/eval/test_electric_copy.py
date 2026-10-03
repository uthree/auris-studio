"""Checks for cohort isolation, bounded downloads and gain-independent DI evaluation."""

import json
from pathlib import Path
from types import SimpleNamespace

import numpy as np
import pytest
from electric_copy import amp_audio, di_audio, measurements, paired
from fetch_electric_reference import cohort, range_bytes


def test_canonical_cohort_has_every_pitch_once_and_keeps_pairs_in_the_same_split():
    notes = list(cohort())
    assert sorted(pitch for _, _, pitch in notes) == list(range(40, 87))
    manifest = json.loads(
        (Path(__file__).parent / "references/electric-notes.json").read_text()
    )
    groups = {}
    for note in manifest["notes"]:
        groups.setdefault(note["pitch"], []).append(note)
    assert len(groups) == 47
    for group in groups.values():
        assert len(group) == 4
        assert len({note["split"] for note in group}) == 1
        assert {(note["effect"], note["pickup"]) for note in group} == {
            (effect, pickup) for effect in ("Clean", "BluesDriver") for pickup in ("Neck", "Bridge")
        }
    assert sum(note["split"] == "validation" for note in manifest["notes"]) == 92


@pytest.mark.parametrize("status, header, content", [
    (200, "bytes 10-13/100", b"abcd"),
    (206, "bytes 0-3/100", b"abcd"),
    (206, "bytes 10-13/100", b"abc"),
])
def test_range_fetch_rejects_unbounded_or_incorrect_responses(monkeypatch, status, header, content):
    response = SimpleNamespace(status_code=status, headers={"Content-Range": header},
                               iter_content=lambda size: [content], close=lambda: None,
                               raise_for_status=lambda: None)
    monkeypatch.setattr("fetch_electric_reference.requests.get", lambda *args, **kwargs: response)
    with pytest.raises(ValueError, match="range"):
        range_bytes("https://example.org/cohort.zip", 10, 13)


def test_pickup_groups_render_in_reference_order_with_independent_controls():
    calls = []

    class Renderer:
        def render(self, model, notes, controls, seconds, hold, **kwargs):
            calls.append((notes, controls))
            return np.full((len(notes), 3), controls["pickup_position"])

    notes = [{"pickup": "Bridge", "pitch": 64}, {"pickup": "Neck", "pitch": 64}]
    result = di_audio(Renderer(), notes, {"hardness": 0.3, "neck_position": 0.24, "bridge_position": 0.06})
    np.testing.assert_allclose(result, [[0.06] * 3, [0.24] * 3])
    assert [controls["hardness"] for _, controls in calls] == [0.3, 0.3]
    assert all("neck_position" not in controls for _, controls in calls)


def test_measurements_ignore_gain_and_polarity_but_retain_attack_delay():
    time = np.arange(72000) / 24000
    audio = (np.sin(2 * np.pi * 440 * time) * np.exp(-time / 0.3))[None, :]
    notes = [{"id": "note", "pitch": 69, "pickup": "Neck"}]
    row = measurements(notes, audio, -audio * 0.01)[0]
    assert row["mel"] < 1e-12
    assert row["envelope_db"] < 1e-10
    assert row["t90_seconds"] < 1 / 24000
    delayed = np.pad(audio, [(0, 0), (1200, 0)])[:, :72000]
    assert measurements(notes, audio, delayed)[0]["attack"] > 0.05


def test_paired_report_rejects_changed_cohort_and_bootstraps_pitch_groups():
    keys = ("mel", "attack", "body", "tail", "envelope_db", "t90_seconds")
    before = [{"id": f"{pitch}-{pickup}", "pitch": pitch, **dict.fromkeys(keys, 1.0)}
              for pitch in (64, 66, 68) for pickup in ("Neck", "Bridge")]
    after = [row | dict.fromkeys(keys, 0.5) for row in before]
    result = paired(before, after)["mel"]
    assert result["reduction_percent"] == 50
    assert result["paired_delta_ci95"] == [-0.5, -0.5]
    with pytest.raises(ValueError, match="cohorts differ"):
        paired(before, after[::-1])


def test_amp_surrogate_is_causal_finite_and_does_not_generate_silence_energy():
    params = {"drive_db": 30, "bass_db": 0, "mid_db": 0, "treble_db": 0}
    assert not np.any(amp_audio(np.zeros((1, 2400)), params))
    impulse = np.zeros((1, 2400))
    impulse[0, 300] = 0.2
    audio = amp_audio(impulse, params)
    assert not np.any(audio[0, :300])
    assert np.max(abs(audio)) > 0.01
    assert np.isfinite(audio).all()
