"""Numerical contracts for the development-only spectral fitter."""

import numpy as np
import soundfile as sf
from fit_physical_body import GRID, envelope, response


def test_zero_gain_is_identity():
    np.testing.assert_allclose(response(np.zeros(12)), np.zeros_like(GRID), atol=1e-10)


def test_envelope_is_independent_of_recording_gain(tmp_path):
    time = np.arange(48000) / 48000
    audio = np.sin(2 * np.pi * 220 * time) + 0.3 * np.sin(2 * np.pi * 660 * time)
    paths = [tmp_path / "quiet.wav", tmp_path / "loud.wav"]
    for path, gain in zip(paths, (0.02, 0.5), strict=True):
        sf.write(path, audio * gain, 48000, subtype="FLOAT")
    np.testing.assert_allclose(envelope(paths[:1]), envelope(paths[1:]), atol=1e-5)


def test_eq_gain_changes_the_corresponding_frequency_band():
    gains = np.zeros(12)
    gains[5] = 6
    colored = response(gains)
    assert 5.9 < np.max(colored) < 6.01
    assert abs(colored[0]) < 0.15
    assert abs(colored[-1]) < 0.15
