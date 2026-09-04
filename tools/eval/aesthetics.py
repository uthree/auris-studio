# /// script
# requires-python = ">=3.10"
# dependencies = [
#   "audiobox_aesthetics>=0.0.4",
#   "soundfile>=0.12",
#   "requests>=2.31",
# ]
# ///
"""Learned aesthetic scores for rendered Auris audio.

Runs Meta's Audiobox Aesthetics model (arXiv:2502.05139) over WAV files and prints its four
axes, each on a 1-10 scale predicted from human ratings:

    CE  Content Enjoyment    - how much there is to enjoy in the piece
    CU  Content Usefulness   - how usable it is as material
    PC  Production Complexity- how busy the audio scene is (neither end is "better")
    PQ  Production Quality   - technical quality: clarity, dynamics, spectrum

Score WAVs you already have, or let the tool compose and render presets through the CLI:

    uv run tools/eval/aesthetics.py path/to/*.wav
    uv run tools/eval/aesthetics.py --preset city-pop --preset rock --seeds 3
    uv run tools/eval/aesthetics.py --preset all --json before.json
    ... change something, then ...
    uv run tools/eval/aesthetics.py --preset all --baseline before.json

The first run downloads the model checkpoint from Hugging Face (~1 GB, cached in
``~/.cache/huggingface``); scoring itself takes a few seconds per song on CPU. WAVs are read
with soundfile and handed to the model as tensors, deliberately bypassing torchaudio's decoder
backends, which need FFmpeg on Windows.

These numbers are a regression detector and a coarse sieve, not a target: over-optimising a
learned aesthetic score collapses output diversity (arXiv:2504.16839), and the final judge
stays a pair of ears. Treat a drop against the baseline as a reason to listen, and a rise as a
reason to listen too.
"""

from __future__ import annotations

import argparse
import json
import statistics
import subprocess
import sys
import tempfile
import warnings
from pathlib import Path

AXES = ("CE", "CU", "PC", "PQ")
REPO = Path(__file__).resolve().parents[2]
EXTRA_SEEDS = (101, 102, 103, 104, 105, 106, 107)


def preset_names() -> list[str]:
    """The presets the CLI knows, read off `auris presets`.

    A name line is indented exactly two spaces; the key-tempo-groove line under each one is
    indented much further, which is what tells the two apart.
    """
    names = []
    for line in run_cli("presets").splitlines():
        if line.startswith("  ") and not line.startswith("   "):
            names.append(line.split()[0])
    return names


def run_cli(*args: str) -> str:
    done = subprocess.run(
        ["cargo", "run", "-q", "-p", "auris-cli", "--", *args],
        cwd=REPO,
        capture_output=True,
        # The CLI speaks UTF-8 — preset descriptions carry Japanese — and Windows would
        # otherwise decode its output with a legacy code page and fall over.
        encoding="utf-8",
        errors="replace",
        check=False,
    )
    if done.returncode != 0:
        sys.exit(f"auris {' '.join(args)} failed:\n{done.stderr}")
    return done.stdout


def render_presets(presets: list[str], seeds: int, workdir: Path) -> list[Path]:
    """Composes and renders each preset at its own seed plus `seeds - 1` fixed extras."""
    wavs = []
    for name in presets:
        for seed in (None, *EXTRA_SEEDS[: max(seeds, 1) - 1]):
            label = f"{name}-s{seed}" if seed is not None else name
            project = workdir / f"{label}.auris"
            wav = workdir / f"{label}.wav"
            compose = ["compose", "--preset", name, "-o", str(project), "--force"]
            if seed is not None:
                compose += ["--seed", str(seed)]
            run_cli(*compose)
            run_cli(
                "render",
                str(workdir / label / f"{label}.auris"),
                "--bit-depth",
                "32",
                "--no-tail",
                "-o",
                str(wav),
            )
            wavs.append(wav)
    return wavs


def collect_wavs(paths: list[str]) -> list[Path]:
    out: list[Path] = []
    for text in paths:
        path = Path(text)
        if path.is_dir():
            out.extend(sorted(path.rglob("*.wav")))
        elif path.suffix.lower() == ".wav" and path.is_file():
            out.append(path)
        else:
            sys.exit(f"not a wav or a folder of them: {path}")
    return out


def score_labels(wavs: list[Path]) -> list[str]:
    """Stable labels that keep same-named files from overwriting one another."""
    counts: dict[str, int] = {}
    for wav in wavs:
        counts[wav.stem] = counts.get(wav.stem, 0) + 1
    return [wav.stem if counts[wav.stem] == 1 else wav.as_posix() for wav in wavs]


def score(wavs: list[Path]) -> dict[str, dict[str, float]]:
    """One row of the four axes per file, with collision-free keys."""
    # Imported here so `--help` and argument errors stay instant.
    import soundfile
    import torch
    from audiobox_aesthetics.infer import initialize_predictor

    warnings.filterwarnings("ignore")
    predictor = initialize_predictor()
    scores: dict[str, dict[str, float]] = {}
    for wav, label in zip(wavs, score_labels(wavs), strict=True):
        data, rate = soundfile.read(wav, dtype="float32", always_2d=True)
        row = predictor.forward(
            [{"path": torch.from_numpy(data.T), "sample_rate": rate}]
        )[0]
        scores[label] = {axis: round(row[axis], 3) for axis in AXES}
        print(format_row(label, scores[label]), flush=True)
    return scores


def format_row(name: str, row: dict[str, float], delta: dict[str, float] | None = None) -> str:
    cells = []
    for axis in AXES:
        cell = f"{row[axis]:5.2f}"
        if delta is not None:
            cell += f" ({delta[axis]:+.2f})"
        cells.append(cell)
    return f"  {name:<24} " + "  ".join(cells)


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Audiobox Aesthetics scores for rendered Auris audio."
    )
    parser.add_argument("wavs", nargs="*", help="WAV files or folders of them")
    parser.add_argument(
        "--preset",
        action="append",
        default=[],
        help="compose and render this preset first ('all' for every one); repeatable",
    )
    parser.add_argument(
        "--seeds",
        type=int,
        default=1,
        help="renders per preset: its own seed plus fixed extras (default 1)",
    )
    parser.add_argument("--json", type=Path, help="write the scores to this file")
    parser.add_argument(
        "--baseline", type=Path, help="print each score's change against this earlier --json"
    )
    parser.add_argument(
        "--workdir",
        type=Path,
        help="where rendered presets go (default: a temporary folder)",
    )
    args = parser.parse_args()
    if not args.wavs and not args.preset:
        parser.error("nothing to score: pass WAVs, or --preset")

    wavs = collect_wavs(args.wavs)
    if args.preset:
        presets = preset_names() if "all" in args.preset else args.preset
        workdir = args.workdir or Path(tempfile.mkdtemp(prefix="auris-aesthetics-"))
        workdir.mkdir(parents=True, exist_ok=True)
        # Plain "x": Windows consoles still default to legacy code pages, and a mojibake
        # progress line is a poor first impression for a measuring instrument.
        print(f"rendering {len(presets)} preset(s) x {max(args.seeds, 1)} seed(s) into {workdir}")
        wavs += render_presets(presets, args.seeds, workdir)

    print(f"  {'file':<24} " + "  ".join(f"{axis:>5}" for axis in AXES))
    scores = score(wavs)

    if len(scores) > 1:
        means = {
            axis: statistics.mean(row[axis] for row in scores.values()) for axis in AXES
        }
        print(format_row("mean", {axis: round(means[axis], 3) for axis in AXES}))

    if args.baseline:
        before = json.loads(args.baseline.read_text())
        print("\nagainst the baseline:")
        for name, row in scores.items():
            if name not in before:
                print(f"  {name:<24} (not in the baseline)")
                continue
            delta = {axis: row[axis] - before[name][axis] for axis in AXES}
            print(format_row(name, row, delta))

    if args.json:
        args.json.write_text(json.dumps(scores, indent=2) + "\n")
        print(f"\nwrote {args.json}")


if __name__ == "__main__":
    main()
