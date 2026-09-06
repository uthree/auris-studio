# /// script
# requires-python = ">=3.11"
# dependencies = ["onnx==1.22.0"]
# ///
"""Regenerate tiny, deterministic LeapSinger contract fixtures with `uv run generate.py`.

These graphs contain no trained weights or upstream code. Their waveform records the
token, duration, pitch, voiced flag, mel layout, and optional speaker embedding that
the host supplied. Cargo tests consume the checked-in ONNX files without Python.
"""

from pathlib import Path

import onnx
from onnx import TensorProto as T
from onnx import helper as h

ROOT = Path(__file__).parent
MELS = 128


def value(name, dtype, shape):
    return h.make_tensor_value_info(name, dtype, shape)


def tensor(name, dtype, shape, values):
    return h.make_tensor(name, dtype, shape, values)


def node(op, inputs, output, **attrs):
    return h.make_node(op, inputs, [output], **attrs)


def save(name, nodes, inputs, outputs, constants):
    graph = h.make_graph(nodes, name, inputs, outputs, constants)
    model = h.make_model(
        graph,
        producer_name="auris-singer contract fixture",
        opset_imports=[h.make_opsetid("", 13)],
        ir_version=8,
    )
    onnx.checker.check_model(model)
    onnx.save(model, ROOT / name)


def acoustic(full=False, speaker=False, invalid=False):
    inputs = [
        value("tokens", T.INT64, [1, "tokens"]),
        value("durations", T.INT64, [1, "tokens"]),
        value("f0", T.FLOAT, [1, "frames"]),
    ]
    constants = [
        tensor("axis0", T.INT64, [1], [0]),
        tensor("axis1", T.INT64, [1], [1]),
        tensor("axis2", T.INT64, [1], [2]),
        tensor("zero", T.INT64, [], [0]),
        tensor("one", T.INT64, [], [1]),
        tensor("hundredth", T.FLOAT, [], [0.01]),
        tensor("pitch_scale", T.FLOAT, [], [0.00001]),
        tensor("voiced_scale", T.FLOAT, [], [0.1]),
        tensor("speaker_scale", T.FLOAT, [], [0.001]),
        tensor("zero_float", T.FLOAT, [], [0.0]),
        tensor("mel_padding", T.INT64, [6], [0, 0, 0, 0, 0, MELS - 4]),
    ]
    nodes = [
        node("Shape", ["f0"], "f0_shape"),
        node("Gather", ["f0_shape", "one"], "frames", axis=0),
        node("Range", ["zero", "frames", "one"], "positions"),
        node("Unsqueeze", ["positions", "axis1"], "position_column"),
        node("CumSum", ["durations", "one"], "ends"),
        node("GreaterOrEqual", ["position_column", "ends"], "past_token"),
        node("Cast", ["past_token"], "past_token_int", to=T.INT64),
        node("ReduceSum", ["past_token_int", "axis1"], "token_index", keepdims=0),
        node("Squeeze", ["tokens", "axis0"], "token_vector"),
        node("Squeeze", ["durations", "axis0"], "duration_vector"),
        node("Gather", ["token_vector", "token_index"], "frame_tokens", axis=0),
        node("Gather", ["duration_vector", "token_index"], "frame_durations", axis=0),
        node("Cast", ["frame_tokens"], "token_float", to=T.FLOAT),
        node("Cast", ["frame_durations"], "duration_float", to=T.FLOAT),
        node("Unsqueeze", ["token_float", "axis0"], "token_matrix"),
        node("Unsqueeze", ["duration_float", "axis0"], "duration_matrix"),
        node("Mul", ["token_matrix", "hundredth"], "token_scaled"),
        node("Mul", ["duration_matrix", "hundredth"], "duration_scaled"),
        node("Mul", ["f0", "pitch_scale"], "pitch_scaled"),
        node("Mul", ["f0", "zero_float"], "zero_matrix"),
        node("Add", ["token_scaled", "pitch_scaled"], "channel0"),
        node("Unsqueeze", ["channel0", "axis2"], "mel0"),
        node("Unsqueeze", ["duration_scaled", "axis2"], "mel1"),
    ]
    if full:
        inputs.append(value("uv", T.FLOAT, [1, "frames"]))
        nodes.append(node("Mul", ["uv", "voiced_scale"], "voiced_scaled"))
    else:
        nodes.append(node("Identity", ["zero_matrix"], "voiced_scaled"))
    nodes.append(node("Unsqueeze", ["voiced_scaled", "axis2"], "mel2"))
    if speaker:
        inputs.append(value("spk_embed", T.FLOAT, [1, 2]))
        nodes.extend(
            [
                node("ReduceSum", ["spk_embed"], "speaker_sum", keepdims=0),
                node("Mul", ["speaker_sum", "speaker_scale"], "speaker_scaled"),
                node("Add", ["zero_matrix", "speaker_scaled"], "speaker_matrix"),
            ]
        )
    else:
        nodes.append(node("Identity", ["zero_matrix"], "speaker_matrix"))
    nodes.extend(
        [
            node("Unsqueeze", ["speaker_matrix", "axis2"], "mel3"),
            node("Concat", ["mel0", "mel1", "mel2", "mel3"], "first_mels", axis=2),
            node("Pad", ["first_mels", "mel_padding"], "padded_mels"),
        ]
    )
    if invalid:
        inputs.append(value("unsupported_input", T.FLOAT, [1]))
        nodes.append(node("Add", ["padded_mels", "unsupported_input"], "mel"))
    elif full:
        nodes.append(node("Transpose", ["padded_mels"], "mel", perm=[0, 2, 1]))
    else:
        nodes.append(node("Identity", ["padded_mels"], "mel"))
    layout = [1, MELS, "frames"] if full else [1, "frames", MELS]
    name = "invalid_acoustic" if invalid else "full" if full else "diffsinger"
    save(
        f"{name}{'_speaker' if speaker else ''}.onnx",
        nodes,
        inputs,
        [value("mel", T.FLOAT, layout)],
        constants,
    )


def vocoder(hop=256, trim=0, name="vocoder.onnx"):
    inputs = [
        value("mel", T.FLOAT, [1, "frames", MELS]),
        value("f0", T.FLOAT, [1, 1, "frames"]),
        value("uv", T.FLOAT, [1, 1, "frames"]),
    ]
    constants = [
        tensor("axis1", T.INT64, [1], [1]),
        tensor("axis2", T.INT64, [1], [2]),
        tensor("hop", T.INT64, [1], [hop]),
        tensor("waveform_shape", T.INT64, [3], [1, 1, -1]),
        tensor("pitch_scale", T.FLOAT, [], [0.00001]),
        tensor("uv_scale", T.FLOAT, [], [0.05]),
    ]
    nodes = []
    for channel in range(4):
        constants.extend(
            [
                tensor(f"index{channel}", T.INT64, [], [channel]),
                tensor(f"weight{channel}", T.FLOAT, [], [float(channel + 1)]),
            ]
        )
        nodes.extend(
            [
                node("Gather", ["mel", f"index{channel}"], f"mel{channel}", axis=2),
                node("Mul", [f"mel{channel}", f"weight{channel}"], f"weighted{channel}"),
            ]
        )
    nodes.extend(
        [
            node("Add", ["weighted0", "weighted1"], "pair01"),
            node("Add", ["weighted2", "weighted3"], "pair23"),
            node("Add", ["pair01", "pair23"], "mel_sum"),
            node("Squeeze", ["f0", "axis1"], "f0_matrix"),
            node("Squeeze", ["uv", "axis1"], "uv_matrix"),
            node("Mul", ["f0_matrix", "pitch_scale"], "pitch_scaled"),
            node("Mul", ["uv_matrix", "uv_scale"], "uv_scaled"),
            node("Add", ["mel_sum", "pitch_scaled"], "with_pitch"),
            node("Add", ["with_pitch", "uv_scaled"], "per_frame"),
            node("Shape", ["per_frame"], "frame_shape"),
            node("Concat", ["frame_shape", "hop"], "expanded_shape", axis=0),
            node("Unsqueeze", ["per_frame", "axis2"], "per_frame_column"),
            node("Expand", ["per_frame_column", "expanded_shape"], "repeated"),
            node("Reshape", ["repeated", "waveform_shape"], "all_samples"),
        ]
    )
    if trim:
        constants.extend(
            [
                tensor("start", T.INT64, [1], [0]),
                tensor("end", T.INT64, [1], [-trim]),
            ]
        )
        nodes.append(node("Slice", ["all_samples", "start", "end", "axis2"], "waveform"))
    else:
        nodes.append(node("Identity", ["all_samples"], "waveform"))
    save(name, nodes, inputs, [value("waveform", T.FLOAT, [1, 1, "samples"])], constants)


if __name__ == "__main__":
    acoustic()
    acoustic(full=True)
    acoustic(speaker=True)
    acoustic(invalid=True)
    vocoder()
    vocoder(hop=512, trim=256, name="vocoder_v3x.onnx")
    vocoder(trim=1, name="wrong_length_vocoder.onnx")
