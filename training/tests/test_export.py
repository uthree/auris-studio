"""Tests for the ONNX export wrapper."""

from __future__ import annotations

import pytest
import torch

from auris_singer.export import OnnxSingerWrapper
from auris_singer.model import AurisSinger

HOP = 480


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


def test_portrait_roundtrips_and_rejects_the_wrong_things(tmp_path):
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

    (tmp_path / "huge.png").write_bytes(b"\0" * (8 * 1024 * 1024 + 1))
    with pytest.raises(ValueError, match="keep it under"):
        load_portrait(tmp_path / "huge.png")


def test_onnx_export_runs_and_matches_pytorch(model, tmp_path):
    pytest.importorskip("onnxruntime")
    onnx = pytest.importorskip("onnx")
    import json

    from auris_singer.export import METADATA_KEY, export_onnx, verify_onnx

    path = tmp_path / "tiny.onnx"
    export_onnx(
        model,
        path,
        metadata={"symbols": ["<pad>", "a", "s"], "speaker_to_id": {"x": 0}},
        voice={"name": "Test Singer", "description": "a demo voice", "credits": ["someone"]},
        phoneme_durations={
            "unit": "seconds",
            "default": 0.06,
            "seconds": {"s": 0.104},
            "counts": {"s": 1907},
            "measured_from": "unit test",
        },
    )

    # verify_onnx runs onnxruntime at sizes the trace never saw (so a baked-in
    # dimension fails here) and raises on any tolerance violation.
    errors = verify_onnx(model, path)
    assert errors["unvoiced_max_diff"] < 1e-4

    # The metadata rides along both inside the file and as a sidecar.
    props = {entry.key: entry.value for entry in onnx.load(str(path)).metadata_props}
    stored = json.loads(props[METADATA_KEY])
    assert stored["symbols"] == ["<pad>", "a", "s"]
    assert stored["sample_rate"] == 48_000
    assert stored["hop_length"] == HOP
    assert stored["inter_channels"] == model.inter_channels
    assert stored["voice"]["name"] == "Test Singer"
    assert stored["phoneme_durations"]["seconds"] == {"s": 0.104}
    assert stored["phoneme_durations"]["default"] == 0.06
    sidecar = json.loads((tmp_path / "tiny.json").read_text(encoding="utf-8"))
    assert sidecar == stored

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
            metadata={"symbols": ["<pad>", "a"]},
            phoneme_durations={"seconds": {"s": 0.104, "ts": 0.119}},
        )
    assert not (tmp_path / "tiny.onnx").exists(), "refused before tracing anything"


def test_phoneme_durations_are_optional(model, tmp_path):
    """Without a table the metadata simply has no such key, and a consumer
    falls back to its own default."""
    pytest.importorskip("onnxruntime")
    pytest.importorskip("onnx")
    import json

    from auris_singer.export import export_onnx

    export_onnx(model, tmp_path / "tiny.onnx", metadata={"symbols": ["<pad>", "a"]})
    sidecar = json.loads((tmp_path / "tiny.json").read_text(encoding="utf-8"))
    assert "phoneme_durations" not in sidecar


def test_the_graph_avoids_the_constructs_directml_rejects(model, tmp_path):
    """Two ONNX spellings run on CPU and CUDA but fail on onnxruntime's
    DirectML provider, which is what an AMD GPU uses on Windows:

    * ``Reshape`` with ``allowzero=1`` and a ``-1`` in its shape tensor — what
      a traced ``view(b, t, -1)`` becomes;
    * ``ConvTranspose`` carrying an ``output_padding`` attribute at all, even
      ``[0]``.

    Both fail with a bare "the parameter is incorrect", so guard the shape of
    the graph here rather than waiting for a bug report from a GPU nobody in
    CI has.
    """
    onnx = pytest.importorskip("onnx")

    from auris_singer.export import export_onnx

    path = tmp_path / "tiny.onnx"
    export_onnx(model, path, metadata={"symbols": ["<pad>", "a"]})
    graph = onnx.load(str(path)).graph

    constants = {i.name: onnx.numpy_helper.to_array(i) for i in graph.initializer}
    for node in graph.node:
        for attr in node.attribute:
            if node.op_type == "Constant" and attr.name == "value":
                constants[node.output[0]] = onnx.numpy_helper.to_array(attr.t)

    def holds_a_negative(name: str) -> bool:
        """Whether a shape tensor is built from any negative constant."""
        if name in constants:
            return bool((constants[name] < 0).any())
        source = next((n for n in graph.node if name in n.output), None)
        if source is not None and source.op_type == "Concat":
            return any(holds_a_negative(i) for i in source.input)
        return False

    for node in graph.node:
        attrs = {a.name: a for a in node.attribute}
        if node.op_type == "Reshape" and attrs.get("allowzero", None) is not None:
            if attrs["allowzero"].i == 1:
                assert not holds_a_negative(node.input[1]), (
                    f"{node.name}: Reshape allowzero=1 with a -1 in its shape is "
                    "rejected by DirectML; write the dimension out instead of -1"
                )
        assert node.op_type != "ConvTranspose" or "output_padding" not in attrs, (
            f"{node.name}: DirectML rejects any output_padding on a 1-D "
            "ConvTranspose; export_onnx should have folded it into pads"
        )
