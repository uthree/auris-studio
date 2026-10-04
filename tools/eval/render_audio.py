"""CLI rendering helpers shared by the development music evaluator and tuner."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
EXTRA_SEEDS = (101, 102, 103, 104, 105, 106, 107)
CLI: Path | None = None


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
        [str(CLI), *args]
        if CLI
        else ["cargo", "run", "-q", "-p", "auris-cli", "--", *args],
        cwd=REPO,
        capture_output=True,
        # The CLI speaks UTF-8 â€” preset descriptions carry Japanese â€” and Windows would
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
