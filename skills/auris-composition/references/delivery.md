# Delivery reference

## Games

Ask whether the cue is a loop, one-shot, adaptive layer, or transition. For an existing project, add a dedicated cue track or clip after checking occupied ranges and routing; preserve existing content. Keep loopable cues bar-aligned and provide a loop start/end in bars or samples. Export a full mix and, when requested, stems whose names describe their role (`music_drums.wav`, `music_harmony.wav`, `music_melody.wav`). Preserve the project and seed so a later cue can be regenerated consistently.

## Video

Confirm frame rate, start time, duration, and whether the editor needs a two-pop or handle. Choose a tempo/meter and integer bar count that fits the requested seconds; disclose any small duration tradeoff. Render a full-length WAV at the requested sample rate and provide stems only when they improve the edit. Check the final file's duration, peak, and loudness with Auris analysis/verification before reporting it as ready.

Preview files are disposable candidates for structure and rough listening. A final render is the delivery artifact and must be checked independently; do not report a preview as verified output.

## Editable projects and stems

Deliver the saved `.auris` project as the editing source: notes, performance and mixer edits live there. Keep one project per folder and preserve its `Audio/` assets, including synthesized takes. Before/after versions belong in separate project folders. Verify the saved audio references resolve when preparing a portable copy; a voice connection is still needed for re-synthesis, not a replacement for retaining the take.

Auris stems include the master chain. With nonlinear processing such as a limiter or compressor, individually rendered stems need not sum to the full mix. Label them accordingly; do not call them dry or promise exact reconstruction without checking. When additive remix stems are required, use a separate export copy with appropriate nonlinear master processing bypassed, preserve the listening master, and verify the sum. Export a vocal stem for full-song coverage checks even if only the mix is presented to the user.

Record the tap position, graph/revision, shared processing or detector context and whether each export is additive or reference-only. Do not subtract an independently rendered post-master vocal from the full mix to claim a verified backing component. Bypassing only the master is insufficient when a shared bus or sidechain remains nonlinear. Unknown processing contracts mean reference-only until a matching reconstruction is measured.

To inspect backing, prefer an explicit backing render from the isolated snapshot. Subtraction is useful only after the relevant tap, detector context, alignment and additive reconstruction have been verified; moving files to a nominally pre-master tap alone does not establish that contract.

Use a non-persistent render override when the installed API supports it, or an isolated project snapshot for verification mutes, solos, FX bypass and routing changes. A `finally` restoration cannot protect a delivery project from process termination. Keep the saved delivery's mixer/routing unchanged, including on failed or cancelled verification.

## Silence, returns and peak control

For a requested dropout, measure the rendered interval and the first returning attack. Distinguish digital zero from an interval below a stated threshold, and record whether its minimum duration came from the user or a verification default. Account for the actual smoothing, latency and block boundaries; do not reuse an arbitrary early-recovery offset from another project. Compare the same return passage before/after at compensated level when its sound is otherwise unchanged.

Locate a problematic peak in time and inspect its combined source/bus contributions before choosing a processing stage. Record before/after LUFS, true peak, clipping and relevant balance measurements over the same range. Intentional distortion in a sound is separate from clipped PCM output. Band-energy ratios describe that band and routing condition; they do not establish intelligibility or musical quality. Preserve the candidate and use level-matched listening to decide whether the control serves the music.

## Verification report

Include:

- source project path;
- seed, tempo, key/mode, duration, and loop boundaries;
- arrangement changes or deliberate retention, compared section ranges, and any unresolved foreground/transition issues;
- rendered file paths and format;
- measurements actually performed (duration, peak/true peak, LUFS, and render status);
- unresolved musical or integration assumptions.

Do not turn a preview into a delivery claim. If the platform's loudness target, sample rate, or loop behavior was not supplied, state the chosen default and make it easy to revise.
