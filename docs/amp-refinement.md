# Guitar amplifier refinement

The current amplifier uses one symmetric soft-clipping stage:

```text
y = tanh(10^(drive_db / 20) * x)
```

The tone stack, 8× oversampling, 257-tap Blackman-windowed FIR, 32-frame
reported latency, DC blocker and analytic cabinet remain unchanged. The earlier
two-stage measurements remain as historical diagnostics.

The Drive control now spans 0–48 dB, extended from 0–42 dB. Drive was fitted
on all 48 training notes from EGFxSet, with the same 0–48 dB
bound used by the simple tanh baseline. The fitted value was 47.994488 dB;
the simple baseline fit was 47.994049 dB. Native 48 kHz output was rendered in
8-note chunks for all 48 training notes and all 46 held-out validation notes,
then resampled to 24 kHz for the fixed log-mel metric.

| cohort | historical two-stage amp | refined amp | simple tanh |
| --- | ---: | ---: | ---: |
| training | 0.08384799459 | 0.06936643792 | 0.07075090728 |
| validation | 0.07967830676 | 0.06622412116 | 0.06733945544 |

The refined model lowers validation mel error by 16.89% against the historical
two-stage candidate and 1.66% against the separately fitted simple tanh. The
training reductions are 17.27% and 1.96%, respectively. These are descriptive
metrics for the held-out pitch split, not perceptual scores or evidence of a
specific commercial circuit reconstruction.

The 16.89% comparison includes the changed saturation, extended Drive range
and a new training fit; it does not isolate a single circuit choice. The new
fit reaches the upper Drive bound, and its four-control search retains the
flat-EQ initial point. The 1.66% comparison uses equal bounds for both current
models. Both receive the same recorded DI with cabinet bypass. A paired
2,000-resample pitch bootstrap gives validation mel-delta intervals
[−0.017270, −0.009835] against the previous amp and [−0.001333, −0.000912]
against simple tanh. This cohort supplies no recorded speaker-cabinet target.

Local final timing at 48 kHz stereo, 256 frames and 24 electric-guitar voices
plus amp is 0.270 ms mean and 0.290 ms p99 over 512 blocks. Existing nine-preset
WAV hashes, symbolic rows and Audiobox scores are unchanged. These presets
do not exercise the changed plugins. The timing, tuning and matched-level
audition records are in `tools/eval/references/physical-next-regressions.json`.

The complete per-note report is `tools/eval/references/amp-refinement.json`,
the fitted parameters are in `tools/eval/references/amp-refinement-fit.json`, and the
reproducible chunked evaluator is `tools/eval/amp_refinement.py`. The report
keeps the source, renderer and metric configuration needed to audit the result.
