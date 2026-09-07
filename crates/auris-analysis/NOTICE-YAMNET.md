# Google YAMNet attribution

Copyright 2019 The TensorFlow Authors. All Rights Reserved.
YAMNet architecture, preprocessing, labels and weights are distributed under Apache-2.0.
The license text is in the repository's `LICENSE`.

`src/yamnet-labels.json` is the `display_name` column of Google's
`research/audioset/yamnet/yamnet_class_map.csv`, converted to a JSON string array.
Source revision: `d598fb8b23d9cd2fb26b5789b8242de3f494aca7` in
[tensorflow/models](https://github.com/tensorflow/models/tree/d598fb8b23d9cd2fb26b5789b8242de3f494aca7/research/audioset/yamnet).

The explicit preparation script `tools/music-models/export_yamnet.py` fetches and verifies
the unmodified upstream Python source and official `yamnet.h5`. Its ONNX export uses a
fixed 15,600-sample waveform input, Google's portable DFT preprocessing, and only the
classification output. It is an Auris conversion, not an official Google ONNX release.
No weights are committed to this repository.

The [maintainer's answer about architecture and weight licensing](https://groups.google.com/g/audioset-users/c/Ly4guQnlv_o)
identifies Apache-2.0. The source and original weights URL are recorded in the preparation
script. Preserve this notice and Apache-2.0 license with any redistributed export.
