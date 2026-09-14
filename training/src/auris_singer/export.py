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
import logging
import math
import os
import shutil
import tempfile
import uuid
from collections.abc import Callable
from contextlib import contextmanager
from pathlib import Path
from typing import Any

import torch
import torch.nn as nn

from auris_singer.model import AurisSinger
from auris_singer.phoneme_durations import METADATA_FIELD as DURATIONS_FIELD
from auris_singer.phoneme_levels import METADATA_FIELD as LEVELS_FIELD
from auris_singer.text import PAD, SPECIAL_SYMBOLS
from auris_singer.utils.durability import fsync_directory, fsync_file
from auris_singer.utils.masks import sequence_mask

logger = logging.getLogger(__name__)

__all__ = [
    "OnnxSingerWrapper",
    "export_onnx",
    "verify_onnx",
    "load_portrait",
    "read_export_metadata",
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
PORTRAIT_MIME = {
    ".png": "image/png",
    ".jpg": "image/jpeg",
    ".jpeg": "image/jpeg",
    ".webp": "image/webp",
}

#: Refuse portraits larger than this — the whole point of embedding is one
#: self-contained model file, not a model file that is mostly artwork.
PORTRAIT_MAX_BYTES = 8 * 1024 * 1024


def _metadata_int(value: Any, label: str) -> int:
    """Read an integer metadata field without accepting truncation or booleans."""
    if type(value) is not int or value <= 0 or value > 0xFFFF_FFFF:
        raise ValueError(f"{label} must be a positive integer no greater than 4294967295")
    return value


def _validate_metadata_audio_clock(model: AurisSinger, metadata: dict[str, Any]) -> dict[str, int]:
    """Reject checkpoint metadata that describes another model clock."""
    nested = metadata.get("audio", {})
    if nested is None:
        nested = {}
    if not isinstance(nested, dict):
        raise ValueError("metadata audio must be a mapping")
    resolved = {}
    for name, raw_expected in (
        ("sample_rate", model.sample_rate),
        ("hop_length", model.hop_length),
    ):
        expected = _metadata_int(raw_expected, f"model {name}")
        for source in (metadata, nested):
            if name not in source:
                continue
            actual = _metadata_int(source[name], f"metadata {name}")
            if actual != expected:
                raise ValueError(
                    f"metadata {name}={actual} disagrees with the model's {name}={expected}"
                )
        resolved[name] = expected
    return resolved


def _validated_metadata_spectral_config(
    model: AurisSinger, metadata: dict[str, Any]
) -> dict[str, Any]:
    """Return host-analysis settings consistent with the model's spectrogram axis."""
    nested = metadata.get("audio", {})
    if nested is None:
        nested = {}
    if not isinstance(nested, dict):
        raise ValueError("metadata audio must be a mapping")
    spec_channels = _metadata_int(model.spec_channels, "model spec_channels")
    n_fft = 2 * (spec_channels - 1)
    if n_fft <= 0 or n_fft > 0xFFFF_FFFF:
        raise ValueError("model spec_channels cannot be represented by a host n_fft")
    for source in (metadata, nested):
        if "n_fft" in source:
            supplied = _metadata_int(source["n_fft"], "metadata n_fft")
            if supplied != n_fft:
                raise ValueError(
                    f"metadata n_fft={supplied} disagrees with model spec_channels={spec_channels} "
                    f"({n_fft} expected)"
                )
    raw_win_length = nested.get("win_length", metadata.get("win_length", n_fft))
    win_length = _metadata_int(raw_win_length, "metadata win_length")
    if win_length > n_fft:
        raise ValueError(f"metadata win_length={win_length} exceeds n_fft={n_fft}")
    return {**nested, "n_fft": n_fft, "win_length": win_length}


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
    size = path.stat().st_size
    if size > PORTRAIT_MAX_BYTES:
        raise ValueError(
            f"portrait is {size / 1e6:.1f} MB; keep it under {PORTRAIT_MAX_BYTES / 1e6:.0f} MB"
        )
    data = path.read_bytes()
    if len(data) > PORTRAIT_MAX_BYTES:
        raise ValueError(
            f"portrait is {len(data) / 1e6:.1f} MB; keep it under {PORTRAIT_MAX_BYTES / 1e6:.0f} MB"
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

        m_p, logs_p = model.prior_encoder(x_frame, y_mask, f0=f0, energy=energy, voiced=voiced, g=g)
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
        ends = list(pads.ints)[len(values) :] if pads is not None else [0] * len(values)
        if any(op > end for op, end in zip(values, ends)):
            continue
        if any(values) and pads is not None:
            for i, op in enumerate(values):
                pads.ints[len(values) + i] = ends[i] - op
        node.attribute.remove(out_pad)
        folded += 1
    return folded


def _remove_redundant_reshape_allowzero(proto) -> int:
    """Drop ``allowzero=1`` only where equivalence is statically provable.

    DirectML rejects every ``Reshape`` carrying this attribute. Torch 2.14
    emits it both for constant nonzero shapes and for shapes assembled from
    symbolic input dimensions. Removing it is semantics-preserving when each
    element is either a nonzero constant or the same symbolic dimension as the
    corresponding data axis: under the default rule a runtime zero then copies
    that identical zero-sized axis. Anything else is left for the compatibility
    guard to reject rather than guessing about a future graph.

    Returns the number of nodes rewritten.
    """
    import onnx

    constants = {
        initializer.name: onnx.numpy_helper.to_array(initializer)
        for initializer in proto.graph.initializer
    }
    for node in proto.graph.node:
        if node.op_type != "Constant" or not node.output:
            continue
        value = next(
            (attribute.t for attribute in node.attribute if attribute.name == "value"),
            None,
        )
        if value is not None:
            constants[node.output[0]] = onnx.numpy_helper.to_array(value)

    producers = {output: node for node in proto.graph.node for output in node.output if output}
    dimensions: dict[str, list[tuple[str, int | str] | None]] = {}
    for value in (*proto.graph.input, *proto.graph.output, *proto.graph.value_info):
        if not value.type.HasField("tensor_type"):
            continue
        keys: list[tuple[str, int | str] | None] = []
        for dimension in value.type.tensor_type.shape.dim:
            if dimension.HasField("dim_value"):
                keys.append(("value", dimension.dim_value))
            elif dimension.dim_param:
                keys.append(("param", dimension.dim_param))
            else:
                keys.append(None)
        dimensions[value.name] = keys

    def shape_elements(
        value: str, seen: frozenset[str] = frozenset()
    ) -> list[tuple[str, int | tuple[str, int | str]]] | None:
        """Resolve a 1-D shape tensor into constants or symbolic dimensions."""
        if value in seen:
            return None
        if (constant := constants.get(value)) is not None:
            if constant.dtype.kind not in "iu":
                return None
            return [("constant", int(item)) for item in constant.reshape(-1)]

        producer = producers.get(value)
        if producer is None:
            return None
        attributes = {attribute.name: attribute for attribute in producer.attribute}
        next_seen = seen | {value}
        if producer.op_type == "Concat" and attributes.get("axis") is not None:
            if attributes["axis"].i not in (0, -1):
                return None
            result: list[tuple[str, int | tuple[str, int | str]]] = []
            for input_value in producer.input:
                part = shape_elements(input_value, next_seen)
                if part is None:
                    return None
                result.extend(part)
            return result

        if producer.op_type != "Shape" or len(producer.input) != 1:
            return None
        source_dimensions = dimensions.get(producer.input[0])
        if source_dimensions is None:
            return None
        rank = len(source_dimensions)
        start = attributes.get("start").i if attributes.get("start") is not None else 0
        end = attributes.get("end").i if attributes.get("end") is not None else rank
        if start < 0:
            start += rank
        if end < 0:
            end += rank
        if not 0 <= start <= end <= rank:
            return None
        result = []
        for key in source_dimensions[start:end]:
            if key is None:
                return None
            if key[0] == "value":
                result.append(("constant", int(key[1])))
            else:
                result.append(("dimension", key))
        return result

    rewritten = 0
    for node in proto.graph.node:
        if node.op_type != "Reshape" or len(node.input) < 2:
            continue
        allowzero = next(
            (attribute for attribute in node.attribute if attribute.name == "allowzero"),
            None,
        )
        if allowzero is None or allowzero.i != 1:
            continue
        data_dimensions = dimensions.get(node.input[0])
        shape = shape_elements(node.input[1])
        if data_dimensions is None or shape is None:
            continue
        safe = True
        for axis, (kind, value) in enumerate(shape):
            if kind == "constant":
                if value == 0:
                    safe = False
                    break
            elif axis >= len(data_dimensions) or data_dimensions[axis] != value:
                safe = False
                break
        if not safe:
            continue
        node.attribute.remove(allowzero)
        rewritten += 1
    return rewritten


def _reject_directml_incompatible_graph(proto) -> None:
    """Fail closed when a rewrite could not prove a DirectML-safe graph."""
    for node in proto.graph.node:
        attributes = {attribute.name: attribute for attribute in node.attribute}
        if (
            node.op_type == "Reshape"
            and (allowzero := attributes.get("allowzero")) is not None
            and allowzero.i == 1
        ):
            raise ValueError(
                f"ONNX node {node.name or node.output[0]!r} retains Reshape allowzero=1, "
                "which DirectML rejects"
            )
        if node.op_type == "ConvTranspose" and "output_padding" in attributes:
            raise ValueError(
                f"ONNX node {node.name or node.output[0]!r} retains ConvTranspose "
                "output_padding, which DirectML rejects"
            )


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

    Checkpoint metadata supplies descriptive fields such as the phoneme table
    and speaker map. Structural fields are always derived from ``model``;
    conflicting audio-clock metadata is refused instead of overriding them.
    """
    metadata = dict(metadata or {})
    audio_clock = _validate_metadata_audio_clock(model, metadata)
    spectral = _validated_metadata_spectral_config(model, metadata)
    block: dict[str, Any] = {
        **metadata,
        "format_version": FORMAT_VERSION,
        "sample_rate": audio_clock["sample_rate"],
        "hop_length": audio_clock["hop_length"],
        "inter_channels": model.inter_channels,
        "n_speakers": model.n_speakers,
        "f0_min": model.generator.source_generator.f0_min,
        "audio": spectral,
    }
    if voice is not None:
        block["voice"] = voice
    if phoneme_durations is not None:
        block[DURATIONS_FIELD] = phoneme_durations
    if phoneme_levels is not None:
        block[LEVELS_FIELD] = phoneme_levels
    return block


def _validate_metadata_shape(
    model: AurisSinger, metadata: dict[str, Any]
) -> tuple[set[str], set[str]]:
    """Validate the token and speaker axes that index the exported graph."""
    n_vocab = _metadata_int(model.n_vocab, "model n_vocab")
    n_speakers = _metadata_int(model.n_speakers, "model n_speakers")
    _metadata_int(model.inter_channels, "model inter_channels")
    if n_speakers > 4_096:
        raise ValueError("model n_speakers exceeds the host limit of 4096")
    symbols = metadata.get("symbols")
    if not isinstance(symbols, list):
        raise ValueError("export metadata symbols must be a JSON array")
    if len(symbols) != n_vocab:
        raise ValueError(
            f"export metadata has {len(symbols)} symbols but the model has {n_vocab} embeddings"
        )
    if any(not isinstance(symbol, str) or not symbol for symbol in symbols):
        raise ValueError("export metadata symbols must be non-empty strings")
    if len(set(symbols)) != len(symbols):
        raise ValueError("export metadata symbols must be unique")
    if symbols[0] != PAD:
        raise ValueError(f"export metadata symbol 0 must be {PAD!r} for padded model inputs")
    missing_special = sorted(set(SPECIAL_SYMBOLS) - set(symbols))
    if missing_special:
        raise ValueError(f"export metadata is missing reserved symbols: {missing_special}")

    speaker_to_id = metadata.get("speaker_to_id")
    if not isinstance(speaker_to_id, dict):
        raise ValueError("export metadata speaker_to_id must be a JSON object")
    if any(not isinstance(name, str) or not name.strip() for name in speaker_to_id):
        raise ValueError("export metadata speaker names must be non-empty strings")
    ids = list(speaker_to_id.values())
    if any(type(speaker_id) is not int for speaker_id in ids):
        raise ValueError("export metadata speaker ids must be integers")
    expected_ids = set(range(n_speakers))
    if len(ids) != n_speakers or set(ids) != expected_ids:
        raise ValueError(
            "export metadata must name every model speaker exactly once with ids "
            f"0..{n_speakers - 1}"
        )
    return set(symbols), set(speaker_to_id)


def _validate_voice_card(voice: Any) -> None:
    """Reject fields whose JSON types the Rust host cannot deserialize."""
    if not isinstance(voice, dict):
        raise ValueError("export voice card must be a JSON object")
    for field in ("name", "description", "version", "license", "url"):
        if field in voice and not isinstance(voice[field], str):
            raise ValueError(f"export voice card {field} must be a string")
    if "credits" in voice and (
        not isinstance(voice["credits"], list)
        or any(not isinstance(credit, str) for credit in voice["credits"])
    ):
        raise ValueError("export voice card credits must be an array of strings")


def _measurement_number(value: Any, *, positive: bool) -> bool:
    """Whether ``value`` is a finite host-readable measurement."""
    return (
        not isinstance(value, bool)
        and isinstance(value, (int, float))
        and math.isfinite(float(value))
        and (not positive or value > 0)
    )


def _validate_measurement_table(
    name: str,
    table: Any,
    value_key: str,
    unit: str,
    *,
    positive: bool,
) -> dict[str, dict[str, Any]]:
    """Validate the ancillary schema consumed by ``VoiceInfo`` in Rust."""
    if not isinstance(table, dict):
        raise ValueError(f"{name} must be a JSON object")
    if "speakers" not in table:
        raise ValueError(
            f"{name} is a format-1 table, one for the whole model; format {FORMAT_VERSION} "
            "measures one per speaker — re-measure it with the current script"
        )
    if table.get("unit", unit) != unit:
        raise ValueError(f"{name} unit must be {unit!r}")
    speakers = table["speakers"]
    if not isinstance(speakers, dict):
        raise ValueError(f"{name} speakers must be a JSON object")
    for speaker, own in speakers.items():
        if not isinstance(speaker, str) or not speaker:
            raise ValueError(f"{name} speaker names must be non-empty strings")
        if not isinstance(own, dict):
            raise ValueError(f"{name} for {speaker} must be a JSON object")
        if not _measurement_number(own.get("default"), positive=positive):
            qualifier = "a positive finite number" if positive else "a finite number"
            raise ValueError(f"{name} default for {speaker} must be {qualifier}")
        values = own.get(value_key, {})
        if not isinstance(values, dict):
            raise ValueError(f"{name} {value_key} for {speaker} must be a JSON object")
        if any(not isinstance(symbol, str) or not symbol for symbol in values):
            raise ValueError(f"{name} {value_key} keys for {speaker} must be non-empty strings")
        if any(not _measurement_number(value, positive=positive) for value in values.values()):
            qualifier = "positive finite numbers" if positive else "finite numbers"
            raise ValueError(f"{name} {value_key} for {speaker} must contain {qualifier}")
    return speakers


def _replace_file(source: str | Path, destination: str | Path) -> None:
    """Replace one file; split out so commit failures are injectable in tests."""
    os.replace(source, destination)


def _backup_file(path: Path, directory: Path) -> Path | None:
    """Keep a cheap same-volume rollback copy of ``path`` when it exists."""
    if not path.exists():
        return None
    backup = directory / path.name
    try:
        os.link(path, backup)
    except OSError:
        shutil.copy2(path, backup)
        # Unlike the hard-link path, this created a new inode whose contents
        # may still live only in the page cache.  A rollback can rename that
        # inode over the public file, so make both its bytes and directory
        # entry durable before publication starts.
        fsync_file(backup)
    # Both paths created a new rollback directory entry. The existing inode
    # behind a hard link is already durable, but that new name is not until
    # its directory is flushed.
    fsync_directory(directory)
    return backup


@contextmanager
def _publication_lock(target: Path):
    """Serialize exporters targeting the same public model pair."""
    lock_path = target.parent / f".{target.name}.lock"
    with lock_path.open("a+b") as stream:
        if stream.seek(0, os.SEEK_END) == 0:
            stream.write(b"\0")
            stream.flush()
        stream.seek(0)
        if os.name == "nt":
            import msvcrt

            msvcrt.locking(stream.fileno(), msvcrt.LK_LOCK, 1)
            try:
                yield
            finally:
                stream.seek(0)
                msvcrt.locking(stream.fileno(), msvcrt.LK_UNLCK, 1)
        else:
            import fcntl

            fcntl.flock(stream.fileno(), fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(stream.fileno(), fcntl.LOCK_UN)


def _commit_export_pair(
    candidate: Path,
    candidate_sidecar: Path,
    target: Path,
    sidecar: Path,
) -> None:
    """Replace an ONNX/JSON pair and roll both back if either replace fails."""
    target.parent.mkdir(parents=True, exist_ok=True)
    if target.parent != sidecar.parent:
        raise ValueError("ONNX and JSON sidecar must share a directory")
    # Both files must be durable before either public name can refer to them.
    fsync_file(candidate)
    fsync_file(candidate_sidecar)
    fsync_directory(candidate.parent)
    with _publication_lock(target):
        _commit_export_pair_unlocked(candidate, candidate_sidecar, target, sidecar)


def _commit_export_pair_unlocked(
    candidate: Path,
    candidate_sidecar: Path,
    target: Path,
    sidecar: Path,
) -> None:
    """Commit after the per-target publication lock has been acquired."""
    rollback_dir = Path(tempfile.mkdtemp(prefix=f".{target.name}.rollback-", dir=target.parent))
    preserve_rollback = False
    try:
        originals = {
            target: _backup_file(target, rollback_dir),
            sidecar: _backup_file(sidecar, rollback_dir),
        }
        # Mark a destination before calling replace. A signal can be delivered
        # after the kernel completed the rename but before this Python frame
        # regains control; treating every attempted destination as possibly
        # changed is what makes that interruption recoverable.
        attempted: list[Path] = []
        try:
            attempted.append(target)
            _replace_file(candidate, target)
            attempted.append(sidecar)
            _replace_file(candidate_sidecar, sidecar)
            fsync_directory(target.parent)
        except BaseException as error:
            rollback_errors: list[tuple[Path, BaseException]] = []
            for destination in reversed(attempted):
                backup = originals[destination]
                try:
                    if backup is None:
                        destination.unlink(missing_ok=True)
                    else:
                        _replace_file(backup, destination)
                except BaseException as rollback_error:
                    # Keep trying: restoring the other half is still useful,
                    # and the caller needs every failure rather than whichever
                    # happened first.
                    rollback_errors.append((destination, rollback_error))
            try:
                fsync_directory(target.parent)
            except BaseException as sync_error:
                rollback_errors.append((target.parent, sync_error))
            if rollback_errors:
                preserve_rollback = True
                details = "; ".join(
                    f"{destination.name}: {rollback_error!r}"
                    for destination, rollback_error in rollback_errors
                )
                raise RuntimeError(
                    "export publication failed and its previous pair could not be fully "
                    f"restored ({details})"
                ) from error
            raise
    finally:
        if not preserve_rollback:
            try:
                shutil.rmtree(rollback_dir)
            except BaseException as error:
                # Publication (or its rollback) is already durable. Cleanup of
                # an unreachable private directory must not turn that result
                # into a false failure, especially under Windows AV locks.
                logger.warning(
                    "could not remove export rollback directory %s: %r", rollback_dir, error
                )


def _read_embedded_metadata(path: Path) -> dict[str, Any]:
    """Read the metadata block stored inside one exported ONNX model."""
    import onnx

    props = {
        entry.key: entry.value
        for entry in onnx.load(str(path), load_external_data=False).metadata_props
    }
    if METADATA_KEY not in props:
        raise ValueError(
            f"{path} carries no {METADATA_KEY!r} metadata; it is not an exported voice"
        )
    value = json.loads(props[METADATA_KEY])
    if not isinstance(value, dict):
        raise ValueError(f"{path}'s {METADATA_KEY!r} metadata must be a JSON object")
    return value


def read_export_metadata(path: str | Path) -> dict[str, Any]:
    """Read one consistent ONNX/JSON metadata generation.

    Reading both atomic files and comparing their generation marker turns the
    short interval between replacements into an explicit error, never a
    silently mixed pair. The full block comparison also rejects a mismatched
    pair left by an interrupted external copy or an unrecoverable rollback.
    Legacy exports without a generation marker remain valid when their two
    blocks agree.

    The embedded metadata is authoritative when the optional JSON sidecar is
    absent. Reading an existing sidecar requires the ``export`` dependencies,
    because its consistency cannot be established without parsing the ONNX.
    """
    path = Path(path)
    embedded = _read_embedded_metadata(path)
    sidecar_path = path.with_suffix(".json")
    if not sidecar_path.is_file():
        return embedded
    sidecar = json.loads(sidecar_path.read_text(encoding="utf-8"))
    if not isinstance(sidecar, dict):
        raise ValueError(f"{sidecar_path} must contain a JSON object")

    embedded_generation = embedded.get("export_generation")
    sidecar_generation = sidecar.get("export_generation")
    if embedded_generation != sidecar_generation:
        raise ValueError(
            "ONNX/JSON export_generation mismatch: "
            f"model={embedded_generation!r}, sidecar={sidecar_generation!r}"
        )
    if embedded != sidecar:
        raise ValueError("ONNX/JSON metadata differs despite matching export_generation")
    return sidecar


def _write_json(path: Path, value: dict[str, Any]) -> None:
    """Write and fsync a staged JSON sidecar."""
    with path.open("x", encoding="utf-8", newline="\n") as stream:
        json.dump(value, stream, ensure_ascii=False, indent=2, allow_nan=False)
        stream.write("\n")
        stream.flush()
        os.fsync(stream.fileno())


def _publish_export_pair(
    model: AurisSinger,
    candidate: Path,
    candidate_sidecar: Path,
    target: Path,
    validator: Callable[[AurisSinger, str | Path], dict[str, float]] | None,
) -> dict[str, float] | None:
    """Validate a staged model before committing either public file."""
    result = validator(model, candidate) if validator is not None else None
    _commit_export_pair(candidate, candidate_sidecar, target, target.with_suffix(".json"))
    return result


def _cleanup_stale_external_data(path: Path) -> None:
    """Best-effort cleanup after a self-contained pair is already committed."""
    stale = Path(str(path) + ".data")
    try:
        stale.unlink(missing_ok=True)
        fsync_directory(path.parent)
    except BaseException as error:
        # The new model embeds all weights and does not consult this legacy
        # file. A lock or read-only bit must not turn a successful publication
        # into a false failure after rollback is no longer possible.
        logger.warning("could not remove stale ONNX external data %s: %s", stale, error)


def _export_candidate(
    model: AurisSinger,
    path: Path,
    metadata: dict[str, Any],
    opset: int,
) -> None:
    """Trace, annotate and structurally validate one staged ONNX file."""
    from torch.export import Dim

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

    import onnx

    proto = onnx.load(str(path))
    _remove_redundant_reshape_allowzero(proto)
    _fold_conv_transpose_output_padding(proto)
    _reject_directml_incompatible_graph(proto)
    entry = proto.metadata_props.add()
    entry.key = METADATA_KEY
    entry.value = json.dumps(metadata, ensure_ascii=False, allow_nan=False)
    onnx.checker.check_model(proto)
    onnx.save(proto, str(path))

    # Loading and saving inlines the dynamo exporter's external weights.
    Path(str(path) + ".data").unlink(missing_ok=True)


def export_onnx(
    model: AurisSinger,
    path: str | Path,
    metadata: dict[str, Any] | None = None,
    opset: int = 18,
    voice: dict[str, Any] | None = None,
    phoneme_durations: dict[str, Any] | None = None,
    phoneme_levels: dict[str, Any] | None = None,
    verify: bool = False,
) -> dict[str, float] | None:
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

    When ``verify`` is true, the onnxruntime comparison runs against the staged
    candidate. Neither public file is replaced unless tracing, metadata
    writing, structural ONNX checks and numerical verification all succeed.
    If either replace fails, both previous files are restored.
    """
    merged = metadata_block(
        model,
        metadata,
        voice=voice,
        phoneme_durations=phoneme_durations,
        phoneme_levels=phoneme_levels,
    )
    # Both representations carry the same opaque generation marker. A reader
    # that elects to consume the optional sidecar can detect a pair copied or
    # observed across publication boundaries.
    merged["export_generation"] = uuid.uuid4().hex

    # Python's default encoder emits NaN and Infinity even though JSON and the
    # Rust host reject them. Validate before the one-way weight-norm fold and
    # use strict encoding for both public representations below.
    try:
        json.dumps(merged, ensure_ascii=False, allow_nan=False)
    except (TypeError, ValueError) as error:
        raise ValueError("export metadata must contain finite JSON data") from error

    symbols, speakers = _validate_metadata_shape(model, merged)
    if "voice" in merged and merged["voice"] is not None:
        _validate_voice_card(merged["voice"])
    for name, key, unit, positive in (
        (DURATIONS_FIELD, "seconds", "seconds", True),
        (LEVELS_FIELD, "db", "db", False),
    ):
        if name not in merged or merged[name] is None:
            continue
        table = merged[name]
        speaker_tables = _validate_measurement_table(name, table, key, unit, positive=positive)
        # A table for a speaker this model has not, or keyed by symbols it cannot be given,
        # is somebody else's, and shipping one silently hides a phoneme table or a speaker
        # map that has moved on since it was measured. Fail before the trace, not after.
        strangers = sorted(set(speaker_tables) - speakers)
        if strangers:
            raise ValueError(
                f"{name} names speakers the model has not: {strangers}; the model's are "
                f"{sorted(speakers)}"
            )
        for speaker, own in speaker_tables.items():
            stray = sorted(set(own.get(key) or ()) - symbols)
            if stray:
                raise ValueError(
                    f"{name} for {speaker} names symbols outside the model's table: {stray}; "
                    "re-measure them against this checkpoint's phoneme table"
                )

    model = model.eval()
    model.remove_weight_norm()

    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=f".{path.name}.staging-", dir=path.parent) as raw:
        staging = Path(raw)
        candidate = staging / path.name
        candidate_sidecar = candidate.with_suffix(".json")
        _export_candidate(model, candidate, merged, opset)
        _write_json(candidate_sidecar, merged)
        result = _publish_export_pair(
            model,
            candidate,
            candidate_sidecar,
            path,
            verify_onnx if verify else None,
        )

    # A prior pre-transactional export may have left external weights beside
    # the public name. The new model is self-contained; remove that stale copy
    # only after the complete pair has committed.
    _cleanup_stale_external_data(path)
    return result


def _verification_inputs(
    model: AurisSinger, batch: int, s: int, t: int, voiced: bool, seed: int
) -> dict[str, torch.Tensor]:
    torch.manual_seed(seed)
    durations = torch.full((batch, s), t // s, dtype=torch.long)
    durations[:, : t % s] += 1
    return {
        "phonemes": torch.randint(1, model.n_vocab, (batch, s)),
        "phoneme_lengths": torch.full((batch,), s, dtype=torch.long),
        "durations": durations,
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
        wav, source = session.run(["wav", "source"], {k: v.numpy() for k, v in inputs.items()})
        ref = ref.numpy()
        expected = (inputs["phonemes"].shape[0], 1, inputs["f0"].shape[1] * model.hop_length)
        if ref.shape != expected or wav.shape != expected or source.shape != expected:
            raise ValueError(
                "exported ONNX returned an invalid waveform or excitation shape: "
                f"reference={ref.shape}, wav={wav.shape}, source={source.shape}, expected={expected}"
            )
        if (
            not np.isfinite(ref).all()
            or not np.isfinite(wav).all()
            or not np.isfinite(source).all()
        ):
            raise ValueError("exported ONNX produced non-finite waveform or excitation values")
        return ref, wav, source

    # Sizes deliberately different from the export-time example inputs, so a
    # dimension the trace accidentally baked in fails loudly here.
    ref, out, _ = run(_verification_inputs(model, batch=1, s=5, t=23, voiced=False, seed=0))
    unvoiced_diff = float(np.abs(out - ref).max())
    if not math.isfinite(unvoiced_diff) or unvoiced_diff > 1e-4:
        raise ValueError(f"unvoiced output differs from PyTorch by {unvoiced_diff:.2e}")

    inputs = _verification_inputs(model, batch=2, s=9, t=64, voiced=True, seed=1)
    _, out, source = run(inputs)
    with torch.no_grad():
        z, f0, energy, voiced, g = wrapper.latent(
            **{k: v for k, v in inputs.items() if k != "source_noise"}
        )
        ref, _ = model.generator(z, f0, energy, voiced, g=g, source=torch.from_numpy(source))
    voiced_diff = float(np.abs(out - ref.numpy()).max())
    if not math.isfinite(voiced_diff) or voiced_diff > 1e-4:
        raise ValueError(
            f"voiced output differs from PyTorch by {voiced_diff:.2e} given the same excitation"
        )

    f0_hz = float(inputs["f0"][0, 0])
    period = model.sample_rate / f0_hz
    excitation = source[0, 0]
    peak = float(excitation.max())
    impulses = np.flatnonzero(excitation > peak * 0.5) if peak > 0.0 else np.empty(0, dtype=int)
    expected_impulses = excitation.size / period
    minimum_impulses = max(2, math.floor(expected_impulses) - 1)
    maximum_impulses = math.ceil(expected_impulses) + 1
    if not minimum_impulses <= impulses.size <= maximum_impulses:
        raise ValueError(
            f"excitation has {impulses.size} impulses, expected about {expected_impulses:.1f}"
        )
    gaps = np.diff(impulses)
    spacing = float(gaps.mean())
    worst_gap_error = float(np.abs(gaps - period).max())
    if (
        not math.isfinite(spacing)
        or not math.isfinite(worst_gap_error)
        or worst_gap_error > max(1.0, period * 0.02)
    ):
        raise ValueError(
            f"excitation impulse spacing is {spacing:.1f} samples, expected {period:.1f}"
        )

    return {
        "unvoiced_max_diff": unvoiced_diff,
        "voiced_max_diff": voiced_diff,
        "impulse_spacing_error": abs(spacing - period) / period,
    }
