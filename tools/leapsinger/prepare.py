"""Package an upstream ONNX export as a portable Auris LeapSinger voice.

Run this with the Python interpreter of the upstream LeapSinger uv environment.
The checkpoint supplies its exact vocabulary and projected speaker vectors.
"""

import argparse
import importlib
import json
import shutil
import sys
from pathlib import Path


def phoneme_dictionary(config: dict) -> str:
    """Keep checkpoint token IDs intact instead of using a newer repository dictionary."""
    phonemes = config.get("phonemes")
    if (
        not isinstance(phonemes, list)
        or not phonemes
        or phonemes[0] != "pau"
        or len(phonemes) != config.get("n_phonemes")
        or any(
            not isinstance(token, str)
            or not token
            or token != token.strip()
            or len(token.split()) != 1
            or "#" in token
            for token in phonemes
        )
        or len(set(phonemes)) != len(phonemes)
    ):
        raise ValueError(
            "The checkpoint needs its complete, unique, silence-first phoneme vocabulary"
        )
    return "\n".join(phonemes) + "\n"


def main() -> None:
    """Copy the matching vocoder and write a manifest beside an existing acoustic export."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--upstream", required=True, type=Path)
    parser.add_argument("--checkpoint", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--acoustic", default="acoustic.onnx")
    parser.add_argument("--name", default="LeapSinger singer")
    parser.add_argument("--variant", choices=["full", "diffsinger"], default="full")
    parser.add_argument("--hop", type=int, choices=[256, 512], default=256)
    parser.add_argument(
        "--baked", action="store_true", help="The acoustic export has a fixed speaker"
    )
    parser.add_argument(
        "--speaker-names", nargs="+", help="Labels in checkpoint speaker ID order"
    )
    args = parser.parse_args()
    if args.variant == "full" and args.hop != 256:
        parser.error("full exports use hop 256")
    acoustic = Path(args.acoustic)
    if acoustic.is_absolute() or ".." in acoustic.parts:
        parser.error("the acoustic path must be relative to the output folder")
    if not (args.output / args.acoustic).is_file():
        parser.error("export the acoustic ONNX model into the output folder first")
    entry = args.output / "singer.leapsinger.json"
    if entry.exists():
        parser.error(
            "the output already contains singer.leapsinger.json; use the setup UI to edit it"
        )
    sys.path.insert(0, str(args.upstream.resolve()))
    infer = importlib.import_module("infer")
    helper = importlib.import_module("export.spk_embed")
    model, config = infer.load_acoustic(str(args.checkpoint), device="cpu")
    dictionary = phoneme_dictionary(config)
    speakers = []
    if helper.has_speakers(model) and not args.baked:
        count = int(config["n_speakers"])
        names = args.speaker_names or [f"Speaker {index}" for index in range(count)]
        if (
            len(names) != count
            or len(set(names)) != count
            or any(not name.strip() for name in names)
        ):
            parser.error(
                "provide one distinct speaker name per checkpoint speaker, in ID order"
            )
        speakers = [
            {"name": name, "embedding": helper.speaker_vector(model, index).tolist()}
            for index, name in enumerate(names)
        ]
    suffix = "x" if args.hop == 512 else ""
    vocoder_name = f"nhv_v3_2_1{suffix}.onnx"
    vocoder = args.upstream / "checkpoints" / vocoder_name
    if not vocoder.is_file():
        parser.error(f"the current upstream vocoder was not found: {vocoder}")
    manifest = {
        "format_version": 1,
        "name": args.name,
        "acoustic": Path(args.acoustic).as_posix(),
        "vocoder": vocoder_name,
        "phonemes": "ja.phonemes",
        "variant": args.variant,
        "sample_rate": 44100,
        "hop_size": args.hop,
        "num_mel_bins": int(config["mel_bins"]),
        "speakers": speakers,
    }
    shutil.copyfile(vocoder, args.output / vocoder_name)
    (args.output / "ja.phonemes").write_text(dictionary, encoding="utf-8")
    # Preserve the release's acoustic credits and singer terms beside the exported voice.
    for terms in args.checkpoint.parent.glob("*.txt"):
        shutil.copyfile(terms, args.output / terms.name)
    # Exclusive creation prevents rerunning the helper from replacing a customized voice entry.
    with entry.open("x", encoding="utf-8") as output:
        json.dump(manifest, output, ensure_ascii=False, indent=2, allow_nan=False)
        output.write("\n")
    print(entry.resolve())


if __name__ == "__main__":
    main()
