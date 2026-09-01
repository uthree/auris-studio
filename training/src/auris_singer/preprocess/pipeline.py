"""Dataset preprocessing.

Each utterance is turned into a single ``.npz`` file holding the resampled
waveform plus every frame-level feature the model needs:

===========  =========  ==============================================
key          dtype      contents
===========  =========  ==============================================
``wav``      int16      waveform at ``sample_rate``, length ``T * hop``
``phonemes`` int32      IPA phoneme ids, shape ``(S,)``
``f0``       float32    Hz per frame, 0 on unvoiced frames, ``(T,)``
``energy``   float32    per-frame RMS, ``(T,)``
``voiced``   uint8      voiced flag, ``(T,)``
===========  =========  ==============================================

The linear spectrogram is *not* cached: recomputing it in the dataloader costs
little and keeps the dataset roughly 4x smaller on disk.
"""

from __future__ import annotations

import json
import logging
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from pathlib import Path

import numpy as np
import soundfile as sf
import torch
import torchaudio
from omegaconf import DictConfig
from tqdm import tqdm

from auris_singer.preprocess.f0 import FcpeExtractor
from auris_singer.text import DEFAULT_PHONEME_TABLE, PhonemeTable, get_frontend
from auris_singer.utils.audio import frame_energy

logger = logging.getLogger(__name__)

__all__ = ["Utterance", "collect_utterances", "run_preprocess"]


@dataclass
class Utterance:
    """One (audio, text) pair belonging to a speaker."""

    utt_id: str
    wav_path: Path
    text_path: Path | None
    speaker: str


def collect_utterances(sources) -> list[Utterance]:
    """Discover utterances described by the ``dataset.sources`` config list.

    Each source needs a ``name`` and a ``wav_dir``.  Transcripts are looked up
    in ``text_dir`` (default: ``wav_dir``) under the same stem with
    ``text_suffix`` (default ``.txt``).
    """
    utterances: list[Utterance] = []
    for source in sources:
        speaker = str(source["name"])
        wav_dir = Path(source["wav_dir"])
        text_dir = Path(source.get("text_dir") or wav_dir)
        text_suffix = str(source.get("text_suffix", ".txt"))
        wav_suffix = str(source.get("wav_suffix", ".wav"))
        if not wav_dir.is_dir():
            raise FileNotFoundError(f"wav_dir does not exist: {wav_dir}")

        for wav_path in sorted(wav_dir.rglob(f"*{wav_suffix}")):
            relative = wav_path.relative_to(wav_dir).with_suffix("")
            text_path = (text_dir / relative).with_suffix(text_suffix)
            utterances.append(
                Utterance(
                    utt_id=f"{speaker}/{relative.as_posix()}",
                    wav_path=wav_path,
                    text_path=text_path if text_path.is_file() else None,
                    speaker=speaker,
                )
            )
    return utterances


def _load_audio(path: Path, sample_rate: int, peak_normalize: bool, peak: float):
    wav, sr = sf.read(str(path), dtype="float32", always_2d=True)
    wav = torch.from_numpy(wav.mean(axis=1))
    if sr != sample_rate:
        wav = torchaudio.functional.resample(wav, sr, sample_rate)
    if peak_normalize:
        scale = wav.abs().max()
        if scale > 1e-6:
            wav = wav * (peak / scale)
    return wav.clamp(-1.0, 1.0)


def run_preprocess(config: DictConfig) -> dict[str, int]:
    """Run the full preprocessing pipeline described by ``config``.

    Returns:
        A summary dict with the number of processed and skipped utterances.
    """
    audio_cfg = config.audio
    f0_cfg = config.f0
    dataset_cfg = config.dataset

    sample_rate = int(audio_cfg.sample_rate)
    hop_length = int(audio_cfg.hop_length)
    n_fft = int(audio_cfg.n_fft)
    win_length = int(audio_cfg.win_length)

    output_dir = Path(dataset_cfg.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)

    utterances = collect_utterances(dataset_cfg.sources)
    if not utterances:
        raise RuntimeError("no utterances found; check dataset.sources in the config")

    speakers = sorted({u.speaker for u in utterances})
    speaker_to_id = {name: i for i, name in enumerate(speakers)}

    frontend = get_frontend(str(config.text.language), **dict(config.text.get("options", {})))
    table = (
        PhonemeTable.load(config.text.phoneme_table)
        if config.text.get("phoneme_table")
        else DEFAULT_PHONEME_TABLE
    )
    extractor = FcpeExtractor(
        device=str(f0_cfg.get("device", "cpu")),
        f0_min=float(f0_cfg.f0_min),
        f0_max=float(f0_cfg.f0_max),
        threshold=float(f0_cfg.get("threshold", 0.006)),
        decoder_mode=str(f0_cfg.get("decoder_mode", "local_argmax")),
    )

    min_samples = int(float(audio_cfg.get("min_seconds", 0.0)) * sample_rate)
    max_samples = int(float(audio_cfg.get("max_seconds", 1e9)) * sample_rate)
    peak_normalize = bool(audio_cfg.get("peak_normalize", True))
    peak = float(audio_cfg.get("peak", 0.95))

    def stage_one(utt: Utterance):
        """Audio loading and grapheme-to-phoneme; safe to run in threads."""
        if utt.text_path is None:
            return utt, None, None, "missing transcript"
        text = utt.text_path.read_text(encoding="utf-8").strip()
        if not text:
            return utt, None, None, "empty transcript"
        phonemes = frontend.g2p(text)
        if not phonemes:
            return utt, None, None, "empty phoneme sequence"
        wav = _load_audio(utt.wav_path, sample_rate, peak_normalize, peak)
        return utt, wav, (text, phonemes), None

    records: list[dict] = []
    skipped: dict[str, int] = {}

    def skip(reason: str) -> None:
        skipped[reason] = skipped.get(reason, 0) + 1

    num_workers = int(config.get("num_workers", 4))
    with ThreadPoolExecutor(max_workers=max(num_workers, 1)) as pool:
        for utt, wav, text_info, error in tqdm(
            pool.map(stage_one, utterances), total=len(utterances), desc="preprocess"
        ):
            if error is not None:
                logger.warning("skipping %s: %s", utt.utt_id, error)
                skip(error)
                continue

            if wav.numel() < max(min_samples, hop_length):
                skip("too short")
                continue
            if wav.numel() > max_samples:
                wav = wav[:max_samples]

            n_frames = wav.numel() // hop_length
            wav = wav[: n_frames * hop_length]

            text, phonemes = text_info
            unknown = table.unknown_symbols(phonemes)
            if unknown:
                logger.warning("%s contains symbols missing from the table: %s", utt.utt_id, unknown)
            phoneme_ids = table.encode(phonemes)
            if n_frames < len(phoneme_ids):
                # Monotonic alignment search needs at least one frame per phoneme.
                skip("fewer frames than phonemes")
                continue

            energy = frame_energy(wav, n_fft, hop_length, win_length)
            f0, voiced = extractor(wav, sample_rate, n_frames)

            out_path = output_dir / f"{utt.utt_id}.npz"
            out_path.parent.mkdir(parents=True, exist_ok=True)
            np.savez(
                out_path,
                wav=(wav.numpy() * 32767.0).astype(np.int16),
                phonemes=np.asarray(phoneme_ids, dtype=np.int32),
                f0=f0.numpy().astype(np.float32),
                energy=energy.numpy().astype(np.float32),
                voiced=voiced.numpy().astype(np.uint8),
            )
            records.append(
                {
                    "id": utt.utt_id,
                    "path": str(out_path.relative_to(output_dir)),
                    "speaker": utt.speaker,
                    "speaker_id": speaker_to_id[utt.speaker],
                    "n_frames": int(n_frames),
                    "n_phonemes": len(phoneme_ids),
                    "seconds": round(n_frames * hop_length / sample_rate, 3),
                    "text": text,
                }
            )

    if not records:
        raise RuntimeError(f"every utterance was skipped: {skipped}")

    with (output_dir / "metadata.jsonl").open("w", encoding="utf-8") as fp:
        for record in records:
            fp.write(json.dumps(record, ensure_ascii=False) + "\n")
    (output_dir / "speakers.json").write_text(
        json.dumps(speaker_to_id, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    table.save(output_dir / "phonemes.json")
    (output_dir / "audio_config.json").write_text(
        json.dumps(
            {
                "sample_rate": sample_rate,
                "n_fft": n_fft,
                "hop_length": hop_length,
                "win_length": win_length,
            },
            indent=2,
        ),
        encoding="utf-8",
    )

    logger.info("processed %d utterances, skipped %s", len(records), skipped or "none")
    return {"processed": len(records), "skipped": sum(skipped.values()), **skipped}
