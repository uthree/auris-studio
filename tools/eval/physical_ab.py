# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "soundfile>=0.12"]
# ///
"""Make an RMS-matched A/B audition from the six-model physical_demo WAVs."""

import argparse
import json
from pathlib import Path

import numpy as np
import soundfile as sf


def match_levels(before, after):
    """Use a common RMS target while keeping both peaks below 0.9."""
    rms = [np.sqrt(np.mean(audio**2)) for audio in (before, after)]
    if min(rms) < 1e-8:
        raise ValueError("cannot level-match a silent probe")
    target = min(0.05, *(0.9 * level / np.max(abs(audio))
                         for audio, level in zip((before, after), rms, strict=True)))
    gains = [target / level for level in rms]
    return [audio * gain for audio, gain in zip((before, after), gains, strict=True)], gains


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    before, rate = sf.read(args.before)
    after, after_rate = sf.read(args.after)
    if rate != after_rate or before.shape != after.shape or before.ndim != 1:
        raise ValueError("matching mono physical_demo renders are required")
    parts, report = [], {}
    for name, index in (("piano", 0), ("guitar", 1), ("violin", 5)):
        region = slice(index * rate * 4, (index + 1) * rate * 4)
        pair, gains = match_levels(before[region], after[region])
        for audio in pair:
            parts.extend((audio, np.zeros(int(rate * 0.3))))
        report[name] = {"order": ["before", "after"], "gains": gains}
    sf.write(args.output, np.concatenate(parts), rate, subtype="PCM_24")
    args.output.with_suffix(".json").write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
