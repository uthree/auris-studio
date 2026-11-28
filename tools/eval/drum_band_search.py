# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy==2.5.3", "scipy==1.18.1", "soundfile==0.14.0"]
# ///
"""Search one pad's per-band decay and diffuse level on the training mel, development only.

Coordinate search over `t30` (x1.25, then x1.12) and `field_db` (+-2 dB, then +-1 dB) of each
band from `first` up, in `crates/auris-synth/src/drumkit/acoustics.rs`, rebuilding the
`physical_fit_render` worker for every candidate and keeping a move only when the loss of the
pad's training hits falls. The loss is the mean mel loss plus `ENVELOPE_WEIGHT` times the mean
absolute difference in dB between the kit's and the captures' octave levels from 80 ms to 2.8 s,
over the cells the captures hold within 40 dB of their loudest: the mel loss alone barely sees a
ring once it is quiet, and lengthened an open hat's 8 kHz tail already 6 dB too bright. The file
is left holding the best tables; the calibration's single decay cannot give a cymbal's octaves
different rings, and this can.

usage: drum_band_search.py <reference> <repo> <PAD> <family> <first band> <worker copy>
"""

import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
from fit_worker import Renderer
from physical_pack import analysis_audio, errors, load, pack_features
from scipy.signal import butter, sosfilt

root, repo, pad, family, first = Path(sys.argv[1]), Path(sys.argv[2]), sys.argv[3], sys.argv[4], int(sys.argv[5])
table = repo / "crates/auris-synth/src/drumkit/acoustics.rs"
worker = repo / "target/release/examples/physical_fit_render.exe"
copy = Path(sys.argv[6])
manifest = json.loads((root / "notes.json").read_text(encoding="utf-8"))
notes, ref = load(root, manifest, family, "train")
target = pack_features(ref)
RATE = 24000
OCTAVES = (500, 1000, 2000, 4000, 8000)
WINDOWS = ((0.08, 0.2), (0.2, 0.5), (0.5, 1.2), (1.2, 2.8))
ENVELOPE_WEIGHT = 0.01


def grid(audio):
    """Median octave level per window, in dB against the whole note."""
    x = analysis_audio(audio)
    x = x / np.sqrt(np.mean(x ** 2, axis=-1, keepdims=True))
    out = np.zeros((len(OCTAVES), len(WINDOWS)))
    for row, center in enumerate(OCTAVES):
        sos = butter(3, [center / np.sqrt(2), min(center * np.sqrt(2), RATE * 0.49)], fs=RATE,
                     btype="bandpass", output="sos")
        y = sosfilt(sos, x, axis=-1)
        for column, (start, stop) in enumerate(WINDOWS):
            power = np.mean(y[:, int(start * RATE):int(stop * RATE)] ** 2, axis=-1)
            out[row, column] = np.median(10 * np.log10(power + 1e-12))
    return out


captured = grid(ref)
audible = captured > captured.max() - 40
# A long worktree path overflows Windows' command line under sccache.
env = {key: value for key, value in os.environ.items() if key != "RUSTC_WRAPPER"}


def locate(key):
    """The table's text and the match of `key`'s values. A read can come back without them while
    another process (a virus scan, say) holds the file, so it is retried."""
    for _ in range(20):
        text = table.read_text(encoding="utf-8")
        m = re.search(rf"(const {pad}: Acoustics = Acoustics \{{[^}}]*?\n    {key}: \[)([^\]]*)(\])", text, re.DOTALL)
        if m:
            return text, m
        time.sleep(0.5)
    raise RuntimeError(f"{pad}.{key} not found in {table}")


def read(key):
    _, m = locate(key)
    return np.array([float(v) for v in m[2].split(",") if v.strip()])


def write(key, values):
    text, m = locate(key)
    table.write_text(text[:m.start(2)] + ", ".join(f"{v:.4f}" for v in values) + text[m.end(2):], encoding="utf-8")


def score():
    subprocess.run(["cargo", "build", "-j", "2", "--release", "-p", "auris-synth", "--example",
                    "physical_fit_render"], cwd=repo, env=env, check=True, capture_output=True)
    shutil.copy(worker, copy)
    r = Renderer(copy.resolve())
    try:
        audio = r.render("drums", notes, {}, 3, 3)
    finally:
        r.close()
    mel = float(np.mean(errors(target, pack_features(audio))["mel"]))
    envelope = float(np.mean(abs(grid(audio) - captured)[audible]))
    print(f"  mel {mel:.4f} envelope {envelope:.2f} dB", flush=True)
    return mel + ENVELOPE_WEIGHT * envelope


params = {"t30": read("t30"), "field_db": read("field_db")}
best = score()
print(pad, "start", round(best, 4), flush=True)
for factor, step in ((1.25, 2.0), (1.12, 1.0)):
    for band in range(first, len(params["t30"]) - 1):
        for key, moves in (("t30", (factor, 1 / factor)), ("field_db", (step, -step))):
            for move in moves:
                trial = params[key].copy()
                trial[band] = trial[band] * move if key == "t30" else trial[band] + move
                if key == "t30" or band == len(trial) - 2:
                    trial[-1] = trial[-2] if key == "t30" else trial[-1] + (trial[-2] - params[key][-2])
                write(key, trial)
                value = score()
                if value < best - 1e-4:
                    best, params[key] = value, trial
                    print(pad, key, band, round(move, 3), "->", round(best, 4), flush=True)
                    break
                write(key, params[key])
print(pad, "best", round(best, 4), "t30", params["t30"].round(4).tolist(), "field_db", params["field_db"].round(3).tolist(),
      flush=True)
