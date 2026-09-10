import json
from pathlib import Path

import pytest
from structure import build_report, comparison, measure, voice_motion


def note(at: int, length: int = 100, pitch: int = 60, velocity: float = 0.8) -> dict:
    return {"start": at, "length": length, "pitch": pitch, "velocity": velocity}


def track(role: str, notes: list[dict], length: int = 3840, start: int = 0) -> dict:
    return {
        "name": role,
        "mixer": {"mute": False},
        "kind": {
            "type": "instrument",
            "clips": [
                {
                    "start": start,
                    "length": length,
                    "notes": notes,
                    "recipe": {"preset": role},
                }
            ],
        },
    }


def test_polyphonic_attacks_and_sounding_time_count_once() -> None:
    comp = [note(at, 100, pitch) for at in (0, 100) for pitch in (48, 52, 55)]
    metrics, counts = measure(
        {"tracks": [track("lead", [note(0), note(200)]), track("chords", comp)]}
    )
    assert counts == {"chords": 6, "lead": 2}
    assert metrics["support_attack_count"] == 2
    assert metrics["support_sounding_ticks"] == 200
    assert metrics["support_foreground_onset_collision_fraction"] == 0.5
    assert metrics["support_foreground_duration_overlap_fraction"] == 0.5


def test_arpeggios_count_as_support_without_becoming_chord_voicings() -> None:
    metrics, _ = measure(
        {
            "tracks": [
                track("lead", [note(0), note(200)]),
                track(
                    "arp", [note(0, pitch=60), note(100, pitch=64), note(200, pitch=67)]
                ),
            ]
        }
    )
    assert metrics["support_attack_count"] == 3
    assert metrics["support_foreground_onset_collision_fraction"] == pytest.approx(
        2 / 3
    )
    assert metrics["support_foreground_duration_overlap_fraction"] == pytest.approx(
        2 / 3
    )
    assert metrics["comp_voice_motion_semitones"] is None
    assert metrics["comp_matched_voice_count"] == 0


def test_empty_and_monophonic_voicings_are_safe() -> None:
    metrics, counts = measure({"tracks": []})
    assert counts == {}
    assert metrics["comp_voice_motion_semitones"] is None
    assert metrics["melody_large_leap_fraction"] is None
    assert metrics["foreground_adjacent_bar_rhythm_repeat_fraction"] is None
    assert metrics["active_note_count"] == 0
    assert voice_motion([], [60]) == (0, 0)
    assert voice_motion([60], [62]) == (2, 1)
    assert voice_motion([60, 64, 67], [60, 65]) == (1, 2)
    assert voice_motion([60, 65], [60, 64, 67]) == (1, 2)


def test_melodic_leaps_never_bridge_rests_polyphony_or_clips() -> None:
    lead = track(
        "lead",
        [
            note(0, 100, 60),
            note(100, 100, 67),
            note(500, 100, 80),
            note(600, 100, 60),
            note(600, 100, 64),
        ],
    )
    lead["kind"]["clips"].append(
        {
            "start": 3840,
            "length": 3840,
            "recipe": {"preset": "lead"},
            "notes": [note(0, 100, 20)],
        }
    )
    metrics, _ = measure({"tracks": [lead]})
    assert metrics["melody_continuous_note_pairs"] == 1
    assert metrics["melody_large_leap_fraction"] == 1.0


def test_bar_patterns_compare_onsets_and_retain_the_denominator() -> None:
    notes = [note(bar * 3840 + offset) for bar in range(3) for offset in (0, 480)]
    notes.append(note(3 * 3840 + 960))
    metrics, _ = measure({"tracks": [track("lead", notes, length=4 * 3840)]})
    assert metrics["foreground_nonempty_bars"] == 4
    assert metrics["foreground_adjacent_bar_pairs"] == 3
    assert metrics["foreground_adjacent_bar_rhythm_repeat_fraction"] == pytest.approx(
        2 / 3
    )
    assert metrics["foreground_bar_rhythm_unique_fraction"] == 0.5


def test_signature_changes_and_partial_bars_are_not_false_repeats() -> None:
    project = {
        "signatures": {
            "points": [
                {"tick": 0, "signature": {"numerator": 4, "denominator": 4}},
                {"tick": 3840, "signature": {"numerator": 3, "denominator": 4}},
            ]
        },
        "tracks": [track("lead", [note(0), note(3840), note(6720)], length=7680)],
    }
    metrics, _ = measure(project)
    assert metrics["foreground_nonempty_bars"] == 2
    assert metrics["foreground_adjacent_bar_pairs"] == 0
    assert metrics["foreground_bar_rhythm_unique_fraction"] == 1.0


def test_actual_singer_notes_and_track_mute_control_foreground() -> None:
    singer = track("unknown", [note(0)])
    singer["kind"]["type"] = "singer"
    lead = track("lead", [note(100)])
    lead["mixer"]["mute"] = True
    metrics, counts = measure(
        {"tracks": [singer, lead, track("chords", [note(0), note(100)])]}
    )
    assert counts["vocal"] == 1
    assert counts["lead"] == 1  # Stored counts also describe muted material.
    assert metrics["active_note_count"] == 3
    assert metrics["support_foreground_onset_collision_fraction"] == 0.5


def test_bounds_and_nonfinite_values_are_reported_without_poisoning_metrics() -> None:
    data = track(
        "lead",
        [
            note(0),
            note(100, pitch=140),
            note(3800, length=100),
            note(200, velocity=float("nan")),
        ],
    )
    metrics, counts = measure({"tracks": [data]})
    assert metrics["invalid_note_count"] == 3
    assert metrics["nonfinite_values"] == 1
    assert metrics["active_note_count"] == 1
    assert counts["lead"] == 4
    json.dumps(metrics, allow_nan=False)


def test_baseline_pairs_labels_hashes_and_equal_weight_preset_means(
    tmp_path: Path,
) -> None:
    before_path = tmp_path / "before" / "pop-band-s101.auris"
    after_path = tmp_path / "after" / "pop-band-s101.auris"
    for path, notes in [(before_path, [note(0)]), (after_path, [note(0), note(200)])]:
        path.parent.mkdir()
        path.write_text(
            json.dumps({"tracks": [track("lead", notes)]}), encoding="utf-8"
        )
    before = build_report([before_path])
    after = build_report([after_path])
    delta = comparison(after, before)
    paired = delta["paired_projects"]["pop-band-s101"]
    assert paired["before_sha256"] != paired["after_sha256"]
    assert paired["note_counts_by_role"]["lead"] == 1
    assert (
        delta["preset_mean_deltas"]["pop-band"]["metrics"]["active_note_count"] == 1.0
    )
    assert not delta["missing_from_current"]
    before["schema_version"] = -1
    with pytest.raises(ValueError, match="schema"):
        comparison(after, before)
    with pytest.raises(ValueError, match="duplicate"):
        build_report([before_path, after_path])
