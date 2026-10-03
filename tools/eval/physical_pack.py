"""Fixed percussive copy-synthesis metrics and reference loading, development only."""

import hashlib

import numpy as np
import soundfile as sf
from physical_mel import RATE, configuration, features, normalize
from scipy.signal import butter, sosfilt

FAMILIES = ("bass", "bell", "mallet", "kick", "snare", "closed_hat", "open_hat", "crash", "ride", "tom")
PHASES = (("attack", 0, 5, 0.3), ("body", 5, 25, 0.3), ("tail", 25, None, 0.4))


def analysis_audio(audio):
    """Reject sub-band microphone rumble before whole-note level normalization."""
    return sosfilt(butter(2, 30, fs=RATE, btype="highpass", output="sos"), audio, axis=-1)


def pack_features(audio):
    return features(analysis_audio(audio))


def errors(target, candidate):
    result = {}
    for name, start, stop, _ in PHASES:
        result[name] = np.mean([abs(a[..., start:stop] - b[..., start:stop]).mean(axis=(1, 2))
                               for a, b in zip(target, candidate, strict=True)], axis=0)
    result["mel"] = sum(weight * result[name] for name, _, _, weight in PHASES)
    return result


def envelope(audio):
    x = normalize(analysis_audio(audio))
    count = x.shape[-1] // 480
    rms = np.sqrt(np.mean(x[:, :count * 480].reshape(len(x), count, 480)**2, axis=-1))
    return 20 * np.log10(rms + 1e-4)


def t90(audio):
    """Seconds containing 90% of the band-limited energy in the fixed excerpt."""
    x = analysis_audio(audio)
    cumulative = np.cumsum(x**2, axis=-1)
    return np.argmax(cumulative >= cumulative[:, -1:] * 0.9, axis=-1) / RATE


def spectrum(audio, rate, start, end):
    """Match the offline drum analyzer's Hann-windowed power measurements."""
    size = 2048
    powers = np.zeros(size // 2 + 1)
    window = 0.5 - 0.5 * np.cos(2 * np.pi * np.arange(size) / size)
    for offset in range(start, end, size // 2):
        block = np.zeros(size)
        count = min(size, end - offset)
        block[:count] = audio[offset:offset + count]
        powers += abs(np.fft.rfft(block * window))**2
    hz = np.fft.rfftfreq(size, 1 / rate)
    powers[hz < 20] = 0
    index = int(np.argmax(powers))
    powers /= powers.sum()
    low_bins = np.flatnonzero((hz >= 35) & (hz <= 800))
    return {"concentration": float(powers[max(0, index - 1):index + 2].sum()),
            "midrange": float(powers[(hz >= 250) & (hz < 4000)].sum()),
            "low": float(powers[hz < 250].sum()), "body": float(powers[(hz >= 250) & (hz < 2000)].sum()),
            "centroid": float(np.sum(hz * powers)), "low_peak": float(hz[low_bins[np.argmax(powers[low_bins])]])}


def transient(audio, rate):
    hop = round(rate * 0.01)
    loudest = max(np.mean(audio[index:index + hop]**2) for index in range(0, len(audio), hop))
    sustained = np.mean(audio[len(audio) * 4 // 5:]**2) / loudest
    return max(0, 1 - sustained / 0.3)


def snare_fitness(audio, rate=48000):
    """Offline snare guard matching the concentration/midrange criterion in drum_analysis."""
    start = int(np.flatnonzero(audio**2 >= np.max(audio**2) * 0.0001)[0])
    measured = spectrum(audio, rate, start, len(audio))
    return float(min(1, measured["midrange"] / 0.45) * max(0, 1 - measured["concentration"] / 0.3) * transient(audio, rate))


def tom_role_margin(audio, rate=48000):
    """Tom minus kick acoustic fitness, without passing a MIDI key to the criterion."""
    start = int(np.flatnonzero(audio**2 >= np.max(audio**2) * 0.0001)[0])
    measured = spectrum(audio, rate, start, len(audio))
    attack = spectrum(audio, rate, start, start + round(rate * 0.06))
    body = spectrum(audio, rate, start + round(rate * 0.06), start + round(rate * 0.2))
    fall = 12 * np.log2(attack["low_peak"] / body["low_peak"])
    rise = lambda value, low, high: np.clip((value - low) / (high - low), 0, 1)
    tonal = min(1, measured["concentration"] / 0.45)
    deep = 1 - rise(measured["centroid"], 85, 140)
    falling = rise(fall, 4, 9) * (1 - rise(body["low_peak"], 100, 200))
    kick = measured["low"] * (0.55 + 0.45 * tonal) * max(deep, falling)
    tom = min(1, measured["low"] + measured["body"] * 0.65) * tonal * (1 - rise(abs(fall), 4, 10))
    tom *= 0.25 + 0.75 * rise(measured["centroid"], 80, 140)
    return float((tom - kick) * transient(audio, rate))


def snare_wire_ratio(audio, rate=48000):
    """Preserve diffuse wires over the pitched head at 25..200 ms, as in the DSP test."""
    samples = audio[round(rate * 0.025):round(rate * 0.2)]
    time = np.arange(len(samples)) / rate

    def amplitude(center):
        frequencies = center * 2**(np.linspace(-1, 1, 15) / 8)
        powers = [abs(np.sum(samples * np.exp(-2j * np.pi * hz * time)))**2 for hz in frequencies]
        return np.sqrt(np.mean(powers))

    return float(amplitude(2000) / max(amplitude(185), 1e-15))


def rows(notes, reference, audio):
    values = errors(pack_features(reference), pack_features(audio))
    values["envelope_db"] = abs(envelope(reference) - envelope(audio)).mean(axis=-1)
    values["t90_error_seconds"] = abs(t90(reference) - t90(audio))
    return [{"id": note["id"], "source": note["source"],
             **{name: float(array[index]) for name, array in values.items()}}
            for index, note in enumerate(notes)]


def metric_configuration():
    return configuration() | {"loss": "0.3 attack + 0.3 body + 0.4 tail log-mel L1, mean of resolutions",
                              "attack_frames": 5,
                              "phases_seconds": {"attack": [0, 0.05], "body": [0.05, 0.25], "tail": [0.25, 3]},
                              "preprocessing": "causal second-order 30 Hz highpass on both signals before whole-note RMS",
                              "secondary": "20 ms RMS-envelope L1 in dB; absolute 90%-energy-time error in seconds"}


def load(root, manifest, family, split):
    notes = [note for note in manifest["notes"] if note.get("family", note["model"]) == family and note["split"] == split]
    if not notes:
        raise ValueError(f"empty {family} {split} cohort")
    audio = []
    for note in notes:
        path = root / note["path"]
        if hashlib.sha256(path.read_bytes()).hexdigest() != note["sha256"]:
            raise ValueError(f"reference hash mismatch: {path}")
        samples, rate = sf.read(path)
        if rate != RATE or samples.ndim != 1 or len(samples) != int(manifest["seconds"] * RATE):
            raise ValueError(f"invalid prepared reference: {path}")
        audio.append(samples)
    return notes, np.array(audio)
