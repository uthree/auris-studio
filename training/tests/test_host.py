"""The host as seen from here: frames files, the energy scale, the command line."""

from __future__ import annotations

import json

import numpy as np
import pytest

from auris_singer.host import (
    REPO_ROOT,
    SILENCE,
    Host,
    HostError,
    HostFrames,
    concatenate_frames,
    energy_full_scale,
    frames_from_curves,
)


def test_the_energy_scale_is_read_off_the_hosts_source():
    scale = energy_full_scale()
    assert 0.0 < scale <= 1.0, scale


def test_a_checkout_without_the_host_says_so(tmp_path):
    with pytest.raises(HostError, match="missing"):
        energy_full_scale(tmp_path)


def test_a_source_that_lost_the_constant_is_a_parser_failure_not_a_default(tmp_path):
    fake = tmp_path / "crates/auris-singer/src/score.rs"
    fake.parent.mkdir(parents=True)
    fake.write_text("// pub const ENERGY_FULL_SCALE: f32 = 0.5;\n", encoding="utf-8")
    with pytest.raises(HostError, match="not found"):
        energy_full_scale(tmp_path)


def test_curves_lay_out_as_the_frames_the_host_reads():
    phonemes = ["<sil>", "k", "a", "<sil>"]
    durations = [2, 1, 3, 2]
    f0 = [0, 0, 220, 220, 221, 222, 0, 0]
    energy = [0, 0, 0.1, 0.2, 0.2, 0.1, 0, 0]
    frames = frames_from_curves(phonemes, durations, f0, energy, 0.01, energy_scale=0.25)

    assert frames.inventory == [SILENCE, "k", "a"], "<sil> is the frames' own silence, first"
    assert frames.phonemes == [0, 0, 1, 2, 2, 2, 0, 0]
    assert frames.f0_hz == pytest.approx(f0)
    # The host multiplies by its scale on the way in, so what reaches the model is the curve.
    assert np.asarray(frames.energy) * 0.25 == pytest.approx(energy)
    assert frames.seconds == pytest.approx(0.08)

    data = frames.to_dict()
    assert set(data) == {"hop_seconds", "inventory", "phonemes", "f0_hz", "energy"}, (
        "these are auris_vocal::SingerFrames's serde field names"
    )


def test_a_loud_frame_is_not_clamped():
    frames = frames_from_curves(["a"], [1], [220.0], [0.4], 0.01, energy_scale=0.25)
    assert frames.energy[0] == pytest.approx(1.6), "the host does not clamp, so neither do we"


def test_mismatched_curves_are_refused():
    with pytest.raises(ValueError, match="durations"):
        frames_from_curves(["a", "i"], [1], [220.0], [0.1], 0.01, 0.25)
    with pytest.raises(ValueError, match="sum to"):
        frames_from_curves(["a"], [2], [220.0], [0.1], 0.01, 0.25)
    with pytest.raises(ValueError, match="positive"):
        frames_from_curves(["a"], [1], [220.0], [0.1], 0.01, 0.0)


def test_frames_refuse_a_wrong_inventory_or_ragged_sequences():
    with pytest.raises(ValueError, match="inventory\\[0\\]"):
        HostFrames(0.01, ["a"], [0], [220.0], [0.5])
    with pytest.raises(ValueError, match="one length"):
        HostFrames(0.01, [SILENCE, "a"], [1, 1], [220.0], [0.5, 0.5])
    with pytest.raises(ValueError, match="outside"):
        HostFrames(0.01, [SILENCE], [3], [0.0], [0.0])


def test_frames_round_trip_through_the_file(tmp_path):
    frames = frames_from_curves(["<sil>", "a"], [1, 2], [0, 330, 330], [0, 0.1, 0.1], 0.01, 0.25)
    path = frames.write(tmp_path / "deep/melody.frames.json")
    text = path.read_text(encoding="utf-8")
    assert "\n" not in text.strip(), "compact, one machine to another"
    assert HostFrames.read(path) == frames
    assert json.loads(text)["inventory"][0] == SILENCE


def test_a_song_is_the_parts_with_silence_between_and_spans_to_cut_it_by():
    a = frames_from_curves(["a"], [3], [220.0] * 3, [0.1] * 3, 0.01, 0.25)
    b = frames_from_curves(["i", "k"], [2, 1], [330.0] * 3, [0.1] * 3, 0.01, 0.25)
    song, spans = concatenate_frames([a, b], gap_frames=2)
    assert spans == [(0, 3), (5, 8)]
    assert len(song) == 8
    assert song.inventory == [SILENCE, "a", "i", "k"]
    assert song.tokens() == ["a", "a", "a", SILENCE, SILENCE, "i", "i", "k"]
    assert song.f0_hz[3:5] == [0.0, 0.0]
    assert song.energy[3:5] == [0.0, 0.0]
    for (start, end), part in zip(spans, (a, b)):
        assert song.f0_hz[start:end] == part.f0_hz
        assert song.tokens()[start:end] == part.tokens()


def test_a_song_needs_parts_on_one_clock():
    a = frames_from_curves(["a"], [1], [220.0], [0.1], 0.01, 0.25)
    b = frames_from_curves(["a"], [1], [220.0], [0.1], 0.02, 0.25)
    with pytest.raises(ValueError, match="same hop"):
        concatenate_frames([a, b], 1)
    with pytest.raises(ValueError, match="nothing"):
        concatenate_frames([], 1)


def test_the_host_is_the_named_binary_before_it_is_cargo(monkeypatch, tmp_path):
    monkeypatch.setenv("AURIS_CLI", "/somewhere/auris")
    host = Host.find(tmp_path)
    assert host.command == ["/somewhere/auris"]

    monkeypatch.delenv("AURIS_CLI")
    monkeypatch.setattr("shutil.which", lambda name: None)
    with pytest.raises(HostError, match="AURIS_CLI"):
        Host.find(tmp_path)
    assert not Host.available(tmp_path)


def test_without_a_manifest_there_is_nothing_for_cargo_to_run(monkeypatch, tmp_path):
    monkeypatch.delenv("AURIS_CLI", raising=False)
    monkeypatch.setattr("shutil.which", lambda name: "/usr/bin/cargo")
    with pytest.raises(HostError):
        Host.find(tmp_path)
    (tmp_path / "Cargo.toml").write_text("[workspace]\n")
    host = Host.find(tmp_path, release=True)
    assert host.command[:2] == ["cargo", "run"] and "--release" in host.command
    assert host.command[-1] == "--", "the host's own options follow the separator"


def test_a_failed_command_carries_what_the_host_said(tmp_path):
    script = tmp_path / "auris"
    script.write_text("#!/bin/sh\necho 'auris: nope' >&2\nexit 1\n")
    script.chmod(0o755)
    host = Host(command=[str(script)], cwd=tmp_path)
    with pytest.raises(HostError, match="nope"):
        host.run("sing-frames", "x.json")


def test_the_repository_root_is_where_the_host_lives():
    assert (REPO_ROOT / "Cargo.toml").is_file()
    assert (REPO_ROOT / "crates/auris-singer/src/score.rs").is_file()


def test_paths_reach_the_host_absolute_whatever_they_were_given_as(tmp_path, monkeypatch):
    """The host runs from the repository root, and the trainer from ``training/``."""
    seen: list[list[str]] = []

    class Recording(Host):
        def run(self, *args: str) -> str:
            seen.append(list(args))
            report = tmp_path / "out" / "take.report.json"
            report.parent.mkdir(parents=True, exist_ok=True)
            report.write_text('{"seconds": 1.0}')
            return ""

    monkeypatch.chdir(tmp_path)
    host = Recording(command=["auris"], cwd=tmp_path)
    facts = host.sing_frames("in.json", "v.onnx", "out/take.wav")
    assert facts["seconds"] == 1.0 and "wall_seconds" in facts
    for arg in seen[0]:
        if arg.endswith((".json", ".onnx", ".wav")):
            assert arg.startswith(str(tmp_path)), arg
    assert "--speaker" not in seen[0], "no speaker asked for, none named: the model's first sings"
    host.sing_frames("in.json", "v.onnx", "out/take.wav", speaker="bob")
    assert seen[-1][-2:] == ["--speaker", "bob"]


@pytest.mark.parametrize("nested", [True, False])
def test_a_composed_project_is_found_wherever_the_folder_rule_put_it(tmp_path, nested):
    """One folder, one project: `-o dir/x.auris` lands in `dir/x/x.auris`, unless `dir` is
    already the folder named `x`, when it lands in `dir/x.auris` itself."""
    script = tmp_path / "auris"
    where = "$(dirname \"$4\")/$(basename \"$4\" .auris)/$(basename \"$4\")" if nested else "$4"
    script.write_text(f'#!/bin/sh\nmkdir -p "$(dirname {where})" && echo x > "{where}"\n')
    script.chmod(0o755)
    host = Host(command=[str(script)], cwd=tmp_path)
    project = host.compose(tmp_path / "s.asong", tmp_path / "out" / "song.auris")
    expected = tmp_path / "out" / "song" / "song.auris" if nested else tmp_path / "out" / "song.auris"
    assert project == expected and project.is_file()
