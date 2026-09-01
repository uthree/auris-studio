"""IPA phoneme table.

The model consumes IPA symbols, so a single table serves every language the
front-end supports.  The table is defined here rather than derived from the
data, which keeps phoneme ids stable across datasets and checkpoints.

Symbols are multi-character strings (``"tɕ"``, ``"kʲ"``, ...), so phoneme
sequences must be tokenized as lists, never by iterating over a string.
"""

from __future__ import annotations

import json
from pathlib import Path

__all__ = [
    "PAD",
    "UNK",
    "SIL",
    "PAU",
    "SPECIAL_SYMBOLS",
    "IPA_SYMBOLS",
    "PhonemeTable",
    "DEFAULT_PHONEME_TABLE",
]

PAD = "<pad>"
UNK = "<unk>"
SIL = "<sil>"
PAU = "<pau>"

#: Special symbols. ``PAD`` must stay at index 0 so padded positions embed to a
#: dedicated vector and are masked out anyway.
SPECIAL_SYMBOLS: tuple[str, ...] = (PAD, UNK, SIL, PAU)

#: IPA inventory. It covers Japanese exhaustively and includes the common
#: additional symbols needed for other languages, so the table does not have to
#: change when a new language front-end is added.
IPA_SYMBOLS: tuple[str, ...] = (
    # --- vowels -------------------------------------------------------
    "a", "i", "u", "e", "o", "ɯ", "ɨ", "ə", "ɛ", "ɔ", "æ", "ʌ", "ɑ", "ɒ",
    "ʊ", "ɪ", "y", "ø", "œ", "ɐ",
    # devoiced Japanese vowels
    "ḁ", "i̥", "ɯ̥", "e̥", "o̥",
    # length mark, used as a standalone token by some front-ends
    "ː",
    # --- nasals -------------------------------------------------------
    "m", "mʲ", "n", "nʲ", "ɲ", "ŋ", "ɴ",
    # --- plosives -----------------------------------------------------
    "p", "pʲ", "b", "bʲ", "t", "tʲ", "d", "dʲ", "k", "kʲ", "kʷ",
    "g", "gʲ", "gʷ", "ʔ",
    # --- affricates ---------------------------------------------------
    "ts", "dz", "tɕ", "dʑ", "tʃ", "dʒ",
    # --- fricatives ---------------------------------------------------
    "ɸ", "ɸʲ", "β", "f", "v", "θ", "ð", "s", "z", "ɕ", "ʑ", "ʃ", "ʒ",
    "ç", "x", "ɣ", "h", "ɦ",
    # --- approximants / liquids --------------------------------------
    "j", "w", "ɰ", "ɹ", "ɾ", "ɾʲ", "r", "l", "ʎ",
)

#: Symbols produced without vocal-fold vibration: voiceless obstruents, the
#: devoiced vowels, and every non-sound special symbol. This is what decides a
#: frame's ``voiced`` flag when a caller supplies phonemes but no explicit
#: voicing — a score front-end writes f0 as a contour across consonants, so
#: voicing must come from the phoneme class, never from ``f0 > 0``.
VOICELESS: frozenset[str] = frozenset(
    SPECIAL_SYMBOLS
) | frozenset(
    {
        # ḁ is `a` + U+0325, the spelling the inventory and the front-end use — the
        # precomposed U+1E01 is another string to a set, and was here until a test looked
        "ḁ", "i̥", "ɯ̥", "e̥", "o̥",
        "p", "pʲ", "t", "tʲ", "k", "kʲ", "kʷ", "ʔ",
        "ts", "tɕ", "tʃ",
        "ɸ", "ɸʲ", "f", "θ", "s", "ɕ", "ʃ", "ç", "x", "h",
    }
)


def is_voiceless(symbol: str) -> bool:
    """``True`` for a phoneme produced without vocal-fold vibration.

    Unknown symbols answer ``False`` — a wrongly-voiced frame keeps the pitch
    contour intact, while a wrongly-unvoiced one silences it.
    """
    return symbol in VOICELESS


class PhonemeTable:
    """Bidirectional map between IPA symbols and integer ids."""

    def __init__(self, symbols: tuple[str, ...] | list[str] | None = None):
        if symbols is None:
            symbols = list(SPECIAL_SYMBOLS) + list(IPA_SYMBOLS)
        self.symbols: list[str] = list(symbols)
        if len(set(self.symbols)) != len(self.symbols):
            duplicates = {s for s in self.symbols if self.symbols.count(s) > 1}
            raise ValueError(f"duplicate symbols in phoneme table: {sorted(duplicates)}")
        self._symbol_to_id = {s: i for i, s in enumerate(self.symbols)}

    def __len__(self) -> int:
        return len(self.symbols)

    def __contains__(self, symbol: str) -> bool:
        return symbol in self._symbol_to_id

    @property
    def pad_id(self) -> int:
        return self._symbol_to_id[PAD]

    @property
    def unk_id(self) -> int:
        return self._symbol_to_id[UNK]

    def encode(self, phonemes: list[str]) -> list[int]:
        """Map IPA symbols to ids; unknown symbols become ``UNK``."""
        unk = self.unk_id
        return [self._symbol_to_id.get(p, unk) for p in phonemes]

    def decode(self, ids: list[int]) -> list[str]:
        return [self.symbols[i] for i in ids]

    def unknown_symbols(self, phonemes: list[str]) -> list[str]:
        """Symbols from ``phonemes`` that are missing from the table."""
        return sorted({p for p in phonemes if p not in self._symbol_to_id})

    def save(self, path: str | Path) -> None:
        Path(path).write_text(
            json.dumps({"symbols": self.symbols}, ensure_ascii=False, indent=2),
            encoding="utf-8",
        )

    @classmethod
    def load(cls, path: str | Path) -> PhonemeTable:
        data = json.loads(Path(path).read_text(encoding="utf-8"))
        return cls(data["symbols"])


#: Shared default instance.
DEFAULT_PHONEME_TABLE = PhonemeTable()
