"""The host as a measuring instrument: driving `auris` from here.

The Rust side of this repository plays what this side exports, and it plays it its own way —
its own chunking of a long timeline, its own run-length arrangement of the frames, its own
energy scale, its own random streams, its own copy of onnxruntime. None of that is exercised by
:mod:`auris_singer.export`'s verification, which compares the graph against PyTorch on one
runtime with one set of inputs. So a voice that verifies can still sing differently in the
application, and the only way to know is to ask the application.

This module asks. It talks to ``auris``, the command line frontend, which drives the same
session the window does: ``sing-frames`` sings a frames file through a voice exactly as a take
is sung, ``frames`` writes the frames a track's notes become, ``compose`` and ``sing`` walk the
whole path a person walks. Everything crosses as files — a frames JSON in, a WAV and a report
out — because the two languages are kept apart on purpose: ``uv run pytest`` must not need a
Rust toolchain, and ``cargo test`` must not need a Python one. That is also why the host is
found rather than imported, and why the two contract values this module has to know
(:data:`SILENCE`, :func:`energy_full_scale`) are checked against the Rust sources as text in
``tests/test_host_contract.py``, the way every other shared constant is.
"""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

import numpy as np

__all__ = [
    "REPO_ROOT",
    "SILENCE",
    "energy_full_scale",
    "HostFrames",
    "frames_from_curves",
    "concatenate_frames",
    "Host",
    "HostError",
]

#: The repository root: this file is ``training/src/auris_singer/host.py``.
REPO_ROOT = Path(__file__).resolve().parents[3]

#: Where the host keeps the energy scale a frames file is read on.
SCORE_RS = "crates/auris-singer/src/score.rs"

#: The frames' own silence token — ``auris_vocal::SILENCE``, always ``inventory[0]``.
#:
#: Not the model's ``<sil>``: the host maps one to the other when it arranges a chunk, and a
#: frames file that wrote ``<sil>`` directly would be sung as an unknown symbol. Pinned to the
#: Rust source by the contract test.
SILENCE = "sil"


class HostError(RuntimeError):
    """The host refused a command, with what it said on stderr."""


def energy_full_scale(repo_root: Path = REPO_ROOT) -> float:
    """What a frame energy of 1.0 means to the host, in the model's linear-RMS terms.

    A frames file carries energy as a musical dynamic from 0 to 1, and the host multiplies it by
    ``auris_singer::ENERGY_FULL_SCALE`` before the model sees it. Curves measured from a corpus
    are already on the model's scale, so :func:`frames_from_curves` divides by this first —
    which has to be *the host's* number, read out of its source, or the two would drift apart
    without a test noticing.
    """
    path = repo_root / SCORE_RS
    if not path.is_file():
        raise HostError(
            f"{SCORE_RS} is missing under {repo_root}; the host's sources are needed to read "
            "its energy scale"
        )
    source = re.sub(r"//[^\n]*", "", path.read_text(encoding="utf-8"))
    match = re.search(r"pub\s+const\s+ENERGY_FULL_SCALE\s*:\s*f32\s*=\s*([0-9.]+)\s*;", source)
    if match is None:
        raise HostError(f"ENERGY_FULL_SCALE was not found in {SCORE_RS}; the parser needs updating")
    return float(match.group(1))


@dataclass
class HostFrames:
    """A singer track sampled onto the model's clock — ``auris_vocal::SingerFrames``, as JSON.

    One entry per frame in each of the three sequences. Phonemes are indices into
    ``inventory``, whose first entry is always :data:`SILENCE`.
    """

    hop_seconds: float
    inventory: list[str]
    phonemes: list[int]
    f0_hz: list[float]
    energy: list[float]

    def __post_init__(self) -> None:
        if not self.inventory or self.inventory[0] != SILENCE:
            raise ValueError(f"inventory[0] must be {SILENCE!r}, got {self.inventory[:1]}")
        n = len(self.phonemes)
        if len(self.f0_hz) != n or len(self.energy) != n:
            raise ValueError(
                f"the three sequences must be one length: {n} phonemes, "
                f"{len(self.f0_hz)} f0, {len(self.energy)} energy"
            )
        if any(not 0 <= p < len(self.inventory) for p in self.phonemes):
            raise ValueError("a phoneme index is outside the inventory")

    def __len__(self) -> int:
        return len(self.phonemes)

    @property
    def seconds(self) -> float:
        return len(self) * self.hop_seconds

    def to_dict(self) -> dict:
        return {
            "hop_seconds": self.hop_seconds,
            "inventory": list(self.inventory),
            "phonemes": [int(p) for p in self.phonemes],
            "f0_hz": [float(f) for f in self.f0_hz],
            "energy": [float(e) for e in self.energy],
        }

    def write(self, path: str | Path) -> Path:
        """Write the file ``auris sing-frames`` reads: compact, one machine to another."""
        path = Path(path)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(self.to_dict(), ensure_ascii=False), encoding="utf-8")
        return path

    @classmethod
    def from_dict(cls, data: dict) -> HostFrames:
        return cls(
            hop_seconds=float(data["hop_seconds"]),
            inventory=list(data["inventory"]),
            phonemes=[int(p) for p in data["phonemes"]],
            f0_hz=[float(f) for f in data["f0_hz"]],
            energy=[float(e) for e in data["energy"]],
        )

    @classmethod
    def read(cls, path: str | Path) -> HostFrames:
        """Read a file ``auris frames`` wrote."""
        return cls.from_dict(json.loads(Path(path).read_text(encoding="utf-8")))

    def tokens(self) -> list[str]:
        """The phoneme symbol on every frame."""
        return [self.inventory[p] for p in self.phonemes]


#: The model's own silence symbol, as it appears in a corpus's phoneme sequence.
MODEL_SILENCE = "<sil>"


def frames_from_curves(
    phonemes: list[str],
    durations: list[int] | np.ndarray,
    f0: list[float] | np.ndarray,
    energy: list[float] | np.ndarray,
    hop_seconds: float,
    energy_scale: float,
) -> HostFrames:
    """Lay a corpus utterance's curves out as the frames the host reads.

    Args:
        phonemes: the utterance's IPA symbols, ``<sil>`` included.
        durations: frames per phoneme; must sum to ``len(f0)``.
        f0: per-frame Hz, 0 where unvoiced — the corpus's own curve.
        energy: per-frame linear RMS on the model's scale — the corpus's own curve.
        hop_seconds: the model's hop.
        energy_scale: the host's :func:`energy_full_scale`, which it will multiply back.

    ``<sil>`` becomes the frames' :data:`SILENCE`, at inventory index 0 as the host requires;
    every other symbol keeps its spelling and enters the inventory in order of first use.
    The energy is divided by ``energy_scale`` so that what reaches the model is the corpus
    curve to the float — which can leave a loud frame above 1.0, and does: the host does not
    clamp, and a clamp here would quietly cap the very dynamics under test.
    """
    durations = [int(d) for d in durations]
    if len(durations) != len(phonemes):
        raise ValueError(f"{len(durations)} durations for {len(phonemes)} phonemes")
    f0 = np.asarray(f0, dtype=np.float32)
    energy = np.asarray(energy, dtype=np.float32)
    total = sum(durations)
    if f0.shape != (total,) or energy.shape != (total,):
        raise ValueError(
            f"durations sum to {total} frames but f0 has {f0.shape} and energy {energy.shape}"
        )
    if energy_scale <= 0:
        raise ValueError(f"the energy scale must be positive, got {energy_scale}")

    inventory = [SILENCE]
    ids: dict[str, int] = {SILENCE: 0}
    per_frame: list[int] = []
    for symbol, count in zip(phonemes, durations):
        token = SILENCE if symbol == MODEL_SILENCE else symbol
        if token not in ids:
            ids[token] = len(inventory)
            inventory.append(token)
        per_frame.extend([ids[token]] * count)
    return HostFrames(
        hop_seconds=hop_seconds,
        inventory=inventory,
        phonemes=per_frame,
        f0_hz=f0.tolist(),
        energy=(energy / energy_scale).tolist(),
    )


def concatenate_frames(
    parts: list[HostFrames], gap_frames: int
) -> tuple[HostFrames, list[tuple[int, int]]]:
    """Several utterances on one timeline, ``gap_frames`` of silence between each pair.

    This is how a *song* is put in front of the host: a stretch long enough to be cut into
    chunks, with seams for the stitching to leave marks on. Returns the joined frames and each
    part's ``(start, end)`` frame span, for slicing the render back apart.
    """
    if not parts:
        raise ValueError("nothing to concatenate")
    hop = parts[0].hop_seconds
    if any(abs(p.hop_seconds - hop) > hop * 1e-9 for p in parts):
        raise ValueError("every part must be sampled at the same hop")
    inventory = [SILENCE]
    ids: dict[str, int] = {SILENCE: 0}
    phonemes: list[int] = []
    f0: list[float] = []
    energy: list[float] = []
    spans: list[tuple[int, int]] = []
    for at, part in enumerate(parts):
        if at > 0:
            phonemes.extend([0] * gap_frames)
            f0.extend([0.0] * gap_frames)
            energy.extend([0.0] * gap_frames)
        start = len(phonemes)
        for token in part.tokens():
            if token not in ids:
                ids[token] = len(inventory)
                inventory.append(token)
            phonemes.append(ids[token])
        f0.extend(part.f0_hz)
        energy.extend(part.energy)
        spans.append((start, len(phonemes)))
    return HostFrames(hop, inventory, phonemes, f0, energy), spans


@dataclass
class Host:
    """The ``auris`` command line, found or built.

    ``AURIS_CLI`` names a built binary and skips cargo entirely, which is what a machine that
    measures often wants; otherwise the command is ``cargo run`` from the repository root, so
    a fresh checkout works with nothing but a toolchain. ``release`` asks cargo for the
    optimised build — the honest one for timings — at the cost of a compile the first time.
    """

    command: list[str]
    cwd: Path = REPO_ROOT
    #: What each run spent, for the caller that wants wall-clock beside the host's own timings.
    last_wall_seconds: float = field(default=0.0, init=False)

    @classmethod
    def find(cls, repo_root: Path = REPO_ROOT, release: bool = False) -> Host:
        named = os.environ.get("AURIS_CLI")
        if named:
            return cls(command=[named], cwd=repo_root)
        if shutil.which("cargo") is None or not (repo_root / "Cargo.toml").is_file():
            raise HostError(
                "no `auris` to drive: set AURIS_CLI to a built binary, or run from a checkout "
                "with cargo on the PATH"
            )
        command = ["cargo", "run", "-q", "-p", "auris-cli"]
        if release:
            command.append("--release")
        command.append("--")
        return cls(command=command, cwd=repo_root)

    @classmethod
    def available(cls, repo_root: Path = REPO_ROOT) -> bool:
        """Whether :meth:`find` would succeed — what a test checks before it skips."""
        try:
            cls.find(repo_root)
        except HostError:
            return False
        return True

    def run(self, *args: str) -> str:
        """Run one command and return its stdout; a nonzero exit is a :class:`HostError`.

        The host runs from the repository root — that is where ``cargo run`` has to be — so
        every path handed to it must be absolute; the methods below make them so.
        """
        started = time.perf_counter()
        done = subprocess.run(
            [*self.command, *args],
            cwd=self.cwd,
            capture_output=True,
            # The CLI speaks UTF-8 — voice names carry Japanese — and Windows would otherwise
            # decode its output with a legacy code page.
            encoding="utf-8",
            errors="replace",
        )
        self.last_wall_seconds = time.perf_counter() - started
        if done.returncode != 0:
            raise HostError(f"auris {' '.join(args)} failed:\n{done.stderr.strip()}")
        return done.stdout

    def sing_frames(
        self,
        frames: str | Path,
        voice: str | Path,
        output: str | Path,
        seed: int = 0,
        acceleration: str = "auto",
        speaker: str | None = None,
    ) -> dict:
        """``auris sing-frames``: the frames file through the voice into ``output``.

        ``speaker`` names one of a multi-speaker voice's speakers; ``None`` is the model's
        first, and a name the voice does not have is the host's refusal, listing its own.

        Returns the host's own report of the render — seconds, sample rate, chunks, load and
        render time, whether the GPU sang — with ``wall_seconds`` added for the whole process,
        toolchain start-up included.
        """
        output = Path(output).resolve()
        output.parent.mkdir(parents=True, exist_ok=True)
        report = output.with_suffix(".report.json")
        self.run(
            "sing-frames",
            str(Path(frames).resolve()),
            "--voice",
            str(Path(voice).resolve()),
            "--seed",
            str(seed),
            "--acceleration",
            acceleration,
            "-o",
            str(output),
            "--report",
            str(report),
            *(["--speaker", speaker] if speaker is not None else []),
        )
        facts = json.loads(report.read_text(encoding="utf-8"))
        facts["wall_seconds"] = self.last_wall_seconds
        return facts

    def frames(self, project: str | Path, output: str | Path, track: str | None = None) -> HostFrames:
        """``auris frames``: what the project's singer track will be sung as."""
        output = Path(output).resolve()
        args = ["frames", str(Path(project).resolve()), "-o", str(output)]
        if track is not None:
            args += ["--track", track]
        self.run(*args)
        return HostFrames.read(output)

    def compose(self, spec: str | Path, output: str | Path, seed: int | None = None) -> Path:
        """``auris compose``: a project from a specification, answering with the project file.

        The CLI makes a folder named after the output and puts the project inside it — one
        folder, one project — unless the folder asked for already *is* that folder, in which
        case the file lands where it was asked; so the path answered may not be the one given.
        """
        output = Path(output).resolve()
        args = ["compose", str(Path(spec).resolve()), "-o", str(output), "--force"]
        if seed is not None:
            args += ["--seed", str(seed)]
        self.run(*args)
        for project in (output.parent / output.stem / output.name, output):
            if project.is_file():
                return project
        raise HostError(f"compose reported success but {output.name} is in neither place it could be")

    def sing(
        self,
        project: str | Path,
        voice: str | Path,
        seed: int,
        track: str | None = None,
        speaker: str | None = None,
    ) -> Path:
        """``auris sing``: the whole path a person walks, answering with the take's WAV.

        The take lands in the project's ``Audio/`` folder under the track's name; a project
        sung once holds exactly one file there, which is how it is found without reading the
        document's own bookkeeping.
        """
        project = Path(project).resolve()
        args = ["sing", str(project), "--voice", str(Path(voice).resolve()), "--seed", str(seed)]
        if track is not None:
            args += ["--track", track]
        if speaker is not None:
            args += ["--speaker", speaker]
        before = set((project.parent / "Audio").glob("*.wav"))
        self.run(*args)
        after = set((project.parent / "Audio").glob("*.wav")) - before
        if len(after) != 1:
            raise HostError(f"expected one new take in {project.parent / 'Audio'}, found {sorted(after)}")
        return after.pop()


def _main() -> None:  # pragma: no cover - a smoke check by hand
    host = Host.find()
    sys.stdout.write(host.run("help"))


if __name__ == "__main__":  # pragma: no cover
    _main()
