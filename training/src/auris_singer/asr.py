"""Recognisers: what a listener would have heard, one language at a time.

The second intelligibility instrument (:mod:`auris_singer.intelligibility`) needs a listener,
and the nearest thing to one that can be run a thousand times is a speech recogniser. This
module is the seam between the evaluation and whichever recogniser a language has: a
:class:`Recogniser` turns audio into text in its language, the language's own front-end from
:mod:`auris_singer.text` turns the text back into IPA, and the phoneme error rate is taken
between that and the phonemes that were asked for. Only the first step knows a language; the
second is what the trainer already has for every language it can train on, and the third
is arithmetic. So adding a language is one class registered under its code in
:data:`RECOGNISERS` — and, where the trainer does not yet have a front-end for it, one in
``text/``.

Japanese is ReazonSpeech (`reazon-research/ReazonSpeech`), the k2 model through
sherpa-onnx: trained on 35 000 hours of Japanese broadcast speech, Apache-2.0, a 200 MB
download on first use into the Hugging Face cache, and fast on a CPU. It transcribes a
recording of a sung phrase near-perfectly and a mid-training synthesis as the nonsense it
is, which is exactly the spread a metric wants. It is an optional extra (``asr``), since
nothing else here needs it and the package is installed from git.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from typing import Any, Protocol

import numpy as np

from auris_singer.intelligibility import hearable
from auris_singer.text import get_frontend

__all__ = [
    "Recogniser",
    "ReazonSpeech",
    "RECOGNISERS",
    "recogniser_for",
    "Listener",
    "Heard",
]


class Recogniser(Protocol):
    """Audio in one language to text in that language."""

    #: The language code, as :func:`auris_singer.text.get_frontend` spells it.
    language: str

    def transcribe(self, wav: np.ndarray, sample_rate: int) -> str:
        """The words in a mono float waveform, as the language writes them."""


class ReazonSpeech:
    """Japanese, through ReazonSpeech's k2 transducer on sherpa-onnx.

    The model is loaded on first use and kept. ``precision`` is ``"fp32"`` or ``"int8"``;
    ``device`` is ``"cpu"``, ``"cuda"`` or ``"coreml"`` as sherpa-onnx spells them, and the
    CPU is fast enough that the GPU is rarely worth the provider.
    """

    language = "ja"

    def __init__(self, precision: str = "fp32", device: str = "cpu"):
        self.precision = precision
        self.device = device
        self._model = None

    @property
    def model(self):
        if self._model is None:
            try:
                from reazonspeech.k2.asr import load_model
            except ImportError as error:  # pragma: no cover - depends on the environment
                raise ImportError(
                    "ReazonSpeech is not installed; `uv pip install -e '.[asr]'` adds it"
                ) from error
            self._model = load_model(device=self.device, precision=self.precision, language="ja")
        return self._model

    #: Below this peak the audio is digital silence, and is not played to the model at all:
    #: ReazonSpeech answers two seconds of exact zeros with うん, which no listener would.
    SILENCE_PEAK = 1e-4

    def transcribe(self, wav: np.ndarray, sample_rate: int) -> str:
        from reazonspeech.k2.asr import audio_from_numpy, transcribe

        wav = np.ascontiguousarray(np.asarray(wav, dtype=np.float32))
        if wav.size == 0 or not np.any(np.abs(wav) > self.SILENCE_PEAK):
            return ""
        return transcribe(self.model, audio_from_numpy(wav, sample_rate)).text.strip()


#: Every recogniser this module knows, by the language code its front-end answers to.
RECOGNISERS: dict[str, Callable[..., Recogniser]] = {"ja": ReazonSpeech}


def recogniser_for(language: str, **options: Any) -> Recogniser:
    """The recogniser for ``language``, with ``options`` handed to its constructor."""
    key = language.lower()
    if key in {"jp", "japanese"}:
        key = "ja"
    try:
        factory = RECOGNISERS[key]
    except KeyError:
        known = ", ".join(sorted(RECOGNISERS))
        raise ValueError(
            f"no recogniser for {language!r}; this build listens in: {known}"
        ) from None
    return factory(**options)


@dataclass
class Heard:
    """What the listener made of a render: the text, and the phonemes it amounts to."""

    text: str
    phonemes: list[str]


@dataclass
class Listener:
    """A recogniser and its language's front-end, together: audio to comparable IPA.

    The front-end is built without boundary silence — a listener reports words, not rests —
    and both what it hears and what was asked for go through :func:`hearable` before they
    are compared, so the two sides are held to the same spelling.
    """

    recogniser: Recogniser
    frontend: Any = None

    def __post_init__(self) -> None:
        if self.frontend is None:
            self.frontend = get_frontend(self.recogniser.language, add_boundary_silence=False)

    @classmethod
    def for_language(cls, language: str, **options: Any) -> Listener:
        return cls(recogniser_for(language, **options))

    def hear(self, wav: np.ndarray, sample_rate: int) -> Heard:
        text = self.recogniser.transcribe(wav, sample_rate)
        phonemes = self.frontend.g2p(text) if text.strip() else []
        return Heard(text=text, phonemes=hearable(phonemes))
