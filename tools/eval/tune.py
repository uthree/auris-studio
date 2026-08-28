# /// script
# requires-python = ">=3.11"
# dependencies = [
#   "audiobox_aesthetics>=0.0.4",
#   "soundfile>=0.12",
#   "requests>=2.31",
#   "optuna>=4.0",
# ]
# ///
"""Black-box tuning of a preset's dials against the learned aesthetic score.

For each preset, Optuna's TPE searches the *continuous* dials of the specification — humanize,
dynamics, fill, variation, the four mood numbers, tempo within a narrow band, and swing only
where the preset already swings — while everything that makes the genre the genre (key, groove,
progression, form, roster) stays exactly as written. Each trial composes and renders the preset
through `auris-cli` with `--set` overrides and scores the audio with Audiobox Aesthetics
(arXiv:2502.05139); the objective is Content Enjoyment averaged over two fixed seeds, so the
search is deterministic and never chases one lucky draw.

    uv run tools/eval/tune.py --preset all --trials 18 --out tune-results.json
    uv run tools/eval/tune.py --preset chiptune --trials 30

The best dials are then *validated* on two seeds the search never saw, against the preset's own
dials on the same seeds — the number to trust is that held-out delta, not the search score. A
candidate that wins in search and loses in validation has learned the seeds, not the music.

This is a lead generator, not a judge. Over-optimising a learned score collapses diversity
(arXiv:2504.16839), the axes are blind to whether a piece is *the same piece* every seed, and
nothing here listens. Adopt a winner by editing `preset.rs` only after hearing the renders the
tool leaves in its workdir.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
import tempfile
import tomllib
import warnings
from pathlib import Path

AXES = ("CE", "CU", "PC", "PQ")
REPO = Path(__file__).resolve().parents[2]

# Two seeds to search on, two the search never sees. Fixed, so every run is reproducible and
# two candidates are always compared on identical draws.
SEARCH_SEEDS = (None, 101)  # None = the preset's own seed
VALIDATION_SEEDS = (103, 104)

# The continuous dials and the range each may roam. Deliberately narrower than what the format
# allows: the search is refining a genre, not escaping one.
SPACE = {
    "humanize": (0.0, 0.7),
    "dynamics": (0.4, 1.0),
    "fill": (0.0, 1.0),
    "variation": (0.0, 0.5),
    "energy": (0.25, 0.9),
    "tension": (0.2, 0.9),
    "brightness": (0.25, 0.75),
    "syncopation": (0.05, 0.75),
}
TEMPO_BAND = 0.06  # ±6 %: enough to breathe, not enough to change what dance this is.
SWING_BAND = 6  # and only for presets that already swing; straight stays straight.


def cli(*args: str) -> str:
    """Runs the auris CLI, preferring the already-built binary over a cargo round trip."""
    binary = REPO / "target" / "debug" / "auris.exe"
    command = (
        [str(binary), *args]
        if binary.exists()
        else ["cargo", "run", "-q", "-p", "auris-cli", "--", *args]
    )
    done = subprocess.run(
        command, cwd=REPO, capture_output=True, encoding="utf-8", errors="replace"
    )
    if done.returncode != 0:
        sys.exit(f"auris {' '.join(args)} failed:\n{done.stderr}")
    return done.stdout


def preset_names() -> list[str]:
    return [
        line.split()[0]
        for line in cli("presets").splitlines()
        if line.startswith("  ") and not line.startswith("   ")
    ]


def current_dials(preset: str) -> dict[str, float]:
    """The preset's resolved dials, read off `compose --print` so nothing is guessed."""
    resolved = tomllib.loads(cli("compose", "--preset", preset, "--print"))
    dials = {name: float(resolved[name]) for name in SPACE}
    dials["tempo"] = float(resolved["tempo"])
    dials["swing"] = float(resolved["swing"])
    return dials


class Scorer:
    """Renders a dial setting and scores it, one model held for the whole run."""

    def __init__(self, workdir: Path):
        import torch  # noqa: F401 — imported for audiobox's benefit
        from audiobox_aesthetics.infer import initialize_predictor

        warnings.filterwarnings("ignore")
        self.predictor = initialize_predictor()
        self.workdir = workdir
        self.cache: dict[str, dict[str, float]] = {}

    def score(self, preset: str, dials: dict[str, float], seed: int | None) -> dict[str, float]:
        import soundfile
        import torch

        key = json.dumps({"p": preset, "s": seed, **dials}, sort_keys=True)
        if key in self.cache:
            return self.cache[key]
        label = f"{preset}-{'own' if seed is None else seed}"
        sets = []
        for name, value in dials.items():
            rounded = round(value) if name == "swing" else round(value, 3)
            sets += ["--set", f"{name}: {rounded}"]
        if seed is not None:
            sets += ["--seed", str(seed)]
        project = self.workdir / f"{label}.auris"
        wav = self.workdir / f"{label}.wav"
        cli("compose", "--preset", preset, *sets, "-o", str(project), "--force")
        cli(
            "render",
            str(self.workdir / label / f"{label}.auris"),
            "--bit-depth",
            "32",
            "--no-tail",
            "-o",
            str(wav),
        )
        data, rate = soundfile.read(wav, dtype="float32", always_2d=True)
        row = self.predictor.forward([{"path": torch.from_numpy(data.T), "sample_rate": rate}])[0]
        self.cache[key] = {axis: round(row[axis], 3) for axis in AXES}
        return self.cache[key]

    def objective(self, preset: str, dials: dict[str, float], seeds) -> dict[str, float]:
        """The axes averaged over `seeds`."""
        rows = [self.score(preset, dials, seed) for seed in seeds]
        return {axis: round(sum(row[axis] for row in rows) / len(rows), 3) for axis in AXES}


def tune(preset: str, trials: int, scorer: Scorer) -> dict:
    import optuna

    optuna.logging.set_verbosity(optuna.logging.WARNING)
    base = current_dials(preset)
    swings = base["swing"] > 50.0

    def suggest(trial: "optuna.Trial") -> dict[str, float]:
        dials = {name: trial.suggest_float(name, low, high) for name, (low, high) in SPACE.items()}
        dials["tempo"] = trial.suggest_float(
            "tempo", base["tempo"] * (1 - TEMPO_BAND), base["tempo"] * (1 + TEMPO_BAND)
        )
        dials["swing"] = (
            trial.suggest_float(
                "swing", max(50.0, base["swing"] - SWING_BAND), base["swing"] + SWING_BAND
            )
            if swings
            else 50.0
        )
        return dials

    study = optuna.create_study(
        direction="maximize", sampler=optuna.samplers.TPESampler(seed=0)
    )
    # The preset as it stands is trial zero: the search starts from the map's one known point,
    # and the printed history always shows how far anything actually moved from it.
    study.enqueue_trial({name: base[name] for name in (*SPACE, "tempo", *(("swing",) if swings else ()))})

    history = []

    def objective(trial: "optuna.Trial") -> float:
        dials = suggest(trial)
        axes = scorer.objective(preset, dials, SEARCH_SEEDS)
        history.append({"trial": trial.number, **axes})
        print(
            f"  {preset} trial {trial.number:>2}: CE {axes['CE']:.2f}  PQ {axes['PQ']:.2f}",
            flush=True,
        )
        return axes["CE"]

    study.optimize(objective, n_trials=trials)

    best = suggest_to_dials(study.best_params, base, swings)
    # Held-out validation: the number to trust. Both settings on seeds the search never saw.
    held_base = scorer.objective(preset, base, VALIDATION_SEEDS)
    held_best = scorer.objective(preset, best, VALIDATION_SEEDS)
    return {
        "current": base,
        "best": best,
        "search_best_CE": round(study.best_value, 3),
        "history": history,
        "validation": {
            "current": held_base,
            "best": held_best,
            "delta_CE": round(held_best["CE"] - held_base["CE"], 3),
            "delta_PQ": round(held_best["PQ"] - held_base["PQ"], 3),
        },
    }


def suggest_to_dials(params: dict, base: dict[str, float], swings: bool) -> dict[str, float]:
    dials = dict(params)
    if not swings:
        dials["swing"] = 50.0
    return dials


def main() -> None:
    parser = argparse.ArgumentParser(description="Tune preset dials against the learned ear.")
    parser.add_argument("--preset", action="append", default=[], help="'all' or a name; repeatable")
    parser.add_argument("--trials", type=int, default=18)
    parser.add_argument("--out", type=Path, help="write full results to this JSON")
    parser.add_argument("--workdir", type=Path, help="where renders go (default: temporary)")
    args = parser.parse_args()
    if not args.preset:
        parser.error("pass --preset all, or name one")

    presets = preset_names() if "all" in args.preset else args.preset
    workdir = args.workdir or Path(tempfile.mkdtemp(prefix="auris-tune-"))
    workdir.mkdir(parents=True, exist_ok=True)
    scorer = Scorer(workdir)

    results = {}
    for preset in presets:
        print(f"tuning {preset} ({args.trials} trials, renders in {workdir})", flush=True)
        results[preset] = tune(preset, args.trials, scorer)
        held = results[preset]["validation"]
        print(
            f"  {preset}: held-out CE {held['current']['CE']:.2f} -> {held['best']['CE']:.2f} "
            f"({held['delta_CE']:+.2f}), PQ {held['delta_PQ']:+.2f}",
            flush=True,
        )
        if args.out:
            args.out.write_text(json.dumps(results, indent=2) + "\n")

    print("\ndials that won their held-out validation:")
    for preset, result in results.items():
        if result["validation"]["delta_CE"] <= 0.0:
            print(f"  {preset}: none - keep it as it is")
            continue
        moved = {
            name: round(value, 3)
            for name, value in result["best"].items()
            if abs(value - result["current"][name]) > 0.01
        }
        print(f"  {preset}: {moved}")


if __name__ == "__main__":
    main()
