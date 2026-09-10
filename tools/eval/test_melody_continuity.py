import hashlib
import json
from copy import deepcopy

import pytest
from melody_continuity import analyze_manifest, analyze_project, main


def project(patterns):
    return {
        "tempo_map": {"points": [{"tick": 0, "bpm": 124}]},
        "signatures": {
            "points": [{"tick": 0, "signature": {"numerator": 4, "denominator": 4}}]
        },
        "sections": {
            "points": [
                {"tick": 0, "label": "chorus"},
                {"tick": 30720, "label": "outro"},
            ]
        },
        "tracks": [
            {
                "kind": {
                    "type": "instrument",
                    "clips": [
                        {
                            "start": 0,
                            "length": 30720,
                            "recipe": {"preset": "lead"},
                            "notes": [
                                {
                                    "start": bar * 3840 + at * 240,
                                    "length": length * 240,
                                    "pitch": 60 + index % 3,
                                    "velocity": 0.7,
                                }
                                for bar, pattern in enumerate(patterns)
                                for index, (at, length) in enumerate(pattern)
                            ],
                        }
                    ],
                }
            }
        ],
    }


def pop_patterns(name):
    """Compact rhythm-only shapes from the three discussed examples, with synthetic pitches."""
    if name == "C102":
        base = [(0, 1), (1, 1), (2, 6), (10, 1), (11, 1), (12, 4)]
        patterns = [base[:] for _ in range(8)]
        patterns[2] = base[:-1]
        patterns[3] = patterns[7] = [(0, 1), (1, 1), (2, 10)]
        patterns[6] = base[:-1] + [(12, 2), (14, 2)]
    elif name == "B105":
        patterns = [[(0, 4), (4, 2), (6, 2), (10, 6)] for _ in range(8)]
        patterns[3] = patterns[7] = [(0, 4), (4, 2), (6, 6)]
    else:
        patterns = [
            [(0, 2), (2, 2), (4, 4), (8, 2), (10, 2), (12, 4)] for _ in range(8)
        ]
        patterns[3] = [(0, 2), (2, 2), (4, 8)]
        patterns[6] = [(0, 2), (2, 2), (4, 4), (10, 2), (12, 4)]
        patterns[7] = [(0, 2), (2, 2), (4, 4), (8, 4)]
    return patterns


def test_known_pop_rhythms_separate_density_from_early_repeated_pause():
    c, b, d = [
        analyze_project(project(pop_patterns(name)))
        for name in ["C102", "B105", "D103"]
    ]
    assert [x["note_count"] for x in [c, b, d]] == [42, 30, 42]
    assert [
        x["nonending_summary"]["early_stationary_bar_count"] for x in [c, b, d]
    ] == [6, 0, 0]
    assert [x["nonending_summary"]["early_long_hold_count"] for x in [c, b, d]] == [
        6,
        0,
        0,
    ]
    assert [x["nonending_summary"]["late_long_hold_count"] for x in [c, b, d]] == [
        0,
        6,
        0,
    ]
    assert [
        x["nonending_summary"]["maximum_complete_in_bar_ioi_beats"] for x in [c, b, d]
    ] == [2, 1, 1.5]
    assert c["bars"][0]["beat_three_span"] == {
        "previous_attack_beat": 0.5,
        "next_attack_beat": 2.5,
        "ioi_beats": 2,
        "span_in_bar_beats": 2,
        "sounding_in_bar_beats": 1.5,
        "rest_in_bar_beats": 0.5,
        "crosses_bar_boundary": False,
    }
    assert (
        c["early_stationary_recurrence"]["longest_consecutive_nonending_bar_run"] == 3
    )
    assert (
        c["early_stationary_recurrence"]["adjacent_identical_nonending_pair_count"] == 4
    )
    assert c["early_stationary_recurrence"]["most_common_pattern_bar_count"] == 6
    assert d["nonending_summary"]["beat_three_attack_bar_count"] == 5


def test_endings_are_explicit_and_break_runs_instead_of_being_silently_excluded():
    p = project([[(0, 2), (2, 8), (10, 6)]] * 8)
    default, all_bars = analyze_project(p), analyze_project(p, ())
    assert default["nonending_summary"]["early_stationary_bar_count"] == 6
    assert default["ending_summary"]["early_stationary_bar_count"] == 2
    assert (
        default["early_stationary_recurrence"]["longest_consecutive_nonending_bar_run"]
        == 3
    )
    assert (
        all_bars["early_stationary_recurrence"]["longest_consecutive_nonending_bar_run"]
        == 8
    )
    assert all_bars["ending_summary"]["maximum_attack_free_span_beats"] is None


def test_beat_three_attack_and_same_pitch_reattack_are_not_an_ioi_crossing():
    p = project([[(0, 8), (8, 8)]] * 8)
    for note in p["tracks"][0]["kind"]["clips"][0]["notes"]:
        note["pitch"] = 60
    result = analyze_project(p)
    assert all(bar["beat_three_span"] is None for bar in result["bars"])
    assert result["nonending_summary"]["early_stationary_bar_count"] == 0
    assert result["consecutive_same_pitch_attack_count"] == 15


def test_empty_and_held_across_bar_preserve_coverage_without_fake_boundary_attack():
    p = project([[(0, 24)], [], [(0, 4)], [], [(0, 4)], [], [(0, 4)], []])
    result = analyze_project(p)
    held = result["bars"][1]
    assert held["note_count"] == 0
    assert held["maximum_attack_free_span_beats"] == 4
    assert held["maximum_complete_in_bar_ioi_beats"] is None
    assert held["sounding_beats"] == 2
    assert held["beat_three_span"]["ioi_beats"] == 8
    assert held["beat_three_span"]["rest_in_bar_beats"] == 2
    assert held["early_stationary_span"] is False
    trailing = result["bars"][7]["beat_three_span"]
    assert trailing["next_attack_beat"] is None
    assert trailing["ioi_beats"] is None


def test_overlapping_holds_use_union_coverage_and_leading_rest_is_not_an_ioi():
    p = project([[(10, 6)]] + [[(0, 12), (2, 2), (12, 4)]] * 7)
    result = analyze_project(p)
    assert result["bars"][0]["beat_three_span"]["previous_attack_beat"] is None
    assert result["bars"][0]["early_stationary_span"] is False
    overlap = result["bars"][1]["beat_three_span"]
    assert overlap["sounding_in_bar_beats"] == 2.5
    assert overlap["rest_in_bar_beats"] == 0
    assert result["bars"][1]["sounding_beats"] == 4


def test_timing_quantization_is_shared_with_existing_metrics():
    p = project(pop_patterns("B105"))
    shifted = deepcopy(p)
    for note in shifted["tracks"][0]["kind"]["clips"][0]["notes"]:
        note["start"] += 29
        note["length"] -= 29
    a, b = analyze_project(p), analyze_project(shifted)
    assert a["bars"] == b["bars"]
    assert b["quantization"]["max_onset_error_ticks"] == 29


@pytest.mark.parametrize("endings", [(0, 8), (4, 9), (4, 4), (True, 8)])
def test_invalid_phrase_endings_are_rejected(endings):
    with pytest.raises(ValueError, match="phrase endings"):
        analyze_project(project(pop_patterns("C102")), endings)


@pytest.mark.parametrize("kind", ["first_empty", "duplicate_attack"])
def test_unsupported_lead_geometry_is_rejected_consistently(kind):
    p = project(pop_patterns("B105"))
    notes = p["tracks"][0]["kind"]["clips"][0]["notes"]
    if kind == "first_empty":
        notes[:] = [note for note in notes if note["start"] >= 3840]
    else:
        notes.append(deepcopy(notes[0]))
    with pytest.raises(ValueError):
        analyze_project(p)


def test_manifest_verifies_hash_and_cli_refuses_existing_output(tmp_path, monkeypatch):
    path = tmp_path / "score.auris"
    path.write_text(json.dumps(project(pop_patterns("C102"))), encoding="utf8")
    manifest = tmp_path / "manifest.json"
    data = {
        "files": {
            "example": {
                "project": {
                    "path": path.name,
                    "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
                }
            }
        }
    }
    manifest.write_text(json.dumps(data), encoding="utf8")
    assert (
        analyze_manifest(manifest)["files"]["example"]["continuity"]["note_count"] == 42
    )
    output = tmp_path / "continuity.json"
    monkeypatch.setattr(
        "sys.argv",
        ["melody_continuity", "--manifest", str(manifest), "--json", str(output)],
    )
    main()
    original = output.read_bytes()
    with pytest.raises(SystemExit):
        main()
    assert output.read_bytes() == original
    path.write_text("{}", encoding="utf8")
    with pytest.raises(ValueError, match="SHA256"):
        analyze_manifest(manifest)
