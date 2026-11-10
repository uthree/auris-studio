from melody_form import clip_notes, contour_type, measure, period, same_cycle

BEAT = 960
BAR = 4 * BEAT


def note(at: int, length: int = 768, pitch: int = 60) -> dict:
    return {"start": at, "length": length, "pitch": pitch, "velocity": 0.8}


def singer(lines: list[tuple[int, list[dict]]]) -> dict:
    """A singer track with one clip per line; each line's notes are relative to its start."""
    clips = [
        {"id": index, "start": start, "length": 4 * BAR, "notes": notes, "muted": False}
        for index, (start, notes) in enumerate(lines)
    ]
    return {
        "id": 1,
        "name": "Vocal",
        "mixer": {"mute": False},
        "kind": {"type": "singer", "clips": clips},
    }


def song(
    tracks: list[dict],
    sections: list[tuple[int, str | None]] = ((0, "Verse"),),
    key: str = "C major",
    chords: list[tuple[int, str | None]] = (),
    lyrics: list[tuple[int, str | None]] = (),
) -> dict:
    return {
        "signatures": {
            "points": [{"tick": 0, "signature": {"numerator": 4, "denominator": 4}}]
        },
        "sections": {
            "points": [{"tick": tick, "label": label} for tick, label in sections]
        },
        "harmony": {
            "keys": {"points": [{"tick": 0, "key": key}]},
            "chords": {
                "points": [{"tick": tick, "chord": chord} for tick, chord in chords]
            },
        },
        "lyrics": {"points": [{"tick": tick, "text": text} for tick, text in lyrics]},
        "tracks": tracks,
    }


PHRASE = [
    note(i * BEAT, pitch=p) for i, p in enumerate([60, 62, 64, 65, 64, 62, 60, 62])
]


def flag_codes(report: dict) -> set[str]:
    return {flag["code"] for flag in report["flags"]}


def test_a_section_of_one_copied_line_is_flagged_and_grows_predictable() -> None:
    lines = [(i * 4 * BAR, PHRASE) for i in range(4)]
    report = measure(song([singer(lines)], sections=[(0, "Verse"), (16 * BAR, None)]))
    assert report["sections"][0]["form"] == ["A", "A", "A", "A"]
    assert "identical_lines" in flag_codes(report)
    bits = [line["information_bits"] for line in report["lines"]]
    assert bits[0] > bits[1] > bits[3]
    assert bits[3] < 1.0


def test_same_rhythm_under_new_pitches_is_a_prime_and_a_new_rhythm_a_new_letter() -> (
    None
):
    answer = [dict(n, pitch=n["pitch"] + 2) for n in PHRASE]
    turn = [note(i * BEAT // 2, length=400, pitch=67) for i in range(8)]
    lines = [(0, PHRASE), (4 * BAR, answer), (8 * BAR, turn), (12 * BAR, PHRASE)]
    report = measure(song([singer(lines)], sections=[(0, "Verse"), (16 * BAR, None)]))
    assert report["sections"][0]["form"] == ["A", "A'", "B", "A"]
    assert "identical_lines" not in flag_codes(report)
    assert report["summary"]["distinct_rhythms"] == 2


def test_a_pickup_entry_and_an_anticipated_barline_are_counted() -> None:
    # Two eighths on beat 4 of bar 1, then a note on 4-and that ties over the next barline.
    line = [note(3 * BEAT, 400, 60), note(3 * BEAT + 480, 960, 62)] + [
        note(BAR + BEAT + i * BEAT, 768, 64) for i in range(3)
    ]
    report = measure(song([singer([(0, line)])]))
    features = report["lines"][0]
    assert features["entry"] == "pickup"
    assert features["entry_beat"] == 4.0
    assert features["syncopated"] == 1
    assert features["anticipations"] == 1
    assert features["positions"] == {"beat": 4, "half": 1}


def test_line_endings_are_named_as_scale_degrees_of_the_key() -> None:
    tonic = [note(0, pitch=67), note(BEAT, pitch=64)]
    subtonic = [note(0, pitch=67), note(BEAT, pitch=62)]
    report = measure(song([singer([(0, tonic), (BAR, subtonic)])], key="E minor"))
    assert [line["final_degree"] for line in report["lines"]] == ["1", "b7"]


def test_a_chorus_looping_the_verse_progression_from_another_chord_is_flagged() -> None:
    assert same_cycle(["i", "i", "VI", "VII"], ["VI", "VII", "i", "i"])
    assert not same_cycle(["i", "i", "VI", "VII"], ["iv", "VI", "VII", "V"])
    assert period(["i", "VI", "VII", "V"] * 3) == ["i", "VI", "VII", "V"]
    verse = [
        (bar * BAR, chord) for bar, chord in enumerate(["i", "i", "VI", "VII"] * 2)
    ]
    chorus = [
        (8 * BAR + bar * BAR, chord)
        for bar, chord in enumerate(["VI", "VII", "i", "i"] * 2)
    ]
    lines = [(0, PHRASE), (8 * BAR, [dict(n, pitch=n["pitch"] + 7) for n in PHRASE])]
    project = song(
        [singer(lines)],
        sections=[(0, "Verse"), (8 * BAR, "Chorus"), (16 * BAR, None)],
        key="E minor",
        chords=verse + chorus,
    )
    assert "chorus_reuses_verse_harmony" in flag_codes(measure(project))


def test_sections_sharing_one_drum_bar_are_flagged() -> None:
    groove = [note(0, 100, 36), note(2 * BEAT, 100, 38)]
    clips = [
        {"start": bar * BAR, "length": BAR, "notes": groove, "muted": False}
        for bar in range(9)
    ]
    drums = {
        "id": 2,
        "name": "Kit",
        "mixer": {"mute": False},
        "kind": {"type": "drum", "clips": clips},
    }
    project = song(
        [singer([(0, PHRASE)]), drums],
        sections=[
            (0, "Intro"),
            (3 * BAR, "Verse"),
            (6 * BAR, "Outro"),
            (9 * BAR, None),
        ],
    )
    report = measure(project)
    shared = [flag for flag in report["flags"] if flag["code"] == "shared_main_groove"]
    assert shared and shared[0]["sections"] == ["Intro", "Verse", "Outro"]


def test_a_looped_drum_clip_is_read_once_for_every_pass() -> None:
    groove = [note(0, 100, 36), note(2 * BEAT, 100, 38)]
    other = [note(0, 100, 36), note(BEAT, 100, 36), note(3 * BEAT, 100, 38)]
    clip = {"length": BAR, "muted": False}
    cut_short = dict(clip, start=6 * BAR, notes=groove, loop_end=5 * BAR // 2)
    drums = {
        "id": 2,
        "name": "Kit",
        "mixer": {"mute": False},
        "kind": {
            "type": "drum",
            "clips": [
                dict(clip, start=0, notes=groove, loop_end=3 * BAR),
                dict(clip, start=3 * BAR, notes=groove),
                dict(clip, start=4 * BAR, notes=other, loop_end=2 * BAR),
                cut_short,
            ],
        },
    }
    project = song(
        [singer([(0, PHRASE)]), drums],
        sections=[
            (0, "Intro"),
            (3 * BAR, "Verse"),
            (6 * BAR, "Outro"),
            (9 * BAR, None),
        ],
    )
    report = measure(project)
    assert "shared_main_groove" not in flag_codes(report)
    hits = {s["section"]: s["main_groove"] for s in report["sections"]}
    assert hits == {"Intro": 2, "Verse": 3, "Outro": 2}
    assert [n["at"] - 6 * BAR for n in clip_notes(cut_short)] == [
        0,
        2 * BEAT,
        BAR,
        BAR + 2 * BEAT,
        2 * BAR,
    ]


def guitar(transforms: list[dict]) -> dict:
    chords = [
        note(bar * BAR, 1200, pitch) for bar in range(8) for pitch in (40, 47, 52)
    ]
    clip = {
        "start": 0,
        "length": 8 * BAR,
        "notes": chords,
        "muted": False,
        "transforms": transforms,
    }
    return {
        "id": 3,
        "name": "Rhythm guitar",
        "mixer": {"mute": False},
        "kind": {"type": "instrument", "clips": [clip]},
    }


def test_strum_spread_is_read_per_chord_and_extremes_are_flagged() -> None:
    tight = measure(
        song(
            [
                singer([(0, PHRASE)]),
                guitar([{"kind": "strum", "settings": {"spread_ms": 7.0}}]),
            ]
        )
    )
    assert tight["strums"][0]["min_spread_ms"] == 7.0
    assert "tight_strum" in flag_codes(tight)
    per_string = guitar(
        [{"kind": "strum", "settings": {"spread_ms": 7.0, "string_ms": 9.0}}]
    )
    report = measure(song([singer([(0, PHRASE)]), per_string]))
    assert report["strums"][0]["min_spread_ms"] == 18.0
    assert "tight_strum" not in flag_codes(report)
    none = measure(song([singer([(0, PHRASE)]), guitar([])]))
    assert "unstrummed_chords" in flag_codes(none)


def test_lines_are_cut_at_rests_and_at_lyric_annotation_lines() -> None:
    first = [note(i * BEAT, pitch=60 + i) for i in range(4)]
    second = [note(BAR + 2 * BEAT + i * BEAT, pitch=67 - i) for i in range(4)]
    one_clip = singer([(0, first + second)])
    assert len(measure(song([one_clip]))["lines"]) == 2
    # An annotation laid over the whole clip does not hide the rest between its phrases.
    whole = song([one_clip], lyrics=[(0, "ふたつのぎょう"), (3 * BAR, None)])
    assert len(measure(whole)["lines"]) == 2
    # Annotated lines are cut apart where no rest separates them.
    legato = singer([(0, [note(i * BEAT, pitch=60 + i % 5) for i in range(8)])])
    assert len(measure(song([legato]))["lines"]) == 1
    two = song([legato], lyrics=[(0, "いち"), (BAR, "に"), (2 * BAR, None)])
    assert len(measure(two)["lines"]) == 2


def test_a_line_longer_than_four_bars_is_cut_at_its_longest_rest() -> None:
    # Eight phrases of twelve eighths, an eighth rest after each: no rest of a beat anywhere.
    eighths = [
        note(phrase * 6240 + i * 480, length=480, pitch=60 + i % 5)
        for phrase in range(8)
        for i in range(12)
    ]
    clip = {"id": 1, "start": 0, "length": 13 * BAR, "notes": eighths, "muted": False}
    track = {
        "id": 1,
        "name": "Vocal",
        "mixer": {"mute": False},
        "kind": {"type": "singer", "clips": [clip]},
    }
    report = measure(song([track]))
    # Thirteen bars halve to 6.5 and again to 3.25: four lines of two phrases each.
    assert [line["notes"] for line in report["lines"]] == [24, 24, 24, 24]
    # With no rest of half a beat a line stays whole, however long.
    clip["notes"] = [note(i * 480, length=480, pitch=60 + i % 5) for i in range(100)]
    assert len(measure(song([track]))["lines"]) == 1


def test_a_line_entering_before_its_section_is_that_sections_line() -> None:
    verse = [note(i * BEAT, length=BEAT) for i in range(4)]
    # Two pickup eighths on beat 4 of the verse's last bar, then six in the chorus.
    chorus = [note(2 * BAR - BEAT + i * 480, length=480, pitch=67) for i in range(8)]
    report = measure(
        song(
            [singer([(0, verse + chorus)])],
            sections=((0, "Verse"), (2 * BAR, "Chorus"), (4 * BAR, None)),
        )
    )
    assert [line["section"] for line in report["lines"]] == ["Verse", "Chorus"]
    assert report["lines"][1]["entry"] == "pickup"
    assert report["summary"]["second_half_entries"] == 1


def test_entries_gathered_at_one_point_are_flagged_short_of_all_of_them() -> None:
    def lines(entry_of) -> dict:
        return singer(
            [
                (
                    index * 4 * BAR,
                    [
                        note(entry_of(index) + i * 480, length=480, pitch=60 + i)
                        for i in range(6)
                    ],
                )
                for index in range(10)
            ]
        )

    gathered = measure(song([lines(lambda index: BEAT if index == 4 else 0)]))
    assert gathered["summary"]["most_common_entry_share"] == 0.9
    assert gathered["summary"]["second_half_entries"] == 0
    assert {"same_entry", "no_second_half_entry"} <= flag_codes(gathered)
    # Two lines entering on beat 4, a pickup into the next bar, clear both flags.
    spread = measure(song([lines(lambda index: 3 * BEAT if index % 5 == 4 else 0)]))
    assert spread["summary"]["second_half_entries"] == 2
    assert not {"same_entry", "no_second_half_entry"} & flag_codes(spread)


def test_contours_follow_first_middle_and_last_pitch() -> None:
    assert contour_type([60, 62, 64]) == "ascending"
    assert contour_type([60, 67, 62]) == "convex"
    assert contour_type([67, 64, 60]) == "descending"
    assert contour_type([64, 60, 64]) == "concave"
