"""Japanese grapheme-to-phoneme front-end.

``jpreprocess`` (an OpenJTalk rewrite) produces the usual romanized OpenJTalk
phoneme set; this module maps it onto the IPA inventory in
:mod:`auris_singer.text.ipa`.
"""

from __future__ import annotations

import threading

from auris_singer.text.ipa import PAU, SIL

__all__ = ["OPENJTALK_TO_IPA", "openjtalk_to_ipa", "JapaneseFrontend"]

#: OpenJTalk phoneme -> IPA.
#:
#: Uppercase vowels are OpenJTalk's devoiced vowels; ``cl`` is the sokuon
#: (geminate closure) and ``N`` the moraic nasal.
OPENJTALK_TO_IPA: dict[str, str] = {
    # vowels
    "a": "a",
    "i": "i",
    "u": "ɯ",
    "e": "e",
    "o": "o",
    # devoiced vowels
    "A": "ḁ",
    "I": "i̥",
    "U": "ɯ̥",
    "E": "e̥",
    "O": "o̥",
    # nasals
    "m": "m",
    "my": "mʲ",
    "n": "n",
    "ny": "ɲ",
    "N": "ɴ",
    # plosives
    "p": "p",
    "py": "pʲ",
    "b": "b",
    "by": "bʲ",
    "t": "t",
    "ty": "tʲ",
    "d": "d",
    "dy": "dʲ",
    "k": "k",
    "ky": "kʲ",
    "kw": "kʷ",
    "g": "g",
    "gy": "gʲ",
    "gw": "gʷ",
    "cl": "ʔ",
    # affricates
    "ts": "ts",
    "ch": "tɕ",
    "j": "dʑ",
    "z": "dz",
    # fricatives
    "s": "s",
    "sh": "ɕ",
    "f": "ɸ",
    "fy": "ɸʲ",
    "v": "v",
    "h": "h",
    "hy": "ç",
    # approximants
    "y": "j",
    "w": "w",
    "r": "ɾ",
    "ry": "ɾʲ",
    # boundaries
    "sil": SIL,
    "pau": PAU,
}


def openjtalk_to_ipa(phonemes: list[str], keep_unknown: bool = False) -> list[str]:
    """Translate an OpenJTalk phoneme sequence to IPA.

    Args:
        phonemes: OpenJTalk phoneme symbols.
        keep_unknown: pass unmapped symbols through instead of dropping them.
            Useful when debugging a new dictionary.
    """
    out: list[str] = []
    for p in phonemes:
        mapped = OPENJTALK_TO_IPA.get(p)
        if mapped is not None:
            out.append(mapped)
        elif keep_unknown:
            out.append(p)
    return out


class JapaneseFrontend:
    """Japanese text -> IPA phoneme sequence.

    The underlying ``jpreprocess`` object is built lazily (it loads a ~30 MB
    dictionary) and shared across calls; a lock keeps construction safe when
    several dataloader workers touch it at once.

    Args:
        dictionary_version: ``jpreprocess`` dictionary release to use.
        user_dictionary: optional compiled user dictionary path.
        add_boundary_silence: wrap the sequence in ``<sil>`` tokens. Singing
          data almost always starts and ends with silence, and giving the model
          an explicit token for it keeps the alignment well behaved.
    """

    _lock = threading.Lock()

    def __init__(
        self,
        dictionary_version: str = "v0.15.0",
        user_dictionary: str | None = None,
        add_boundary_silence: bool = True,
    ):
        self.dictionary_version = dictionary_version
        self.user_dictionary = user_dictionary
        self.add_boundary_silence = add_boundary_silence
        self._engine = None

    @property
    def engine(self):
        if self._engine is None:
            with self._lock:
                if self._engine is None:
                    import jpreprocess

                    self._engine = jpreprocess.jpreprocess(
                        dictionary_version=self.dictionary_version,
                        user_dictionary=self.user_dictionary,
                    )
        return self._engine

    def g2p(self, text: str) -> list[str]:
        """Convert Japanese text into IPA phonemes."""
        raw = self.engine.g2p(text)
        phonemes = raw.split() if isinstance(raw, str) else list(raw)
        ipa = openjtalk_to_ipa(phonemes)
        if self.add_boundary_silence:
            if not ipa or ipa[0] != SIL:
                ipa.insert(0, SIL)
            if ipa[-1] != SIL:
                ipa.append(SIL)
        return ipa

    def __call__(self, text: str) -> list[str]:
        return self.g2p(text)
