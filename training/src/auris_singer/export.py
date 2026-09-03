"""ONNX export of the inference path.

The graph exported here is :meth:`AurisSinger.infer` rewritten as a pure
function: every stochastic draw becomes an input.  A caller that feeds the
same noise twice gets the same waveform twice — which is what a DAW needs for
reproducible renders, and what makes the export verifiable against PyTorch at
all (with graph-internal random ops the two runtimes could only ever be
compared statistically).

:class:`OnnxSingerWrapper` is that pure function as an ``nn.Module``;
:func:`export_onnx` traces it into an ``.onnx`` file with dynamic sequence
lengths and embeds the checkpoint's metadata (phoneme table, speaker map,
audio parameters) so the consumer needs nothing but the one file.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import torch
import torch.nn as nn

from auris_singer.model import AurisSinger
from auris_singer.phoneme_durations import METADATA_FIELD as DURATIONS_FIELD
from auris_singer.phoneme_levels import METADATA_FIELD as LEVELS_FIELD
from auris_singer.utils.masks import sequence_mask

__all__ = [
    "OnnxSingerWrapper",
    "export_onnx",
    "verify_onnx",
    "load_portrait",
    "metadata_block",
    "METADATA_KEY",
    "FORMAT_VERSION",
]

#: The ``metadata_props`` key under which the model's JSON metadata is stored.
#: The host reads the same string from ``crates/auris-singer/src/metadata.rs``;
#: ``tests/test_host_contract.py`` is what keeps the two spellings one.
METADATA_KEY = "auris_singer"

#: The version stamped on every export's metadata block.
#:
#: A host reads exactly one number and refuses the rest rather than
#: half-understanding a file, so this rises only when a change would make a
#: reader of the old number wrong — a renamed or repurposed field, not a new
#: optional one. What it is, and why:
#:
#: * **1** — the first export: audio parameters, the phoneme table, the
#:   speakers, the card, and later one consonant-width table and one
#:   consonant-level table for the whole model.
#: * **2** — the two tables are measured per speaker, under ``speakers``. A
#:   version-1 table has no ``speakers`` and would read as no table at all,
#:   which is why this is a bump and not a default.
FORMAT_VERSION = 2

#: Image types a voice-card portrait may use.
PORTRAIT_MIME = {".png": "image/png", ".jpg": "image/jpeg", ".jpeg": "image/jpeg",
                 ".webp": "image/webp"}

#: Refuse portraits larger than this — the whole point of embedding is one
#: self-contained model file, not a model file that is mostly artwork.
PORTRAIT_MAX_BYTES = 8 * 1024 * 1024


def load_portrait(path: str | Path) -> dict[str, str]:
    """Read an image into the ``portrait`` field of a voice card.

    Returns ``{"mime": ..., "base64": ...}`` — the shape a consumer decodes
    back into bytes. Raises on an unknown extension or an oversized file.
    """
    import base64

    path = Path(path)
    mime = PORTRAIT_MIME.get(path.suffix.lower())
    if mime is None:
        raise ValueError(
            f"unsupported portrait type {path.suffix!r}; use one of {sorted(PORTRAIT_MIME)}"
        )
    data = path.read_bytes()
    if len(data) > PORTRAIT_MAX_BYTES:
        raise ValueError(
            f"portrait is {len(data) / 1e6:.1f} MB; keep it under "
            f"{PORTRAIT_MAX_BYTES / 1e6:.0f} MB"
        )
    return {"mime": mime, "base64": base64.b64encode(data).decode("ascii")}


class OnnxSingerWrapper(nn.Module):
    """The inference path as a pure function of tensors.

    Differences from :meth:`AurisSinger.infer`, all in the name of a clean
    ONNX graph:

    * ``voiced`` is required — deriving it from ``f0`` would silently voice
      the consonant frames of a front-end that writes pitch as a contour
      (auris-studio does exactly that);
    * the prior sample and the excitation noise are inputs (``z_noise``,
      ``source_noise``) instead of internal draws;
    * ``sum(durations)`` must equal ``f0.size(-1)`` — the wrapper does not
      trim the curves the way ``infer`` does, because data-dependent slicing
      does not belong in a traced graph.
    """

    def __init__(self, model: AurisSinger):
        super().__init__()
        self.model = model

    def latent(
        self,
        phonemes: torch.Tensor,
        phoneme_lengths: torch.Tensor,
        durations: torch.Tensor,
        f0: torch.Tensor,
        energy: torch.Tensor,
        voiced: torch.Tensor,
        speaker_ids: torch.Tensor,
        noise_scale: torch.Tensor,
        z_noise: torch.Tensor,
    ) -> tuple[torch.Tensor, ...]:
        """Everything up to the decoder: the masked latent and the curves as
        the decoder wants them, ``(z, f0, energy, voiced, g)``."""
        model = self.model
        g = model.speaker_embedding(speaker_ids).unsqueeze(-1)

        x, _, _, x_mask = model.text_encoder(phonemes, phoneme_lengths, g=g)

        durations = durations.to(torch.long) * x_mask.squeeze(1).long()
        y_lengths = durations.sum(dim=1).clamp(min=1)
        y_mask = sequence_mask(y_lengths, f0.size(-1)).unsqueeze(1).to(x.dtype)

        attn = model._path_from_durations(durations, x_mask, y_mask)
        x_frame = torch.matmul(x, attn)

        f0 = f0.unsqueeze(1)
        energy = energy.unsqueeze(1)
        voiced = voiced.unsqueeze(1)

        m_p, logs_p = model.prior_encoder(
            x_frame, y_mask, f0=f0, energy=energy, voiced=voiced, g=g
        )
        z_p = m_p + z_noise * torch.exp(logs_p) * noise_scale
        z = model.flow(z_p, y_mask, g=g, reverse=True)
        return z * y_mask, f0, energy, voiced, g

    def forward(
        self,
        phonemes: torch.Tensor,
        phoneme_lengths: torch.Tensor,
        durations: torch.Tensor,
        f0: torch.Tensor,
        energy: torch.Tensor,
        voiced: torch.Tensor,
        speaker_ids: torch.Tensor,
        noise_scale: torch.Tensor,
        z_noise: torch.Tensor,
        source_noise: torch.Tensor,
    ) -> tuple[torch.Tensor, torch.Tensor]:
        """
        Args:
            phonemes: ``(B, S)`` phoneme ids, int64.
            phoneme_lengths: ``(B,)`` int64.
            durations: ``(B, S)`` frames per phoneme, int64; each row must sum
                to ``T``.
            f0: ``(B, T)`` f0 in Hz, float32; 0 on unvoiced and silent frames.
            energy: ``(B, T)`` linear RMS energy, float32.
            voiced: ``(B, T)`` float32, 1.0 on voiced frames.
            speaker_ids: ``(B,)`` int64.
            noise_scale: scalar float32 — the prior sampling temperature.
            z_noise: ``(B, inter_channels, T)`` standard normal draws, float32.
            source_noise: ``(B, 1, T * hop_length)`` uniform noise on
                ``[-1, 1]``, float32.

        Returns:
            ``(waveform, source)``, both ``(B, 1, T * hop_length)`` float32.
            The excitation is a second graph output for verification and
            diagnostics; a runtime asked only for ``wav`` prunes it away.
        """
        model = self.model
        z, f0, energy, voiced, g = self.latent(
            phonemes,
            phoneme_lengths,
            durations,
            f0,
            energy,
            voiced,
            speaker_ids,
            noise_scale,
            z_noise,
        )
        source = model.generator.source_generator(f0, energy, voiced, noise=source_noise)
        wav, _ = model.generator(z, f0, energy, voiced, g=g, source=source)
        return wav, source


def _example_inputs(model: AurisSinger) -> dict[str, torch.Tensor]:
    """Example inputs for tracing.

    Every dynamic dimension is sized well away from 0 and 1, which
    ``torch.export`` would otherwise specialize on.
    """
    batch, s, t = 2, 8, 40
    f0 = torch.full((batch, t), 220.0)
    return {
        "phonemes": torch.randint(1, model.n_vocab, (batch, s)),
        "phoneme_lengths": torch.tensor([s, s - 1], dtype=torch.long),
        "durations": torch.full((batch, s), t // s, dtype=torch.long),
        "f0": f0,
        "energy": torch.full((batch, t), 0.1),
        "voiced": torch.ones(batch, t),
        "speaker_ids": torch.zeros(batch, dtype=torch.long),
        "noise_scale": torch.tensor(0.667),
        "z_noise": torch.randn(batch, model.inter_channels, t),
        "source_noise": torch.rand(batch, 1, t * model.hop_length) * 2.0 - 1.0,
    }


def _fold_conv_transpose_output_padding(proto) -> int:
    """Rewrite ``ConvTranspose`` so it carries no ``output_padding`` attribute.

    onnxruntime's DirectML provider rejects a 1-D ``ConvTranspose`` that has an
    ``output_padding`` attribute at all — even ``[0]`` — with a bare
    "the parameter is incorrect". The attribute is redundant: the output length
    is ``stride * (in - 1) + output_padding + (kernel - 1) * dilation + 1 -
    pads_begin - pads_end``, so ``output_padding`` enters exactly as a smaller
    ``pads_end`` does. Subtracting it there produces the same crop of the same
    transposed convolution, element for element, and the graph then runs on
    DirectML as well as on the CPU provider.

    The fold needs ``pads_end >= output_padding``, which holds for the
    generator's upsampling schedule (``padding = (kernel - rate + 1) // 2``
    always covers ``output_padding = rate + 2 * padding - kernel``). A node
    that would need a negative pad is left alone rather than silently changed.

    Returns the number of nodes rewritten.
    """
    folded = 0
    for node in proto.graph.node:
        if node.op_type != "ConvTranspose":
            continue
        attrs = {a.name: a for a in node.attribute}
        out_pad = attrs.get("output_padding")
        if out_pad is None:
            continue
        values = list(out_pad.ints)
        pads = attrs.get("pads")
        # `pads` is [begin_0, ..., begin_n, end_0, ..., end_n]; absent means all
        # zero, in which case a nonzero output_padding cannot be folded.
        ends = list(pads.ints)[len(values):] if pads is not None else [0] * len(values)
        if any(op > end for op, end in zip(values, ends)):
            continue
        if any(values) and pads is not None:
            for i, op in enumerate(values):
                pads.ints[len(values) + i] = ends[i] - op
        node.attribute.remove(out_pad)
        folded += 1
    return folded


def metadata_block(
    model: AurisSinger,
    metadata: dict[str, Any] | None = None,
    *,
    voice: dict[str, Any] | None = None,
    phoneme_durations: dict[str, Any] | None = None,
    phoneme_levels: dict[str, Any] | None = None,
) -> dict[str, Any]:
    """The JSON block an export carries: what the model is, and what it needs.

    Separate from :func:`export_onnx` because it is the half a host actually
    parses, and the only half that can be checked without tracing a graph —
    ``tests/test_host_contract.py`` builds one from a stand-in model and holds
    its keys against the fields the Rust reader requires.

    ``metadata`` is merged last, so a checkpoint's own record of the phoneme
    table and the audio configuration wins over anything derived here.
    """
    block: dict[str, Any] = {
        "format_version": FORMAT_VERSION,
        "sample_rate": model.sample_rate,
        "hop_length": model.hop_length,
        "inter_channels": model.inter_channels,
        "n_speakers": model.n_speakers,
        "f0_min": model.generator.source_generator.f0_min,
        **(metadata or {}),
    }
    if voice:
        block["voice"] = voice
    if phoneme_durations:
        block[DURATIONS_FIELD] = phoneme_durations
    if phoneme_levels:
        block[LEVELS_FIELD] = phoneme_levels
    return block


def export_onnx(
    model: AurisSinger,
    path: str | Path,
    metadata: dict[str, Any] | None = None,
    opset: int = 18,
    voice: dict[str, Any] | None = None,
    phoneme_durations: dict[str, Any] | None = None,
    phoneme_levels: dict[str, Any] | None = None,
) -> None:
    """Export the inference path to ``path`` as ONNX.

    The model is put in eval mode and its weight norm is folded into the
    weights — a one-way operation, so pass a model loaded for export, not one
    that will keep training.

    ``metadata`` (typically the checkpoint's: phoneme ``symbols``,
    ``speaker_to_id``, the audio config) is merged with the model's own
    parameters and stored twice: as JSON under the :data:`METADATA_KEY` key of
    the ONNX ``metadata_props``, and as a ``.json`` sidecar next to ``path``
    for consumers that would rather not parse protobuf.

    ``voice`` is the presentational **voice card** — free-form fields a host
    application shows to people rather than feeds to the model: ``name``,
    ``description``, ``author``, ``license``, ``credits``, and a ``portrait``
    (see :func:`load_portrait`). It is stored under the ``voice`` key of the
    same JSON, so the one ``.onnx`` file carries everything a UI needs.

    ``phoneme_durations`` is the per-phoneme consonant width table built by
    :func:`auris_singer.phoneme_durations.summarize`, stored under a key of the
    same name. It says how long each consonant should be *given* to this voice,
    which a front-end has to decide before it can turn a note into frames. The
    numbers are a property of the corpus the model was trained on, so they
    belong with the model rather than hard-coded in the front-end;
    ``doc/inference.md`` documents the format for consumers.

    ``phoneme_levels`` is its companion from
    :func:`auris_singer.phoneme_levels.summarize`: how loud each consonant is
    against the vowel after it, in decibels, so a front-end that writes one
    energy per note can turn the consonants down to where the voice sang them.
    """
    from torch.export import Dim

    symbols = set((metadata or {}).get("symbols") or ())
    speakers = set((metadata or {}).get("speaker_to_id") or ())
    for name, table, key in (
        (DURATIONS_FIELD, phoneme_durations, "seconds"),
        (LEVELS_FIELD, phoneme_levels, "db"),
    ):
        if not table:
            continue
        if "speakers" not in table:
            raise ValueError(
                f"{name} is a format-1 table, one for the whole model; format {FORMAT_VERSION} "
                "measures one per speaker — re-measure it with the current script"
            )
        # A table for a speaker this model has not, or keyed by symbols it cannot be given,
        # is somebody else's, and shipping one silently hides a phoneme table or a speaker
        # map that has moved on since it was measured. Fail before the trace, not after.
        strangers = sorted(set(table["speakers"]) - speakers)
        if speakers and strangers:
            raise ValueError(
                f"{name} names speakers the model has not: {strangers}; the model's are "
                f"{sorted(speakers)}"
            )
        for speaker, own in table["speakers"].items():
            stray = sorted(set(own.get(key) or ()) - symbols)
            if symbols and stray:
                raise ValueError(
                    f"{name} for {speaker} names symbols outside the model's table: {stray}; "
                    "re-measure them against this checkpoint's phoneme table"
                )

    model = model.eval()
    model.remove_weight_norm()

    batch, s, t = Dim("batch"), Dim("phonemes"), Dim("frames")
    dynamic_shapes = {
        "phonemes": {0: batch, 1: s},
        "phoneme_lengths": {0: batch},
        "durations": {0: batch, 1: s},
        "f0": {0: batch, 1: t},
        "energy": {0: batch, 1: t},
        "voiced": {0: batch, 1: t},
        "speaker_ids": {0: batch},
        "noise_scale": None,
        "z_noise": {0: batch, 2: t},
        "source_noise": {0: batch, 2: model.hop_length * t},
    }

    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with torch.no_grad():
        torch.onnx.export(
            OnnxSingerWrapper(model).eval(),
            (),
            str(path),
            kwargs=_example_inputs(model),
            input_names=list(dynamic_shapes),
            output_names=["wav", "source"],
            dynamic_shapes=dynamic_shapes,
            opset_version=opset,
            dynamo=True,
        )

    merged = metadata_block(
        model, metadata, voice=voice, phoneme_durations=phoneme_durations, phoneme_levels=phoneme_levels
    )

    import onnx

    proto = onnx.load(str(path))
    _fold_conv_transpose_output_padding(proto)
    entry = proto.metadata_props.add()
    entry.key = METADATA_KEY
    entry.value = json.dumps(merged, ensure_ascii=False)
    onnx.save(proto, str(path))

    # The dynamo exporter parks the weights in a sidecar "<name>.onnx.data";
    # onnx.load pulled them back in and onnx.save above inlined them (it
    # would have failed past 2 GB), so the sidecar is now a stale duplicate
    # that would only mislead whoever ships the file.
    stale = Path(str(path) + ".data")
    if stale.exists():
        stale.unlink()

    path.with_suffix(".json").write_text(
        json.dumps(merged, ensure_ascii=False, indent=2) + "\n", encoding="utf-8"
    )


def _verification_inputs(
    model: AurisSinger, batch: int, s: int, t: int, voiced: bool, seed: int
) -> dict[str, torch.Tensor]:
    torch.manual_seed(seed)
    return {
        "phonemes": torch.randint(1, model.n_vocab, (batch, s)),
        "phoneme_lengths": torch.full((batch,), s, dtype=torch.long),
        "durations": torch.full((batch, s), max(1, t // s), dtype=torch.long),
        "f0": torch.full((batch, t), 220.0 if voiced else 0.0),
        "energy": torch.full((batch, t), 0.1),
        "voiced": torch.full((batch, t), 1.0 if voiced else 0.0),
        "speaker_ids": torch.arange(batch, dtype=torch.long) % model.n_speakers,
        "noise_scale": torch.tensor(0.5),
        "z_noise": torch.randn(batch, model.inter_channels, t),
        "source_noise": torch.rand(batch, 1, t * model.hop_length) * 2.0 - 1.0,
    }


def verify_onnx(model: AurisSinger, path: str | Path) -> dict[str, float]:
    """Check the exported graph against PyTorch, at sizes the trace never saw.

    Exact waveform comparison across runtimes is ill-posed in one spot: the
    impulse positions come from thresholding a long float32 cumulative sum,
    so a one-ulp difference in the runtimes' interpolation or summation order
    eventually moves an impulse by one sample — inaudible (training adds a
    whole random phase offset on top), but every comparison downstream of it
    is ruined. So the excitation is checked on its own terms and everything
    around it strictly:

    * **Unvoiced input**: no impulses, everything else exercised — the
      waveforms must match to float precision.
    * **Voiced input**: the graph's own ``source`` output is fed back through
      the PyTorch decoder; given the same excitation the waveforms must again
      match to float precision. The excitation itself is checked structurally:
      its impulse spacing must be the requested period.

    ``model`` must be the exported one (eval, weight norm folded). Raises
    ``ValueError`` when a tolerance is exceeded; returns the measured errors.
    """
    import numpy as np
    import onnxruntime

    wrapper = OnnxSingerWrapper(model)
    session = onnxruntime.InferenceSession(str(path), providers=["CPUExecutionProvider"])

    def run(inputs: dict[str, torch.Tensor]) -> tuple[np.ndarray, np.ndarray, np.ndarray]:
        with torch.no_grad():
            ref, _ = wrapper(**inputs)
        wav, source = session.run(
            ["wav", "source"], {k: v.numpy() for k, v in inputs.items()}
        )
        return ref.numpy(), wav, source

    # Sizes deliberately different from the export-time example inputs, so a
    # dimension the trace accidentally baked in fails loudly here.
    ref, out, _ = run(_verification_inputs(model, batch=1, s=5, t=23, voiced=False, seed=0))
    unvoiced_diff = float(np.abs(out - ref).max())
    if unvoiced_diff > 1e-4:
        raise ValueError(f"unvoiced output differs from PyTorch by {unvoiced_diff:.2e}")

    inputs = _verification_inputs(model, batch=2, s=9, t=64, voiced=True, seed=1)
    _, out, source = run(inputs)
    with torch.no_grad():
        z, f0, energy, voiced, g = wrapper.latent(
            **{k: v for k, v in inputs.items() if k != "source_noise"}
        )
        ref, _ = model.generator(
            z, f0, energy, voiced, g=g, source=torch.from_numpy(source)
        )
    voiced_diff = float(np.abs(out - ref.numpy()).max())
    if voiced_diff > 1e-4:
        raise ValueError(
            f"voiced output differs from PyTorch by {voiced_diff:.2e} "
            "given the same excitation"
        )

    f0_hz = float(inputs["f0"][0, 0])
    impulses = np.flatnonzero(source[0, 0] > source[0, 0].max() * 0.5)
    spacing = float(np.diff(impulses).mean())
    period = model.sample_rate / f0_hz
    if abs(spacing - period) > period * 0.02:
        raise ValueError(
            f"excitation impulse spacing is {spacing:.1f} samples, "
            f"expected {period:.1f}"
        )

    return {
        "unvoiced_max_diff": unvoiced_diff,
        "voiced_max_diff": voiced_diff,
        "impulse_spacing_error": abs(spacing - period) / period,
    }
