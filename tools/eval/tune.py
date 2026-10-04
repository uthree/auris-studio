# /// script
# requires-python = ">=3.11,<3.12"
# dependencies = [
#   "torch==2.7.1", "torchaudio==2.7.1", "torchvision==0.22.1",
#   "numpy==1.23.5", "scipy==1.13.1", "librosa==0.10.2.post1",
#   "soundfile==0.13.1", "transformers==4.57.6", "huggingface-hub==0.36.2",
#   "laion-clap==1.1.7", "muq==0.1.0",
#   "optuna==5.0.0",
# ]
# ///
"""Pareto tuning of preset dials using TuneJury and MuQ-MuLan.

Both objectives must improve on independent validation seeds before adoption.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path

from learned_models import LearnedModels
from music import MANIFEST, METRICS, load_manifest, score_file, sha256

AXES = METRICS
CLI: Path | None = None
RESOLVER: Path | None = None
REPO = Path(__file__).resolve().parents[2]

# Two seeds to search on, two the search never sees. Fixed, so every run is reproducible and
# two candidates are always compared on identical draws.
SEARCH_SEEDS = (101, 102)
VALIDATION_SEEDS = (301, 302)

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


def effective_dials(dials: dict[str, float]) -> dict[str, float]:
    """Match the numeric values actually passed through the CLI."""
    return {
        name: round(value) if name == "swing" else round(value, 3)
        for name, value in dials.items()
    }


def cli(*args: str) -> str:
    """Runs the auris CLI, preferring the already-built binary over a cargo round trip."""
    binary = CLI or REPO / "target" / "debug" / (
        "auris.exe" if sys.platform == "win32" else "auris"
    )
    if CLI is not None and not binary.is_file():
        raise FileNotFoundError(f"Requested renderer does not exist: {binary}")
    command = (
        [str(binary), *args]
        if binary.exists()
        else ["cargo", "run", "-q", "-p", "auris-cli", "--", *args]
    )
    done = subprocess.run(
        command,
        cwd=REPO,
        capture_output=True,
        encoding="utf-8",
        errors="replace",
        check=False,
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
    text = cli("compose", "--preset", preset, "--print")
    command = (
        [str(RESOLVER)]
        if RESOLVER
        else [
            "cargo",
            "run",
            "-q",
            "-p",
            "auris-compose",
            "--example",
            "resolved_dials",
        ]
    )
    done = subprocess.run(
        command, input=text, cwd=REPO, capture_output=True, encoding="utf-8", check=True
    )
    return {name: float(value) for name, value in json.loads(done.stdout).items()}


class Scorer:
    """Renders a dial setting and scores it, one model held for the whole run."""

    def __init__(self, workdir: Path, cache: Path, device: str, segments: int):
        self.predictor = LearnedModels(cache, device)
        self.profiles = load_manifest(MANIFEST)["presets"]
        self.segments = segments
        self.workdir = workdir
        self.cache: dict[str, dict[str, float]] = {}

    def score(
        self, preset: str, dials: dict[str, float], seed: int | None
    ) -> dict[str, float]:
        effective = effective_dials(dials)
        key = json.dumps({"p": preset, "s": seed, **effective}, sort_keys=True)
        if key in self.cache:
            return self.cache[key]
        label = f"{preset}-{seed}-" + hashlib.sha256(key.encode()).hexdigest()[:12]
        sets = []
        for name, value in effective.items():
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
        row = score_file(
            wav, preset, self.profiles[preset], self.predictor, self.segments
        )
        if row["status"] != "ok":
            raise ValueError(f"Invalid candidate audio: {row}")
        row["effective_dials"] = effective
        (self.workdir / f"{label}.scores.json").write_text(
            json.dumps(row, indent=2, allow_nan=False) + "\n"
        )
        self.cache[key] = row["aggregate"]
        return self.cache[key]

    def objective(
        self, preset: str, dials: dict[str, float], seeds
    ) -> dict[str, float]:
        """The axes averaged over `seeds`."""
        rows = [self.score(preset, dials, seed) for seed in seeds]
        return {axis: statistics.mean(row[axis] for row in rows) for axis in AXES}


def tune(
    preset: str, trials: int, scorer: Scorer, dials: list[str] | None = None
) -> dict:
    import optuna

    optuna.logging.set_verbosity(optuna.logging.WARNING)
    base = current_dials(preset)
    swings = base["swing"] > 50.0
    active = set(dials if dials is not None else (*SPACE, "tempo", "swing"))
    if not active or active - {*SPACE, "tempo", "swing"}:
        raise ValueError("Select at least one known dial")

    def suggest(trial: optuna.Trial) -> dict[str, float]:
        dials = {
            **base,
            **{
                name: trial.suggest_float(
                    name, min(low, base[name]), max(high, base[name])
                )
                for name, (low, high) in SPACE.items()
                if name in active
            },
        }
        if "tempo" in active:
            dials["tempo"] = trial.suggest_float(
                "tempo",
                base["tempo"] * (1 - TEMPO_BAND),
                base["tempo"] * (1 + TEMPO_BAND),
            )
        if "swing" in active:
            dials["swing"] = (
                trial.suggest_float(
                    "swing",
                    max(50.0, base["swing"] - SWING_BAND),
                    base["swing"] + SWING_BAND,
                )
                if swings
                else 50.0
            )
        return dials

    study = optuna.create_study(
        directions=["maximize", "maximize"],
        sampler=optuna.samplers.TPESampler(seed=0, n_startup_trials=4),
    )
    # The preset as it stands is trial zero: the search starts from the map's one known point,
    # and the printed history always shows how far anything actually moved from it.
    study.enqueue_trial(
        {name: base[name] for name in active if name != "swing" or swings}
    )

    history = []

    def objective(trial: optuna.Trial) -> tuple[float, float]:
        dials = suggest(trial)
        axes = scorer.objective(preset, dials, SEARCH_SEEDS)
        history.append({"trial": trial.number, "dials": effective_dials(dials), **axes})
        print(
            f"  {preset} trial {trial.number:>2}: reward {axes['tunejury_reward']:.4f} identity {axes['positive_cosine']:.4f}",
            flush=True,
        )
        return axes["tunejury_reward"], axes["positive_cosine"]

    study.optimize(objective, n_trials=trials)

    baseline = history[0]
    eligible = [
        t
        for t in study.best_trials
        if t.values[0] >= baseline["tunejury_reward"]
        and t.values[1] >= baseline["positive_cosine"]
    ]
    winner = (
        max(eligible, key=lambda t: (t.values[0], t.values[1]))
        if eligible
        else study.trials[0]
    )
    best = next(row["dials"] for row in history if row["trial"] == winner.number)
    # Held-out validation: the number to trust. Both settings on seeds the search never saw.
    held_base = scorer.objective(preset, base, VALIDATION_SEEDS)
    held_best = scorer.objective(preset, best, VALIDATION_SEEDS)
    return {
        "current": base,
        "best": best,
        "selected_trial": winner.number,
        "active_dials": sorted(active),
        "model": scorer.predictor.provenance,
        "prompts_sha256": sha256(MANIFEST),
        "search_seeds": SEARCH_SEEDS,
        "validation_seeds": VALIDATION_SEEDS,
        "segments": scorer.segments,
        "history": history,
        "validation": {
            "current": held_base,
            "best": held_best,
            "delta": {axis: held_best[axis] - held_base[axis] for axis in AXES},
            "accepted": dominates(held_best, held_base),
        },
    }


def dominates(candidate: dict, baseline: dict) -> bool:
    """Require improvement in both independent objectives; never mix their scales."""
    return all(
        candidate[key] >= baseline[key]
        for key in ("tunejury_reward", "positive_cosine")
    ) and any(
        candidate[key] > baseline[key] for key in ("tunejury_reward", "positive_cosine")
    )


def main() -> None:
    global CLI, RESOLVER
    parser = argparse.ArgumentParser(
        description="Tune preset dials against the learned ear."
    )
    parser.add_argument(
        "--preset", action="append", default=[], help="'all' or a name; repeatable"
    )
    parser.add_argument("--trials", type=int, default=18)
    parser.add_argument(
        "--dials",
        nargs="+",
        choices=(*SPACE, "tempo", "swing"),
        help="optimize only these dials and freeze all others",
    )
    parser.add_argument("--out", type=Path, help="write full results to this JSON")
    parser.add_argument(
        "--workdir", type=Path, help="where renders go (default: temporary)"
    )
    parser.add_argument("--cli", type=Path)
    parser.add_argument(
        "--dials-resolver",
        type=Path,
        help="archived resolved_dials example matching the composer",
    )
    parser.add_argument(
        "--cache", type=Path, default=Path("target/composition-eval/models")
    )
    parser.add_argument("--device", default="cpu", choices=("cpu", "cuda"))
    parser.add_argument("--segments", type=int, default=3)
    args = parser.parse_args()
    CLI = args.cli.resolve() if args.cli else None
    RESOLVER = args.dials_resolver.resolve() if args.dials_resolver else None
    if args.trials < 1 or args.segments < 1:
        parser.error("trials and segments must be positive")
    if not args.preset:
        parser.error("pass --preset all, or name one")

    presets = preset_names() if "all" in args.preset else args.preset
    workdir = args.workdir or Path(tempfile.mkdtemp(prefix="auris-tune-"))
    workdir.mkdir(parents=True, exist_ok=True)
    if args.out:
        args.out.parent.mkdir(parents=True, exist_ok=True)
    scorer = Scorer(workdir.resolve(), args.cache, args.device, args.segments)

    results = {}
    for preset in presets:
        print(
            f"tuning {preset} ({args.trials} trials, renders in {workdir})", flush=True
        )
        results[preset] = tune(preset, args.trials, scorer, args.dials)
        if args.out:
            args.out.write_text(
                json.dumps(results, indent=2, allow_nan=False) + "\n", encoding="utf-8"
            )
        held = results[preset]["validation"]
        print(
            f"  {preset}: held-out {held['delta']}, accepted={held['accepted']}",
            flush=True,
        )

    print("\ndials that won their held-out validation:")
    for preset, result in results.items():
        if not result["validation"]["accepted"]:
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
