# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""Fetch and prepare a fixed University of Iowa Bb clarinet cohort.

The source recordings are the pre-2012, mono 44.1 kHz MIS captures.  The
cohort deliberately uses three chromatic one-octave files per dynamic, which
keeps the download bounded while covering the low, middle and high registers.
"""

import argparse
import hashlib
import json
from html.parser import HTMLParser
from itertools import pairwise
from pathlib import Path
from urllib.parse import unquote, urljoin, urlsplit
from urllib.request import urlopen

import numpy as np
import soundfile as sf
from scipy.io import wavfile
from scipy.signal import resample_poly

BASE = "https://theremin.music.uiowa.edu/"
PAGE = BASE + "MISBbclarinet.html"
TERMS_PAGE = BASE + "MIS.html"
RATE = 24_000
# The Iowa filenames use the sounding concert-pitch chromatic sequence.
# Broad-spectrum checks confirm D3B3=50..59, C4B4=60..71, C5B5=72..83.
PITCHES = {"D3B3": list(range(50, 60)), "C4B4": list(range(60, 72)),
           "C5B5": list(range(72, 84))}
CAPTURES = [(d, f"BbClar.{d}.{span}.aiff", pitches)
            for d in ("pp", "mf", "ff") for span, pitches in PITCHES.items()]


class Links(HTMLParser):
    def __init__(self):
        super().__init__()
        self.urls = []

    def handle_starttag(self, tag, attrs):
        if tag == "a":
            self.urls.extend(value for key, value in attrs if key == "href")


def regions(mask):
    edges = np.diff(np.r_[False, mask, False].astype(int))
    return list(zip(np.flatnonzero(edges == 1), np.flatnonzero(edges == -1), strict=True))


def locate(audio, pitches):
    # These captures are ordered chromatic phrases. Segment their broad RMS
    # envelope first; harmonic winner masks fragment clarinet notes when the
    # fundamental is weak. A 5 ms envelope and short-gap merge retain the true
    # first onset while rejecting the silence between adjacent notes.
    hop = round(44_100 * .005)
    count = len(audio) // hop
    level = np.sqrt(np.mean(audio[:count * hop].reshape(count, hop) ** 2, axis=1))
    active = level > level.max() * .02
    raw = regions(active)
    spans = []
    for start, end in raw:
        if spans and (start - spans[-1][1]) * hop / 44_100 < .30:
            spans[-1] = (spans[-1][0], end)
        elif (end - start) * hop / 44_100 >= .50:
            spans.append((start, end))
    if len(spans) != len(pitches):
        raise ValueError(f"expected {len(pitches)} chronological note regions, found {len(spans)}")
    result = [(start * hop / 44_100, end * hop / 44_100) for start, end in spans]
    if any(a[0] >= b[0] for a, b in pairwise(result)):
        raise ValueError("source note regions are not chronological")
    return result


def tuning_cents(clip, pitch):
    expected = 440 * 2 ** ((pitch - 69) / 12)
    sample = clip[int(.25 * len(clip)):int(.8 * len(clip))]
    spectrum = abs(np.fft.rfft(sample * np.hanning(len(sample))))
    frequencies = np.fft.rfftfreq(len(sample), 1 / 44_100)
    fundamental = np.flatnonzero(abs(frequencies - expected) < expected * .045)
    if not len(fundamental) or spectrum[fundamental].max() < spectrum.max() * .005:
        raise ValueError(f"no audible fundamental for MIDI {pitch}")
    estimates, weights = [], []
    for harmonic in (1, 2, 3, 4, 5):
        band = np.flatnonzero(abs(frequencies - expected * harmonic) < expected * harmonic * .045)
        if not len(band):
            continue
        index = band[np.argmax(spectrum[band])]
        estimates.append(frequencies[index] / harmonic)
        weights.append(spectrum[index] / harmonic)
    if not weights:
        raise ValueError(f"no f0 search band for MIDI {pitch}")
    order = np.argsort(estimates)
    actual = np.asarray(estimates)[order][np.searchsorted(np.cumsum(np.asarray(weights)[order]),
                                                          np.sum(weights) * .5)]
    cents = float(1200 * np.log2(actual / expected))
    if abs(cents) > 75:
        raise ValueError(f"unexpected clarinet pitch {pitch}: {cents:.1f} cents")
    return cents


def prepare_note(audio, start, end):
    # Fixed onset lookback and duration make the comparison causal and replayable.
    onset = max(0, round(max(0, start - .005) * 44_100))
    finish = min(len(audio), round((end + .25) * 44_100))
    clip = audio[onset:finish]
    clip = np.pad(clip, (0, max(0, (3 * 44_100) - len(clip))))
    clip = resample_poly(clip, 80, 147)
    clip = clip[:72_000]
    clip = np.pad(clip, (0, max(0, 72_000 - len(clip))))
    return clip.astype("<f4")


def fetch(root, lock=None, offline=False):
    root.mkdir(parents=True, exist_ok=True)
    expected = json.loads(lock.read_text(encoding="utf-8")) if lock is not None else None
    page_path = root / "MISBbclarinet.html"
    if offline:
        raw = page_path.read_bytes()
    else:
        with urlopen(PAGE, timeout=60) as stream:
            raw = stream.read(1_000_001)
        if len(raw) > 1_000_000:
            raise ValueError("oversized recording page")
        page_path.write_bytes(raw)
        with urlopen(TERMS_PAGE, timeout=60) as stream:
            terms = stream.read(1_000_001)
        if len(terms) > 1_000_000:
            raise ValueError("oversized terms page")
        (root / "MIS.html").write_bytes(terms)
    parser = Links()
    parser.feed(raw.decode("utf-8", errors="replace"))
    links = {Path(unquote(urlsplit(u).path)).name:
             urljoin(PAGE, u.replace(" ", "%20")) for u in parser.urls}
    notes = []
    for dynamic, name, pitches in CAPTURES:
        url = links.get(name)
        if url is None:
            raise ValueError(f"recording link missing: {name}")
        path = root / name
        if not path.exists():
            if offline:
                raise ValueError(f"missing cached capture: {name}")
            with urlopen(url, timeout=90) as stream:
                data = stream.read(40_000_001)
            if len(data) > 40_000_000 or data[:4] != b"FORM":
                raise ValueError(f"invalid or oversized AIFF: {url}")
            path.write_bytes(data)
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if expected is not None:
            hashes = {note["capture_sha256"] for note in expected["notes"] if note["source"] == name}
            if hashes != {digest}:
                raise ValueError(f"capture differs from frozen source: {name}")
        audio, rate = sf.read(path, always_2d=False)
        if rate != 44_100 or audio.ndim != 1:
            raise ValueError(f"unexpected recording format: {name}")
        spans = locate(audio, pitches)
        for pitch, (start, end) in zip(pitches, spans, strict=True):
            note_id = f"{dynamic}-{name.split('.')[2]}-{pitch}"
            note_path = root / "notes" / f"{note_id}.wav"
            note_path.parent.mkdir(exist_ok=True)
            wavfile.write(note_path, RATE, prepare_note(audio, start, end))
            _, prepared_rate = sf.read(note_path)
            if prepared_rate != RATE:
                raise ValueError(f"prepared rate mismatch: {note_path}")
            cents = tuning_cents(audio[round(start * 44_100):round(end * 44_100)], pitch)
            notes.append({"id": note_id, "model": "clarinet", "dynamic": dynamic, "pitch": pitch,
                          "split": "train" if pitch % 2 == 0 else "validation",
                          "tuning_cents": cents,
                          "source_start_seconds": start, "source_end_seconds": end,
                          "hold_seconds": max(0.1, end - start),
                          "source": name, "url": url,
                          "sha256": hashlib.sha256(note_path.read_bytes()).hexdigest(),
                          "capture_sha256": digest, "path": note_path.relative_to(root).as_posix()})
    manifest = {"source": PAGE, "license": "University of Iowa MIS: may be downloaded and used for any projects, without restrictions.",
                "recording": "Buffet R13 Bb clarinet, 1998, anechoic chamber, Neumann KM84, mono 16-bit 44.1 kHz",
                "terms_source": TERMS_PAGE,
                "preparation": "chronological 5-ms RMS regions at 2% of capture peak; 5-ms onset preroll; region duration as approximate hold; region end + 250 ms then zero pad to 3 s; polyphase 80:147 resampling",
                "rate": RATE, "seconds": 3.0, "hold": 2.9,
                "notes": notes}
    if expected is not None and manifest != expected:
        raise ValueError("prepared cohort differs from frozen manifest")
    (root / "notes.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--lock", type=Path, help="verify source and prepared hashes against a frozen manifest")
    parser.add_argument("--offline", action="store_true", help="prepare using previously downloaded captures and recording page")
    args = parser.parse_args()
    fetch(args.directory, args.lock, args.offline)
