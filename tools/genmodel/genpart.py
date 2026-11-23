"""Regenerate or add one part of a song with a local music generation model.

Development experiment only. Auris's experimental `generate_part` tool runs this with
a request file and reads one JSON object back from stdout:

    python tools/genmodel/genpart.py request.json

The request names the operation, the rendered stems of the bar range (one WAV per
track, all the same length) and where to write the result:

    {
      "operation": "stem" | "repaint" | "loop" | "restyle",
      "stems": {"Lead": "C:/.../Lead.wav", ...},
      "exclude": ["Drums"],            # stem: tracks left out of the context
      "target": "Piano",               # repaint: the stem to regenerate
      "instrument": "drums",           # stem: ACE-Step's track name
      "prompt": "tight pop drum kit",
      "bpm": 120, "key": "C major", "beats_per_bar": 4,
      "repaint_seconds": [4.0, 8.0],   # repaint: region inside the range
      "loop_bars": 8,                  # loop: Foundation-1's loop length
      "seed": 0,
      "output": "C:/.../generated.wav"
    }

* `stem` asks ACE-Step 1.5 `lego` for a new part over the other stems; the output is
  that part alone.
* `repaint` asks ACE-Step `repaint` to rewrite a region of one stem. Only the region
  is taken from the model; the rest is the stem as it was, joined by short crossfades,
  because the model also changes what lies outside the region.
* `loop` asks Foundation-1 for a tempo-synced loop, tiled over the range.
* `restyle` plays one stem again in another sound (Foundation-1 audio-to-audio from the
  stem): `target`, `prompt`, and `strength` 0..1 for how far it may move from the stem.

The models run in their own environments (see README.md), found under
`AURIS_GENMODEL_HOME`. A constant lateness of the generated audio against the bar grid
is measured and removed, since ACE-Step's parts come out tens of milliseconds late.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def base_python(venv: Path) -> Path:
    """The interpreter a venv was made from, by its real path.

    A uv venv's own `python.exe` is a trampoline with its base interpreter's path built in,
    and that path can be one only some processes see (README.md), so the interpreter
    pyvenv.cfg names is run directly and the venv's packages are added with `site.addsitedir`.
    """
    for line in (venv / "pyvenv.cfg").read_text(encoding="utf-8").splitlines():
        key, _, value = line.partition("=")
        if key.strip() == "home":
            return Path(os.path.realpath(value.strip())) / "python.exe"
    raise RuntimeError(f"{venv} has no home in pyvenv.cfg")


def site_packages(venv: Path) -> Path:
    return venv / "Lib" / "site-packages"


# Runs a script under a base interpreter with a venv's packages: argv is the site-packages
# folder, then the script and its arguments.
BOOTSTRAP = (
    "import runpy, site, sys; site.addsitedir(sys.argv[1]); "
    "sys.argv = sys.argv[2:]; runpy.run_path(sys.argv[0], run_name='__main__')"
)


def venv_command(venv: Path, script: Path, *args: str) -> list[str]:
    return [
        str(base_python(venv)),
        "-c",
        BOOTSTRAP,
        str(site_packages(venv)),
        str(script),
        *args,
    ]


try:
    import numpy as np
except ImportError:  # started by a base interpreter: take ACE-Step's packages
    import site

    site.addsitedir(
        str(
            site_packages(
                Path(os.environ["AURIS_GENMODEL_HOME"]) / "ace-step" / "venv_rocm"
            )
        )
    )
    import numpy as np

import soundfile as sf
from scipy.signal import find_peaks, resample_poly, stft

HERE = Path(__file__).resolve().parent
# Python writes pipes in the ANSI code page on Windows, which cannot hold the Japanese track
# names the stems are named after; every child is told to use UTF-8.
UTF8_ENV = {"PYTHONUTF8": "1", "PYTHONIOENCODING": "utf-8"}
LEGO_TRACKS = {
    "vocals",
    "backing_vocals",
    "drums",
    "bass",
    "guitar",
    "keyboard",
    "percussion",
    "strings",
    "synth",
    "fx",
    "brass",
    "woodwinds",
}
FOUNDATION_BPMS = {100, 110, 120, 128, 130, 140, 150}
CROSSFADE_SECONDS = 0.03


class RequestError(Exception):
    """A request the models cannot serve, reported to the caller as such."""


def home() -> Path:
    value = os.environ.get("AURIS_GENMODEL_HOME")
    if not value:
        raise RequestError("AURIS_GENMODEL_HOME is not set")
    return Path(value)


def read(path: str) -> tuple[np.ndarray, int]:
    audio, rate = sf.read(path, dtype="float32", always_2d=True)
    if audio.shape[1] == 1:
        audio = np.repeat(audio, 2, axis=1)
    return audio[:, :2], rate


def fit(audio: np.ndarray, frames: int) -> np.ndarray:
    if len(audio) >= frames:
        return audio[:frames]
    return np.pad(audio, ((0, frames - len(audio)), (0, 0)))


def onsets(audio: np.ndarray, rate: int) -> np.ndarray:
    mono = audio.mean(axis=1)
    hop = 256
    _, times, spectrum = stft(mono, rate, nperseg=1024, noverlap=1024 - hop)
    magnitude = np.log1p(np.abs(spectrum) * 100)
    flux = np.maximum(0, np.diff(magnitude, axis=1)).sum(axis=0)
    if flux.max() <= 0:
        return np.array([])
    flux /= flux.max()
    peaks, _ = find_peaks(flux, height=0.25, distance=max(1, int(0.06 * rate / hop)))
    return times[1:][peaks]


def grid_offset(audio: np.ndarray, rate: int, bpm: float) -> tuple[float, float, float]:
    """(offset in s, median grid error before, after) against the sixteenth grid."""
    times = onsets(audio, rate)
    if len(times) < 8:
        return 0.0, float("nan"), float("nan")
    step = 60.0 / bpm / 4

    def error(shift: float) -> float:
        late = (times - shift + step / 2) % step - step / 2
        return float(np.median(np.abs(late)))

    candidates = np.linspace(-0.08, 0.08, 321)
    best = min(candidates, key=error)
    return float(best), error(0.0), error(best)


def shift_earlier(audio: np.ndarray, frames: int) -> np.ndarray:
    if frames <= 0:
        return audio
    return np.pad(audio[frames:], ((0, frames), (0, 0)))


def run_ace(job: dict, workdir: Path) -> tuple[np.ndarray, int, dict]:
    root = home() / "ace-step"
    jobs = workdir / "ace-jobs.json"
    jobs.write_text(json.dumps([job]), encoding="utf-8")
    out = workdir / "ace-out"
    env = {
        **os.environ,
        **UTF8_ENV,
        "ACESTEP_ROCM_DTYPE": os.environ.get("ACESTEP_ROCM_DTYPE", "bfloat16"),
    }
    proc = subprocess.run(  # noqa: PLW1510 - the report file, not the exit code, says what happened
        [
            *venv_command(root / "venv_rocm", HERE / "ace_step_jobs.py"),
            "--ace-step",
            str(root),
            "--model",
            "acestep-v15-base",
            "--sdpa",
            "math",
            "--jobs",
            str(jobs),
            "--output",
            str(out),
        ],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        timeout=1800,
    )
    report_path = out / "report.json"
    if not report_path.is_file():
        raise RequestError("ACE-Step did not start: " + proc.stderr[-800:])
    entry = json.loads(report_path.read_text(encoding="utf-8"))["jobs"][0]
    if not entry.get("success"):
        raise RequestError("ACE-Step failed: " + str(entry.get("error"))[:400])
    audio, rate = read(entry["audio"])
    return (
        audio,
        rate,
        {
            "model": "ACE-Step 1.5 base",
            "seconds": entry["seconds"],
            "peak_gib": entry["peak_gib"],
        },
    )


def run_foundation_jobs(
    jobs: list[dict], workdir: Path
) -> tuple[list[tuple[np.ndarray, int]], dict]:
    """Runs Foundation-1 jobs in one process, so the model loads once."""
    root = home() / "stable-audio-tools"
    jobs_path = workdir / "f1-jobs.json"
    jobs_path.write_text(json.dumps(jobs), encoding="utf-8")
    out = workdir / "f1-out"
    env = {
        **os.environ,
        **UTF8_ENV,
        "MIOPEN_FIND_MODE": "FAST",
        "HF_HUB_DISABLE_SYMLINKS_WARNING": "1",
    }
    proc = subprocess.run(  # noqa: PLW1510 - the report file, not the exit code, says what happened
        [
            *venv_command(root / "venv_rocm", HERE / "foundation1_jobs.py"),
            "--jobs",
            str(jobs_path),
            "--output",
            str(out),
        ],
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
        env=env,
        timeout=1800,
    )
    report_path = out / "report.json"
    if not report_path.is_file():
        raise RequestError("Foundation-1 failed: " + proc.stderr[-800:])
    entries = json.loads(report_path.read_text(encoding="utf-8"))["jobs"]
    if len(entries) != len(jobs):
        raise RequestError("Foundation-1 stopped early: " + proc.stderr[-800:])
    takes = [read(entry["audio"]) for entry in entries]
    info = {
        "model": "Foundation-1",
        "seconds": round(sum(entry["seconds"] for entry in entries), 2),
        "peak_gib": max(entry["peak_gib"] for entry in entries),
    }
    return takes, info


def run_foundation(
    prompt: str, bpm: int, bars: int, seed: int, workdir: Path
) -> tuple[np.ndarray, int, dict]:
    job = {"name": "loop", "prompt": prompt, "bpm": bpm, "bars": bars, "seed": seed}
    [(audio, rate)], info = run_foundation_jobs([job], workdir)
    return audio, rate, info


RESTYLE_PAD_SECONDS = 0.1
RESTYLE_MAX_SECONDS = 19.0


def restyle(
    request: dict,
    stems: dict,
    frames: int,
    rate: int,
    workdir: Path,
    notes: list[str],
) -> tuple[np.ndarray, dict]:
    """Plays one stem again in the prompt's sound with Foundation-1 audio-to-audio.

    The stem is the starting point and the noise level says how far the take may move
    from it. Measured on a piano lead and a chord part (2026-11-23), a sigma of 1 to 2.5
    keeps the notes (chroma correlation 0.7 to 0.97 with the input) while the tone colour
    moves 4 to 15 dB per third-octave band; from 5 up the notes give way. `strength` 0..1
    maps to sigma 1..5 logarithmically, so 0.4 is about 1.9.

    Foundation-1 takes at most about 20 seconds, so the range is played in bar-aligned
    chunks, each with a little of its neighbours, and joined by crossfades. A chunk that is
    silent in the stem stays silent, and each take is set to its chunk's level.
    """
    target = request.get("target")
    if target not in stems:
        raise RequestError(f"target must be one of the rendered stems: {sorted(stems)}")
    strength = min(1.0, max(0.0, float(request.get("strength", 0.4))))
    sigma = 5.0**strength
    original = fit(stems[target][0], frames)
    seconds = frames / rate
    bpm = float(request["bpm"])
    bar = request.get("beats_per_bar", 4) * 60.0 / bpm
    per = max(1, min(8, int(RESTYLE_MAX_SECONDS // bar)))
    tags = [request.get("prompt", "")]
    if per in (4, 8):
        tags.append(f"{per} Bars")
    if round(bpm) in FOUNDATION_BPMS and abs(bpm - round(bpm)) < 1e-6:
        tags.append(f"{round(bpm)} BPM")
    if request.get("key"):
        tags.append(request["key"])
    prompt = ", ".join(tag for tag in tags if tag)
    pad = RESTYLE_PAD_SECONDS
    chunks = []
    start = 0.0
    while start < seconds - 1e-6:
        end = min(seconds, start + per * bar)
        if seconds - end < 0.5 * bar:  # a sliver left over joins the last chunk
            end = seconds
        chunks.append((start, end))
        start = end
    jobs, spans = [], []
    seed = int(request.get("seed", 0))
    for index, (a, b) in enumerate(chunks):
        lo, hi = int(max(0.0, a - pad) * rate), int(min(seconds, b + pad) * rate)
        piece = original[lo:hi]
        if float(np.sqrt(np.mean(piece**2))) < 1e-4:
            continue  # silent here: nothing to play
        path = workdir / f"restyle-in-{index}.wav"
        sf.write(path, piece, rate, subtype="FLOAT")
        jobs.append(
            {
                "name": f"restyle-{index}",
                "prompt": prompt,
                "init_audio": str(path),
                "init_noise_level": sigma,
                "seed": seed + index,
            }
        )
        spans.append((lo, hi, a > 0, b < seconds))
    if not jobs:
        raise RequestError(f"the {target} stem is silent over the whole range")
    takes, info = run_foundation_jobs(jobs, workdir)
    total = np.zeros_like(original)
    weight = np.zeros(frames, dtype=np.float32)
    for (lo, hi, fade_in, fade_out), (take, take_rate) in zip(spans, takes):
        take = fit(resample(take, take_rate, rate), hi - lo)
        source_rms = float(np.sqrt(np.mean(original[lo:hi] ** 2)))
        take_rms = float(np.sqrt(np.mean(take**2)))
        if take_rms > 0:
            take = take * (source_rms / take_rms)
        window = np.ones(hi - lo, dtype=np.float32)
        ramp = min(int(2 * pad * rate), (hi - lo) // 2)
        if fade_in:
            window[:ramp] = np.linspace(0, 1, ramp, dtype=np.float32)
        if fade_out:
            window[-ramp:] = np.linspace(1, 0, ramp, dtype=np.float32)
        total[lo:hi] += take * window[:, None]
        weight[lo:hi] += window
    audio = np.where(
        weight[:, None] > 1e-6, total / np.maximum(weight, 1e-6)[:, None], 0.0
    )
    notes.append(
        f"{len(jobs)} take(s) of up to {per} bars at sigma {sigma:.2f}, "
        "joined by crossfades and set to the stem's level; the notes are the original part's"
    )
    return audio.astype(np.float32), {**info, "sigma": round(sigma, 3)}


def resample(audio: np.ndarray, rate: int, target: int) -> np.ndarray:
    if rate == target:
        return audio
    g = np.gcd(rate, target)
    return resample_poly(audio, target // g, rate // g, axis=0).astype(np.float32)


def metas(request: dict) -> dict:
    return {
        "bpm": round(request["bpm"]),
        "keyscale": request.get("key", ""),
        "timesignature": str(request.get("beats_per_bar", 4)),
        "seed": int(request.get("seed", 0)),
        "inference_steps": 50,
        "instrumental": True,
        "lyrics": "[Instrumental]",
    }


def generate(request: dict, workdir: Path) -> dict:
    stems = {name: read(path) for name, path in request["stems"].items()}
    if not stems:
        raise RequestError("no stems were rendered for the range")
    rate = next(iter(stems.values()))[1]
    frames = max(len(audio) for audio, _ in stems.values())
    seconds = frames / rate
    operation = request["operation"]
    notes: list[str] = []

    if operation == "stem":
        instrument = request.get("instrument", "")
        if instrument not in LEGO_TRACKS:
            raise RequestError(f"instrument must be one of {sorted(LEGO_TRACKS)}")
        exclude = set(request.get("exclude", []))
        context = [
            fit(a, frames) for name, (a, _) in stems.items() if name not in exclude
        ]
        if not context:
            raise RequestError(
                "every stem was excluded; the model needs something to play along with"
            )
        mix = np.sum(context, axis=0)
        peak = float(np.abs(mix).max())
        if peak > 0:
            mix = mix / peak * 0.89
        src = workdir / "context.wav"
        sf.write(src, mix, rate, subtype="PCM_16")
        job = {
            "name": "part",
            "task_type": "lego",
            "src_audio": str(src),
            "instruction": f"Generate the {instrument} track based on the audio context:",
            "caption": request.get("prompt", ""),
            "repainting_start": 0.0,
            "repainting_end": -1,
            **metas(request),
        }
        audio, out_rate, info = run_ace(job, workdir)
        audio = fit(resample(audio, out_rate, rate), frames)
    elif operation == "repaint":
        target = request.get("target")
        if target not in stems:
            raise RequestError(
                f"target must be one of the rendered stems: {sorted(stems)}"
            )
        start, end = request["repaint_seconds"]
        if not (0 <= start < end <= seconds):
            raise RequestError(
                f"repaint_seconds must lie inside the range (0 to {seconds:.2f} s)"
            )
        original = fit(stems[target][0], frames)
        job = {
            "name": "part",
            "task_type": "repaint",
            "src_audio": request["stems"][target],
            "caption": request.get("prompt", ""),
            "repainting_start": float(start),
            "repainting_end": float(end),
            **metas(request),
        }
        new, out_rate, info = run_ace(job, workdir)
        new = fit(resample(new, out_rate, rate), frames)
        # Keep the original outside the region; the model alters it too.
        a, b = int(start * rate), int(end * rate)
        fade = int(CROSSFADE_SECONDS * rate)
        weight = np.zeros(frames, dtype=np.float32)
        weight[a:b] = 1.0
        ramp = np.linspace(0, 1, fade, dtype=np.float32)
        if a > 0:
            weight[max(0, a - fade) : a] = ramp[-(a - max(0, a - fade)) :]
        if b < frames:
            weight[b : min(frames, b + fade)] = ramp[::-1][: min(frames, b + fade) - b]
        audio = original * (1 - weight[:, None]) + new * weight[:, None]
        notes.append(
            "outside the region the stem is the original, joined by 30 ms crossfades"
        )
    elif operation == "loop":
        bpm = round(request["bpm"])
        if bpm not in FOUNDATION_BPMS or abs(request["bpm"] - bpm) > 1e-6:
            raise RequestError(
                f"Foundation-1 only knows {sorted(FOUNDATION_BPMS)} BPM; the song is at {request['bpm']}"
            )
        bars = int(request.get("loop_bars", 8))
        if bars not in (4, 8):
            raise RequestError("loop_bars must be 4 or 8")
        prompt = request.get("prompt", "")
        key = request.get("key", "")
        full_prompt = f"{prompt}, {bars} Bars, {bpm} BPM, {key}".strip(", ")
        loop, out_rate, info = run_foundation(
            full_prompt, bpm, bars, int(request.get("seed", 0)), workdir
        )
        loop = resample(loop, out_rate, rate)
        loop_frames = round(bars * request.get("beats_per_bar", 4) * 60.0 / bpm * rate)
        loop = fit(loop, loop_frames)
        audio = fit(np.tile(loop, (frames // loop_frames + 1, 1)), frames)
        notes.append("Foundation-1 is not told the chords; it only knows the key")
    elif operation == "restyle":
        audio, info = restyle(request, stems, frames, rate, workdir, notes)
    else:
        raise RequestError(f"unknown operation {operation!r}")

    offset, before, after = grid_offset(audio, rate, request["bpm"])
    applied = 0.0
    # A restyled take keeps its stem's timing, so only new parts are moved.
    if operation != "restyle" and offset > 0.005 and after < 0.7 * before:
        audio = shift_earlier(audio, round(offset * rate))
        applied = offset
    peak = float(np.abs(audio).max())
    if peak > 0.98:
        audio = audio / peak * 0.98
    sf.write(request["output"], audio, rate, subtype="FLOAT")
    rms = float(np.sqrt(np.mean(audio**2)))
    return {
        "output": request["output"],
        **info,
        "seconds_of_audio": round(seconds, 3),
        "shifted_earlier_ms": round(applied * 1000, 1),
        "grid_error_ms": None
        if np.isnan(after)
        else round((after if applied else before) * 1000, 1),
        "rms_dbfs": round(20 * np.log10(rms + 1e-12), 1),
        "notes": notes,
    }


def main() -> int:
    sys.stdout.reconfigure(encoding="utf-8")
    sys.stderr.reconfigure(encoding="utf-8")
    if len(sys.argv) != 2:
        print(json.dumps({"error": "usage: genpart.py request.json"}))
        return 2
    started = time.perf_counter()
    try:
        request = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
        with tempfile.TemporaryDirectory(prefix="genpart-") as tmp:
            result = generate(request, Path(tmp))
        result["wall_seconds"] = round(time.perf_counter() - started, 1)
        print(json.dumps(result, ensure_ascii=False))
        return 0
    except RequestError as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=False))
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
