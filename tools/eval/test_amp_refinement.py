"""Guards for paired, memory-bounded native amplifier evaluation."""

from pathlib import Path

import amp_refinement as amp
import numpy as np
import pytest


def test_pairing_rejects_reordered_wet_notes(monkeypatch):
    notes = [{"pickup": "Neck", "pitch": pitch} for pitch in (40, 42)]
    monkeypatch.setattr(amp, "load", lambda root, manifest, effect, split:
                        (notes if effect == "Clean" else notes[::-1], np.zeros((2, 72000))))
    with pytest.raises(ValueError, match="pairs differ"):
        amp.aligned(Path("unused"), {}, "train")


def test_native_chunks_use_antialiasing_and_retain_all_notes(monkeypatch, tmp_path):
    notes = [{"id": str(i), "pitch": 40 + i, "pickup": "Neck"} for i in range(9)]
    monkeypatch.setattr(amp, "aligned", lambda *args: (notes, None, np.zeros((9, 72000))))
    monkeypatch.setattr(amp, "input_files", lambda root, group, destination:
                        ([tmp_path / row["id"] for row in group], np.zeros((len(group), 144000))))
    observed = []

    def measurements(group, target, audio):
        observed.append((len(group), audio))
        return [{**note, **dict.fromkeys(("mel", "attack", "body", "tail", "envelope_db", "t90_seconds"), 1.)}
                for note in group]

    monkeypatch.setattr(amp, "measurements", measurements)

    class Worker:
        def render(self, model, group, params, seconds, hold, **kwargs):
            assert kwargs["rate"] == 48000
            tone = np.sin(2 * np.pi * 15000 * np.arange(144000) / 48000)
            return np.tile(tone, (len(group), 1))

    fitted = {"parameters": dict.fromkeys(amp.BOUNDS, 0.), "simple": {"parameters": {"drive_db": 0.}}}
    report = amp.compare(tmp_path, {}, Worker(), Worker(), "validation", fitted, {}, tmp_path, 4)
    assert [size for size, _ in observed] == [4, 4, 4, 4, 4, 4, 1, 1, 1]
    for _, audio in observed:
        assert audio.shape[-1] == 72000
        # Naive decimation would alias this tone to 9 kHz with RMS 0.707.
        assert np.sqrt(np.mean(audio[:, 1000:-1000] ** 2)) < .005
    for rows in report["rows"].values():
        assert [row["id"] for row in rows] == [str(i) for i in range(9)]
