# /// script
# requires-python = ">=3.10"
# dependencies = ["numpy>=1.26", "scipy>=1.11", "soundfile>=0.12"]
# ///
"""What slapping does to a Physical Bass note, beside what it does to recorded basses.

The recordings are single notes of the IDMT-SMT-Bass dataset (Fraunhofer IDMT, CC BY-NC-ND
4.0): three electric basses on their neck pickup, every note played finger-style, with the
thumb, and popped. No two basses, pickups or recording chains sound alike, so a rendered
note is not held against a recorded one. What is compared is what the technique *does*: each
measure of a thumbed or popped note less the same measure of the same note plucked, on the
same instrument. The model has to change a plucked note the way a player's hand changes one.

    uv run tools/eval/bass_slap.py fetch target/slap-reference
    cargo build --release -p auris-synth --example physical_fit_render
    uv run tools/eval/bass_slap.py measure target/slap-reference \
        target/release/examples/physical_fit_render.exe target/slap-reference/measure.json

The model is voiced toward the recorded basses first (`VOICING` and the hardness of each
technique below), because a difference in decibels means little between a bright note and a
dull one: these basses' plucked notes are darker than the default patch's.

`fetch` takes the 108 notes it measures out of the 1.6 GB archive by HTTP range requests,
about 40 MB. Nothing it downloads is committed. The model's four measured numbers were set
on the open strings and the fifth and twelfth frets, which is what both commands use unless
told otherwise; `--fret 3 --fret 8` reads notes that had no part in that.
"""

import argparse
import http.client
import io
import json
import urllib.request
import zipfile
from itertools import pairwise
from pathlib import Path

import numpy as np
import soundfile as sf

ARCHIVE = "https://zenodo.org/api/records/7188892/files/IDMT-SMT-BASS.zip/content"
RATE = 44_100
STYLES = {"FS": "pluck", "ST": "thumb", "SP": "pop"}
STRINGS = {1: 28, 2: 33, 3: 38, 4: 43}
FRETS = (0, 5, 12)
BASSES = (1, 2, 3)
#: Octave bands, in hertz; the last is everything above.
BANDS = (90, 180, 360, 720, 1440, 2880, 5760)
#: What every rendered note is played with: a pickup as dull as the recorded basses'.
VOICING = {"tone": 1500.0}
#: The technique parameter's value, where the hand is along the open string, and how hard
#: what meets the string is: a fingertip, and the bone of the thumb.
TECHNIQUES = {"pluck": {"hardness": 0.1},
              "thumb": {"technique": 2.0, "position": 0.32, "hardness": 0.9},
              "pop": {"technique": 3.0, "position": 0.32, "hardness": 0.9}}
#: The measures that are one number each, as the table prints them.
SCALARS = ("level_db", "fall_db", "attack_over_held_db", "crest_db", "fundamental_db")


def note_name(bass, style, string, fret):
    return f"BS_{bass}_EQ_1_{style}_NO_{string}_{fret}.wav"


class Remote(io.RawIOBase):
    """A file at a URL, read a range at a time: enough for a zip's directory and members.

    A zip is read in small pieces, each of which would be a request of its own. `expect`
    says how much is about to be read from where, and that much is fetched at once.
    """

    def __init__(self, url):
        super().__init__()
        self.url, self.size = self.locate(url)
        self.position = 0
        self.window = (0, b"")
        self.ahead = 1 << 16

    def locate(self, url):
        """Where `url` ends up after redirection, and how long the file there is."""
        with urllib.request.urlopen(urllib.request.Request(url, method="HEAD"), timeout=60) as head:
            return head.url, int(head.headers["Content-Length"])

    def fetch_range(self, start, end):
        """Bytes `start` to `end`, inclusive. A connection that drops is picked up where it
        stopped, for as long as each try brings something."""
        data = b""
        while len(data) < end + 1 - start:
            headers = {"Range": f"bytes={start + len(data)}-{end}"}
            request = urllib.request.Request(self.url, headers=headers)
            try:
                with urllib.request.urlopen(request, timeout=120) as response:
                    if response.status != 206:
                        raise RuntimeError("the archive's host does not serve ranges")
                    more = response.read()
            except http.client.IncompleteRead as cut:
                more = cut.partial
            if not more:
                raise RuntimeError("the archive's host sent nothing")
            data += more
        return data

    def expect(self, start, length):
        self.window = (start, self.fetch_range(start, min(start + length, self.size) - 1))

    def seekable(self):
        return True

    def readable(self):
        return True

    def tell(self):
        return self.position

    def seek(self, offset, whence=0):
        self.position = (offset, self.position + offset, self.size + offset)[whence]
        return self.position

    def readinto(self, buffer):
        if self.position >= self.size or not len(buffer):
            return 0
        start, held = self.window
        if not start <= self.position < start + len(held):
            self.expect(self.position, max(len(buffer), self.ahead))
            start, held = self.window
        data = held[self.position - start:self.position - start + len(buffer)]
        buffer[:len(data)] = data
        self.position += len(data)
        return len(data)


def members(remote, names):
    """The contents of the members `names` of the zip at `remote`, in order."""
    archive = zipfile.ZipFile(remote)
    for name in names:
        info = archive.getinfo(name)
        # A member's own header, its name and extra field, and its data.
        remote.expect(info.header_offset, 30 + len(info.filename) + 1024 + info.compress_size)
        yield archive.read(name)


def fetch(directory, frets=FRETS):
    notes = directory / "notes"
    notes.mkdir(parents=True, exist_ok=True)
    wanted = [f"PS/{style}/{note_name(bass, style, string, fret)}"
              for bass in BASSES for style in STYLES for string in STRINGS for fret in frets]
    missing = [name for name in wanted if not (notes / Path(name).name).exists()]
    if missing:
        for name, data in zip(missing, members(Remote(ARCHIVE), missing)):
            (notes / Path(name).name).write_bytes(data)
    print(f"{len(wanted)} notes in {notes}, {len(missing)} fetched")


def onset(samples):
    """Index of the first sample a tenth as large as the largest."""
    return int(np.argmax(np.abs(samples) >= 0.1 * np.max(np.abs(samples))))


def spectrum(samples):
    """Power spectrum of `samples` under a Hann window, and its frequencies."""
    windowed = samples * np.hanning(len(samples))
    return np.abs(np.fft.rfft(windowed)) ** 2, np.fft.rfftfreq(len(samples), 1 / RATE)


def db(ratio):
    """A ratio of amplitudes, in decibels."""
    return float(20 * np.log10(max(ratio, 1e-10)))


def rms(samples):
    return float(np.sqrt(np.mean(samples ** 2)))


def features(samples, hz):
    """What is read of one note: `samples` at `RATE`, sounding at `hz`.

    The note is taken from its onset. Its *attack* is the first 40 ms, the blow and what the
    frets make of it; what it leaves is *held*, from a tenth of a second to four tenths.
    """
    samples = np.asarray(samples, dtype=np.float64)
    start = onset(samples)
    samples = samples[start:start + int(0.6 * RATE)]
    if len(samples) < int(0.55 * RATE):
        raise ValueError("note shorter than the stretch that is measured")
    attack, held = samples[:int(0.04 * RATE)], samples[int(0.1 * RATE):int(0.4 * RATE)]
    result = {}
    edges = (0, *BANDS, RATE)
    for name, stretch in (("attack", attack), ("held", held)):
        power, hertz = spectrum(stretch)
        result[f"{name}_bands_db"] = [
            db(np.sqrt(power[(hertz >= low) & (hertz < high)].sum() / power.sum()))
            for low, high in pairwise(edges)]
    power, hertz = spectrum(held)
    near = (hertz > hz * 0.8) & (hertz < hz * 1.25)
    result["fundamental_db"] = db(np.sqrt(power[near].sum() / power.sum()))
    # How much louder the blow is than the note it leaves, and how far the peak stands over it.
    result["attack_over_held_db"] = db(rms(attack) / rms(held))
    result["crest_db"] = db(np.max(np.abs(samples)) / rms(samples[:int(0.4 * RATE)]))
    result["level_db"] = db(rms(samples[:int(0.4 * RATE)]))
    # How far the note has fallen from its first tenth of a second to its fifth.
    result["fall_db"] = db(rms(samples[int(0.05 * RATE):int(0.15 * RATE)])
                           / rms(samples[int(0.45 * RATE):int(0.55 * RATE)]))
    return result


def difference(slapped, plucked):
    """Each measure of a slapped note less the same measure of the same note plucked."""
    return {key: (np.subtract(value, plucked[key]).tolist() if isinstance(value, list)
                  else value - plucked[key]) for key, value in slapped.items()}


def gather(rows):
    """Mean and spread of each measure over `rows`."""
    result = {}
    for key in rows[0]:
        values = np.asarray([row[key] for row in rows], dtype=np.float64)
        result[key] = {"mean": np.round(values.mean(axis=0), 2).tolist(),
                       "sd": np.round(values.std(axis=0), 2).tolist()}
    return result


def hertz_of(pitch):
    return 440.0 * 2 ** ((pitch - 69) / 12)


def recorded(directory, frets=FRETS):
    """What is read of every recorded note, by how it was played."""
    rows = {name: [] for name in STYLES.values()}
    for bass in BASSES:
        for string, open_pitch in STRINGS.items():
            for fret in frets:
                for style, name in STYLES.items():
                    audio, rate = sf.read(directory / "notes" / note_name(bass, style, string, fret),
                                          always_2d=True)
                    if rate != RATE:
                        raise ValueError(f"expected {RATE} Hz recordings")
                    rows[name].append(features(audio.mean(axis=1), hertz_of(open_pitch + fret)))
    return rows


def rendered(worker, velocities, extra, dials=None, frets=FRETS):
    """The same of the model's notes, at each of `velocities`."""
    from fit_worker import Renderer
    renderer = Renderer(worker)
    rows = {name: [] for name in TECHNIQUES}
    try:
        pitches = [open_pitch + fret for open_pitch in STRINGS.values() for fret in frets]
        for velocity in velocities:
            notes = [{"pitch": pitch, "velocity": velocity} for pitch in pitches]
            for name, params in TECHNIQUES.items():
                audio = renderer.render("bass", notes, {**VOICING, **params, **extra}, 1.0, 0.9,
                                        rate=RATE, dials=dials)
                rows[name] += [features(note, hertz_of(pitch)) for note, pitch in zip(audio, pitches)]
    finally:
        renderer.close()
    return rows


def summarise(rows):
    """Each technique's notes, and each slapped technique's less the same notes plucked."""
    result = {name: gather(notes) for name, notes in rows.items()}
    for name in ("thumb", "pop"):
        result[f"{name}_less_pluck"] = gather(
            [difference(slapped, plucked) for slapped, plucked in zip(rows[name], rows["pluck"])])
    return result


def table(report):
    """The report as lines: the scalar measures, then the bands from 180 Hz to 5.76 kHz."""
    head = "level   fall  a/held  crest   fund | attack bands             | held bands"
    lines = [f"{'':>27} |  {head}"]
    for name in report["recorded"]:
        for source in ("recorded", "rendered"):
            part = report[source][name]
            scalars = " ".join(f"{part[key]['mean']:>6.1f}" for key in SCALARS)
            bands = [" ".join(f"{value:>4.0f}" for value in part[f"{stretch}_bands_db"]["mean"][2:7])
                     for stretch in ("attack", "held")]
            lines.append(f"{name:>17} {source:>9} | {scalars} | {bands[0]} | {bands[1]}")
    return lines


def measure(directory, worker, out, velocities, extra, dials, frets=FRETS):
    report = {"velocities": velocities, "voicing": {**VOICING, **extra}, "techniques": TECHNIQUES,
              "dials": dials, "frets": list(frets), "bands_hz": BANDS,
              "recorded": summarise(recorded(directory, frets)),
              "rendered": summarise(rendered(worker, velocities, extra, dials, frets))}
    out.write_text(json.dumps(report, indent=1), encoding="utf-8")
    print("\n".join(table(report)))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    fetching = commands.add_parser("fetch")
    measuring = commands.add_parser("measure")
    for command in (fetching, measuring):
        command.add_argument("directory", type=Path)
        command.add_argument("--fret", type=int, action="append", choices=range(13),
                             metavar="0-12", help="a fret to read on every string; 0, 5 and 12 "
                             "unless given")
    measuring.add_argument("worker", type=Path)
    measuring.add_argument("out", type=Path)
    measuring.add_argument("--velocity", type=float, action="append",
                           help="a velocity the model is played at; 0.8 unless given")
    measuring.add_argument("--param", action="append", default=[], metavar="KEY=VALUE",
                           help="a Physical Bass parameter set for every technique")
    measuring.add_argument("--dial", action="append", default=[], metavar="INDEX=VALUE",
                           help="a voicing dial; needs a worker built with physical-calibration")
    args = parser.parse_args()
    frets = tuple(args.fret or FRETS)
    if args.command == "fetch":
        fetch(args.directory, frets)
    else:
        extra = {key: float(value) for key, value in (item.split("=") for item in args.param)}
        dials = {int(index): float(value)
                 for index, value in (item.split("=") for item in args.dial)}
        measure(args.directory, args.worker, args.out, args.velocity or [0.8], extra, dials,
                frets)


if __name__ == "__main__":
    main()
