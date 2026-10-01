"""Pitch trajectories and phase-balanced losses for sustained copy synthesis."""

import numpy as np
from physical_mel import HOP, RATE, features, normalize
from scipy.ndimage import gaussian_filter1d
from scipy.signal import stft


def pitch_track(audio, lower, upper):
    """Track a fundamental in a known sub-octave interval at 10 ms resolution.

    A 4096-sample Hann window and log-parabolic peak interpolation are fixed for
    references and renders. This tracker is deliberately unsuitable for polyphony
    or intervals spanning an octave; reject those instead of accepting octave errors.
    """
    if not 0 < lower < upper < lower * 2:
        raise ValueError("pitch tracker requires a sub-octave interval")
    samples = np.asarray(audio, dtype=float)
    if samples.ndim != 1 or len(samples) < 4096 or not np.isfinite(samples).all():
        raise ValueError("invalid trajectory audio")
    frequencies, times, spectrum = stft(samples, RATE, nperseg=4096,
                                        noverlap=4096 - HOP, boundary="zeros", padded=True)
    power = abs(spectrum)
    bins = np.flatnonzero((frequencies >= lower) & (frequencies <= upper))
    if len(bins) < 3:
        raise ValueError("pitch interval narrower than three FFT bins")
    peaks = bins[np.argmax(power[bins], axis=0)]
    columns = np.arange(len(times))
    a, b, c = [np.log(power[peaks + shift, columns] + 1e-15) for shift in (-1, 0, 1)]
    denominator = a - 2 * b + c
    fraction = np.divide(0.5 * (a - c), denominator, out=np.zeros_like(a),
                         where=abs(denominator) > 1e-12)
    hz = (peaks + np.clip(fraction, -0.5, 0.5)) * RATE / 4096
    level = np.sqrt(np.mean(power**2, axis=0))
    fundamental = power[peaks, columns]
    voiced = ((level > max(level.max() * 0.02, 1e-7))
              & (fundamental > power.max(axis=0) * 0.01)
              & (peaks != bins[0]) & (peaks != bins[-1]))
    return times, hz, voiced


def curve(audio, pitch, low_pitch, high_pitch, hold):
    base = 440 * 2 ** ((pitch - 69) / 12)
    lower = 440 * 2 ** ((low_pitch - 69 - 0.8) / 12)
    upper = 440 * 2 ** ((high_pitch - 69 + 0.8) / 12)
    times, hz, voiced = pitch_track(audio, lower, upper)
    valid = voiced & (times >= 0.12) & (times < hold - 0.08)
    if valid.sum() < 20:
        raise ValueError("insufficient voiced trajectory")
    # Silence and attacks are not pitch evidence. Interpolate only those missing
    # measurements; retain measured vibrato and portamento without time warping.
    # Guard decimal rounding and f32 conversion at note-off. An event exactly at
    # hold is release, not a final pitch measurement (e.g. 7.069999999 -> 7.07).
    time = times[times < hold - 1e-5]
    bends = np.interp(time, times[valid], 12 * np.log2(hz[valid] / base))
    if np.max(abs(bends)) > 12:
        raise ValueError("trajectory exceeds the instrument bend range")
    return [{"seconds": round(float(t), 5), "value": round(float(value), 6)}
            for t, value in zip(time, bends, strict=True)], (lower, upper)


def expression_curve(audio, hold):
    """Fixed, gain-independent 50 ms bow-expression guide from a real envelope.

    Smooth 20 ms RMS with a 60 ms Gaussian before decimation. CC11 affects both
    bow motion and output gain, so use the square root of relative amplitude.
    This is supplied performance information, not a predicted player gesture.
    """
    audio = np.asarray(audio, dtype=float)
    count = len(audio) // 480
    rms = np.sqrt(np.mean(audio[:count * 480].reshape(count, 480)**2, axis=1))
    times = (np.arange(count) + 0.5) * 0.02
    smoothed = gaussian_filter1d(rms, 3)
    scale = np.percentile(smoothed[times < hold], 95)
    if scale < 1e-8:
        raise ValueError("silent expression guide")
    control_times = np.arange(0, hold - 1e-5, 0.05)
    amplitude = np.interp(control_times, times, smoothed) / scale
    return [{"seconds": round(float(t), 5), "value": round(float(np.sqrt(np.clip(a, 0.0064, 1))), 6)}
            for t, a in zip(control_times, amplitude, strict=True)]


def phase_masks(note, frame_count):
    times = np.arange(frame_count) * HOP / RATE
    hold = note.get("hold", note["seconds"])
    valid = times < note["seconds"]
    attack = (times < 0.15) & valid
    release = (times >= hold) & valid if hold < note["seconds"] else np.zeros(frame_count, dtype=bool)
    transition = np.zeros(frame_count, dtype=bool)
    if "transition" in note:
        start, end = note["transition"]
        transition = (times >= start) & (times < end) & ~attack & ~release & valid
    early = (times >= 0.15) & (times < 1.0) & ~transition & ~release & valid
    late = (times >= 1.0) & ~transition & ~release & valid
    masks = {"attack": attack, "early": early, "late": late,
             "transition": transition, "release": release}
    weights = {"attack": 0.15, "early": 0.15, "late": 0.40,
               "transition": 0.20, "release": 0.10}
    present = {key: mask for key, mask in masks.items() if mask.any()}
    total = sum(weights[key] for key in present)
    return present, {key: weights[key] / total for key in present}


def temporal_error(target, candidate, note):
    """Keep attack, late sustain, glissando and release separately visible."""
    errors = {}
    for real, synth in zip(target, candidate, strict=True):
        if real.shape != synth.shape:
            raise ValueError("trajectory feature dimensions differ")
        masks, weights = phase_masks(note, real.shape[-1])
        difference = abs(real - synth)
        for phase, mask in masks.items():
            errors.setdefault(phase, []).append(float(difference[..., mask].mean()))
    phases = {key: float(np.mean(values)) for key, values in errors.items()}
    return sum(phases[key] * weights[key] for key in phases), phases


def envelope_error(reference, candidate):
    """Whole-note-normalized 20 ms RMS-envelope error in decibels, no alignment."""
    a, b = normalize(reference)[0], normalize(candidate)[0]
    count = len(a) // 480
    rms_a = np.sqrt(np.mean(a[:count * 480].reshape(count, 480)**2, axis=1))
    rms_b = np.sqrt(np.mean(b[:count * 480].reshape(count, 480)**2, axis=1))
    return float(np.mean(abs(20 * np.log10((rms_a + 1e-4) / (rms_b + 1e-4)))))


def metrics(reference, candidate, note, target=None):
    mel, phases = temporal_error(target or features(reference), features(candidate), note)
    result = {"mel": mel, "phases": phases, "envelope_db": envelope_error(reference, candidate)}
    if "pitch_bounds_hz" in note:
        bounds = note["pitch_bounds_hz"]
        times, real_hz, real_voiced = pitch_track(reference, *bounds)
        _, synth_hz, synth_voiced = pitch_track(candidate, *bounds)
        mask = real_voiced & (times >= 0.15) & (times < note["hold"] - 0.08)
        if mask.sum() < 20:
            raise ValueError("insufficient reference pitch evidence")
        both = mask & synth_voiced
        result["voiced_coverage"] = float(both.sum() / mask.sum())
        result["pitch_cents"] = (float(np.mean(abs(1200 * np.log2(synth_hz[both] / real_hz[both]))))
                                 if both.any() else None)
        # Include missing voiced frames in pitch loss rather than rewarding silence.
        result["pitch_penalty_cents"] = float(np.mean(np.where(
            synth_voiced[mask], abs(1200 * np.log2(synth_hz[mask] / real_hz[mask])), 100.0)))
    return result
