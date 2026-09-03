"""Measuring an exported voice the way the application will play it.

Training's validation answers "does the model follow the curves it was given" — it sings the
validation set through :meth:`AurisSinger.infer` in PyTorch and re-analyses the result with
FCPE (:mod:`auris_singer.metrics`). Export's verification answers "is the graph the same
function" — on one runtime, at sizes the trace never saw. Between the two and the person who
presses *Sing* there is a third thing: the host. It reads the file, cuts a long timeline into
chunks and stitches the answers, arranges frames into tokens, scales the energy, draws its own
noise and runs its own copy of onnxruntime. This module holds that third thing to the same
numbers the other two are held to.

Three questions, each a column in the report:

* **host** — the corpus's own curves, sung by the host through the exported ``.onnx``, against
  the recording and against the curves. The same metrics validation logs, so a number here is
  comparable to ``val/…`` in the training log — after export, after the host.
* **reference** — the same curves sung by :class:`auris_singer.infer.Synthesizer` from the
  checkpoint the voice was exported from. The delta is what export and host cost together;
  the noise differs between the two (each draws its own), so the comparison is metric to
  metric, never sample to sample.
* **song** — the same utterances laid end to end on one timeline with silence between, sung
  as one file, sliced back apart and measured again. This is the only column where the host's
  chunking and stitching run, since a corpus utterance is shorter than one chunk; a seam that
  costs something shows up as the difference from the host column.

And a fourth, on its own: **score** sings notes and words through the whole path a person
walks — ``compose``, ``frames``, ``sing`` — and compares the take against the frames the
session itself said it would sing. There is no recording to hold it against, so it reports
control fidelity and timing alone, but every piece of the pipeline is in the picture.

The numbers are a regression detector, read together, never one alone; ``docs/evaluation.md``
at the repository root says why, and it applies here word for word.
"""

from __future__ import annotations

import json
import logging
import math
import random
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import numpy as np
import soundfile as sf
import torch

from auris_singer.data.dataset import read_metadata
from auris_singer.export import METADATA_KEY
from auris_singer.host import (
    SILENCE,
    Host,
    HostFrames,
    concatenate_frames,
    energy_full_scale,
    frames_from_curves,
)
from auris_singer.infer import Synthesizer, frame_voicing
from auris_singer.intelligibility import (
    CLASS_METRICS,
    class_spectral_metrics,
    confusion_rows,
    confusion_table,
    hearable,
    phoneme_error_rate,
    tally_confusions,
)
from auris_singer.metrics import energy_metrics, pitch_metrics
from auris_singer.text.ipa import PhonemeTable, is_voiceless
from auris_singer.utils.audio import frame_energy, mel_spectrogram, spectrogram

logger = logging.getLogger(__name__)

__all__ = [
    "NOISE_SCALE",
    "VoiceInfo",
    "voice_info",
    "Utterance",
    "validation_records",
    "Corpus",
    "Aligner",
    "Analyst",
    "evaluate",
    "evaluate_score",
    "summarize",
    "average_rows",
    "format_report",
    "phonemes_of_frames",
    "METRICS",
    "COLUMNS",
]

#: The prior's sampling temperature the host sings at — ``auris_singer::NOISE_SCALE`` — and
#: the exporter's default. The reference render uses the same number so the two columns
#: differ by runtime and noise stream alone. Pinned to the Rust source by the contract test.
NOISE_SCALE = 0.667

#: Every metric a column can hold, in the order the table prints them.
METRICS = (
    "mel_l1",
    *CLASS_METRICS,
    "f0_rmse_cent",
    "f0_accuracy",
    "f0_corr",
    "vuv_error",
    "voiced_ratio_error",
    "energy_rmse_db",
    "energy_bias_db",
    "energy_corr",
    "peak",
    "per",
)

#: The columns a report can hold, in table order. ``recording`` holds the listener's own
#: ceiling — what it makes of the real singer — and nothing else.
COLUMNS = ("host", "reference", "song", "recording", "score")


# ----------------------------------------------------------------------------------------------
# the voice
# ----------------------------------------------------------------------------------------------
@dataclass
class VoiceInfo:
    """What an exported voice says about itself — the metadata the host reads."""

    path: Path
    sample_rate: int
    hop_length: int
    symbols: list[str]
    name: str = ""

    @property
    def hop_seconds(self) -> float:
        return self.hop_length / self.sample_rate

    @property
    def table(self) -> PhonemeTable:
        return PhonemeTable(self.symbols)


def voice_info(path: str | Path) -> VoiceInfo:
    """Read a voice's metadata: from the ``.json`` sidecar the export writes, or from the
    ``.onnx`` itself where the sidecar has gone missing."""
    path = Path(path)
    sidecar = path.with_suffix(".json")
    if sidecar.is_file():
        block = json.loads(sidecar.read_text(encoding="utf-8"))
    else:
        import onnx

        props = {p.key: p.value for p in onnx.load(str(path), load_external_data=False).metadata_props}
        if METADATA_KEY not in props:
            raise ValueError(f"{path} carries no {METADATA_KEY!r} metadata; it is not an exported voice")
        block = json.loads(props[METADATA_KEY])
    return VoiceInfo(
        path=path,
        sample_rate=int(block["sample_rate"]),
        hop_length=int(block["hop_length"]),
        symbols=list(block["symbols"]),
        name=str((block.get("voice") or {}).get("name", "")),
    )


# ----------------------------------------------------------------------------------------------
# the corpus
# ----------------------------------------------------------------------------------------------
@dataclass
class Utterance:
    """One corpus utterance with everything both a render and a measurement need."""

    id: str
    speaker_id: int
    phonemes: list[str]
    durations: list[int]
    f0: np.ndarray
    energy: np.ndarray
    wav: np.ndarray

    @property
    def n_frames(self) -> int:
        return int(self.f0.shape[0])

    @property
    def voiced(self) -> np.ndarray:
        """The voicing the host will decide: phoneme class, cleared where f0 is zero."""
        return frame_voicing(self.phonemes, self.durations, self.f0)

    @property
    def tokens(self) -> list[str]:
        """The phoneme on every frame, the alignment written out."""
        return [p for p, d in zip(self.phonemes, self.durations) for _ in range(d)]


def validation_records(root: str | Path, seed: int = 1234, val_size: int = 8) -> list[dict]:
    """The validation split exactly as :class:`SingingDataModule` draws it.

    Same shuffle, same seed, same cap, so the utterances measured here are the ones the
    training log's ``val/…`` numbers were measured on.
    """
    records = read_metadata(root)
    rng = random.Random(seed)
    rng.shuffle(records)
    val_size = min(val_size, max(len(records) // 10, 1))
    return records[:val_size]


class Corpus:
    """A preprocessed dataset directory, read the way the dataset reads it."""

    def __init__(self, root: str | Path):
        self.root = Path(root)
        audio = json.loads((self.root / "audio_config.json").read_text(encoding="utf-8"))
        self.sample_rate = int(audio["sample_rate"])
        self.n_fft = int(audio["n_fft"])
        self.hop_length = int(audio["hop_length"])
        self.win_length = int(audio["win_length"])
        self.table = PhonemeTable.load(self.root / "phonemes.json")

    def records(self, split: str, count: int, seed: int, val_size: int) -> list[dict]:
        if split == "val":
            records = validation_records(self.root, seed=seed, val_size=val_size)
        elif split == "all":
            records = read_metadata(self.root)
            random.Random(seed).shuffle(records)
        else:
            raise ValueError(f"split must be 'val' or 'all', not {split!r}")
        return records[:count]

    def load(self, record: dict) -> tuple[list[str], np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
        """``(phonemes, f0, energy, voiced, wav)`` for one record, trimmed to whole frames."""
        with np.load(self.root / record["path"]) as data:
            wav = data["wav"].astype(np.float32) / 32768.0
            ids = data["phonemes"].astype(np.int64).tolist()
            f0 = data["f0"].astype(np.float32)
            energy = data["energy"].astype(np.float32)
            voiced = data["voiced"].astype(np.float32)
        n_frames = min(wav.shape[0] // self.hop_length, f0.shape[0])
        wav = wav[: n_frames * self.hop_length]
        return self.table.decode(ids), f0[:n_frames], energy[:n_frames], voiced[:n_frames], wav

    def durations(self, record: dict) -> list[int] | None:
        """The labelled frames per phoneme, where the preprocessor stored them.

        Where the corpus had labels, these are the alignment — and the one every voice
        is measured on, whether it trained by the labels or by the search — so what the
        table compares is the voices and not two alignments.
        """
        with np.load(self.root / record["path"]) as data:
            if "durations" not in data:
                return None
            return data["durations"].astype(np.int64).tolist()


class Aligner:
    """Frames per phoneme for a corpus utterance, from the checkpoint's own alignment.

    A corpus without labels stores no durations — training recovers them by monotonic
    alignment search — so an utterance can only be laid out on the model's clock by the
    model that learned it. This runs the training forward pass with the recording's
    spectrogram, exactly as validation does, and reads the path's column sums. Where the
    preprocessor stored labelled durations, :meth:`Corpus.durations` answers first and
    this is not asked.
    """

    def __init__(self, checkpoint: str | Path, device: str = "cpu"):
        from auris_singer.lightning_module import AurisSingerModule

        module = AurisSingerModule.load_from_checkpoint(str(checkpoint), map_location="cpu")
        self.synthesizer = Synthesizer(module, device=device)
        self.device = torch.device(device)

    @property
    def table(self) -> PhonemeTable:
        return self.synthesizer.phoneme_table

    @torch.no_grad()
    def durations(
        self,
        phonemes: list[str],
        wav: np.ndarray,
        f0: np.ndarray,
        energy: np.ndarray,
        voiced: np.ndarray,
        speaker_id: int,
        n_fft: int,
        hop_length: int,
        win_length: int,
    ) -> list[int]:
        model = self.synthesizer.model
        wav_t = torch.from_numpy(wav).to(self.device)
        spec = spectrogram(wav_t, n_fft, hop_length, win_length).unsqueeze(0)
        n_frames = spec.size(-1)
        ids = torch.tensor(self.table.encode(phonemes), dtype=torch.long, device=self.device)
        out = model(
            phonemes=ids.unsqueeze(0),
            phoneme_lengths=torch.tensor([len(phonemes)], device=self.device),
            spec=spec,
            spec_lengths=torch.tensor([n_frames], device=self.device),
            f0=torch.from_numpy(f0[:n_frames]).to(self.device).unsqueeze(0),
            energy=torch.from_numpy(energy[:n_frames]).to(self.device).unsqueeze(0),
            voiced=torch.from_numpy(voiced[:n_frames]).to(self.device).unsqueeze(0),
            speaker_ids=torch.tensor([speaker_id], device=self.device),
        )
        durations = out["durations"].round().long().squeeze(0).tolist()
        total = sum(durations)
        if total != n_frames:
            raise RuntimeError(f"the alignment covers {total} frames of {n_frames}")
        return durations

    @torch.inference_mode()
    def reference(self, utterance: Utterance, seed: int) -> np.ndarray:
        """The checkpoint singing the utterance itself, its noise pinned by ``seed``."""
        torch.manual_seed(seed)
        return self.synthesizer.synthesize(
            phonemes=utterance.phonemes,
            durations=utterance.durations,
            f0=utterance.f0,
            energy=utterance.energy,
            speaker=utterance.speaker_id,
            voiced=utterance.voiced,
            noise_scale=NOISE_SCALE,
        )


# ----------------------------------------------------------------------------------------------
# the measurement
# ----------------------------------------------------------------------------------------------
class Analyst:
    """Turns a rendered waveform and what was asked for into the report's numbers."""

    def __init__(
        self,
        sample_rate: int,
        n_fft: int,
        hop_length: int,
        win_length: int,
        n_mels: int = 128,
        pitch: bool = True,
        device: str = "cpu",
        f0_min: float = 40.0,
        f0_max: float = 1600.0,
        tolerance_cents: float = 50.0,
    ):
        self.sample_rate = sample_rate
        self.n_fft = n_fft
        self.hop_length = hop_length
        self.win_length = win_length
        self.n_mels = n_mels
        self.tolerance_cents = tolerance_cents
        self.extractor = None
        if pitch:
            from auris_singer.preprocess.f0 import FcpeExtractor

            self.extractor = FcpeExtractor(device=device, f0_min=f0_min, f0_max=f0_max)

    def trim(self, wav: np.ndarray, n_frames: int) -> torch.Tensor:
        wanted = n_frames * self.hop_length
        if wav.shape[0] < wanted:
            raise ValueError(f"the render holds {wav.shape[0]} samples where {wanted} were asked")
        return torch.from_numpy(np.asarray(wav[:wanted], dtype=np.float32))

    def mel(self, wav: torch.Tensor) -> torch.Tensor:
        return mel_spectrogram(
            wav, self.sample_rate, self.n_fft, self.hop_length, self.win_length, self.n_mels
        )

    def measure(
        self,
        wav: np.ndarray,
        f0: np.ndarray,
        energy: np.ndarray,
        voiced: np.ndarray,
        reference: np.ndarray | None = None,
        tokens: list[str] | None = None,
    ) -> dict[str, float]:
        """The metrics of one render against the curves it was asked for.

        ``mel_l1`` is against ``reference`` — the recording — and absent where there is none;
        with ``tokens``, the phoneme on each frame, it is also split by manner class and the
        sibilant tilt is measured (:mod:`auris_singer.intelligibility`). Everything else is
        the trainer's own :mod:`auris_singer.metrics`, so the numbers mean what ``val/…``
        means in the training log.
        """
        n_frames = int(f0.shape[0])
        pred = self.trim(wav, n_frames)
        out: dict[str, float] = {}
        if reference is not None:
            real = self.trim(reference, n_frames)
            mel_pred, mel_real = self.mel(pred), self.mel(real)
            out["mel_l1"] = float((mel_pred - mel_real).abs().mean())
            if tokens is not None:
                power = lambda w: spectrogram(w, self.n_fft, self.hop_length, self.win_length, power=2.0)  # noqa: E731
                out.update(
                    class_spectral_metrics(
                        mel_pred, mel_real, power(pred), power(real), tokens,
                        self.sample_rate, self.n_fft,
                    )
                )

        valid = torch.ones(1, n_frames)
        target_energy = torch.from_numpy(np.asarray(energy, dtype=np.float32)).unsqueeze(0)
        pred_energy = frame_energy(pred, self.n_fft, self.hop_length, self.win_length).unsqueeze(0)
        for name, value in energy_metrics(target_energy, pred_energy, valid).items():
            out[name] = float(value)

        if self.extractor is not None:
            pred_f0, pred_voiced = self.extractor(pred, self.sample_rate, n_frames)
            metrics = pitch_metrics(
                target_f0=torch.from_numpy(np.asarray(f0, dtype=np.float32)).unsqueeze(0),
                target_voiced=torch.from_numpy(np.asarray(voiced, dtype=np.float32)).unsqueeze(0),
                pred_f0=pred_f0.unsqueeze(0),
                pred_voiced=pred_voiced.unsqueeze(0),
                valid=valid,
                tolerance_cents=self.tolerance_cents,
            )
            for name, value in metrics.items():
                out[name] = float(value)

        out["peak"] = float(pred.abs().max()) if n_frames else 0.0
        return out


def phonemes_of_frames(tokens: list[str]) -> list[str]:
    """The phoneme sequence a frames file spells: runs collapsed, then made hearable.

    The host run-length-encodes the frames before the model sees them, so two notes on one
    vowel are one token to it — and one vowel to a listener.
    """
    collapsed: list[str] = []
    for token in tokens:
        if not collapsed or collapsed[-1] != token:
            collapsed.append(token)
    return hearable(collapsed)


def read_wav(path: str | Path, sample_rate: int) -> np.ndarray:
    """A mono float32 waveform, refusing a file at the wrong rate rather than resampling it —
    the host writes at the model's rate, and anything else is a fault worth seeing."""
    wav, rate = sf.read(str(path), dtype="float32", always_2d=True)
    if rate != sample_rate:
        raise ValueError(f"{path} is at {rate} Hz, the voice sings at {sample_rate} Hz")
    return wav[:, 0]


def summarize(rows: list[dict[str, float]]) -> dict[str, float]:
    """Per-metric means over the rows that have the metric, ignoring NaN.

    A metric that no row could answer — pitch with the extractor off — is left out rather
    than reported as zero, for the reason :mod:`auris_singer.metrics` returns NaN.
    """
    out: dict[str, float] = {}
    for name in METRICS:
        values = [
            row[name] for row in rows if name in row and row[name] is not None and math.isfinite(row[name])
        ]
        if values:
            out[name] = float(sum(values) / len(values))
    return out


# ----------------------------------------------------------------------------------------------
# the corpus run
# ----------------------------------------------------------------------------------------------
@dataclass
class Settings:
    """Every choice a run makes, written into the report so a baseline can be read honestly."""

    split: str = "val"
    utterances: int = 8
    seed: int = 1234
    val_size: int = 8
    take_seed: int = 0
    acceleration: str = "auto"
    song: bool = True
    reference: bool = True
    pitch: bool = True
    song_gap_seconds: float = 0.5
    n_mels: int = 128
    tolerance_cents: float = 50.0
    device: str = "cpu"
    #: How many takes to average each column over, seeds ``take_seed`` onwards. One take
    #: is one draw of the prior, and on a small voice a draw can put a phrase an octave out;
    #: the mean over several is the voice, one is the throw.
    take_seeds: int = 1
    #: Which of the voice's speakers sings, by name; ``None`` is the model's first.
    speaker: str | None = None
    #: Whether to run the listener: the phoneme error rate, and the recording's ceiling.
    asr: bool = False
    #: The language the listener listens in — the recogniser and the front-end behind it.
    asr_language: str = "ja"
    #: Options for the recogniser's constructor (``precision``, ``device`` for ReazonSpeech).
    asr_options: dict[str, Any] = field(default_factory=dict)
    extra: dict[str, Any] = field(default_factory=dict)


def make_listener(settings: Settings, listener: Any = None) -> Any:
    """The listener a run uses: the one handed in, or the language's, or none."""
    if listener is not None:
        return listener
    if not settings.asr:
        return None
    from auris_singer.asr import Listener

    return Listener.for_language(settings.asr_language, **settings.asr_options)


def listen(
    listener: Any,
    wav: np.ndarray,
    sample_rate: int,
    asked: list[str],
    into: dict,
    tally: dict[tuple[str, str], int] | None = None,
) -> None:
    """Hear ``wav`` and write the rate against ``asked`` into ``into``, with what was heard.

    ``tally`` collects every ``(asked, heard)`` pair of the alignment across calls, so a run
    can say which phonemes were lost and what they were heard as, not only how many.
    """
    started = time.perf_counter()
    heard = listener.hear(wav, sample_rate)
    into["per"] = phoneme_error_rate(asked, heard.phonemes)
    into["heard"] = heard.text
    into["asr_seconds"] = time.perf_counter() - started
    if tally is not None:
        tally_confusions(asked, heard.phonemes, tally)


def average_rows(rows: list[dict]) -> dict:
    """One row from several takes: every number its mean over the takes that answered it,
    NaN left out; what was heard, the first take's."""
    out: dict[str, Any] = {}
    for key in {k for row in rows for k in row}:
        values = [row[key] for row in rows if key in row]
        numbers = [v for v in values if isinstance(v, (int, float)) and not isinstance(v, bool)]
        if numbers:
            finite = [v for v in numbers if math.isfinite(v)]
            out[key] = sum(finite) / len(finite) if finite else math.nan
        else:
            out[key] = values[0]
    return out


def sum_facts(facts: list[dict]) -> dict:
    """The host's reports of several takes as one: times and chunks added, the rest the
    first's, and ``takes`` saying how many were added."""
    out = dict(facts[0])
    for key in ("seconds", "chunks", "load_seconds", "render_seconds", "wall_seconds", "asr_seconds"):
        if key in facts[0]:
            out[key] = sum(f[key] for f in facts)
    out["on_gpu"] = all(f.get("on_gpu", False) for f in facts)
    out["takes"] = len(facts)
    return out


def evaluate(
    voice: str | Path,
    data_root: str | Path,
    checkpoint: str | Path,
    host: Host,
    workdir: str | Path,
    settings: Settings | None = None,
    listener: Any = None,
) -> dict:
    """Sing corpus utterances through the host and measure every column.

    ``workdir`` keeps every file that crossed the language boundary — the frames, the WAVs,
    the host's reports — so a number in the table can be listened to. With ``settings.asr``
    a listener hears every render and the recording too, and the ``recording`` column is
    the listener's ceiling; ``listener`` hands one in ready-made, which is how a test
    listens without a model.
    """
    settings = settings or Settings()
    listener = make_listener(settings, listener)
    workdir = Path(workdir)
    workdir.mkdir(parents=True, exist_ok=True)
    info = voice_info(voice)
    corpus = Corpus(data_root)
    if corpus.sample_rate != info.sample_rate or corpus.hop_length != info.hop_length:
        raise ValueError(
            f"the corpus is at {corpus.sample_rate} Hz / hop {corpus.hop_length} and the voice "
            f"at {info.sample_rate} Hz / hop {info.hop_length}; they do not share a clock"
        )
    scale = energy_full_scale()
    aligner = Aligner(checkpoint, device=settings.device)
    analyst = Analyst(
        corpus.sample_rate,
        corpus.n_fft,
        corpus.hop_length,
        corpus.win_length,
        n_mels=settings.n_mels,
        pitch=settings.pitch,
        device=settings.device,
        tolerance_cents=settings.tolerance_cents,
    )

    records = corpus.records(settings.split, settings.utterances, settings.seed, settings.val_size)
    if not records:
        raise ValueError(f"no utterances in {data_root}")
    seeds = [settings.take_seed + at for at in range(max(1, settings.take_seeds))]

    utterances: list[Utterance] = []
    tallies: dict[str, dict[tuple[str, str], int]] = {column: {} for column in COLUMNS}
    rows: list[dict] = []
    frames_list: list[HostFrames] = []
    for record in records:
        phonemes, f0, energy, voiced, wav = corpus.load(record)
        durations = corpus.durations(record) or aligner.durations(
            phonemes, wav, f0, energy, voiced, int(record["speaker_id"]),
            corpus.n_fft, corpus.hop_length, corpus.win_length,
        )
        if sum(durations) != f0.shape[0]:
            raise RuntimeError(f"{record['id']}: {sum(durations)} frames of durations for {f0.shape[0]}")
        utterance = Utterance(
            id=record["id"], speaker_id=int(record["speaker_id"]), phonemes=phonemes,
            durations=durations, f0=f0, energy=energy, wav=wav,
        )
        utterances.append(utterance)
        stem = record["id"].replace("/", "_")
        frames = frames_from_curves(phonemes, durations, f0, energy, info.hop_seconds, scale)
        frames_list.append(frames)
        frames_path = frames.write(workdir / f"{stem}.frames.json")
        sf.write(str(workdir / f"{stem}.real.wav"), wav, corpus.sample_rate)

        asked = hearable(phonemes)
        takes: list[dict] = []
        for seed in seeds:
            suffix = "" if len(seeds) == 1 else f".s{seed}"
            rendered = workdir / f"{stem}{suffix}.host.wav"
            facts = host.sing_frames(
                frames_path, info.path, rendered, seed=seed, acceleration=settings.acceleration,
                speaker=settings.speaker,
            )
            sung = read_wav(rendered, info.sample_rate)
            take: dict[str, Any] = {
                "host": analyst.measure(
                    sung, f0, energy, utterance.voiced, reference=wav, tokens=utterance.tokens
                ),
                "timing": facts,
            }
            if listener is not None:
                listen(listener, sung, info.sample_rate, asked, take["host"], tallies["host"])
            if settings.reference:
                started = time.perf_counter()
                reference = aligner.reference(utterance, seed)
                take["reference_seconds"] = time.perf_counter() - started
                sf.write(str(workdir / f"{stem}{suffix}.reference.wav"), reference, info.sample_rate)
                take["reference"] = analyst.measure(
                    reference, f0, energy, utterance.voiced, reference=wav, tokens=utterance.tokens
                )
                if listener is not None:
                    listen(
                        listener, reference, info.sample_rate, asked, take["reference"], tallies["reference"]
                    )
            takes.append(take)
        row: dict[str, Any] = {
            "id": record["id"],
            "speaker_id": int(record["speaker_id"]),
            "n_frames": utterance.n_frames,
            "seconds": utterance.n_frames * info.hop_seconds,
            "seeds": list(seeds),
            "host": average_rows([take["host"] for take in takes]),
            "timing": sum_facts([take["timing"] for take in takes]),
        }
        if settings.reference:
            row["reference"] = average_rows([take["reference"] for take in takes])
            row["reference_seconds"] = sum(take["reference_seconds"] for take in takes)
        if listener is not None:
            row["asked"] = " ".join(asked)
            row["recording"] = {}
            listen(listener, wav, info.sample_rate, asked, row["recording"], tallies["recording"])
        rows.append(row)
        logger.info("%s: %s", record["id"], _one_line(row["host"]))

    song_facts: dict | None = None
    if settings.song and len(utterances) > 1:
        gap = int(round(settings.song_gap_seconds / info.hop_seconds))
        joined, spans = concatenate_frames(frames_list, gap)
        frames_path = joined.write(workdir / "song.frames.json")
        song_takes: list[dict] = []
        per_row: list[list[dict]] = [[] for _ in rows]
        for seed in seeds:
            suffix = "" if len(seeds) == 1 else f".s{seed}"
            rendered = workdir / f"song{suffix}.host.wav"
            song_takes.append(
                host.sing_frames(
                    frames_path, info.path, rendered, seed=seed, acceleration=settings.acceleration,
                    speaker=settings.speaker,
                )
            )
            sung = read_wav(rendered, info.sample_rate)
            for at, (utterance, (start, end)) in enumerate(zip(utterances, spans)):
                piece = sung[start * info.hop_length : end * info.hop_length]
                measured = analyst.measure(
                    piece, utterance.f0, utterance.energy, utterance.voiced,
                    reference=utterance.wav, tokens=utterance.tokens,
                )
                if listener is not None:
                    listen(
                        listener, piece, info.sample_rate, hearable(utterance.phonemes), measured,
                        tallies["song"],
                    )
                per_row[at].append(measured)
        for row, measured in zip(rows, per_row):
            row["song"] = average_rows(measured)
        song_facts = sum_facts(song_takes)
        song_facts["seconds_of_frames"] = joined.seconds

    audio_seconds = sum(row["seconds"] for row in rows) * len(seeds)
    render_seconds = sum(row["timing"]["render_seconds"] for row in rows)
    timing = {
        "takes": len(seeds),
        "audio_seconds": audio_seconds,
        "render_seconds": render_seconds,
        "rtf": render_seconds / audio_seconds if audio_seconds else math.nan,
        "load_seconds_mean": sum(row["timing"]["load_seconds"] for row in rows) / (len(rows) * len(seeds)),
        "wall_seconds": sum(row["timing"]["wall_seconds"] for row in rows),
        "on_gpu": all(row["timing"]["on_gpu"] for row in rows),
        "chunks": sum(row["timing"]["chunks"] for row in rows),
    }
    if settings.reference:
        timing["reference_seconds"] = sum(row["reference_seconds"] for row in rows)
    if song_facts is not None:
        timing["song"] = song_facts
    if listener is not None:
        timing["asr_seconds"] = sum(
            row[column]["asr_seconds"] for row in rows for column in COLUMNS if column in row
        )

    summary: dict[str, Any] = {"host": summarize([row["host"] for row in rows]), "timing": timing}
    if settings.reference:
        summary["reference"] = summarize([row["reference"] for row in rows])
    if song_facts is not None:
        summary["song"] = summarize([row["song"] for row in rows])
    if listener is not None:
        summary["recording"] = summarize([row["recording"] for row in rows])
        summary["confusions"] = {
            column: confusion_rows(tally) for column, tally in tallies.items() if tally
        }

    return {
        "kind": "corpus",
        "voice": {
            "path": str(info.path), "name": info.name,
            "sample_rate": info.sample_rate, "hop_length": info.hop_length,
        },
        "checkpoint": str(checkpoint),
        "dataset": {"root": str(corpus.root), "utterances": [row["id"] for row in rows]},
        "host": {"command": host.command, "acceleration": settings.acceleration},
        "settings": settings.__dict__ | {"energy_full_scale": scale, "noise_scale": NOISE_SCALE},
        "utterances": rows,
        "summary": summary,
    }


# ----------------------------------------------------------------------------------------------
# the score run
# ----------------------------------------------------------------------------------------------
#: The specification sung when none is given: a lyric, and nothing decided for the composer.
DEFAULT_SPEC = """\
# What the host evaluation sings when no specification is given: a verse of kana, so that the
# built-in table reads it and no dictionary needs installing.
title = "Host evaluation"
form  = "verse"

[section.verse]
lyrics = "さくら さいた、はるが きた\\nかぜに ゆれて、ひかり ふる"
"""


def evaluate_score(
    voice: str | Path,
    host: Host,
    workdir: str | Path,
    spec: str | Path | None = None,
    settings: Settings | None = None,
    listener: Any = None,
) -> dict:
    """Notes and words through the whole path a person walks, measured against the frames.

    ``compose`` writes a project from the specification, ``frames`` writes what its singer
    track will be sung as, ``sing`` sings it into the project; the take is then measured
    against those frames — the pitch and energy the session itself asked for — with the
    frames' energy put back on the model's scale the way the host puts it. No recording
    exists to measure spectral distance against, so ``mel_l1`` is absent here by design.
    """
    settings = settings or Settings()
    listener = make_listener(settings, listener)
    workdir = Path(workdir)
    workdir.mkdir(parents=True, exist_ok=True)
    info = voice_info(voice)
    scale = energy_full_scale()
    if spec is None:
        spec = workdir / "score.asong"
        Path(spec).write_text(DEFAULT_SPEC, encoding="utf-8")

    project = host.compose(spec, workdir / "score.auris", seed=settings.seed)
    asked = None
    tokens: list[str] = []
    analyst = Analyst(
        info.sample_rate, 2048, info.hop_length, 2048,
        n_mels=settings.n_mels, pitch=settings.pitch, device=settings.device,
        tolerance_cents=settings.tolerance_cents,
    )
    seeds = [settings.take_seed + at for at in range(max(1, settings.take_seeds))]
    takes: list[dict] = []
    tally: dict[tuple[str, str], int] = {}
    wall = 0.0
    take = None
    frames = None
    for seed in seeds:
        take = host.sing(project, info.path, seed, speaker=settings.speaker)
        wall += host.last_wall_seconds
        if frames is None:
            # Only now: choosing the voice put its clock, its consonant widths and its levels
            # into the document, and the frames the take was sung from are the ones the
            # document writes *after* that. Written before, they would be another song's.
            frames = host.frames(project, workdir / "score.frames.json")
            if abs(frames.hop_seconds - info.hop_seconds) > 1e-9:
                raise ValueError(
                    f"the track's hop is {frames.hop_seconds} s and the voice's "
                    f"{info.hop_seconds} s even after the voice was chosen"
                )
            tokens = frames.tokens()
            f0 = np.asarray(frames.f0_hz, dtype=np.float32)
            energy = np.asarray(frames.energy, dtype=np.float32) * scale
            voiced = np.asarray(
                [1.0 if (hz > 0 and t != SILENCE and not is_voiceless(t)) else 0.0 for hz, t in zip(f0, tokens)],
                dtype=np.float32,
            )
            asked = phonemes_of_frames(tokens)
        sung = read_wav(take, info.sample_rate)
        measured = analyst.measure(sung, f0, energy, voiced)
        if listener is not None:
            listen(listener, sung, info.sample_rate, asked, measured, tally)
        takes.append(measured)
    metrics = average_rows(takes)
    seconds = frames.seconds * len(seeds)
    return {
        "kind": "score",
        "voice": {"path": str(info.path), "name": info.name, "sample_rate": info.sample_rate},
        "spec": str(spec),
        "project": str(project),
        "take": str(take),
        "host": {"command": host.command},
        "settings": settings.__dict__ | {"energy_full_scale": scale},
        "frames": {"count": len(frames), "seconds": frames.seconds, "inventory": frames.inventory},
        "asked": " ".join(asked),
        "seeds": seeds,
        "score": metrics,
        "summary": {
            "score": metrics,
            "timing": {
                "takes": len(seeds),
                "audio_seconds": seconds,
                "wall_seconds": wall,
                "wall_rtf": wall / seconds if seconds else math.nan,
            },
        }
        | ({"confusions": {"score": confusion_rows(tally)}} if tally else {}),
    }


# ----------------------------------------------------------------------------------------------
# the table
# ----------------------------------------------------------------------------------------------
def _one_line(metrics: dict[str, Any]) -> str:
    """The numbers of one row on one log line; what was heard is text and stays out."""
    return "  ".join(
        f"{name}={value:.3f}" for name, value in metrics.items() if isinstance(value, (int, float))
    )


def _cell(value: float | None, width: int = 9) -> str:
    if value is None or not math.isfinite(value):
        return "—".rjust(width)
    return f"{value:.3f}".rjust(width)


def _delta(a: float | None, b: float | None, width: int = 9) -> str:
    if a is None or b is None or not (math.isfinite(a) and math.isfinite(b)):
        return "".rjust(width)
    return f"{a - b:+.3f}".rjust(width)


def format_report(report: dict, baseline: dict | None = None) -> str:
    """The report as a table for a terminal, with a baseline's deltas beside it when given."""
    summary = report["summary"]
    lines: list[str] = []
    name = report["voice"].get("name") or Path(report["voice"]["path"]).stem
    lines.append(f"{name} — {report['voice']['path']}")

    columns = [key for key in COLUMNS if key in summary]
    if baseline is not None:
        base = baseline.get("summary", {})
    else:
        base = {}
    header = "metric".ljust(20) + "".join(c.rjust(10) for c in columns)
    if "reference" in columns:
        header += "  host−ref".rjust(10)
    if "song" in columns:
        header += " song−host".rjust(10)
    if baseline is not None:
        header += "  Δ baseline".rjust(12)
    lines.append(header)
    lines.append("-" * len(header))
    for metric in METRICS:
        if not any(metric in summary[c] for c in columns):
            continue
        line = metric.ljust(20)
        for column in columns:
            line += " " + _cell(summary[column].get(metric))
        if "reference" in columns:
            line += " " + _delta(summary["host"].get(metric), summary["reference"].get(metric))
        if "song" in columns:
            line += " " + _delta(summary["song"].get(metric), summary["host"].get(metric))
        if baseline is not None:
            main = columns[0]
            line += "   " + _delta(summary[main].get(metric), base.get(main, {}).get(metric))
        lines.append(line)

    timing = summary.get("timing", {})
    if "rtf" in timing:
        rtf = timing["rtf"]
        faster = f"{1 / rtf:.0f}× realtime" if rtf and math.isfinite(rtf) and rtf > 0 else "—"
        gpu = "GPU" if timing.get("on_gpu") else "CPU"
        takes = timing.get("takes", 1)
        lines.append(
            f"timing: {timing['render_seconds']:.2f} s to sing {timing['audio_seconds']:.1f} s "
            f"of audio on the {gpu} · RTF {rtf:.3f} ({faster}) · model open in "
            f"{timing['load_seconds_mean']:.2f} s · {timing['chunks']} chunk(s)"
            + (f" · {takes} takes averaged" if takes > 1 else "")
        )
        if "reference_seconds" in timing:
            lines.append(f"        reference (PyTorch) took {timing['reference_seconds']:.2f} s")
        song = timing.get("song")
        if song:
            lines.append(
                f"        song: {song['seconds_of_frames']:.1f} s of frames in {song['chunks']} "
                f"chunk(s), sung in {song['render_seconds']:.2f} s"
            )
        if "asr_seconds" in timing:
            lines.append(f"        the listener took {timing['asr_seconds']:.2f} s over every column")
    elif "wall_rtf" in timing:
        lines.append(
            f"timing: `auris sing` took {timing['wall_seconds']:.2f} s wall for "
            f"{timing['audio_seconds']:.1f} s of audio (session and model included) · "
            f"wall RTF {timing['wall_rtf']:.3f}"
        )
    confusions = summary.get("confusions", {})
    main = columns[0] if columns else None
    if main in confusions:
        table = confusion_table({(asked, heard): n for asked, heard, n in confusions[main]})
        if table:
            lines.append(f"what the listener heard, {main} column, worst phonemes first:")
            lines.extend("  " + line for line in table.split("\n"))
    return "\n".join(lines)
