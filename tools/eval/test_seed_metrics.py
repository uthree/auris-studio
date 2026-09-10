import hashlib
import json
from copy import deepcopy
from pathlib import Path

import pytest
from seed_metrics import analyze_manifest, analyze_project, corpus_summary, main


def project() -> dict:
    return {
        "tempo_map": {"points": [{"tick": 0, "bpm": 120.0}]},
        "signatures": {
            "points": [{"tick": 0, "signature": {"numerator": 4, "denominator": 4}}]
        },
        "sections": {
            "points": [
                {"tick": 0, "label": "verse"},
                {"tick": 15360, "label": "chorus"},
                {"tick": 46080, "label": "outro"},
            ]
        },
        "tracks": [
            {
                "id": 1,
                "mixer": {"mute": False},
                "kind": {
                    "type": "instrument",
                    "clips": [
                        {
                            "id": 2,
                            "start": 15360,
                            "length": 30720,
                            "recipe": {"preset": "lead"},
                            "notes": [
                                {
                                    "start": bar * 3840 + at,
                                    "length": length,
                                    "pitch": pitch,
                                    "velocity": 0.7,
                                }
                                for bar in range(8)
                                for at, length, pitch in [
                                    (240, 480, 60),
                                    (960, 960, 64),
                                    (2160, 720, 62),
                                ]
                            ],
                        }
                    ],
                },
            }
        ],
    }


def notes(data: dict) -> list[dict]:
    return data["tracks"][0]["kind"]["clips"][0]["notes"]


def test_swing_and_small_jitter_keep_quantized_signatures_and_record_errors() -> None:
    plain = project()
    swung = deepcopy(plain)
    for index, note in enumerate(notes(swung)):
        note["start"] += 29 if index % 2 else -17
        note["length"] += 13
    a, b = analyze_project(plain), analyze_project(swung)
    assert a["chorus"] == b["chorus"]
    assert a["hashes"]["raw_notes_sha256"] != b["hashes"]["raw_notes_sha256"]
    assert (
        a["hashes"]["quantized_notes_sha256"] == b["hashes"]["quantized_notes_sha256"]
    )
    assert b["quantization"]["max_onset_error_ticks"] == 29
    assert b["quantization"]["max_duration_error_ticks"] == 13


def test_leading_rest_and_duration_are_part_of_rhythm_identity() -> None:
    plain = project()
    delayed, shortened = deepcopy(plain), deepcopy(plain)
    for note in notes(delayed):
        note["start"] += 240
    notes(shortened)[0]["length"] -= 240
    a, b, c = map(analyze_project, (plain, delayed, shortened))
    assert a["first_bar"]["onset_duration"] != b["first_bar"]["onset_duration"]
    assert a["first_bar"]["onset_duration"] != c["first_bar"]["onset_duration"]
    assert a["first_bar"]["onset_duration"][0][0] == 240


def test_quantization_uses_half_up_at_exact_half_steps() -> None:
    data = project()
    notes(data)[0]["start"] = 120
    notes(data)[0]["length"] = 600
    assert analyze_project(data)["first_bar"]["onset_duration"][0] == [240, 720]


def test_transposition_normalizes_pitch_but_octave_jumps_remain_distinct() -> None:
    plain = project()
    transposed, octave_jump = deepcopy(plain), deepcopy(plain)
    for note in notes(transposed):
        note["pitch"] += 7
    notes(octave_jump)[1]["pitch"] += 12
    a, b, c = map(analyze_project, (plain, transposed, octave_jump))
    assert a["chorus"]["relative_pitches"] == b["chorus"]["relative_pitches"]
    assert a["chorus"]["intervals"] == b["chorus"]["intervals"]
    assert a["chorus"]["intervals"] != c["chorus"]["intervals"]


def test_within_chorus_counts_every_bar_pair_and_adjacent_repeat() -> None:
    result = analyze_project(project())
    assert result["within_chorus"] == {
        "unique_rhythm_count": 1,
        "empty_bar_count": 0,
        "duplicate_pair_count": 28,
        "pair_count": 28,
        "adjacent_repeat_count": 7,
        "adjacent_pair_count": 7,
    }


def test_corpus_counts_all_seed_pairs_and_cross_genre_matches() -> None:
    symbolic = analyze_project(project())
    files = {
        f"{preset}-s{seed}": {"preset": preset, "seed": seed, "symbolic": symbolic}
        for preset in ("rock", "city-pop", "pop-band")
        for seed in range(101, 109)
    }
    result = corpus_summary(files)
    for genre in result["per_genre"].values():
        assert genre["pair_count"] == 28
        assert genre["unique_chorus_rhythm_count"] == 1
        assert genre["duplicate_first_bar_rhythm_pair_count"] == 28
        assert genre["duplicate_chorus_interval_pair_count"] == 28
        assert genre["onset_jaccard"]["mean"] == 1.0
    assert result["cross_genre_same_seed"]["pair_count"] == 24
    assert result["cross_genre_same_seed"]["chorus_rhythm_collision_pair_count"] == 24


def test_jaccard_distinguishes_lengths_from_onsets() -> None:
    a, b = project(), project()
    notes(b)[0]["length"] = 240
    result = corpus_summary(
        {
            "a": {"preset": "rock", "seed": 101, "symbolic": analyze_project(a)},
            "b": {"preset": "rock", "seed": 102, "symbolic": analyze_project(b)},
        }
    )
    pair = result["per_genre"]["rock"]["pairs"][0]
    assert pair["onset_jaccard"] == 1.0
    assert pair["onset_duration_jaccard"] == pytest.approx(23 / 25)


def test_partial_duplicates_count_pairs_instead_of_duplicate_rows() -> None:
    repeated, different = project(), project()
    notes(different)[0]["start"] += 240
    result = corpus_summary(
        {
            str(seed): {
                "preset": "rock",
                "seed": seed,
                "symbolic": analyze_project(source),
            }
            for seed, source in [(101, repeated), (102, repeated), (103, different)]
        }
    )["per_genre"]["rock"]
    assert result["pair_count"] == 3
    assert result["unique_chorus_rhythm_count"] == 2
    assert result["duplicate_chorus_rhythm_pair_count"] == 1


@pytest.mark.parametrize(
    "mutation",
    [
        "missing_lead",
        "missing_chorus",
        "short_clip",
        "short_chorus",
        "duplicate_lead",
        "bad_note",
        "muted",
        "bad_meter",
        "tempo_change",
        "collapsed_duration",
        "collapsed_onsets",
        "same_onset",
    ],
)
def test_invalid_or_ambiguous_sources_are_rejected(mutation: str) -> None:
    data = project()
    clip = data["tracks"][0]["kind"]["clips"][0]
    if mutation == "missing_lead":
        clip["recipe"]["preset"] = "chords"
    elif mutation == "missing_chorus":
        data["sections"]["points"][1]["label"] = "bridge"
    elif mutation == "short_clip":
        clip["length"] = 4 * 3840
    elif mutation == "short_chorus":
        data["sections"]["points"][2]["tick"] -= 3840
    elif mutation == "duplicate_lead":
        data["tracks"][0]["kind"]["clips"].append(deepcopy(clip))
    elif mutation == "bad_note":
        notes(data)[0]["pitch"] = 128
    elif mutation == "muted":
        clip["muted"] = True
    elif mutation == "bad_meter":
        data["signatures"]["points"][0]["signature"]["numerator"] = 3
    elif mutation == "tempo_change":
        data["tempo_map"]["points"].append({"tick": 3840, "bpm": 121})
    elif mutation == "collapsed_duration":
        notes(data)[0]["length"] = 50
    elif mutation == "collapsed_onsets":
        notes(data)[1]["start"] = notes(data)[0]["start"] + 29
    else:
        notes(data)[1]["start"] = notes(data)[0]["start"]
    with pytest.raises(ValueError):
        analyze_project(data)


def test_duplicate_preset_seed_rows_are_rejected() -> None:
    row = {"preset": "rock", "seed": 101, "symbolic": analyze_project(project())}
    with pytest.raises(ValueError, match="duplicate preset/seed"):
        corpus_summary({"a": row, "b": row})


def manifest_files(directory: Path) -> tuple[Path, Path]:
    score = directory / "score.auris"
    score.write_text(json.dumps(project()), encoding="utf-8")
    manifest = directory / "manifest.json"
    manifest.write_text(
        json.dumps(
            {
                "schema_version": 1,
                "presets": ["rock"],
                "seeds": [101],
                "design": "Keep the original experiment context",
                "files": {
                    "rock-s101": {
                        "preset": "rock",
                        "seed": 101,
                        "project": {
                            "path": "score.auris",
                            "sha256": hashlib.sha256(score.read_bytes()).hexdigest(),
                        },
                        "excerpt": {"path": "excerpt.wav"},
                    },
                },
            }
        ),
        encoding="utf-8",
    )
    return manifest, directory / "symbolic.json"


def test_manifest_enrichment_checks_relative_sources_and_retains_context(
    tmp_path: Path,
) -> None:
    manifest, output = manifest_files(tmp_path)
    original = manifest.read_bytes()
    result = analyze_manifest(manifest, output)
    assert manifest.read_bytes() == original
    assert result["design"] == "Keep the original experiment context"
    assert result["files"]["rock-s101"]["excerpt"] == {"path": "excerpt.wav"}
    assert result["diversity"]["project_count"] == 1
    assert json.loads(output.read_text(encoding="utf-8")) == result


def test_manifest_rejects_stale_source_hash_before_writing_output(
    tmp_path: Path,
) -> None:
    manifest, output = manifest_files(tmp_path)
    score = tmp_path / "score.auris"
    score.write_text(score.read_text(encoding="utf-8") + "\n", encoding="utf-8")
    with pytest.raises(ValueError, match="SHA256 mismatch"):
        analyze_manifest(manifest, output)
    assert not output.exists()


def test_manifest_refuses_source_and_existing_output_overwrite(tmp_path: Path) -> None:
    manifest, output = manifest_files(tmp_path)
    with pytest.raises(ValueError, match="new file"):
        analyze_manifest(manifest, manifest)
    output.write_text("preserve me", encoding="utf-8")
    with pytest.raises(ValueError, match="new file"):
        analyze_manifest(manifest, output)
    assert output.read_text(encoding="utf-8") == "preserve me"


def test_manifest_rejects_an_incomplete_declared_corpus(tmp_path: Path) -> None:
    manifest, output = manifest_files(tmp_path)
    data = json.loads(manifest.read_text(encoding="utf-8"))
    data["seeds"].append(102)
    manifest.write_text(json.dumps(data), encoding="utf-8")
    with pytest.raises(ValueError, match="declared preset/seed"):
        analyze_manifest(manifest, output)
    assert not output.exists()


def test_cli_enriches_the_manifest(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    manifest, output = manifest_files(tmp_path)
    monkeypatch.setattr(
        "sys.argv",
        ["seed_metrics.py", "--manifest", str(manifest), "--json", str(output)],
    )
    main()
    assert (
        json.loads(output.read_text(encoding="utf-8"))["diversity"]["project_count"]
        == 1
    )
