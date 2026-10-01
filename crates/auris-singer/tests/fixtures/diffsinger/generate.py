# /// script
# dependencies = ["onnx", "numpy"]
# ///
"""Build tiny arithmetic ONNX contracts; these files contain no learned weights."""

from pathlib import Path

import numpy as np
import onnx
from onnx import TensorProto as T
from onnx import helper as h
from onnx import numpy_helper

ROOT = Path(__file__).parent


class Graph:
    def __init__(self):
        self.nodes = []
        self.constants = []

    def constant(self, values, dtype=np.float32):
        name = f"constant_{len(self.constants)}"
        self.constants.append(numpy_helper.from_array(np.asarray(values, dtype=dtype), name))
        return name

    def op(self, operation, *inputs, **attributes):
        name = f"value_{len(self.nodes)}"
        self.nodes.append(h.make_node(operation, list(inputs), [name], **attributes))
        return name

    def scaled(self, value, scale):
        return self.op("Mul", value, self.constant(scale))

    def sum_float(self, value):
        return self.op("ReduceSum", self.op("Cast", value, to=T.FLOAT), keepdims=0)

    def save(self, filename, inputs, outputs):
        graph = h.make_graph(self.nodes, filename, inputs, outputs, self.constants)
        model = h.make_model(graph, opset_imports=[h.make_opsetid("", 17)], ir_version=8)
        onnx.checker.check_model(model)
        onnx.save(model, ROOT / filename)


def vi(name, kind, shape):
    return h.make_tensor_value_info(name, kind, shape)


def acoustic(filename, legacy=False, speaker=False):
    g = Graph()
    inputs = [
        vi("tokens", T.INT64, [1, "N"]),
        vi("durations", T.INT64, [1, "N"]),
        vi("f0", T.FLOAT, [1, "F"]),
    ]
    signal = g.scaled("f0", 0.0001)
    for name, scale in [("tokens", 0.01), ("durations", 0.001)]:
        signal = g.op("Add", signal, g.scaled(g.sum_float(name), scale))
    name = "speedup" if legacy else "steps"
    inputs += [
        vi(name, T.INT64, [1] if legacy else []),
        vi("depth", T.INT64 if legacy else T.FLOAT, [1] if legacy else []),
    ]
    signal = g.op("Add", signal, g.scaled(g.sum_float(name), 0.001))
    signal = g.op("Add", signal, g.scaled(g.sum_float("depth"), 0.0001 if legacy else 0.1))
    if not legacy:
        for name in ["energy", "breathiness", "voicing", "tension", "gender", "velocity"]:
            inputs.append(vi(name, T.FLOAT, [1, "F"]))
            signal = g.op(
                "Add",
                signal,
                g.scaled(
                    name, 0.1 if name in ["energy", "breathiness", "voicing", "tension"] else 0.01
                ),
            )
    if speaker:
        inputs += [vi("spk_embed", T.FLOAT, [1, "F", 2]), vi("languages", T.INT64, [1, "N"])]
        spk = g.op("ReduceSum", "spk_embed", g.constant([2], np.int64), keepdims=0)
        signal = g.op("Add", signal, g.scaled(spk, 0.001))
        signal = g.op("Add", signal, g.scaled(g.sum_float("languages"), 0.01))
    mel = g.op("Unsqueeze", signal, g.constant([2], np.int64))
    shape = g.op("Concat", g.op("Shape", signal), g.constant([2], np.int64), axis=0)
    mel = g.op("Expand", mel, shape)
    g.nodes.append(h.make_node("Identity", [mel], ["mel"]))
    g.save(filename, inputs, [vi("mel", T.FLOAT, [1, "F", 2])])


def linguistic(word_mode):
    g = Graph()
    inputs = [vi("tokens", T.INT64, [1, "N"])]
    names = ["word_div", "word_dur"] if word_mode else ["ph_dur"]
    for name in names:
        inputs.append(vi(name, T.INT64, [1, "W" if word_mode else "N"]))
    value = g.op("Cast", "tokens", to=T.FLOAT)
    for name in names:
        value = g.op("Add", value, g.scaled(g.sum_float(name), 0.01))
    value = g.op("Unsqueeze", value, g.constant([2], np.int64))
    shape = g.op("Concat", g.op("Shape", "tokens"), g.constant([2], np.int64), axis=0)
    value = g.op("Expand", value, shape)
    g.nodes.append(h.make_node("Identity", [value], ["encoder_out"]))
    g.save(
        "linguistic_word.onnx" if word_mode else "linguistic_phone.onnx",
        inputs,
        [vi("encoder_out", T.FLOAT, [1, "N", 2])],
    )


def variance():
    g = Graph()
    channels = ["energy", "breathiness", "voicing", "tension"]
    inputs = [
        vi("encoder_out", T.FLOAT, [1, "N", 2]),
        vi("ph_dur", T.INT64, [1, "N"]),
        vi("pitch", T.FLOAT, [1, "F"]),
    ]
    inputs += [vi(name, T.FLOAT, [1, "F"]) for name in channels]
    inputs += [vi("retake", T.BOOL, [1, "F", 4]), vi("steps", T.INT64, [])]
    encoder = g.scaled(g.op("ReduceMean", "encoder_out", keepdims=0), 0.01)
    outputs = []
    for at, name in enumerate(channels):
        value = g.op("Add", g.scaled("pitch", 0.01 * (at + 1)), encoder)
        g.nodes.append(h.make_node("Identity", [value], [f"{name}_pred"]))
        outputs.append(vi(f"{name}_pred", T.FLOAT, [1, "F"]))
    g.save("variance.onnx", inputs, outputs)


def vocoder():
    g = Graph()
    signal = g.op("ReduceMean", "mel", axes=[2], keepdims=0)
    signal = g.op("Unsqueeze", signal, g.constant([2], np.int64))
    shape = g.op("Concat", g.op("Shape", "f0"), g.constant([512], np.int64), axis=0)
    signal = g.op("Expand", signal, shape)
    signal = g.op("Reshape", signal, g.constant([1, -1], np.int64))
    g.nodes.append(h.make_node("Identity", [signal], ["waveform"]))
    g.save(
        "vocoder.onnx",
        [vi("mel", T.FLOAT, [1, "F", 2]), vi("f0", T.FLOAT, [1, "F"])],
        [vi("waveform", T.FLOAT, [1, "S"])],
    )


if __name__ == "__main__":
    acoustic("modern.onnx")
    acoustic("legacy.onnx", legacy=True)
    acoustic("speaker.onnx", speaker=True)
    linguistic(True)
    linguistic(False)
    variance()
    vocoder()
