"""Tests for the ONNX export wrapper."""

from __future__ import annotations

import json
from pathlib import Path

import pytest
import torch

from auris_singer.export import OnnxSingerWrapper
from auris_singer.model import AurisSinger
from auris_singer.text import DEFAULT_PHONEME_TABLE

HOP = 480


def valid_metadata() -> dict:
    """Metadata whose graph-indexed axes match the shared tiny model."""
    return {
        "symbols": list(DEFAULT_PHONEME_TABLE.symbols),
        "speaker_to_id": {"x": 0, "y": 1},
    }


@pytest.fixture
def model(tiny_model_config):
    torch.manual_seed(0)
    return AurisSinger(**tiny_model_config).eval()


def wrapper_inputs(model: AurisSinger, batch: int = 2, s: int = 6, frames_per: int = 5):
    """A padded batch plus the noise tensors the wrapper wants."""
    torch.manual_seed(1)
    lengths = torch.tensor([s, s - 2][:batch], dtype=torch.long)
    phonemes = torch.randint(1, model.n_vocab, (batch, s))
    durations = torch.full((batch, s), frames_per, dtype=torch.long)
    t = s * frames_per  # the longer row's frame count
    f0 = torch.full((batch, t), 220.0)
    f0[:, :frames_per] = 0.0  # a leading unvoiced stretch
    voiced = (f0 > 0).float()
    energy = torch.full((batch, t), 0.1)
    speaker_ids = torch.arange(batch, dtype=torch.long) % model.n_speakers
    return {
        "phonemes": phonemes,
        "phoneme_lengths": lengths,
        "durations": durations,
        "f0": f0,
        "energy": energy,
        "voiced": voiced,
        "speaker_ids": speaker_ids,
        "noise_scale": torch.tensor(0.667),
        "z_noise": torch.zeros(batch, model.inter_channels, t),
        "source_noise": -torch.ones(batch, 1, t * HOP),
    }


def test_wrapper_matches_infer_when_the_noise_is_pinned(model, monkeypatch):
    """With every random draw forced to a constant, the two paths are the same
    computation: ``infer`` draws zeros for the prior (randn) and ``-1`` for the
    excitation (rand*2-1), and the wrapper is fed exactly those values."""
    inputs = wrapper_inputs(model)
    with torch.no_grad():
        ours, _ = OnnxSingerWrapper(model)(**inputs)

    monkeypatch.setattr(torch, "randn_like", torch.zeros_like)
    monkeypatch.setattr(torch, "rand_like", torch.zeros_like)
    theirs = model.infer(
        phonemes=inputs["phonemes"],
        phoneme_lengths=inputs["phoneme_lengths"],
        durations=inputs["durations"],
        f0=inputs["f0"],
        energy=inputs["energy"],
        voiced=inputs["voiced"],
        speaker_ids=inputs["speaker_ids"],
        noise_scale=0.667,
    )

    assert ours.shape == theirs.shape == (2, 1, inputs["f0"].size(1) * HOP)
    assert torch.allclose(ours, theirs, atol=1e-6)


def test_wrapper_is_deterministic(model):
    inputs = wrapper_inputs(model, batch=1)
    wrapper = OnnxSingerWrapper(model)
    with torch.no_grad():
        wav_a, source_a = wrapper(**inputs)
        wav_b, source_b = wrapper(**inputs)
    assert torch.equal(wav_a, wav_b)
    assert torch.equal(source_a, source_b)


def test_portrait_roundtrips_and_rejects_the_wrong_things(tmp_path, monkeypatch):
    import base64

    from auris_singer.export import load_portrait

    # A 1x1 PNG: enough to prove bytes survive the base64 round trip.
    png = base64.b64decode(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGNgYGBgAAAABQAB"
        "h6FO1AAAAABJRU5ErkJggg=="
    )
    (tmp_path / "portrait.png").write_bytes(png)
    card = load_portrait(tmp_path / "portrait.png")
    assert card["mime"] == "image/png"
    assert base64.b64decode(card["base64"]) == png

    (tmp_path / "portrait.bmp").write_bytes(png)
    with pytest.raises(ValueError, match="unsupported portrait type"):
        load_portrait(tmp_path / "portrait.bmp")

    huge = tmp_path / "huge.png"
    with huge.open("wb") as stream:
        stream.seek(8 * 1024 * 1024)
        stream.write(b"\0")
    read_bytes = Path.read_bytes

    def refuse_large_read(path):
        if path == huge:
            pytest.fail("an oversized portrait was read before its size guard")
        return read_bytes(path)

    monkeypatch.setattr(Path, "read_bytes", refuse_large_read)
    with pytest.raises(ValueError, match="keep it under"):
        load_portrait(huge)


def test_onnx_export_runs_and_matches_pytorch(model, tmp_path):
    pytest.importorskip("onnxruntime")
    onnx = pytest.importorskip("onnx")

    from auris_singer.export import METADATA_KEY, export_onnx, read_export_metadata

    path = tmp_path / "tiny.onnx"
    errors = export_onnx(
        model,
        path,
        metadata=valid_metadata(),
        voice={"name": "Test Singer", "description": "a demo voice", "credits": ["someone"]},
        phoneme_durations={
            "unit": "seconds",
            "measured_from": "unit test",
            "speakers": {"x": {"default": 0.06, "seconds": {"s": 0.104}, "counts": {"s": 1907}}},
        },
        verify=True,
    )

    # Verification runs against the staged candidate before the public pair is
    # replaced (and at sizes the trace never saw).
    assert errors is not None
    assert errors["unvoiced_max_diff"] < 1e-4

    # The metadata rides along both inside the file and as a sidecar.
    props = {entry.key: entry.value for entry in onnx.load(str(path)).metadata_props}
    stored = json.loads(props[METADATA_KEY])
    assert stored["symbols"] == list(DEFAULT_PHONEME_TABLE.symbols)
    assert stored["sample_rate"] == 48_000
    assert stored["hop_length"] == HOP
    assert stored["audio"]["n_fft"] == 2 * (model.spec_channels - 1)
    assert stored["audio"]["win_length"] == 2 * (model.spec_channels - 1)
    assert stored["inter_channels"] == model.inter_channels
    assert stored["voice"]["name"] == "Test Singer"
    assert stored["phoneme_durations"]["speakers"]["x"]["seconds"] == {"s": 0.104}
    assert stored["phoneme_durations"]["speakers"]["x"]["default"] == 0.06
    sidecar = json.loads((tmp_path / "tiny.json").read_text(encoding="utf-8"))
    assert sidecar == stored
    assert read_export_metadata(path) == stored

    # One self-contained file: the exporter's external-data sidecar must be
    # inlined and cleaned up, not left as a stale duplicate.
    assert not (tmp_path / "tiny.onnx.data").exists()


def test_phoneme_durations_outside_the_symbol_table_are_refused(model, tmp_path):
    """A table measured against a different phoneme set is a mistake, not a
    thing to ship silently -- and it is caught before the expensive trace."""
    from auris_singer.export import export_onnx

    with pytest.raises(ValueError, match="outside the model's table"):
        export_onnx(
            model,
            tmp_path / "tiny.onnx",
            metadata=valid_metadata(),
            phoneme_durations={
                "speakers": {"x": {"default": 0.06, "seconds": {"not-a-model-symbol": 0.104}}}
            },
        )
    assert not (tmp_path / "tiny.onnx").exists(), "refused before tracing anything"


@pytest.mark.parametrize(
    ("case", "message"),
    [
        ("extra-symbol", "symbols but the model has"),
        ("duplicate-symbol", "symbols must be unique"),
        ("empty-symbol", "non-empty strings"),
        ("wrong-padding", "symbol 0"),
        ("missing-special", "missing reserved symbols"),
        ("missing-speaker", "every model speaker exactly once"),
        ("duplicate-speaker-id", "every model speaker exactly once"),
        ("out-of-range-speaker-id", "every model speaker exactly once"),
        ("boolean-speaker-id", "speaker ids must be integers"),
        ("empty-speaker-name", "speaker names must be non-empty"),
    ],
)
def test_export_refuses_metadata_that_cannot_index_the_model(
    model, tmp_path, monkeypatch, case, message
):
    from auris_singer.export import export_onnx
    from auris_singer.text import PAU

    metadata = valid_metadata()
    if case == "extra-symbol":
        metadata["symbols"].append("extra")
    elif case == "duplicate-symbol":
        metadata["symbols"][-1] = metadata["symbols"][0]
    elif case == "empty-symbol":
        metadata["symbols"][-1] = ""
    elif case == "wrong-padding":
        metadata["symbols"][0], metadata["symbols"][-1] = (
            metadata["symbols"][-1],
            metadata["symbols"][0],
        )
    elif case == "missing-special":
        metadata["symbols"][metadata["symbols"].index(PAU)] = "custom-pause"
    elif case == "missing-speaker":
        metadata["speaker_to_id"] = {"x": 0}
    elif case == "duplicate-speaker-id":
        metadata["speaker_to_id"] = {"x": 0, "y": 0}
    elif case == "out-of-range-speaker-id":
        metadata["speaker_to_id"] = {"x": 0, "y": 2}
    elif case == "boolean-speaker-id":
        metadata["speaker_to_id"] = {"x": 0, "y": True}
    elif case == "empty-speaker-name":
        metadata["speaker_to_id"] = {"": 0, "y": 1}

    monkeypatch.setattr(
        model,
        "remove_weight_norm",
        lambda: pytest.fail("axis metadata must be rejected before mutating the model"),
    )
    with pytest.raises(ValueError, match=message):
        export_onnx(model, tmp_path / "tiny.onnx", metadata=metadata)
    assert not (tmp_path / "tiny.onnx").exists()


@pytest.mark.parametrize(
    ("voice", "durations", "levels", "message"),
    [
        ({"name": 7}, None, None, "voice card name must be a string"),
        ({"credits": "one person"}, None, None, "credits must be an array of strings"),
        ({"credits": ["one", 2]}, None, None, "credits must be an array of strings"),
        (None, {"unit": "frames", "speakers": {}}, None, "unit must be 'seconds'"),
        (
            None,
            {"speakers": {"x": {"default": 0, "seconds": {}}}},
            None,
            "must be a positive finite number",
        ),
        (
            None,
            {"speakers": {"x": {"default": 0.06, "seconds": {"a": "bad"}}}},
            None,
            "must contain positive finite numbers",
        ),
        (None, None, {"unit": "linear", "speakers": {}}, "unit must be 'db'"),
        (
            None,
            None,
            {"speakers": {"x": {"default": None, "db": {}}}},
            "must be a finite number",
        ),
        (
            None,
            None,
            {"speakers": {"x": {"default": -12.0, "db": {"a": []}}}},
            "must contain finite numbers",
        ),
        (
            None,
            None,
            None,
            "credits must be an array of strings",
        ),
    ],
)
def test_export_refuses_host_unreadable_ancillary_metadata(
    model, tmp_path, monkeypatch, voice, durations, levels, message
):
    from auris_singer.export import export_onnx

    monkeypatch.setattr(
        model,
        "remove_weight_norm",
        lambda: pytest.fail("ancillary metadata must be rejected before mutating the model"),
    )
    metadata = valid_metadata()
    if voice is None and durations is None and levels is None and message.startswith("credits"):
        metadata["voice"] = {"credits": "metadata bypass"}
    with pytest.raises(ValueError, match=message):
        export_onnx(
            model,
            tmp_path / "tiny.onnx",
            metadata=metadata,
            voice=voice,
            phoneme_durations=durations,
            phoneme_levels=levels,
        )


@pytest.mark.parametrize(
    "metadata",
    [
        {"sample_rate": 44_100},
        {"hop_length": 240},
        {"audio": {"sample_rate": 44_100, "hop_length": HOP}},
        {"audio": {"sample_rate": 48_000, "hop_length": 240}},
    ],
)
def test_export_refuses_metadata_with_another_audio_clock(model, tmp_path, metadata):
    from auris_singer.export import export_onnx

    with pytest.raises(ValueError, match="disagrees with the model"):
        export_onnx(model, tmp_path / "tiny.onnx", metadata=metadata)
    assert not (tmp_path / "tiny.onnx").exists()


@pytest.mark.parametrize(
    ("audio", "message"),
    [
        ({"n_fft": 1024}, "disagrees with model spec_channels"),
        ({"n_fft": 2048, "win_length": 4096}, "win_length=4096 exceeds"),
    ],
)
def test_export_refuses_metadata_with_another_spectral_contract(
    model, tmp_path, monkeypatch, audio, message
):
    from auris_singer.export import export_onnx

    metadata = valid_metadata()
    metadata["audio"] = audio
    monkeypatch.setattr(
        model,
        "remove_weight_norm",
        lambda: pytest.fail("spectral metadata must be rejected before mutating the model"),
    )
    with pytest.raises(ValueError, match=message):
        export_onnx(model, tmp_path / "tiny.onnx", metadata=metadata)


def test_export_refuses_more_speakers_than_the_host_can_address(model, tmp_path, monkeypatch):
    from auris_singer.export import export_onnx

    monkeypatch.setattr(model, "n_speakers", 4_097)
    monkeypatch.setattr(
        model,
        "remove_weight_norm",
        lambda: pytest.fail("host limits must be checked before mutating the model"),
    )
    with pytest.raises(ValueError, match="host limit of 4096"):
        export_onnx(model, tmp_path / "tiny.onnx", metadata=valid_metadata())


@pytest.mark.parametrize(
    ("field", "value"),
    [("sample_rate", 0), ("hop_length", 1.5), ("sample_rate", True)],
)
def test_export_refuses_an_invalid_model_audio_clock(model, tmp_path, field, value):
    from auris_singer.export import export_onnx

    setattr(model, field, value)
    with pytest.raises(ValueError, match=f"model {field} must be a positive integer"):
        export_onnx(model, tmp_path / "tiny.onnx")
    assert not (tmp_path / "tiny.onnx").exists()


@pytest.mark.parametrize("bad_number", [float("nan"), float("inf"), -float("inf")])
def test_export_refuses_non_finite_json_before_mutating_the_model(
    model, tmp_path, monkeypatch, bad_number
):
    from auris_singer.export import export_onnx

    monkeypatch.setattr(
        model,
        "remove_weight_norm",
        lambda: pytest.fail("metadata must be rejected before the one-way model mutation"),
    )

    with pytest.raises(ValueError, match="finite JSON"):
        export_onnx(model, tmp_path / "tiny.onnx", voice={"invalid": bad_number})
    assert not (tmp_path / "tiny.onnx").exists()


@pytest.mark.parametrize("failure_type", [OSError, KeyboardInterrupt])
def test_transactional_export_restores_both_old_files_when_second_replace_fails(
    tmp_path, monkeypatch, failure_type
):
    from auris_singer import export as export_module

    target = tmp_path / "voice.onnx"
    sidecar = tmp_path / "voice.json"
    candidate = tmp_path / "candidate.onnx"
    candidate_sidecar = tmp_path / "candidate.json"
    target.write_bytes(b"old model")
    sidecar.write_bytes(b"old metadata")
    candidate.write_bytes(b"new model")
    candidate_sidecar.write_bytes(b"new metadata")

    replace = export_module._replace_file
    calls = 0

    def fail_second(source, destination):
        nonlocal calls
        calls += 1
        if calls == 2:
            # Model the dangerous interruption point: the kernel already
            # replaced the sidecar, but Python has not recorded success yet.
            replace(source, destination)
            raise failure_type("injected sidecar publication failure")
        return replace(source, destination)

    monkeypatch.setattr(export_module, "_replace_file", fail_second)
    with pytest.raises(failure_type, match="injected"):
        export_module._commit_export_pair(candidate, candidate_sidecar, target, sidecar)

    assert target.read_bytes() == b"old model"
    assert sidecar.read_bytes() == b"old metadata"


def test_transactional_export_replaces_model_then_sidecar(tmp_path, monkeypatch):
    from auris_singer import export as export_module

    target = tmp_path / "voice.onnx"
    sidecar = tmp_path / "voice.json"
    candidate = tmp_path / "candidate.onnx"
    candidate_sidecar = tmp_path / "candidate.json"
    target.write_bytes(b"old model")
    sidecar.write_bytes(b"old metadata")
    candidate.write_bytes(b"new model")
    candidate_sidecar.write_bytes(b"new metadata")

    replace = export_module._replace_file
    destinations = []

    def observe(source, destination):
        destinations.append(Path(destination).name)
        return replace(source, destination)

    monkeypatch.setattr(export_module, "_replace_file", observe)
    export_module._commit_export_pair(candidate, candidate_sidecar, target, sidecar)

    assert destinations == ["voice.onnx", "voice.json"]
    assert target.read_bytes() == b"new model"
    assert sidecar.read_bytes() == b"new metadata"


def test_transactional_export_flushes_candidates_before_public_names(tmp_path, monkeypatch):
    from auris_singer import export as export_module

    staging = tmp_path / "staging"
    public = tmp_path / "public"
    staging.mkdir()
    public.mkdir()
    candidate = staging / "voice.onnx"
    candidate_sidecar = staging / "voice.json"
    target = public / "voice.onnx"
    sidecar = public / "voice.json"
    candidate.write_bytes(b"new model")
    candidate_sidecar.write_bytes(b"new metadata")
    events = []
    replace = export_module._replace_file

    monkeypatch.setattr(
        export_module,
        "fsync_file",
        lambda path: events.append(f"sync file {Path(path).name}"),
    )
    monkeypatch.setattr(
        export_module,
        "fsync_directory",
        lambda path: events.append(f"sync dir {Path(path).name}"),
    )

    def observe_replace(source, destination):
        events.append(f"replace {Path(destination).name}")
        return replace(source, destination)

    monkeypatch.setattr(export_module, "_replace_file", observe_replace)
    export_module._commit_export_pair(candidate, candidate_sidecar, target, sidecar)

    assert events == [
        "sync file voice.onnx",
        "sync file voice.json",
        "sync dir staging",
        "replace voice.onnx",
        "replace voice.json",
        "sync dir public",
    ]


def test_rollback_copy_is_durable_before_publication(tmp_path, monkeypatch):
    from auris_singer import export as export_module

    source = tmp_path / "voice.onnx"
    rollback = tmp_path / "rollback"
    source.write_bytes(b"old model")
    rollback.mkdir()
    events = []

    monkeypatch.setattr(
        export_module.os,
        "link",
        lambda _source, _destination: (_ for _ in ()).throw(OSError("no hard links")),
    )
    monkeypatch.setattr(
        export_module,
        "fsync_file",
        lambda path: events.append(("file", Path(path))),
    )
    monkeypatch.setattr(
        export_module,
        "fsync_directory",
        lambda path: events.append(("directory", Path(path))),
    )

    backup = export_module._backup_file(source, rollback)

    assert backup == rollback / source.name
    assert backup.read_bytes() == b"old model"
    assert events == [("file", backup), ("directory", rollback)]


def test_rollback_hard_link_is_durable_before_publication(tmp_path, monkeypatch):
    from auris_singer import export as export_module

    source = tmp_path / "voice.onnx"
    rollback = tmp_path / "rollback"
    source.write_bytes(b"old model")
    rollback.mkdir()
    events = []
    monkeypatch.setattr(
        export_module,
        "fsync_file",
        lambda path: events.append(("file", Path(path))),
    )
    monkeypatch.setattr(
        export_module,
        "fsync_directory",
        lambda path: events.append(("directory", Path(path))),
    )

    backup = export_module._backup_file(source, rollback)

    assert backup == rollback / source.name
    assert backup.read_bytes() == b"old model"
    assert events == [("directory", rollback)]


def test_committed_pair_is_not_reported_failed_when_rollback_cleanup_is_locked(
    tmp_path, monkeypatch, caplog
):
    from auris_singer import export as export_module

    candidate = tmp_path / "candidate.onnx"
    candidate_sidecar = tmp_path / "candidate.json"
    target = tmp_path / "voice.onnx"
    sidecar = tmp_path / "voice.json"
    candidate.write_bytes(b"new model")
    candidate_sidecar.write_bytes(b"new metadata")

    monkeypatch.setattr(
        export_module.shutil,
        "rmtree",
        lambda _path: (_ for _ in ()).throw(OSError("directory is locked")),
    )

    export_module._commit_export_pair(candidate, candidate_sidecar, target, sidecar)

    assert target.read_bytes() == b"new model"
    assert sidecar.read_bytes() == b"new metadata"
    assert "could not remove export rollback directory" in caplog.text


def test_committed_export_is_not_reported_failed_when_stale_data_is_locked(
    model, tmp_path, monkeypatch, caplog
):
    from auris_singer import export as export_module

    target = tmp_path / "voice.onnx"
    stale = Path(str(target) + ".data")
    stale.mkdir()
    monkeypatch.setattr(model, "remove_weight_norm", lambda: None)
    monkeypatch.setattr(
        export_module,
        "_export_candidate",
        lambda _model, path, _metadata, _opset: path.write_bytes(b"self-contained model"),
    )

    result = export_module.export_onnx(model, target, metadata=valid_metadata())

    assert result is None
    assert target.read_bytes() == b"self-contained model"
    assert target.with_suffix(".json").is_file()
    assert stale.is_dir()
    assert "could not remove stale ONNX external data" in caplog.text


def test_committed_export_is_not_reported_failed_when_stale_cleanup_is_interrupted(
    model, tmp_path, monkeypatch, caplog
):
    from auris_singer import export as export_module

    target = tmp_path / "voice.onnx"
    stale = Path(str(target) + ".data")
    stale.write_bytes(b"obsolete")
    monkeypatch.setattr(model, "remove_weight_norm", lambda: None)
    monkeypatch.setattr(
        export_module,
        "_export_candidate",
        lambda _model, path, _metadata, _opset: path.write_bytes(b"self-contained model"),
    )
    unlink = Path.unlink

    def interrupt_stale_cleanup(path, *args, **kwargs):
        if path == stale:
            raise KeyboardInterrupt("after pair commit")
        return unlink(path, *args, **kwargs)

    monkeypatch.setattr(Path, "unlink", interrupt_stale_cleanup)

    result = export_module.export_onnx(model, target, metadata=valid_metadata())

    assert result is None
    assert target.read_bytes() == b"self-contained model"
    assert target.with_suffix(".json").is_file()
    assert stale.read_bytes() == b"obsolete"
    assert "could not remove stale ONNX external data" in caplog.text


def test_metadata_reader_rejects_a_mixed_export_generation(tmp_path, monkeypatch):
    from auris_singer import export as export_module

    target = tmp_path / "voice.onnx"
    target.write_bytes(b"stand-in model")
    target.with_suffix(".json").write_text(
        json.dumps({"export_generation": "sidecar-generation"}),
        encoding="utf-8",
    )
    monkeypatch.setattr(
        export_module,
        "_read_embedded_metadata",
        lambda _path: {"export_generation": "model-generation"},
    )

    with pytest.raises(ValueError, match="export_generation mismatch"):
        export_module.read_export_metadata(target)


def test_transactional_export_reports_every_rollback_failure(tmp_path, monkeypatch):
    from auris_singer import export as export_module

    target = tmp_path / "voice.onnx"
    sidecar = tmp_path / "voice.json"
    candidate = tmp_path / "candidate.onnx"
    candidate_sidecar = tmp_path / "candidate.json"
    target.write_bytes(b"old model")
    sidecar.write_bytes(b"old metadata")
    candidate.write_bytes(b"new model")
    candidate_sidecar.write_bytes(b"new metadata")

    replace = export_module._replace_file
    calls = 0

    def fail_publication_and_rollbacks(source, destination):
        nonlocal calls
        calls += 1
        if calls <= 2:
            replace(source, destination)
            if calls == 2:
                raise OSError("publication interrupted")
            return None
        raise OSError(f"rollback {Path(destination).name} failed")

    monkeypatch.setattr(export_module, "_replace_file", fail_publication_and_rollbacks)
    with pytest.raises(RuntimeError) as caught:
        export_module._commit_export_pair(candidate, candidate_sidecar, target, sidecar)

    message = str(caught.value)
    assert "voice.json" in message and "voice.onnx" in message
    assert message.count("rollback") >= 2
    assert isinstance(caught.value.__cause__, OSError)
    assert str(caught.value.__cause__) == "publication interrupted"


def test_transactional_export_validates_before_replacing_either_file(model, tmp_path):
    from auris_singer import export as export_module

    target = tmp_path / "voice.onnx"
    sidecar = tmp_path / "voice.json"
    candidate = tmp_path / "candidate.onnx"
    candidate_sidecar = tmp_path / "candidate.json"
    target.write_bytes(b"old model")
    sidecar.write_bytes(b"old metadata")
    candidate.write_bytes(b"bad candidate")
    candidate_sidecar.write_bytes(b"new metadata")

    def reject(_model, _path):
        raise ValueError("injected validation failure")

    with pytest.raises(ValueError, match="injected validation"):
        export_module._publish_export_pair(model, candidate, candidate_sidecar, target, reject)

    assert target.read_bytes() == b"old model"
    assert sidecar.read_bytes() == b"old metadata"


@pytest.mark.parametrize(
    ("broken", "message"),
    [
        ("nan-wav", "non-finite"),
        ("zero-source", "excitation has 0 impulses"),
        ("one-source", "excitation has 1 impulses"),
    ],
)
def test_verifier_fails_closed_on_non_finite_or_missing_excitation(
    model, monkeypatch, broken, message
):
    onnxruntime = pytest.importorskip("onnxruntime")
    from auris_singer.export import OnnxSingerWrapper, verify_onnx

    wrapper = OnnxSingerWrapper(model)

    class BrokenSession:
        def __init__(self, *_args, **_kwargs):
            pass

        def run(self, _outputs, feed):
            inputs = {name: torch.from_numpy(value) for name, value in feed.items()}
            with torch.no_grad():
                wav, source = wrapper(**inputs)
                if broken == "nan-wav":
                    wav = torch.full_like(wav, float("nan"))
                elif bool((inputs["f0"] > 0.0).any()):
                    source = torch.zeros_like(source)
                    if broken == "one-source":
                        source[..., 0] = 1.0
                    z, f0, energy, voiced, g = wrapper.latent(
                        **{name: value for name, value in inputs.items() if name != "source_noise"}
                    )
                    wav, _ = model.generator(z, f0, energy, voiced, g=g, source=source)
            return wav.numpy(), source.numpy()

    monkeypatch.setattr(onnxruntime, "InferenceSession", BrokenSession)

    with pytest.raises(ValueError, match=message):
        verify_onnx(model, "unused.onnx")


def test_phoneme_durations_are_optional(model, tmp_path):
    """Without a table the metadata simply has no such key, and a consumer
    falls back to its own default."""
    pytest.importorskip("onnxruntime")
    pytest.importorskip("onnx")
    import json

    from auris_singer.export import export_onnx

    export_onnx(model, tmp_path / "tiny.onnx", metadata=valid_metadata())
    sidecar = json.loads((tmp_path / "tiny.json").read_text(encoding="utf-8"))
    assert "phoneme_durations" not in sidecar


def test_the_graph_avoids_the_constructs_directml_rejects(model, tmp_path):
    """Two ONNX spellings run on CPU and CUDA but fail on onnxruntime's
    DirectML provider, which is what an AMD GPU uses on Windows:

    * any ``Reshape`` carrying ``allowzero=1`` — Torch emits it for views whose
      shapes contain symbolic dimensions;
    * ``ConvTranspose`` carrying an ``output_padding`` attribute at all, even
      ``[0]``.

    Both fail with a bare "the parameter is incorrect", so guard the shape of
    the graph here rather than waiting for a bug report from a GPU nobody in
    CI has.
    """
    onnx = pytest.importorskip("onnx")

    from auris_singer.export import export_onnx

    path = tmp_path / "tiny.onnx"
    export_onnx(model, path, metadata=valid_metadata())
    graph = onnx.load(str(path)).graph

    for node in graph.node:
        attrs = {a.name: a for a in node.attribute}
        assert (
            node.op_type != "Reshape"
            or attrs.get("allowzero", None) is None
            or attrs["allowzero"].i != 1
        ), f"{node.name}: DirectML rejects Reshape allowzero=1"
        assert node.op_type != "ConvTranspose" or "output_padding" not in attrs, (
            f"{node.name}: DirectML rejects any output_padding on a 1-D "
            "ConvTranspose; export_onnx should have folded it into pads"
        )


def test_directml_guard_rejects_an_unknown_allowzero_shape():
    onnx = pytest.importorskip("onnx")
    from auris_singer import export as export_module

    node = onnx.helper.make_node(
        "Reshape", ["input", "shape_from_runtime"], ["output"], allowzero=1
    )
    graph = type("Graph", (), {"node": [node]})()
    proto = type("Model", (), {"graph": graph})()

    with pytest.raises(ValueError, match="allowzero=1"):
        export_module._reject_directml_incompatible_graph(proto)


def _symbolic_allowzero_model(driver_batch: str):
    """Build a reshape whose target starts with two runtime shape values."""
    onnx = pytest.importorskip("onnx")
    data = onnx.helper.make_tensor_value_info(
        "data", onnx.TensorProto.FLOAT, ["batch", "frames", 6]
    )
    driver = onnx.helper.make_tensor_value_info(
        "driver", onnx.TensorProto.FLOAT, [driver_batch, "frames"]
    )
    output = onnx.helper.make_tensor_value_info(
        "output", onnx.TensorProto.FLOAT, ["batch", "frames", 2, 3]
    )
    tail = onnx.helper.make_tensor("tail", onnx.TensorProto.INT64, [2], [2, 3])
    nodes = [
        onnx.helper.make_node("Shape", ["driver"], ["leading"], start=0, end=2),
        onnx.helper.make_node("Concat", ["leading", "tail"], ["target"], axis=0),
        onnx.helper.make_node("Reshape", ["data", "target"], ["output"], allowzero=1),
    ]
    graph = onnx.helper.make_graph(nodes, "allowzero", [data, driver], [output], [tail])
    return onnx.helper.make_model(graph)


def test_directml_rewrite_accepts_matching_symbolic_dimensions():
    from auris_singer import export as export_module

    proto = _symbolic_allowzero_model("batch")
    assert export_module._remove_redundant_reshape_allowzero(proto) == 1
    assert not proto.graph.node[-1].attribute


def test_directml_rewrite_rejects_a_different_symbolic_dimension():
    from auris_singer import export as export_module

    proto = _symbolic_allowzero_model("other_batch")
    assert export_module._remove_redundant_reshape_allowzero(proto) == 0
    with pytest.raises(ValueError, match="allowzero=1"):
        export_module._reject_directml_incompatible_graph(proto)
