"""Gain matching must not turn a loudness change into a quality claim."""

import numpy as np
import pytest
import soundfile as sf
from physical_ab import main, match_levels


def test_matching_equalizes_rms_and_avoids_clipping():
    wave = np.sin(np.arange(48000) * 0.03)
    pair, _ = match_levels(wave * 0.02, wave * 0.4)
    assert np.sqrt(np.mean(pair[0]**2)) == pytest.approx(np.sqrt(np.mean(pair[1]**2)))
    assert max(np.max(abs(audio)) for audio in pair) <= 0.9


def test_matching_rejects_silence():
    with pytest.raises(ValueError, match="silent"):
        match_levels(np.zeros(100), np.ones(100))


def test_phrase_pairs_keep_their_order_and_gain_matching(tmp_path, monkeypatch):
    before, after = tmp_path / "before", tmp_path / "after"
    before.mkdir()
    after.mkdir()
    for index, name in enumerate(("piano", "guitar", "violin")):
        wave = np.sin(np.arange(1000) * (index + 1) * 0.03)
        sf.write(before / f"{name}.wav", wave * 0.1, 48000, subtype="FLOAT")
        sf.write(after / f"{name}.wav", wave * 0.5, 48000, subtype="FLOAT")
    output = tmp_path / "ab.wav"
    monkeypatch.setattr("sys.argv", ["physical_ab", str(before), str(after), str(output), "--phrases"])
    main()
    audio, rate = sf.read(output)
    assert rate == 48000
    assert len(audio) == 6 * (1000 + 14400)
    for index in range(3):
        first = audio[index * 30800:index * 30800 + 1000]
        second = audio[index * 30800 + 15400:index * 30800 + 16400]
        np.testing.assert_allclose(first, second, atol=2e-7)
