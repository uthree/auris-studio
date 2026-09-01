# Architecture

`auris-singer` is VITS adapted to singing voice synthesis at 48 kHz. This
document lists what changed and why.

## Overview

```
phonemes (IPA) ─► TextEncoder ──────────┐
                       │                │
                       │ (MAS during    │ duration expansion
                       │  training)     ▼
durations ────────────►└──────────► PriorEncoder ──► m_p, logs_p
                                        ▲                │
f0, energy curves ──────────────────────┘                │  KL
                                                         │
spectrogram ──► PosteriorEncoder ──► z ──► Flow ──► z_p ─┘
                                     │
                                     ▼
f0, energy ──► SourceSignalGenerator ──► NSF-HiFi-GAN ──► waveform
                                              ▲
                                     speaker embedding
```

At inference the posterior branch is replaced by sampling `z_p ~ N(m_p, σ_p)`
and running the flow in reverse.

## Differences from VITS

| Area | VITS | auris-singer |
| --- | --- | --- |
| Sample rate | 22.05 kHz | 48 kHz (`hop_length = 480`, 100 frames/s) |
| Sequence modules | 1D CNN / WaveNet / relative-position attention | modernized Transformer (RMSNorm, SwiGLU, RoPE, QK-Norm, fused SDPA) |
| Vocoder | HiFi-GAN | NSF-HiFi-GAN driven by an explicit source signal |
| Vocoder activation | LeakyReLU | SiLU |
| Duration | (stochastic) duration predictor | durations are an input; removed entirely |
| Pitch / energy | not modelled | explicit frame-level curves |
| Discriminator | MPD + multi-scale | MPD + multi-resolution STFT, speaker-conditional |
| Vocoder losses | mel L1 | multi-parameter mel + envelope loss (RefineGAN) |

## Modernized Transformer

[`modules/transformer.py`](../src/auris_singer/modules/transformer.py) provides
the encoder shared by `TextEncoder`, `PosteriorEncoder`, `PriorEncoder` and the
flow's coupling layers:

* **RMSNorm** in a pre-norm residual layout.
* **SwiGLU** feed-forward with an inner width of `8/3 · d` rounded up to a
  multiple of 64.
* **RoPE** applied to queries and keys, so the model generalizes over sequence
  lengths without a learned position table.
* **QK-Norm** — RMSNorm on queries and keys per head, which keeps the attention
  logits bounded and removes a common source of loss spikes in GAN training.
* **Flash Attention** via `F.scaled_dot_product_attention`; PyTorch selects the
  fused kernel automatically.

Tensors cross module boundaries channel-first `(B, C, T)` to match the rest of
the VITS code; the transpose happens inside the encoder.

Speaker conditioning is adaLN-style: the speaker vector produces a per-channel
scale and shift for both sub-layer norms. The projection is zero-initialized,
so conditioning starts as an exact no-op and the model is free to grow into it.

## Prior, durations and alignment

The model has no duration predictor: durations come from the DAW front-end.
Training data, however, is `(waveform, text)` with no alignment, so the
phoneme-to-frame alignment is recovered by **monotonic alignment search** (MAS)
exactly as in VITS.

This creates a mismatch: MAS scores frames against a *phoneme-level* Gaussian,
while the pitch and energy curves are *frame-level*. Building a
pitch-conditioned prior for every (phoneme, frame) pair would cost
`O(S · T · C)` memory. The prior is therefore factored in two:

1. `TextEncoder` emits a phoneme-level Gaussian `(m_p0, logs_p0)`. MAS searches
   against this one.
2. The hidden states are expanded by the resulting durations, combined with the
   f0/energy curves, and refined by `PriorEncoder`, which emits the
   `(m_p, logs_p)` used for the main KL term.

Both priors are trained: the loss carries a main KL term against `(m_p, logs_p)`
and an auxiliary term (`kl_aux`) against the duration-expanded
`(m_p0, logs_p0)`. The auxiliary term is exactly the quantity MAS maximizes, so
the alignment objective and the training objective stay consistent.

When a dataset does provide durations, pass them to
`AurisSinger.forward(..., durations=...)` and MAS is skipped.

## Source signal (RefineGAN-style)

Following [RefineGAN](https://arxiv.org/abs/2111.00962), f0 and energy do
**not** enter the decoder as embeddings. They only shape an excitation signal:

* **voiced frames** — an impulse train at the instantaneous f0. Impulses are
  scaled by `sqrt(sample_rate / f0)` so the train has unit RMS regardless of
  pitch; without this, low notes would arrive at the decoder much quieter than
  high ones.
* **unvoiced frames** — uniform noise on `[-1, 1]`.
* both branches are multiplied by the frame-level RMS energy of the reference
  audio, so the excitation already carries the intended loudness envelope.

A small amount of noise is kept in voiced frames (`voiced_noise_amplitude`) to
give the network a stochastic component for breathiness.

The excitation is generated at full sample rate and injected at **every**
upsampling stage of the decoder, downsampled by a strided convolution to match
that stage's resolution.

Because this is the only path by which pitch and loudness reach the decoder,
they are fully controllable at synthesis time.

## Decoder

An NSF-HiFi-GAN with `upsample_rates = [6, 5, 4, 4]` (product 480) and SiLU
throughout. Padding and output padding of each transposed convolution are
derived so that each stage outputs exactly `rate ×` its input length, for both
even and odd `kernel - rate`; the output is therefore always exactly
`n_frames · hop_length` samples.

## Discriminators

* **Multi-period discriminator** — unchanged from HiFi-GAN (periods 2, 3, 5, 7, 11).
* **Multi-resolution STFT discriminator** — replaces the multi-scale
  discriminator. Each sub-discriminator takes the complex STFT (real and
  imaginary parts as two channels) at one resolution and runs a 2D convolution
  stack over it. Supervising several time/frequency trade-offs suits singing,
  where sustained notes need frequency resolution and consonants need time
  resolution.
* **Speaker conditioning** uses the projection formulation (Miyato & Koyama,
  2018): the speaker embedding is projected onto the final feature map and its
  inner product is added to the logit map. The embedding is zero-initialized so
  training starts unconditioned. LeakyReLU is kept here — the SiLU change
  applies to the generator.

## Losses

| Loss | Weight (default) | Notes |
| --- | --- | --- |
| Adversarial (LSGAN) | 1.0 | over both discriminators |
| Feature matching | 1.0 | L1 over intermediate activations (already ×2 internally) |
| Multi-parameter mel | 45.0 | mel L1 at 3–4 STFT parameterizations |
| Envelope | 10.0 | RefineGAN upper/lower max-pool envelopes at 4 window sizes |
| KL | 1.0 | against the refined prior |
| KL (auxiliary) | 1.0 | against the expanded phoneme-level prior; the MAS objective |

The **envelope loss** matches `max_pool(x)` and `-max_pool(-x)` between real and
generated waveforms at several window sizes. A mel loss is largely blind to a
pure gain change; the envelope loss is not, which matters for singing dynamics.

The **multi-parameter mel loss** evaluates the same mel L1 under several
`(n_fft, hop, win, n_mels)` settings, so neither fine spectral detail nor
temporal resolution is traded away.

The KL term uses the VITS single-sample estimator, normalized per frame
(summed over channels). It is zero *in expectation* when prior and posterior
match, and can be negative for an individual sample.

## Posterior collapse: the failure mode this design invites

This is the one thing to understand before changing the loss weights.

Phonetic content can reach the decoder **only** through `z`. Pitch and loudness
arrive separately, through the excitation signal. That asymmetry creates a
shortcut that plain VITS does not have: the decoder can produce plausible,
correctly-pitched, correctly-loud audio from the excitation alone, and treat
`z` as noise.

The KL estimator actively rewards this. Its entropy term is `-logs_q`, which is
minimized by making the posterior *wider*. In VITS the reconstruction term
fights back, because the decoder has nothing but `z` to work with. Here, with
the excitation shortcut available, the entropy term can win: `σ_q` grows, `z`
becomes noise, KL falls to zero because prior and posterior have both collapsed
onto the same wide Gaussian — and the model sings the right notes with no
intelligible words.

A run that has collapsed looks like this: `val/f0_*` metrics near perfect,
`train/kl` at ~0, `train/posterior_sigma` drifting above 1, and `val/mel`
plateauing far too early. Permuting `z` along the time axis costs nothing,
which is exactly what `val/latent_usage` measures.

Two guards are on by default:

* **KL warm-up** (`loss.kl_warmup_steps`) ramps the KL weight from 0, giving the
  reconstruction path a head start before the entropy term applies.
* **Free bits** (`loss.kl_free_bits`, nats per latent channel per frame) stops
  the KL being minimized past a floor. Once a channel is at the floor its
  entropy reward switches off, so the optimizer cannot buy loss by inflating
  `σ_q`.

The auxiliary KL weight also defaults to 0.2 rather than 1.0: its job is to
keep the alignment statistic honest, and at full weight it doubles the total KL
pressure relative to VITS.

Measured effect on the JSUT-song run (21 minutes of audio, `small` preset).
Steps are optimizer steps; the guarded run was taken to 40k:

| | without guards | with guards |
| --- | --- | --- |
| `train/mel` | 0.71 @ 16.3k | 0.53 @ 10k, 0.45 @ 20k, 0.41 @ 40k |
| `train/kl` | 0.0001 | 3.9–4.5 (resting on the floor) |
| `train/posterior_sigma` | 2.08 | 0.98 → 1.33 |
| `train/posterior_mean_rms` | — | 0.80 → 1.29 |
| `val/latent_usage` | ~0 | 0.18 → 0.33 |

`posterior_mean_rms` rising while `posterior_sigma` stays near 1 is the signature
of a healthy latent: the posterior is moving its mean around to carry
information rather than widening to cheat the entropy term.

`kl_free_bits` interacts with `inter_channels`: the floor is per channel, so the
total floor is `inter_channels * kl_free_bits`. Raising it too far disables the
KL entirely, and inference — which samples `z` from the prior — degrades because
prior and posterior are no longer tied.
