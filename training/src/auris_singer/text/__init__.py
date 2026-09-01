"""Text front-ends producing IPA phoneme sequences."""

from __future__ import annotations

from auris_singer.text.ipa import (
    DEFAULT_PHONEME_TABLE,
    IPA_SYMBOLS,
    PAD,
    PAU,
    SIL,
    SPECIAL_SYMBOLS,
    UNK,
    VOICELESS,
    PhonemeTable,
    is_voiceless,
)
from auris_singer.text.japanese import JapaneseFrontend, openjtalk_to_ipa

__all__ = [
    "DEFAULT_PHONEME_TABLE",
    "IPA_SYMBOLS",
    "PAD",
    "PAU",
    "SIL",
    "SPECIAL_SYMBOLS",
    "UNK",
    "VOICELESS",
    "PhonemeTable",
    "is_voiceless",
    "JapaneseFrontend",
    "openjtalk_to_ipa",
    "parse_ipa",
    "get_frontend",
]


def parse_ipa(text: str) -> list[str]:
    """Parse a whitespace-separated IPA phoneme string.

    IPA symbols are multi-character, so sequences must always be written with
    spaces between phonemes (``"k o ɴ ɲ i tɕ i w a"``).
    """
    return text.split()


class IpaFrontend:
    """Pass-through front-end for inputs that are already IPA."""

    def g2p(self, text: str) -> list[str]:
        return parse_ipa(text)

    def __call__(self, text: str) -> list[str]:
        return self.g2p(text)


def get_frontend(language: str, **kwargs):
    """Return the front-end for ``language``.

    Args:
        language: ``"ja"`` for Japanese text, ``"ipa"`` to accept IPA directly.
        **kwargs: forwarded to the front-end constructor.
    """
    language = language.lower()
    if language in {"ja", "jp", "japanese"}:
        return JapaneseFrontend(**kwargs)
    if language in {"ipa", "none", "raw"}:
        return IpaFrontend()
    raise ValueError(
        f"unsupported language {language!r}; use 'ja' for Japanese text or "
        "'ipa' to supply phonemes directly"
    )
