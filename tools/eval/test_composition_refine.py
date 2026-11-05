"""Focused guards for the bounded SongEval refinement orchestration."""

from __future__ import annotations

import copy
import json
from pathlib import Path
from types import SimpleNamespace

import composition_refine as refine
import pytest
from composition_refine import (
    audit_edit,
    compare_scores,
    extract_scores,
    recipe,
    validate_notes,
)


def project() -> dict:
    """Return a tiny singer project with protected backing state."""
    return {
        "tracks": [
            {
                "id": 1,
                "name": "Lead",
                "kind": {
                    "type": "singer",
                    "clips": [
                        {
                            "id": 7,
                            "length": 960,
                            "lyrics": ["la", "la"],
                            "notes": [
                                {"pitch": 60, "start": 0, "length": 480},
                                {"pitch": 62, "start": 520, "length": 400},
                            ],
                        }
                    ],
                },
                "mixer": {"gain_db": -2, "effects": [{"id": "vocal"}]},
            },
            {
                "id": 2,
                "name": "Backing",
                "kind": {
                    "type": "instrument",
                    "clips": [{"id": 8, "length": 960, "notes": []}],
                },
                "mixer": {"gain_db": -8, "effects": [{"id": "delay"}]},
            },
        ],
        "tempo": 112,
        "audio_sources": {"voice": {"path": "Audio/voice.wav"}},
        "harmony": [{"bar": 1, "chord": "C"}],
    }


def step(**overrides) -> dict:
    value = {
        "instruction": "Make the second phrase answer the first with a short anticipation.",
        "fields": ["start"],
        "max_changed_notes": 1,
    }
    value.update(overrides)
    return value


def test_note_validation_rejects_overlap_and_out_of_bounds():
    value = project()
    value["tracks"][0]["kind"]["clips"][0]["notes"][1]["start"] = 200
    with pytest.raises(ValueError, match="overlap"):
        validate_notes(value, "Lead")


def test_audit_rejects_protected_lyrics_backing_and_mixer_changes():
    before = project()
    for mutate in (
        lambda value: value["tracks"][0]["kind"]["clips"][0].update(lyrics=["changed"]),
        lambda value: value["tracks"][1]["mixer"].update(gain_db=-1),
        lambda value: value["tracks"][0]["mixer"]["effects"].append({"id": "reverb"}),
    ):
        after = copy.deepcopy(before)
        mutate(after)
        with pytest.raises(ValueError, match="protected"):
            audit_edit(before, after, "Lead", step())


def test_audit_allows_declared_note_change_and_rejects_budget():
    before = project()
    after = copy.deepcopy(before)
    after["tracks"][0]["kind"]["clips"][0]["notes"][1]["start"] = 500
    result = audit_edit(before, after, "Lead", step())
    assert result["valid"] is True
    assert result["changed_notes"] == 1
    assert result["changes"][0]["fields"]["start"] == {"before": 520, "after": 500}

    over_budget = copy.deepcopy(after)
    over_budget["tracks"][0]["kind"]["clips"][0]["notes"][0]["start"] = 20
    with pytest.raises(ValueError, match="budget"):
        audit_edit(before, over_budget, "Lead", step())


def test_recipe_is_bounded_and_rejects_unknown_fields(tmp_path):
    valid = {
        "schema_version": 2,
        "brief": "Compose a short pop vocal.",
        "focus_track": "Lead",
        "revision_scope": step(),
    }
    path = tmp_path / "recipe.json"
    path.write_text(json.dumps(valid), encoding="utf-8")
    assert recipe(path)["focus_track"] == "Lead"
    invalid = copy.deepcopy(valid)
    invalid["revision_scope"] = step(fields=["lyrics"])
    path.write_text(json.dumps(invalid), encoding="utf-8")
    with pytest.raises(ValueError, match="fields"):
        recipe(path)


def test_score_contract_has_five_axes_and_never_aggregates(tmp_path):
    axes = {
        "Coherence": 3.1,
        "Musicality": 3.2,
        "Memorability": 3.3,
        "Clarity": 3.4,
        "Naturalness": 3.5,
    }
    report = {
        "runtime": {"preprocessing": "fixed", "model": "pinned"},
        "rows": [
            {
                "sha256": "abc",
                "status": "scored",
                "diagnostics": {"admissible": True},
                "scores": axes,
            }
        ],
    }
    extracted = extract_scores(report, "abc")
    delta = compare_scores(
        extracted,
        {
            "scores": {k: v + 0.1 for k, v in axes.items()},
            "conditions": extracted["conditions"],
            "audio_sha256": "def",
        },
    )
    assert set(delta) == set(axes)
    assert "total" not in delta
    bad = copy.deepcopy(report)
    bad["rows"][0]["scores"].pop("Clarity")
    with pytest.raises((KeyError, ValueError)):
        extract_scores(bad, "abc")


def test_extract_scores_rejects_wrong_audio_and_nonfinite_runtime():
    axes = {
        name: 3.0
        for name in (
            "Coherence",
            "Musicality",
            "Memorability",
            "Clarity",
            "Naturalness",
        )
    }
    report = {
        "runtime": {"revision": "pinned"},
        "rows": [
            {
                "sha256": "abc",
                "status": "scored",
                "diagnostics": {"admissible": True},
                "scores": axes,
            }
        ],
    }
    with pytest.raises(ValueError, match="retained mix"):
        extract_scores(report, "different")
    bad = copy.deepcopy(report)
    bad["rows"][0]["scores"]["Naturalness"] = float("nan")
    with pytest.raises(ValueError, match="Invalid SongEval"):
        extract_scores(bad, "abc")
    with pytest.raises(ValueError, match="conditions"):
        compare_scores(
            extract_scores(report, "abc"),
            {"scores": axes, "conditions": {"revision": "changed"}},
        )


def _fake_args(tmp_path: Path) -> SimpleNamespace:
    recipe_path = tmp_path / "recipe.json"
    refine.write_json(
        recipe_path,
        {
            "schema_version": 2,
            "brief": "Compose",
            "focus_track": "Lead",
            "revision_scope": {"fields": ["start"], "max_changed_notes": 1},
        },
    )
    executable = tmp_path / "panel.exe"
    executable.write_bytes(b"panel")
    return SimpleNamespace(
        recipe=recipe_path,
        config=recipe_path,
        output=tmp_path / "run",
        executable=executable,
        auris=executable,
        evaluator_python=executable,
        songeval_config=recipe_path,
        seed_project=None,
        source_suite=None,
        voice=None,
        speaker=None,
        model="fake",
        backend="codex",
        timeout=60,
        auditions=False,
    )


def _patch_fake_pipeline(monkeypatch, candidate, *, fail_render=False):
    calls, evaluated = [], []

    def run_stage(args, root, prompt, seed, *, review=False):
        calls.append((root.name, seed, review))
        folder = root / "agent/song-01"
        folder.mkdir(parents=True)
        value = candidate if review else project()
        snapshot = folder / "project-02.json"
        refine.write_json(snapshot, value)
        refine.write_json(
            folder / "transcript.json",
            [
                {
                    "kind": "assistant",
                    "text": "The answer phrase connects more directly; the breath before the next phrase remains.",
                }
            ],
        )
        return snapshot

    def fake_render(args, snapshot, source, destination, focus_name):
        if fail_render and "01-revision" in str(destination):
            raise RuntimeError("singer failed")
        destination.mkdir(parents=True)
        assets = {}
        modes = ("mix", "foreground", "backing") if args.auditions else ("mix",)
        for mode in modes:
            path = destination / f"{mode}.wav"
            path.write_bytes(mode.encode())
            assets[mode] = {
                "path": str(path),
                "sha256": "same",
                "seconds": 1.0,
                "sample_rate": 48000,
            }
        return {"assets": assets}

    def fake_evaluate(args, root, asset):
        evaluated.append(root.name)
        score = 3.0 if root.name == "00-baseline" else 4.0
        return {
            "scores": {
                axis: score
                for axis in (
                    "Coherence",
                    "Musicality",
                    "Memorability",
                    "Clarity",
                    "Naturalness",
                )
            },
            "conditions": {"revision": "fixed"},
            "audio_sha256": "same",
        }

    monkeypatch.setattr(refine, "run_stage", run_stage)
    monkeypatch.setattr(refine, "render", fake_render)
    monkeypatch.setattr(refine, "evaluate", fake_evaluate)
    return calls, evaluated


def changed_project():
    candidate = copy.deepcopy(project())
    candidate["tracks"][0]["kind"]["clips"][0]["notes"][1]["start"] = 500
    return candidate


def test_one_revision_never_promotes_even_when_all_scores_rise(tmp_path, monkeypatch):
    calls, evaluated = _patch_fake_pipeline(monkeypatch, changed_project())
    args = _fake_args(tmp_path)
    report = refine.run(args)
    assert report["status"] == "completed_review_required"
    assert report["selected_stage"] == "00-baseline"
    assert len(calls) == 2 and calls[1][2] is True
    assert calls[1][1] == Path(report["stages"][0]["project"])
    assert evaluated == ["00-baseline", "01-revision"]
    assert set(report["stages"][1]["score_delta"].values()) == {1.0}
    assert set(report["stages"][1]["render"]["assets"]) == {"mix"}


def test_no_change_skips_second_synthesis_render_and_evaluation(tmp_path, monkeypatch):
    calls, evaluated = _patch_fake_pipeline(monkeypatch, project())
    args = _fake_args(tmp_path)
    report = refine.run(args)
    assert len(calls) == 2
    assert evaluated == ["00-baseline"]
    assert report["status"] == "baseline_retained_no_change"
    assert report["stages"][1]["decision"]["action"] == "keep"
    assert not (args.output / "01-revision/audio").exists()
    assert Path(report["stages"][0]["render"]["assets"]["mix"]["path"]).is_file()


@pytest.mark.parametrize("failure", ["protected_edit", "synthesis"])
def test_failed_revision_returns_baseline_and_preserves_failure(
    tmp_path, monkeypatch, failure
):
    candidate = changed_project()
    if failure == "protected_edit":
        candidate["tracks"][1]["mixer"]["gain_db"] = -1
    _, evaluated = _patch_fake_pipeline(
        monkeypatch, candidate, fail_render=failure == "synthesis"
    )
    args = _fake_args(tmp_path)
    report = refine.run(args)
    assert report["status"] == "baseline_only_revision_failed"
    assert report["selected_stage"] == "00-baseline"
    assert report["stages"][0]["status"] == "baseline_retained"
    assert evaluated == ["00-baseline"]
    saved = refine.load(args.output / "01-revision/stage.json")
    assert saved["status"] == "failed_or_rejected" and saved["error"]
    assert (args.output / "00-baseline/audio/mix.wav").is_file()


def test_initial_failure_does_not_claim_a_usable_baseline(tmp_path, monkeypatch):
    _patch_fake_pipeline(monkeypatch, project())

    def fail(*args, **kwargs):
        raise RuntimeError("backend unavailable")

    monkeypatch.setattr(refine, "run_stage", fail)
    report = refine.run(_fake_args(tmp_path))
    assert report["status"] == "failed"
    assert report["selected_stage"] is None


def test_review_decision_uses_actual_changes_and_limits_to_one_clip(tmp_path):
    transcript = tmp_path / "transcript.json"
    refine.write_json(
        transcript,
        [{"kind": "assistant", "text": "Keep the phrase's existing breath."}],
    )
    assert refine.review_decision(transcript, {"changes": []})["action"] == "keep"
    assert (
        refine.review_decision(transcript, {"changes": [{"clip_id": 7}]})["action"]
        == "revise"
    )
    with pytest.raises(ValueError, match="one clip"):
        refine.review_decision(
            transcript, {"changes": [{"clip_id": 7}, {"clip_id": 8}]}
        )


def test_shared_config_resolves_paths_without_per_run_runtime_flags(tmp_path):
    binary = tmp_path / "binary"
    binary.write_bytes(b"runtime")
    config = tmp_path / "config.json"
    refine.write_json(
        config,
        {
            "schema_version": 1,
            **{
                key: "binary"
                for key in (
                    "executable",
                    "auris",
                    "evaluator_python",
                    "songeval_config",
                )
            },
        },
    )
    args = SimpleNamespace(
        config=config,
        recipe=tmp_path / "brief.json",
        output=tmp_path / "out",
        seed_project=None,
        auditions=False,
    )
    settings = refine.configure(args)
    assert settings.auris == binary
    assert settings.timeout == 900 and settings.voice is None


def test_recipe_defaults_and_rejects_old_multi_candidate_plan(tmp_path):
    path = tmp_path / "brief.json"
    brief = {"schema_version": 2, "brief": "Pop", "focus_track": "Lead"}
    refine.write_json(path, brief)
    assert recipe(path)["revision_scope"]["max_changed_notes"] == 16
    refine.write_json(path, {**brief, "interventions": [step()]})
    with pytest.raises(ValueError, match="predefined"):
        recipe(path)
    refine.write_json(path, {**brief, "revision_scope": {"clip_ids": "7"}})
    with pytest.raises(ValueError, match="clip_ids"):
        recipe(path)


@pytest.mark.parametrize("auditions, count", [(False, 1), (True, 3)])
def test_renderer_defaults_to_one_mix_and_keeps_auditions_in_separate_projects(
    tmp_path, monkeypatch, auditions, count
):
    import numpy as np
    import soundfile as sf

    source = tmp_path / "Source"
    source.mkdir()
    value = project()
    value["tracks"][0]["kind"]["type"] = "instrument"
    snapshot = source / "Source.auris"
    refine.write_json(snapshot, value)
    commands = []

    def render_command(argv, log, timeout):
        commands.append(argv)
        destination = Path(argv[argv.index("-o") + 1])
        sf.write(destination, np.full((48, 2), 0.1), 48000, subtype="FLOAT")

    monkeypatch.setattr(refine, "command", render_command)
    args = SimpleNamespace(auris=Path("auris"), auditions=auditions, timeout=60)
    result = refine.render(args, snapshot, source, tmp_path / "audio", "Lead")
    assert len(commands) == count
    for argv in commands:
        native = Path(argv[2])
        assert len(list(native.parent.glob("*.auris"))) == 1
    assert result["assets"]["mix"]["gain_normalization"] == "none"
    assert refine.load(snapshot) == value


def test_cli_marks_revision_failure_unsuccessful_while_retaining_baseline(
    tmp_path, monkeypatch
):
    candidate = changed_project()
    candidate["tracks"][1]["mixer"]["gain_db"] = -1
    _patch_fake_pipeline(monkeypatch, candidate)
    args = _fake_args(tmp_path)
    monkeypatch.setattr(refine, "configure", lambda _: args)
    monkeypatch.setattr(
        refine.sys,
        "argv",
        [
            "composition_refine.py",
            "--recipe",
            str(args.recipe),
            "--output",
            str(args.output),
        ],
    )
    with pytest.raises(SystemExit) as error:
        refine.main()
    assert error.value.code == 2
    assert refine.load(args.output / "report.json")["selected_stage"] == "00-baseline"


def test_review_delivery_knows_baseline_was_already_evaluated(tmp_path, monkeypatch):
    args = _fake_args(tmp_path)
    stage = tmp_path / "review"
    stage.mkdir()

    def successful_trial(argv, log, timeout):
        output = Path(argv[argv.index("--output") + 1])
        output.mkdir()
        refine.write_json(output / "suite.json", {"trials": [{"transport_pass": True}]})

    monkeypatch.setattr(refine, "command", successful_trial)
    refine.run_stage(args, stage, "Review the melody", None, review=True)
    prompt = refine.load(stage / "manifest.json")["delivery_prompt"]
    assert "baseline has already been synthesized and evaluated" in prompt
    assert "Audio synthesis and evaluation are pending." not in prompt
