# /// script
# requires-python = ">=3.10"
# dependencies = ["mido==1.3.3"]
# ///
"""What the accompaniment writers write, counted, and counted against a score corpus.

    cargo run --release -p auris-compose --example dump_parts > takes.jsonl
    uv run tools/eval/accompaniment_parts.py corpus takes.jsonl corpus/
    uv run tools/eval/accompaniment_parts.py intrinsic takes.jsonl
    uv run tools/eval/accompaniment_parts.py kit corpus/
    cargo run --release -p auris-compose --example dump_parts -- --sweep > sweep.jsonl
    uv run tools/eval/accompaniment_parts.py sweep sweep.jsonl

`dump_parts` writes sixteen-bar takes of the bass, the chords, the pad, the arpeggio and the kit
as JSON lines: six charts, six idioms, three densities and a run of seeds for the pitched parts,
and eight grooves at three densities and three intensities for the kit. Run it on the commit
before a change and on the one after, and read the two reports side by side.

`corpus` reads the bass tracks (General MIDI programs 32 to 39) and the drum tracks of every
4/4 MIDI file in a folder, quantises them to sixteenths, cuts them into sixteen-bar windows
with a note in at least fourteen bars, and prints the same measures for the windows and for the
takes: how often a bar's rhythm is the bar before it, what the intervals and the onset spacings
are, how much of the time a bass sounds, how many bars a kit fills and which drums it strikes.
A drum window counts only if it has a kick and a snare in every bar and two strokes of a
timekeeper: a tambourine part is not a kit playing time.
A fill bar is one with three snare and tom strokes more than the window's commonest bar
has.

`intrinsic` needs no corpus. It reads the pitched takes against the chords they were written
over, which the dump carries: which chord tone a bass note is, which figure a comp plays and
how far its top voice moves at a change, how an arpeggio crosses from one chord to the next.
This is all there is for the chords, the pad and the arpeggio, because no score in the corpus
carries one of them apart from the tune it accompanies.

`kit` prints where a corpus's kits put a crash, an open hat and a tom, and what their fill
bars look like, which is what the kit's turns were written from.

`sweep` reads the dump made with `--sweep` and prints two dials as curves: how many times a
bar the chords strike as their density goes from 0.6 to 1.0, and what a kit's bars do as its
density goes from 0.3 to 0.8. A dial is meant to be a ramp, and a step in one of these is
how a cliff shows.

Three things to know when reading the numbers. A corpus figure is a direction and not a target:
these are scores somebody transcribed, not performances. A window is cut from the top of the
file, so it does not line up with a song's sections, and nothing here can say which bar of a
phrase something belongs in. And a share needs seeds: one draw a take decides a figure, so 24
seeds are 24 draws however many charts they are multiplied by, and a figure meant for two takes
in three once measured nine in ten. Count a share over a thousand.

The corpus stays on the machine that holds it, and so does anything counted from one song.
"""

from __future__ import annotations

import argparse
import itertools
import json
import statistics
import sys
from collections import Counter, defaultdict
from pathlib import Path

import mido

BAR = 3840
STEP = 240

BARS = 16
STEPS = 16

KICK = {35, 36}
SNARE = {37, 38, 40}
CLOSED = {42, 44}
OPEN = {46}
RIDE = {51, 53, 59}
CRASH = {49, 52, 55, 57}
TOMS = {41, 43, 45, 47, 48, 50}


def mean(values):
    values = list(values)
    return sum(values) / len(values) if values else float("nan")


# ---------------------------------------------------------------- corpus reading


def read_midi(path):
    """Tracks as (is_drum, program, [(step, length_steps, pitch, velocity)]), or None off 4/4."""
    mid = mido.MidiFile(path)
    ppq = mid.ticks_per_beat
    signatures = set()
    out = []
    for track in mid.tracks:
        now = 0
        program = {}
        sounding = {}
        notes = defaultdict(list)
        for message in track:
            now += message.time
            if message.type == "time_signature":
                signatures.add((message.numerator, message.denominator))
            elif message.type == "program_change":
                program[message.channel] = message.program
            elif message.type == "note_on" and message.velocity > 0:
                sounding[(message.channel, message.note)] = (now, message.velocity)
            elif message.type in ("note_off", "note_on"):
                started = sounding.pop((message.channel, message.note), None)
                if started:
                    notes[message.channel].append(
                        (started[0], now - started[0], message.note, started[1])
                    )
        for channel, items in notes.items():
            sixteenth = ppq / 4
            quantised = sorted(
                (
                    round(start / sixteenth),
                    max(1, round(length / sixteenth)),
                    pitch,
                    velocity,
                )
                for start, length, pitch, velocity in items
            )
            out.append((channel == 9, program.get(channel, 0), quantised))
    if signatures != {(4, 4)}:
        return None
    return out


def corpus_tracks(corpus: Path):
    """The tracks of every 4/4 MIDI file in a folder; a file that will not parse is named and skipped."""
    for path in sorted(corpus.glob("*.mid")):
        try:
            tracks = read_midi(path)
        except (OSError, ValueError, EOFError, KeyError, IndexError) as error:
            print(f"skipped {path.name}: {error}", file=sys.stderr)
            continue
        if tracks is not None:
            yield tracks


def windows(notes):
    """Sixteen-bar windows of a note list in which at least fourteen bars carry a note."""
    if not notes:
        return
    last_bar = max(step for step, *_ in notes) // STEPS
    by_bar = defaultdict(list)
    for note in notes:
        by_bar[note[0] // STEPS].append(note)
    bar = 0
    while bar + BARS <= last_bar + 1:
        filled = sum(1 for index in range(bar, bar + BARS) if by_bar.get(index))
        if filled >= 14:
            yield [
                (step - bar * STEPS, length, pitch, velocity)
                for index in range(bar, bar + BARS)
                for step, length, pitch, velocity in by_bar.get(index, [])
            ]
            bar += BARS
        else:
            bar += 4


# ---------------------------------------------------------------- bass


def monophonic(notes):
    """The lowest note at each onset."""
    by_step = {}
    for step, length, pitch, velocity in notes:
        if step not in by_step or pitch < by_step[step][2]:
            by_step[step] = (step, length, pitch, velocity)
    return [by_step[step] for step in sorted(by_step)]


def bass_measures(notes):
    notes = monophonic(notes)
    if len(notes) < 8:
        return None
    measures = {}
    bars = [
        tuple(sorted(step % STEPS for step, *_ in notes if step // STEPS == bar))
        for bar in range(BARS)
    ]
    measures["notes_per_bar"] = len(notes) / BARS
    measures["downbeat_struck"] = mean(1.0 if 0 in bar else 0.0 for bar in bars)
    positions = Counter(step % STEPS for step, *_ in notes)
    total = sum(positions.values())
    measures["on_beat"] = sum(positions[p] for p in (0, 4, 8, 12)) / total
    measures["on_offbeat_eighth"] = sum(positions[p] for p in (2, 6, 10, 14)) / total
    measures["on_odd_sixteenth"] = sum(positions[p] for p in range(1, 16, 2)) / total
    # Time sounding, clipping each note at the next onset.
    sounding = 0
    for index, (step, length, _, _) in enumerate(notes):
        limit = notes[index + 1][0] if index + 1 < len(notes) else BARS * STEPS
        sounding += max(0, min(step + length, limit) - step)
    measures["sounding"] = sounding / (BARS * STEPS)
    gaps = 0
    for index, (step, length, _, _) in enumerate(notes[:-1]):
        if notes[index + 1][0] - (step + length) >= 1:
            gaps += 1
    measures["rest_after_note"] = gaps / max(1, len(notes) - 1)
    same = [bars[index] == bars[index - 1] for index in range(1, BARS)]
    measures["bar_rhythm_same_as_previous"] = mean(map(float, same))
    measures["bar_rhythm_same_as_4_back"] = mean(
        float(bars[index] == bars[index - 4]) for index in range(4, BARS)
    )
    measures["bar_rhythm_same_as_2_back"] = mean(
        float(bars[index] == bars[index - 2]) for index in range(2, BARS)
    )
    measures["distinct_bar_rhythms"] = len(set(bars))
    measures["top_bar_rhythm_share"] = Counter(bars).most_common(1)[0][1] / BARS
    counts = [len(bar) for bar in bars]
    measures["note_count_step_between_bars"] = mean(
        abs(counts[index] - counts[index - 1]) for index in range(1, BARS)
    )
    measures["busiest_over_sparsest_phrase"] = max(
        sum(counts[i : i + 4]) for i in range(0, BARS, 4)
    ) / max(1, min(sum(counts[i : i + 4]) for i in range(0, BARS, 4)))
    intervals = [abs(b[2] - a[2]) for a, b in itertools.pairwise(notes)]
    count = max(1, len(intervals))
    measures["interval_unison"] = sum(1 for i in intervals if i == 0) / count
    measures["interval_step"] = sum(1 for i in intervals if i in (1, 2)) / count
    measures["interval_third"] = sum(1 for i in intervals if i in (3, 4)) / count
    measures["interval_fourth_fifth"] = sum(1 for i in intervals if i in (5, 7)) / count
    measures["interval_octave"] = sum(1 for i in intervals if i == 12) / count
    measures["interval_other"] = (
        sum(1 for i in intervals if i in (6, 8, 9, 10, 11) or i > 12) / count
    )
    pitches = [pitch for _, _, pitch, _ in notes]
    measures["range_semitones"] = max(pitches) - min(pitches)
    measures["distinct_pitches"] = len(set(pitches))
    measures["mean_pitch"] = mean(pitches)
    measures["lowest_pitch"] = min(pitches)
    crossing = sum(
        1
        for step, length, _, _ in notes
        if (step + length - 1) // STEPS > step // STEPS and (step + length) % STEPS != 0
    )
    measures["notes_tied_over_bar_line"] = crossing
    # A bar entered by step: the last note before the line a second away from the first after it.
    approached = 0
    lines = 0
    for bar in range(1, BARS):
        before = [n for n in notes if n[0] // STEPS == bar - 1]
        after = [n for n in notes if n[0] // STEPS == bar]
        if not before or not after:
            continue
        lines += 1
        if abs(before[-1][2] - after[0][2]) in (1, 2) and before[-1][0] % STEPS >= 12:
            approached += 1
    measures["bar_lines_approached_by_step"] = approached / max(1, lines)
    spans = Counter()
    for a, b in itertools.pairwise(notes):
        gap = b[0] - a[0]
        spans[gap if gap in (1, 2, 3, 4, 6, 8, 12, 16) else "other"] += 1
    for key in (1, 2, 3, 4, 6, 8, 12, 16, "other"):
        measures[f"ioi_{key}"] = spans[key] / max(1, sum(spans.values()))
    velocities = [velocity for *_, velocity in notes]
    measures["velocity_spread"] = statistics.pstdev(velocities) / max(
        1e-9, mean(velocities)
    )
    return measures


# ---------------------------------------------------------------- drums


def usual_strokes(busy):
    """How many snare and tom strokes a window's ordinary bar has: the commonest count.

    Not the median. A kit that ghosts a stroke in every second bar has a median half a stroke
    above its plain bar, and a fill of three strokes over the backbeat then counted in the bars
    without a ghost and not in the bars with one: the writers' own kits measured 0.63 fill bars
    in sixteen that way and 1.18 this way, while the corpus's measured 1.53 and 1.50.
    """
    return Counter(busy).most_common(1)[0][0]


def drum_measures(notes):
    if len(notes) < 32:
        return None
    measures = {}

    def pattern(pitches, bar):
        return tuple(
            sorted(
                {
                    step % STEPS
                    for step, _, pitch, _ in notes
                    if pitch in pitches and step // STEPS == bar
                }
            )
        )

    groups = {
        "kick": KICK,
        "snare": SNARE,
        "timekeeper": CLOSED | OPEN | RIDE,
    }
    for name, pitches in groups.items():
        bars = [pattern(pitches, bar) for bar in range(BARS)]
        measures[f"{name}_hits_per_bar"] = mean(len(bar) for bar in bars)
        measures[f"{name}_same_as_previous"] = mean(
            float(bars[i] == bars[i - 1]) for i in range(1, BARS)
        )
        measures[f"{name}_distinct_bars"] = len(set(bars))
        measures[f"{name}_two_bar_pattern"] = mean(
            float(bars[i] == bars[i - 2] and bars[i] != bars[i - 1])
            for i in range(2, BARS)
        )
    everything = [pattern(set(range(128)), bar) for bar in range(BARS)]
    whole = [
        tuple(
            sorted(
                {
                    (step % STEPS, pitch)
                    for step, _, pitch, _ in notes
                    if step // STEPS == bar
                }
            )
        )
        for bar in range(BARS)
    ]
    measures["whole_bar_same_as_previous"] = mean(
        float(whole[i] == whole[i - 1]) for i in range(1, BARS)
    )
    measures["whole_bar_distinct"] = len(set(whole))
    measures["top_whole_bar_share"] = Counter(whole).most_common(1)[0][1] / BARS
    used = {pitch for _, _, pitch, _ in notes}
    measures["uses_crash"] = float(bool(used & CRASH))
    measures["uses_open_hat"] = float(bool(used & OPEN))
    measures["uses_ride"] = float(bool(used & RIDE))
    measures["uses_toms"] = float(bool(used & TOMS))
    measures["voices_used"] = len(used)
    crash_bars = [
        bar
        for bar in range(BARS)
        if any(
            p in CRASH and s // STEPS == bar and s % STEPS == 0 for s, _, p, _ in notes
        )
    ]
    measures["bars_with_downbeat_crash"] = len(crash_bars) / BARS
    # A fill: a bar whose snare-and-tom count stands well above the window's usual bar.
    busy = [
        sum(
            1 for s, _, p, _ in notes if (p in SNARE or p in TOMS) and s // STEPS == bar
        )
        for bar in range(BARS)
    ]
    usual = usual_strokes(busy)
    fills = [bar for bar in range(BARS) if busy[bar] >= usual + 3]
    measures["fill_bars"] = len(fills)
    measures["fill_on_fourth_bar"] = (
        mean(float(bar % 4 == 3) for bar in fills) if fills else float("nan")
    )
    measures["fill_on_eighth_bar"] = (
        mean(float(bar % 8 == 7) for bar in fills) if fills else float("nan")
    )
    measures["has_fill_before_bar_16"] = float(any(bar < 15 for bar in fills))
    kicks = Counter(step % STEPS for step, _, pitch, _ in notes if pitch in KICK)
    total = max(1, sum(kicks.values()))
    measures["kick_on_odd_sixteenth"] = sum(kicks[p] for p in range(1, 16, 2)) / total
    measures["kick_on_offbeat_eighth"] = sum(kicks[p] for p in (2, 6, 10, 14)) / total
    # How even the timekeeper is: the share of bars whose pattern is a plain run of one value.
    plain = {
        tuple(range(0, 16, 4)),
        tuple(range(0, 16, 2)),
        tuple(range(16)),
        tuple(range(2, 16, 4)),
    }
    keeper = [pattern(groups["timekeeper"], bar) for bar in range(BARS)]
    played = [bar for bar in keeper if bar]
    measures["timekeeper_plain_run"] = (
        mean(float(bar in plain) for bar in played) if played else float("nan")
    )
    for name, pitches in (
        ("kick", KICK),
        ("snare", SNARE),
        ("timekeeper", CLOSED | OPEN | RIDE),
    ):
        velocities = [velocity for _, _, pitch, velocity in notes if pitch in pitches]
        if len(velocities) > 1:
            measures[f"{name}_velocity_spread"] = statistics.pstdev(velocities) / max(
                1e-9, mean(velocities)
            )
    del everything
    return measures


# ---------------------------------------------------------------- reporting


def summarise(rows):
    keys = sorted({key for row in rows for key in row})
    out = {}
    for key in keys:
        values = [row[key] for row in rows if key in row and row[key] == row[key]]
        if values:
            out[key] = (
                mean(values),
                statistics.pstdev(values) if len(values) > 1 else 0.0,
            )
    return out


def table(title, columns):
    print(f"\n## {title}")
    names = list(columns)
    keys = sorted({key for column in columns.values() for key in column})
    print("| measure | " + " | ".join(names) + " |")
    print("|---|" + "---|" * len(names))
    for key in keys:
        cells = []
        for name in names:
            value = columns[name].get(key)
            cells.append(f"{value[0]:.2f} +-{value[1]:.2f}" if value else "-")
        print(f"| {key} | " + " | ".join(cells) + " |")


def corpus_report(takes_path: Path, corpus: Path) -> None:
    """Prints the bass and kit measures of a corpus beside those of the takes."""
    generated_bass = defaultdict(list)
    generated_drums = defaultdict(list)
    with takes_path.open(encoding="utf-8") as lines:
        for line in lines:
            take = json.loads(line)
            if take["kind"] != "take":
                continue
            take["density"] = round(take["density"], 2)
            take["intensity"] = round(take["intensity"], 2)
            notes = [
                (start // 240, max(1, round(length / 240)), pitch, velocity)
                for start, length, pitch, velocity, _ in take["notes"]
            ]
            if take["preset"] == "bass":
                measured = bass_measures(notes)
                if measured:
                    generated_bass[("all",)].append(measured)
                    generated_bass[(take["style"],)].append(measured)
                    if take["style"] == "none":
                        generated_bass[("none", take["density"])].append(measured)
            elif take["preset"] == "drums":
                measured = drum_measures(notes)
                if measured:
                    generated_drums[("all",)].append(measured)
                    if take["density"] == 0.5:
                        generated_drums[("d0.5",)].append(measured)
                        generated_drums[(take["groove"],)].append(measured)
                        generated_drums[("d0.5", take["intensity"])].append(measured)
                    else:
                        generated_drums[("d", take["density"])].append(measured)

    corpus_bass, corpus_drums = [], []
    bass_files = drum_files = 0
    for tracks in corpus_tracks(corpus):
        bass_seen = drums_seen = False
        for is_drum, program, notes in tracks:
            if is_drum:
                for window in windows(notes):
                    measured = drum_measures(window)
                    # A kit playing time: a tambourine part or a lone cymbal roll is not one.
                    if (
                        measured
                        and measured["kick_hits_per_bar"] >= 1
                        and measured["snare_hits_per_bar"] >= 1
                        and measured["timekeeper_hits_per_bar"] >= 2
                    ):
                        corpus_drums.append(measured)
                        drums_seen = True
            elif 32 <= program <= 39:
                for window in windows(notes):
                    measured = bass_measures(window)
                    if measured:
                        corpus_bass.append(measured)
                        bass_seen = True
        bass_files += bass_seen
        drum_files += drums_seen
    print(
        f"corpus: {len(corpus_bass)} bass windows from {bass_files} files, {len(corpus_drums)} drum windows from {drum_files} files"
    )
    print(
        f"generated: {len(generated_bass[('all',)])} bass takes, {len(generated_drums[('all',)])} drum takes"
    )

    table(
        "Bass",
        {
            "corpus": summarise(corpus_bass),
            "gen all": summarise(generated_bass[("all",)]),
            "none d0.3": summarise(generated_bass[("none", 0.3)]),
            "none d0.5": summarise(generated_bass[("none", 0.5)]),
            "none d0.8": summarise(generated_bass[("none", 0.8)]),
            "pop-band": summarise(generated_bass[("pop-band",)]),
            "rock": summarise(generated_bass[("rock",)]),
        },
    )
    table(
        "Drums",
        {
            "corpus": summarise(corpus_drums),
            "gen d0.5": summarise(generated_drums[("d0.5",)]),
            "gen d0.3": summarise(generated_drums[("d", 0.3)]),
            "gen d0.8": summarise(generated_drums[("d", 0.8)]),
            "d0.5 i0.45": summarise(generated_drums[("d0.5", 0.45)]),
            "d0.5 i0.7": summarise(generated_drums[("d0.5", 0.7)]),
            "d0.5 i0.9": summarise(generated_drums[("d0.5", 0.9)]),
            "basic-rock d0.5": summarise(generated_drums[("basic-rock",)]),
            "eight-beat d0.5": summarise(generated_drums[("eight-beat",)]),
            "sixteen-beat d0.5": summarise(generated_drums[("sixteen-beat",)]),
        },
    )


def chord_at(chords, tick):
    for start, length, root, bass, classes in chords:
        if start <= tick < start + length:
            return start, length, root, bass, classes
    return None


def show(title, groups, order=None):
    print(f"\n## {title}")
    names = list(groups)
    keys = order or sorted(
        {key for rows in groups.values() for row in rows for key in row}
    )
    print("| measure | " + " | ".join(str(name) for name in names) + " |")
    print("|---|" + "---|" * len(names))
    for key in keys:
        cells = []
        for name in names:
            values = [
                row[key] for row in groups[name] if key in row and row[key] == row[key]
            ]
            cells.append(f"{mean(values):.2f}" if values else "-")
        print(f"| {key} | " + " | ".join(cells) + " |")


FIGURES = {
    (0,): "held",
    (0, 4, 8, 12): "beats",
    (0, 2, 6, 10, 14): "offbeats",
    (0, 6): "charleston",
    (0, 3, 6, 8, 11, 14): "cross",
}


def bass_row(take, chords):
    notes = sorted(take["notes"])
    row = {}
    roles = Counter()
    for start, _, pitch, _, _ in notes:
        chord = chord_at(chords, start)
        if not chord:
            continue
        _, _, _root, bass, classes = chord
        pc = pitch % 12
        if pc == bass:
            roles["root"] += 1
        elif len(classes) > 2 and pc == classes[2]:
            roles["fifth"] += 1
        elif len(classes) > 1 and pc == classes[1]:
            roles["third"] += 1
        elif pc in classes:
            roles["other chord tone"] += 1
        else:
            roles["non-chord"] += 1
    total = max(1, sum(roles.values()))
    for name in ("root", "fifth", "third", "other chord tone", "non-chord"):
        row[f"role {name}"] = roles[name] / total
    # The figure of each four-bar phrase, read off its first bar.
    kinds = []
    for phrase in range(4):
        bar = phrase * 4
        inside = [n for n in notes if n[0] // BAR == bar]
        steps = tuple(sorted((n[0] % BAR) // STEP for n in inside))
        pitches = [n[2] for n in inside]
        if len(inside) == 1:
            kind = "held root"
        elif steps == (0, 4, 8, 12) and len(set(pitches)) >= 3:
            kind = "walk"
        elif len(set(pitches)) == 1:
            kind = "pulse"
        elif any(abs(a - b) == 12 for a, b in itertools.pairwise(pitches)):
            kind = "octave"
        else:
            kind = "root-fifth"
        kinds.append(kind)
    row["distinct figures in 16 bars"] = len(set(kinds))
    row["phrases: held root"] = kinds.count("held root") / 4
    row["phrases: root-fifth"] = kinds.count("root-fifth") / 4
    row["phrases: octave"] = kinds.count("octave") / 4
    row["phrases: walk"] = kinds.count("walk") / 4
    row["phrases: pulse"] = kinds.count("pulse") / 4
    row["one figure throughout"] = float(len(set(kinds)) == 1)
    row["has both held and busy phrase"] = float(
        "held root" in kinds and len(set(kinds)) > 1
    )
    # A fifth below the root or above it: how often the line sits under its own root.
    below = 0
    count = 0
    for start, _, pitch, _, _ in notes:
        chord = chord_at(chords, start)
        if not chord:
            continue
        first = next((n for n in notes if n[0] == chord[0]), None)
        if first and pitch % 12 != first[2] % 12:
            count += 1
            below += pitch < first[2]
    row["non-root note below the root"] = below / count if count else float("nan")
    return row


def strikes_of(notes):
    by = defaultdict(list)
    for start, length, pitch, velocity, _ in notes:
        by[start].append((pitch, length, velocity))
    return dict(sorted(by.items()))


def chords_row(take, chords):
    notes = sorted(take["notes"])
    strikes = strikes_of(notes)
    row = {}
    bars = [
        tuple(
            sorted({(start % BAR) // STEP for start in strikes if start // BAR == bar})
        )
        for bar in range(BARS)
    ]
    kinds = Counter()
    for bar, steps in enumerate(bars):
        fixed = steps
        if fixed not in FIGURES:
            # A pushed change: the strike on the line moved half a beat early, into the bar before.
            repaired = tuple(sorted(set(steps) - {14} | {0}))
            if repaired in FIGURES:
                fixed = repaired
            elif tuple(sorted(set(steps) | {0})) in FIGURES:
                fixed = tuple(sorted(set(steps) | {0}))
        kinds[FIGURES.get(fixed, "rolled")] += 1
    for name in ("held", "beats", "offbeats", "charleston", "cross", "rolled"):
        row[f"bars: {name}"] = kinds[name] / BARS
    row["strikes per bar"] = len(strikes) / BARS
    row["bar rhythm same as previous"] = mean(
        float(bars[i] == bars[i - 1]) for i in range(1, BARS)
    )
    row["distinct bar rhythms"] = len(set(bars))
    row["most common figure share"] = kinds.most_common(1)[0][1] / BARS
    row["whole take one held chord per bar"] = float(kinds["held"] == BARS)
    sizes = [len(pitches) for pitches in strikes.values()]
    row["notes per strike"] = mean(sizes)
    tops = []
    lows = []
    spans = []
    muddy = 0
    for start, pitches in strikes.items():
        ordered = sorted(p for p, _, _ in pitches)
        tops.append(ordered[-1])
        lows.append(ordered[0])
        spans.append(ordered[-1] - ordered[0])
        if any(b - a <= 2 and a < 55 for a, b in itertools.pairwise(ordered)):
            muddy += 1
    row["top note mean"] = mean(tops)
    row["top note range"] = max(tops) - min(tops)
    row["lowest note mean"] = mean(lows)
    row["lowest note min"] = min(lows)
    row["voicing span"] = mean(spans)
    row["strikes with a second below G3"] = muddy / max(1, len(strikes))
    # The top voice across chord changes only.
    change_tops = []
    for start, _, _, _, _ in chords:
        near = [s for s in strikes if start - 480 <= s <= start]
        if near:
            change_tops.append(max(p for p, _, _ in strikes[max(near)]))
    moves = [abs(b - a) for a, b in itertools.pairwise(change_tops)]
    row["top voice move per change"] = mean(moves) if moves else float("nan")
    row["top voice static share"] = (
        mean(float(m == 0) for m in moves) if moves else float("nan")
    )
    row["top voice leaps > 4"] = (
        mean(float(m > 4) for m in moves) if moves else float("nan")
    )
    velocities = [v for *_, v, _ in notes]
    row["velocity spread"] = statistics.pstdev(velocities) / mean(velocities)
    sounding = 0
    starts = sorted(strikes)
    for index, start in enumerate(starts):
        longest = max(length for _, length, _ in strikes[start])
        limit = starts[index + 1] if index + 1 < len(starts) else BARS * BAR
        sounding += min(start + longest, limit) - start
    row["sounding"] = sounding / (BARS * BAR)
    row["pushed strikes per 16 bars"] = sum(
        1
        for start in strikes
        if (start % BAR) // STEP == 14
        and any(
            start + length > (start // BAR + 1) * BAR for _, length, _ in strikes[start]
        )
    )
    return row


def pad_row(take, chords):
    notes = sorted(take["notes"])
    row = {}
    row["notes in 16 bars"] = len(notes)
    lengths = [length / BAR for _, length, *_ in notes]
    row["longest note (bars)"] = max(lengths)
    row["notes held 4 bars or more"] = mean(float(length >= 4) for length in lengths)
    row["notes held 8 bars or more"] = mean(float(length >= 8) for length in lengths)
    sounding = []
    for start, *_ in chords:
        at = start + 1
        sounding.append(sorted(p for s, l, p, _, _ in notes if s <= at < s + l))
    row["voices sounding"] = mean(len(s) for s in sounding)
    tops = [s[-1] for s in sounding if s]
    moves = [abs(b - a) for a, b in itertools.pairwise(tops)]
    row["top voice move per change"] = mean(moves)
    row["top voice static share"] = mean(float(m == 0) for m in moves)
    row["top note range"] = max(tops) - min(tops)
    struck = Counter(start for start, *_ in notes)
    row["changes with nothing struck"] = mean(
        float(start not in struck) for start, *_ in chords
    )
    row["notes struck per change"] = mean(struck.get(start, 0) for start, *_ in chords)
    row["lowest note min"] = min(p for _, _, p, _, _ in notes)
    velocities = [v for *_, v, _ in notes]
    row["velocity spread"] = statistics.pstdev(velocities) / mean(velocities)
    return row


def arp_row(take, chords):
    notes = sorted(take["notes"])
    row = {}
    gaps = Counter(b[0] - a[0] for a, b in itertools.pairwise(notes))
    row["rate: sixteenths"] = float(gaps.most_common(1)[0][0] == 240)
    row["rate: eighths"] = float(gaps.most_common(1)[0][0] == 480)
    row["rate: quarters"] = float(gaps.most_common(1)[0][0] == 960)
    row["distinct note values"] = len(gaps)
    pitches = [n[2] for n in notes]
    row["range"] = max(pitches) - min(pitches)
    row["highest note"] = max(pitches)
    row["mean pitch"] = mean(pitches)
    inside, across, restarts, roots = [], [], [], []
    shapes = []
    for index, (start, length, root, bass, classes) in enumerate(chords):
        here = [n for n in notes if start <= n[0] < start + length]
        if not here:
            continue
        inside.extend(abs(b[2] - a[2]) for a, b in itertools.pairwise(here))
        roots.append(float(here[0][2] % 12 == root))
        lowest = min(n[2] for n in here)
        restarts.append(
            float(here[0][2] == lowest or here[0][2] == max(n[2] for n in here))
        )
        # The shape as ranks, so the same figure over another chord compares equal.
        ranks = sorted({n[2] for n in here})
        shapes.append(tuple(ranks.index(n[2]) for n in here))
        if index + 1 < len(chords):
            after = [
                n
                for n in notes
                if chords[index + 1][0]
                <= n[0]
                < chords[index + 1][0] + chords[index + 1][1]
            ]
            if after:
                across.append(abs(after[0][2] - here[-1][2]))
    floors = []
    firsts = []
    for start, length, *_ in chords:
        here = [n[2] for n in notes if start <= n[0] < start + length]
        if here:
            floors.append(min(here))
            firsts.append(here[0])
    row["floor moves per change"] = mean(
        abs(b - a) for a, b in itertools.pairwise(floors)
    )
    row["first note moves per change"] = mean(
        abs(b - a) for a, b in itertools.pairwise(firsts)
    )
    row["interval inside a chord"] = mean(inside)
    row["leap across a chord change"] = mean(across)
    row["leaps across a change > 7"] = mean(float(a > 7) for a in across)
    row["chord starts on its root"] = mean(roots)
    row["chord starts at an end of the figure"] = mean(restarts)
    row["distinct shapes in 16 bars"] = len(set(shapes))
    row["most common shape share"] = Counter(shapes).most_common(1)[0][1] / len(shapes)
    velocities = [n[3] for n in notes]
    row["velocity spread"] = statistics.pstdev(velocities) / mean(velocities)
    lengths = [n[1] for n in notes]
    row["legato"] = mean(
        float(b[0] - a[0] == a[1]) for a, b in itertools.pairwise(notes)
    )
    row["non-chord notes"] = mean(
        float(n[2] % 12 not in chord_at(chords, n[0])[4])
        for n in notes
        if chord_at(chords, n[0])
    )
    del lengths
    return row


def arp_lines(takes_path: Path) -> None:
    """Prints how an unstyled arpeggio crosses a bar line: its rate, and the notes it repeats."""
    rows = defaultdict(Counter)
    with takes_path.open(encoding="utf-8") as lines:
        for line in lines:
            take = json.loads(line)
            if (
                take.get("kind") != "take"
                or take["preset"] != "arp"
                or take["style"] != "none"
            ):
                continue
            row = rows[round(take["density"], 2)]
            notes = sorted(take["notes"])
            for a, b in itertools.pairwise(notes):
                where = "inside" if a[0] // BAR == b[0] // BAR else "line"
                row[where] += 1
                row[f"{where}_unison"] += a[2] == b[2]
            row["takes"] += 1
            row["notes"] += len(notes)
            row["bars"] += len({note[0] // BAR for note in notes})
            row["values"] += len({note[1] for note in notes})
    print("\n## arp at the bar line")
    print(
        "| density | notes per bar | note values | repeats across a bar line | repeats inside a bar |"
    )
    print("|---|---|---|---|---|")
    for density, row in sorted(rows.items()):
        print(
            f"| {density} | {row['notes'] / max(1, row['bars']):.2f} "
            f"| {row['values'] / max(1, row['takes']):.2f} "
            f"| {row['line_unison'] / max(1, row['line']):.3f} "
            f"| {row['inside_unison'] / max(1, row['inside']):.3f} |"
        )


def intrinsic_report(takes_path: Path) -> None:
    """Prints what the pitched takes do against the chords they were written over."""
    charts = {}
    rows = defaultdict(lambda: defaultdict(list))
    with takes_path.open(encoding="utf-8") as lines:
        for line in lines:
            item = json.loads(line)
            if item["kind"] == "chart":
                charts[item["chart"]] = item["chords"]
                continue
            preset = item["preset"]
            if preset == "drums" or not item["notes"]:
                continue
            density = round(item["density"], 2)
            chords = charts[item["chart"]]
            row = {
                "bass": bass_row,
                "chords": chords_row,
                "pad": pad_row,
                "arp": arp_row,
            }[preset](item, chords)
            if item["style"] == "none":
                rows[preset][f"none d{density}"].append(row)
            if density == 0.5:
                rows[preset][f"{item['style']} d0.5"].append(row)
    for preset in ("bass", "chords", "pad", "arp"):
        columns = rows[preset]
        ordered = {
            name: columns[name]
            for name in sorted(columns, key=lambda n: (not n.startswith("none"), n))
        }
        show(preset, ordered)
    arp_lines(takes_path)


def kit_vocabulary(corpus: Path) -> None:
    """Prints where a corpus's kits put a crash, an open hat and a tom, and how they fill."""
    crash_step = Counter()
    crash_bar4 = Counter()
    crash_bar8 = Counter()
    crash_per_window = []
    crash_with_kick = []
    crash_after_fill = []
    open_step = Counter()
    open_per_bar = []
    open_bar4 = Counter()
    tom_in_fill = []
    fill_len = Counter()
    fill_start = Counter()
    fill_bar4 = Counter()
    fill_bar8 = Counter()
    fill_voices = Counter()
    fills_per_window = []
    fill_then_crash = []
    keeper_kind = Counter()
    tom_dir = Counter()
    n = 0
    for tracks in corpus_tracks(corpus):
        for is_drum, _, notes in tracks:
            if not is_drum:
                continue
            for window in windows(notes):
                m = drum_measures(window)
                if not (
                    m
                    and m["kick_hits_per_bar"] >= 1
                    and m["snare_hits_per_bar"] >= 1
                    and m["timekeeper_hits_per_bar"] >= 2
                ):
                    continue
                n += 1
                crashes = [(s, p) for s, _, p, _ in window if p in CRASH]
                crash_per_window.append(len({s for s, _ in crashes}))
                kicks = {s for s, _, p, _ in window if p in KICK}
                for s in {s for s, _ in crashes}:
                    crash_step[s % STEPS] += 1
                    if s % STEPS == 0:
                        crash_bar4[(s // STEPS) % 4] += 1
                        crash_bar8[(s // STEPS) % 8] += 1
                    crash_with_kick.append(float(s in kicks))
                opens = {s for s, _, p, _ in window if p in OPEN}
                if opens:
                    open_per_bar.append(len(opens) / BARS)
                for s in opens:
                    open_step[s % STEPS] += 1
                    open_bar4[(s // STEPS) % 4] += 1
                closed = sum(1 for _, _, p, _ in window if p in CLOSED)
                ride = sum(1 for _, _, p, _ in window if p in RIDE)
                keeper_kind["ride" if ride > closed else "hat"] += 1
                # Fill bars as measure.py finds them.
                busy = [
                    sum(
                        1
                        for s, _, p, _ in window
                        if (p in SNARE or p in TOMS) and s // STEPS == bar
                    )
                    for bar in range(BARS)
                ]
                usual = usual_strokes(busy)
                fills = [bar for bar in range(BARS) if busy[bar] >= usual + 3]
                fills_per_window.append(len(fills))
                for bar in fills:
                    fill_bar4[bar % 4] += 1
                    fill_bar8[bar % 8] += 1
                    hits = sorted(
                        (s % STEPS, p)
                        for s, _, p, _ in window
                        if (p in SNARE or p in TOMS) and s // STEPS == bar
                    )
                    usual_steps = Counter()
                    for other in range(BARS):
                        if other in fills:
                            continue
                        for s, _, p, _ in window:
                            if p in SNARE and s // STEPS == other:
                                usual_steps[s % STEPS] += 1
                    common = {
                        s
                        for s, c in usual_steps.items()
                        if c >= (BARS - len(fills)) / 2
                    }
                    extra = [(s, p) for s, p in hits if s not in common or p in TOMS]
                    if extra:
                        first = min(s for s, _ in extra)
                        fill_start[first // 4] += 1
                        fill_len[len(extra)] += 1
                    toms = [p for _, p in hits if p in TOMS]
                    tom_in_fill.append(float(bool(toms)))
                    fill_voices[
                        "snare only"
                        if not toms
                        else (
                            "toms only"
                            if all(p in TOMS for _, p in extra)
                            else "snare and toms"
                        )
                    ] += 1
                    if len(toms) >= 2:
                        ordered = [p for _, p in hits if p in TOMS]
                        falls = sum(1 for a, b in itertools.pairwise(ordered) if b < a)
                        rises = sum(1 for a, b in itertools.pairwise(ordered) if b > a)
                        tom_dir[
                            "falls"
                            if falls > rises
                            else ("rises" if rises > falls else "level")
                        ] += 1
                    if bar + 1 < BARS:
                        fill_then_crash.append(
                            float(any(s == (bar + 1) * STEPS for s, _ in crashes))
                        )
                for s in {s for s, _ in crashes if s % STEPS == 0 and s // STEPS > 0}:
                    crash_after_fill.append(float((s // STEPS - 1) in fills))
    print(f"{n} windows")
    print(
        "crash onsets per window (16 bars):",
        round(mean(crash_per_window), 2),
        "median",
        statistics.median(crash_per_window),
    )
    total = sum(crash_step.values())
    print(
        "crash step in bar:",
        {k: round(v / total, 3) for k, v in sorted(crash_step.items())},
    )
    t4 = sum(crash_bar4.values())
    print(
        "downbeat crash, bar mod 4:",
        {k: round(v / t4, 3) for k, v in sorted(crash_bar4.items())},
    )
    print(
        "downbeat crash, bar mod 8:",
        {k: round(v / t4, 3) for k, v in sorted(crash_bar8.items())},
    )
    print("crash with a kick under it:", round(mean(crash_with_kick), 3))
    print("downbeat crash preceded by a fill bar:", round(mean(crash_after_fill), 3))
    print("fill followed by a downbeat crash:", round(mean(fill_then_crash), 3))
    total = sum(open_step.values())
    print(
        "open-hat windows:",
        len(open_per_bar),
        "onsets per bar in them:",
        round(mean(open_per_bar), 2),
        "median",
        round(statistics.median(open_per_bar), 2),
    )
    print(
        "open hat step in bar:",
        {k: round(v / total, 3) for k, v in sorted(open_step.items())},
    )
    print(
        "open hat bar mod 4:",
        {k: round(v / total, 3) for k, v in sorted(open_bar4.items())},
    )
    print("timekeeper:", dict(keeper_kind))
    print("fills per window:", round(mean(fills_per_window), 2))
    t = sum(fill_bar4.values())
    print("fill bar mod 4:", {k: round(v / t, 3) for k, v in sorted(fill_bar4.items())})
    print("fill bar mod 8:", {k: round(v / t, 3) for k, v in sorted(fill_bar8.items())})
    t = sum(fill_start.values())
    print(
        "fill starts in beat:",
        {k: round(v / t, 3) for k, v in sorted(fill_start.items())},
    )
    t = sum(fill_len.values())
    print(
        "fill extra strokes:", {k: round(v / t, 3) for k, v in sorted(fill_len.items())}
    )
    print("fill voices:", dict(fill_voices))
    print("tom direction in a fill:", dict(tom_dir))


def sweep_report(takes_path: Path) -> None:
    """Prints the chords' strikes a bar and the kit's bars against the density dial."""
    rows = defaultdict(list)
    with takes_path.open(encoding="utf-8") as lines:
        for line in lines:
            take = json.loads(line)
            if take.get("kind") != "take":
                continue
            density = round(take["density"], 2)
            if take["preset"] == "chords":
                strikes = {start for start, *_ in take["notes"]}
                rows[("chords", take["style"], density)].append(
                    {"strikes a bar": len(strikes) / BARS}
                )
            elif take["preset"] == "drums":

                def bars(voice, notes=take["notes"]):
                    # The last bar is left out: it is the fill's, at every setting.
                    return [
                        tuple(
                            sorted(
                                {
                                    (start % BAR) // STEP
                                    for start, _, _, _, name in notes
                                    if name == voice and start // BAR == bar
                                }
                            )
                        )
                        for bar in range(BARS - 1)
                    ]

                kick, snare, hat = bars("kick"), bars("snare"), bars("hat")
                whole = list(zip(kick, snare, hat, strict=True))
                rows[("drums", round(take["intensity"], 2), density)].append(
                    {
                        "kick strokes a bar": mean(map(len, kick)),
                        "different kick bars": len(set(kick)),
                        "snare strokes a bar": mean(map(len, snare)),
                        "different snare bars": len(set(snare)),
                        "hat strokes a bar": mean(map(len, hat)),
                        "different hat bars": len(set(hat)),
                        "bar the same as the one before": mean(
                            float(now == before)
                            for before, now in itertools.pairwise(whole)
                        ),
                        "bars missing a backbeat": mean(
                            float(not {4, 12} <= set(bar)) for bar in snare
                        ),
                    }
                )
    for key in sorted(rows, key=str):
        taken = rows[key]
        print(
            key,
            "  ".join(
                f"{name}={mean(row[name] for row in taken):.2f}" for name in taken[0]
            ),
        )


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    corpus = commands.add_parser(
        "corpus", help="bass and kit takes beside a corpus's own"
    )
    corpus.add_argument(
        "takes", type=Path, help="JSON lines from the dump_parts example"
    )
    corpus.add_argument("folder", type=Path, help="a folder of MIDI files")
    intrinsic = commands.add_parser(
        "intrinsic", help="pitched takes against their own chords"
    )
    intrinsic.add_argument(
        "takes", type=Path, help="JSON lines from the dump_parts example"
    )
    kit = commands.add_parser("kit", help="where a corpus's kits crash, open and fill")
    kit.add_argument("folder", type=Path, help="a folder of MIDI files")
    sweep = commands.add_parser(
        "sweep", help="the chords' and the kit's density dials as curves"
    )
    sweep.add_argument("takes", type=Path, help="JSON lines from dump_parts --sweep")
    args = parser.parse_args()
    if args.command == "corpus":
        corpus_report(args.takes, args.folder)
    elif args.command == "intrinsic":
        intrinsic_report(args.takes)
    elif args.command == "sweep":
        sweep_report(args.takes)
    else:
        kit_vocabulary(args.folder)


if __name__ == "__main__":
    main()
