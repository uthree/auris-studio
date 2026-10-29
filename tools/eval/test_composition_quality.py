import json
from argparse import Namespace
from pathlib import Path

import composition_quality as quality
import pytest


def suite(root: Path, *, success=True, model="model"):
    root.mkdir()
    folder = root / "loop_title-01"
    folder.mkdir()
    (folder / "result.wav").write_bytes(b"frozen-audio")
    value = {
        "manifest": {
            "schema_version": 1,
            "axes": quality.AXES,
            "cases": [
                {
                    "id": "loop_title",
                    "split": "development",
                    "fixture": "blank",
                    "prompts": ["a short loop"],
                }
            ],
        },
        "manifest_sha256": "manifest",
        "model": model,
        "backend": "codex",
        "prompt_profile": "legacy",
        "candidate_tools_enabled": False,
        "context_tokens": 8192,
        "split": "development",
        "repetitions": 1,
        "runner_sha256": "runner",
        "runtime": {"agent.exe": "runtime"},
        "source_commit": "commit",
        "source_diff": "",
        "untracked_rust_sources": {},
        "selected_cases": ["loop_title"],
        "trials": [
            {
                "id": "loop_title-01",
                "transport_pass": success,
                "delivery_present": success,
                "artifacts": {"result.wav": quality.digest(folder / "result.wav")},
            }
        ],
    }
    quality.write_json(root / "suite.json", value)
    return value


def test_manifest_keeps_development_and_holdout_cohorts_separate():
    data = quality.manifest(quality.MANIFEST)
    assert sum(c["split"] == "development" for c in data["cases"]) == 8
    assert sum(c["split"] == "holdout" for c in data["cases"]) == 4


def test_runtime_smoke_uses_versioned_eight_bar_fixture():
    data = quality.manifest(
        quality.MANIFEST.with_name("composition-runtime-smoke-v2.json")
    )
    assert data["cases"][0]["fixture"] == "melody_six_eight_eight_bars"


def test_freeze_seed_project_copies_only_project_and_audio_without_mutating_source(
    tmp_path,
):
    source_root = tmp_path / "SeedSong"
    source_root.mkdir()
    source = source_root / "SeedSong.auris"
    source.write_text('{"project": "frozen"}', encoding="utf-8")
    (source_root / "Audio").mkdir()
    (source_root / "Audio" / "loop.wav").write_bytes(b"audio")
    (source_root / "unrelated.txt").write_text("do not copy", encoding="utf-8")
    before = source.read_bytes()

    frozen = tmp_path / "suite" / "seed-project"
    provenance = quality.freeze_seed_project(source, frozen)

    assert (frozen / "SeedSong" / "SeedSong.auris").read_bytes() == before
    assert (frozen / "SeedSong" / "Audio" / "loop.wav").read_bytes() == b"audio"
    assert not (frozen / "SeedSong" / "unrelated.txt").exists()
    assert source.read_bytes() == before
    assert provenance["files"] == {
        "SeedSong/Audio/loop.wav": quality.digest(
            frozen / "SeedSong" / "Audio" / "loop.wav"
        ),
        "SeedSong/SeedSong.auris": quality.digest(
            frozen / "SeedSong" / "SeedSong.auris"
        ),
    }


@pytest.mark.parametrize(
    "layout",
    ["Song.auris", "Other/Song.auris"],
)
def test_freeze_seed_project_rejects_ambiguous_project_layout(tmp_path, layout):
    source = tmp_path / layout
    source.parent.mkdir(parents=True, exist_ok=True)
    source.write_text("{}", encoding="utf-8")
    with pytest.raises(ValueError, match="project-named folder"):
        quality.freeze_seed_project(source, tmp_path / "frozen")


def test_runner_isolates_each_repetition_from_the_frozen_seed_project(
    tmp_path, monkeypatch
):
    source_root = tmp_path / "SeedSong"
    source_root.mkdir()
    source = source_root / "SeedSong.auris"
    original = b'{"notes":"initial"}'
    source.write_bytes(original)
    (source_root / "Audio").mkdir()
    (source_root / "Audio" / "backing.wav").write_bytes(b"backing")

    manifest = tmp_path / "manifest.json"
    quality.write_json(
        manifest,
        {
            "schema_version": 1,
            "conditions": "fixed source",
            "axes": quality.AXES,
            "delivery_prompt": "verify",
            "cases": [
                {
                    "id": "fixed_pop_vocal",
                    "split": "development",
                    "fixture": "blank",
                    "prompts": ["revise vocal only"],
                }
            ],
        },
    )
    executable = tmp_path / "fake-agent.exe"
    executable.write_bytes(b"agent")

    def fake_check_output(command, **kwargs):
        if command[:3] == ["git", "rev-parse", "HEAD"]:
            return "fake-commit"
        return ""

    def fake_run(command, *, env, stdout, **kwargs):
        config = json.loads(Path(env["AURIS_COMPOSITION_EVAL"]).read_text())
        seed = Path(config["seed_project"])
        assert seed.read_bytes() == original
        seed.write_bytes(b"agent-edit-" + seed.parent.parent.name.encode())
        return Namespace(returncode=0)

    monkeypatch.setattr(quality.subprocess, "check_output", fake_check_output)
    monkeypatch.setattr(quality.subprocess, "run", fake_run)
    args = Namespace(
        manifest=manifest,
        prompt_condition=None,
        repetitions=2,
        timeout=60,
        context_tokens=8192,
        split="development",
        case=None,
        seed_project=source,
        executable=executable,
        output=tmp_path / "suite",
        model="model",
        backend="codex",
        prompt_profile="legacy",
        candidate_tools=False,
    )

    quality.run(args)

    assert source.read_bytes() == original
    suite_data = json.loads((args.output / "suite.json").read_text())
    assert suite_data["seed_project"]["source_sha256"] == quality.digest(source)
    assert suite_data["seed_project"]["files"][
        "SeedSong/SeedSong.auris"
    ] == quality.digest(args.output / "seed-project" / "SeedSong" / "SeedSong.auris")
    trial_inputs = []
    for trial in suite_data["trials"]:
        config = json.loads((args.output / f"{trial['id']}.config.json").read_text())
        seed_path = Path(config["seed_project"])
        assert seed_path.parent.name == seed_path.stem
        assert seed_path.read_bytes() != original
        trial_inputs.append(seed_path)
    assert len(trial_inputs) == 2
    assert trial_inputs[0].parent.parent != trial_inputs[1].parent.parent
    assert (
        args.output / "seed-project" / "SeedSong" / "SeedSong.auris"
    ).read_bytes() == original


def test_changed_audio_cannot_be_substituted_after_scoring(tmp_path):
    before, after = tmp_path / "before", tmp_path / "after"
    suite(before)
    suite(after)
    (after / "loop_title-01/result.wav").write_bytes(b"replacement")
    with pytest.raises(ValueError, match="Changed listening artifact"):
        quality.pairs(before, after, 42)


def test_comparison_refuses_changed_model_conditions(tmp_path):
    before, after = tmp_path / "before", tmp_path / "after"
    suite(before)
    suite(after, model="different")
    with pytest.raises(ValueError, match="model"):
        quality.pairs(before, after, 42)


def test_failed_trials_remain_visible_in_comparison(tmp_path):
    before, after = tmp_path / "before", tmp_path / "after"
    suite(before)
    suite(after, success=False)
    assert quality.pairs(before, after, 42) == [
        {"trial": "loop_title-01", "status": "failed_or_missing_delivery"}
    ]


def test_missing_trial_is_not_silently_removed(tmp_path):
    before, after = tmp_path / "before", tmp_path / "after"
    suite(before)
    data = suite(after)
    data["trials"] = []
    quality.write_json(after / "suite.json", data)
    with pytest.raises(ValueError, match="cohorts"):
        quality.pairs(before, after, 42)


def test_pair_order_is_reproducible_and_does_not_modify_sources(tmp_path):
    before, after = tmp_path / "before", tmp_path / "after"
    suite(before)
    suite(after)
    result = quality.pairs(before, after, 42)
    assert result == quality.pairs(before, after, 42)
    assert {x["condition"] for x in result[0]["audio"]} == {"before", "after"}
    assert json.loads((before / "suite.json").read_text())["trials"][0][
        "transport_pass"
    ]


def test_matching_but_incomplete_cohorts_cannot_hide_interrupted_runs(tmp_path):
    before, after = tmp_path / "before", tmp_path / "after"
    for root in [before, after]:
        data = suite(root)
        data["repetitions"] = 2
        quality.write_json(root / "suite.json", data)
    with pytest.raises(ValueError, match="Incomplete trial cohorts"):
        quality.pairs(before, after, 42)


def test_frozen_source_uses_original_provenance_and_rejects_changed_runtime(tmp_path):
    reference = tmp_path / "suite.json"
    original = {
        "runtime": {"test.exe": "original-binary"},
        "source_commit": "original-commit",
        "source_diff": "original-diff",
        "untracked_rust_sources": {"draft.rs": "original source"},
    }
    quality.write_json(reference, original)
    snapshot = tmp_path / "source-suite.json"
    bound = quality.frozen_source(reference, original["runtime"], snapshot)
    assert bound["source_commit"] == "original-commit"
    assert bound["untracked_rust_sources"] == original["untracked_rust_sources"]
    assert bound["source_reference_sha256"] == quality.digest(reference)
    assert snapshot.read_bytes() == reference.read_bytes()
    quality.write_json(reference, original | {"trials": [{"id": "later-result"}]})
    assert bound["source_reference_sha256"] == quality.digest(snapshot)
    assert quality.digest(reference) != quality.digest(snapshot)
    with pytest.raises(ValueError, match="runtime"):
        quality.frozen_source(reference, {"test.exe": "different-binary"})


def test_old_manifest_without_listening_rubric_remains_valid(tmp_path):
    path = tmp_path / "old.json"
    quality.write_json(
        path,
        {
            "schema_version": 1,
            "axes": quality.AXES,
            "cases": [
                {
                    "id": "case",
                    "split": "development",
                    "fixture": "blank",
                    "prompts": ["a brief"],
                }
            ],
        },
    )
    assert "listening_rubric" not in quality.manifest(path)


def test_prompt_condition_is_strict_and_frozen(tmp_path):
    path = tmp_path / "condition.json"
    value = {
        "name": "after",
        "source_commit": "abc123",
        "expected_prefix": "legacy prompt",
        "replacement_prefix": "current prompt",
        "skill_context": "melody guidance",
    }
    quality.write_json(path, value)
    condition, payload, checksum = quality.prompt_condition(path)
    assert condition == value
    assert payload == path.read_bytes()
    assert checksum == quality.digest(path)


@pytest.mark.parametrize(
    "change,match",
    [
        ({"unknown": "value"}, "exactly"),
        ({"expected_prefix": ""}, "nonempty"),
        ({"replacement_prefix": ""}, "nonempty"),
        ({"skill_context": 3}, "strings"),
    ],
)
def test_prompt_condition_rejects_invalid_values(tmp_path, change, match):
    path = tmp_path / "condition.json"
    value = {
        "name": "after",
        "source_commit": "abc123",
        "expected_prefix": "legacy prompt",
        "replacement_prefix": "current prompt",
        "skill_context": "",
    }
    value.update(change)
    quality.write_json(path, value)
    with pytest.raises(ValueError, match=match):
        quality.prompt_condition(path)


@pytest.mark.parametrize(
    "rubric",
    [
        {"unknown": ["question"]},
        {"melody": []},
        {"melody": [" "]},
        {"melody": "question"},
        {"melody": ["question", 3]},
    ],
)
def test_manifest_rejects_malformed_listening_rubric(tmp_path, rubric):
    path = tmp_path / "bad.json"
    quality.write_json(
        path,
        {
            "schema_version": 1,
            "axes": quality.AXES,
            "listening_rubric": rubric,
            "cases": [
                {
                    "id": "case",
                    "split": "development",
                    "fixture": "blank",
                    "prompts": ["a brief"],
                }
            ],
        },
    )
    with pytest.raises(ValueError, match="Listening rubric"):
        quality.manifest(path)


def test_blind_page_renders_escaped_rubric_questions(tmp_path):
    before, after = tmp_path / "before", tmp_path / "after"
    before_data = suite(before)
    after_data = suite(after)
    question = '<strong>Hear the lead & "hook"?</strong>'
    for data, root in [(before_data, before), (after_data, after)]:
        data["manifest"]["listening_rubric"] = {"melody": [question]}
        quality.write_json(root / "suite.json", data)
    output = tmp_path / "listening"
    quality.blind(Namespace(before=before, after=after, output=output, seed=42))
    page = (output / "listening.html").read_text(encoding="utf-8")
    assert "Hear the lead &amp; &quot;hook&quot;?" in page
    assert "<strong>Hear the lead" not in page
    assert 'data-axis="melody"' in page


def test_run_can_start_at_second_repetition_and_records_range(tmp_path, monkeypatch):
    manifest = tmp_path / "manifest.json"
    quality.write_json(
        manifest,
        {
            "schema_version": 1,
            "conditions": "fixed",
            "axes": quality.AXES,
            "delivery_prompt": "verify",
            "cases": [
                {
                    "id": "case",
                    "split": "development",
                    "fixture": "blank",
                    "prompts": ["brief"],
                }
            ],
        },
    )
    executable = tmp_path / "fake.exe"
    executable.write_bytes(b"agent")
    monkeypatch.setattr(quality.subprocess, "check_output", lambda *a, **k: "commit")

    def fake_run(command, *, env, stdout, **kwargs):
        config = json.loads(Path(env["AURIS_COMPOSITION_EVAL"]).read_text())
        destination = Path(config["output"])
        destination.mkdir()
        (destination / "result.wav").write_bytes(b"audio")
        (destination / "result.mid").write_bytes(b"midi")
        quality.write_json(
            destination / "report.json",
            {
                "turns": [{"failure": None} for _ in config["prompts"]],
                "argument_or_tool_errors": [],
            },
        )
        return Namespace(returncode=0)

    monkeypatch.setattr(quality.subprocess, "run", fake_run)
    args = Namespace(
        manifest=manifest,
        prompt_condition=None,
        repetition_start=2,
        repetitions=1,
        timeout=60,
        context_tokens=8192,
        split="development",
        case=None,
        seed_project=None,
        executable=executable,
        output=tmp_path / "suite",
        model="model",
        backend="codex",
        prompt_profile="legacy",
        candidate_tools=False,
    )
    quality.run(args)
    data = quality.read_json(args.output / "suite.json")
    assert data["repetition_start"] == 2
    assert data["repetition_end"] == 2
    assert [trial["id"] for trial in data["trials"]] == ["case-02"]
    assert data["trials"][0]["repetition"] == 2


def test_combine_repetitions_copies_artifacts_and_rejects_gap(tmp_path):
    first = tmp_path / "first"
    second = tmp_path / "second"
    suite(first)
    suite(second)
    (first / "loop_title-01" / "unrecorded.txt").write_text("not part of the trial")
    data = quality.read_json(second / "suite.json")
    data["trials"][0]["id"] = "loop_title-02"
    data["trials"][0]["repetition"] = 2
    data["repetition_start"] = 2
    data["repetition_end"] = 2
    (second / "loop_title-01").rename(second / "loop_title-02")
    quality.write_json(second / "suite.json", data)
    output = tmp_path / "combined"
    quality.combine_repetitions([first, second], output)
    combined = quality.read_json(output / "suite.json")
    assert combined["repetitions"] == 2
    assert combined["repetition_start"] == 1
    assert {t["id"] for t in combined["trials"]} == {"loop_title-01", "loop_title-02"}
    assert (output / "loop_title-02" / "result.wav").read_bytes() == b"frozen-audio"
    assert not (output / "loop_title-01" / "unrecorded.txt").exists()
    assert quality.digest(first / "loop_title-01" / "result.wav") == quality.digest(
        output / "loop_title-01" / "result.wav"
    )
    with pytest.raises(FileExistsError):
        quality.combine_repetitions([first, second], output)


def test_combine_repetitions_rejects_noncontiguous_ids(tmp_path):
    first = tmp_path / "first"
    third = tmp_path / "third"
    suite(first)
    suite(third)
    data = quality.read_json(third / "suite.json")
    data["trials"][0]["id"] = "loop_title-03"
    data["trials"][0]["repetition"] = 3
    data["repetition_start"] = 3
    data["repetition_end"] = 3
    (third / "loop_title-01").rename(third / "loop_title-03")
    quality.write_json(third / "suite.json", data)
    with pytest.raises(ValueError, match="contiguous"):
        quality.combine_repetitions([first, third], tmp_path / "bad")


def test_combine_repetitions_rejects_runtime_drift(tmp_path):
    first = tmp_path / "first"
    second = tmp_path / "second"
    suite(first)
    suite(second)
    data = quality.read_json(second / "suite.json")
    data["runtime"] = {"agent.exe": "different"}
    data["trials"][0]["id"] = "loop_title-02"
    data["trials"][0]["repetition"] = 2
    data["repetition_start"] = 2
    data["repetition_end"] = 2
    (second / "loop_title-01").rename(second / "loop_title-02")
    quality.write_json(second / "suite.json", data)
    with pytest.raises(ValueError, match="frozen conditions"):
        quality.combine_repetitions([first, second], tmp_path / "bad")


def test_combine_repetitions_preserves_failed_trials(tmp_path):
    first = tmp_path / "first"
    second = tmp_path / "second"
    suite(first)
    suite(second, success=False)
    (second / "loop_title-01" / "result.wav").unlink()
    (second / "loop_title-01").rmdir()
    data = quality.read_json(second / "suite.json")
    data["trials"][0]["id"] = "loop_title-02"
    data["trials"][0]["repetition"] = 2
    data["trials"][0]["artifacts"] = {}
    data["repetition_start"] = 2
    data["repetition_end"] = 2
    quality.write_json(second / "suite.json", data)
    quality.combine_repetitions([first, second], tmp_path / "combined")
    trials = quality.read_json(tmp_path / "combined/suite.json")["trials"]
    assert len(trials) == 2
    assert any(not trial["transport_pass"] for trial in trials)
    assert (tmp_path / "combined/loop_title-02").is_dir()


def test_combine_repetitions_rejects_recorded_artifact_path_escape(tmp_path):
    first = tmp_path / "first"
    second = tmp_path / "second"
    suite(first)
    suite(second)
    data = quality.read_json(second / "suite.json")
    data["trials"][0]["id"] = "loop_title-02"
    data["trials"][0]["repetition"] = 2
    data["trials"][0]["artifacts"]["../escape.wav"] = "hash"
    data["repetition_start"] = 2
    data["repetition_end"] = 2
    (second / "loop_title-01").rename(second / "loop_title-02")
    quality.write_json(second / "suite.json", data)
    with pytest.raises(ValueError, match="Unsafe artifact path"):
        quality.combine_repetitions([first, second], tmp_path / "bad")
