# MuScriptor implementation attribution

The tensor wrappers in `tools/music-models/export_muscriptor.py` and the token state
machine in `src/mixture/tokens.rs` adapt the inference behavior of MuScriptor 0.3.0:
`modules/conditioners.py`, `modules/transformer.py`, `models/lm.py`, `events.py` and
`tokenizer/mt3.py`. Source: <https://github.com/muscriptor/muscriptor>.
Changes include explicit ONNX cache tensors, ONNX STFT, fixed Small geometry,
greedy Rust decoding, bounded validation and user-controlled local model preparation.

The following MIT notice applies to that upstream software. **It does not license the
model weights.** The user's checkpoint and converted weights retain CC BY-NC 4.0 and
the model's additional terms: <https://huggingface.co/MuScriptor/muscriptor-small>.
No checkpoint or converted weights are included in Auris' source or release artifacts.

MIT License

Copyright (c) 2026 Kyutai x Mirelo

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
