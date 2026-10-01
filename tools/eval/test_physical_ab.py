"""Gain matching must not turn a loudness change into a quality claim."""

import numpy as np
import pytest
from physical_ab import match_levels


def test_matching_equalizes_rms_and_avoids_clipping():
    wave = np.sin(np.arange(48000) * 0.03)
    pair, _ = match_levels(wave * 0.02, wave * 0.4)
    assert np.sqrt(np.mean(pair[0]**2)) == pytest.approx(np.sqrt(np.mean(pair[1]**2)))
    assert max(np.max(abs(audio)) for audio in pair) <= 0.9


def test_matching_rejects_silence():
    with pytest.raises(ValueError, match="silent"):
        match_levels(np.zeros(100), np.ones(100))
