# Arrangement proposals for agents

[日本語](arrangement-harness.ja.md)

The arrangement harness connects instrument capabilities, phrase conditions, section
orchestration, parameter movement and SFX to the shared session editing path. It retains
authored notes, lyrics, harmony, tempo and meter, except for exact releases, pickup
omissions or held arrivals explicitly previewed by a `breath` edit. A proposal is an ephemeral prepared
document, not a new project format.

## Enable and inspect

Start the desktop or saved-project MCP with `AURIS_ARRANGEMENT_HARNESS=1`.
The `arrangement` tool and its standing model instructions are absent when disabled.
Saved MCP places it in the `manual` group. Live Claude, Codex and rig use the same
schema, worker protocol and permission path. Saved calls add `project`.

```json
{"operation":{"kind":"inspect","start_bar":17,"bars":16}}
```

Read `inspect_project` first for stable track and clip IDs. Arrangement inspection
returns a content revision, section spans, verified controller support, family-based
recommendations, exact native parameter keys and bounded phrase observations. Support
is `supported`, `unsupported` or `unknown`; a General MIDI family is a convention, not
evidence for a third-party patch's articulations. CC1 may mean bow pressure, vibrato or
a patch-specific mapping. Bend and CC are channel-wide and can affect overlapping voices.

Inspection pages sixteen tracks and up to sixteen intersecting clips per track.
Follow `next_offset` for tracks. Use `inspect_project`, `read_notes`,
`inspect_performance` and `inspect_instrument` for individual clips or further parameter
pages. Phrase measurements sample at most 2,048 notes and 32 stages. Generated curve
inspection covers clips up to 64 quarter notes. Incomplete measurements are marked;
they are not a successful verification. Large responses use immutable `read_report` pages.

## Prepare a proposal

Each change includes a concrete musical purpose. Validation accepts 1–16 changes and
the current content revision; it does not change the document or its Undo history.

```json
{
  "operation": {
    "kind": "validate",
    "expected_revision": "COPY_FROM_INSPECTION",
    "changes": [
      {
        "purpose": "Open the second chorus with guitar strokes",
        "edit": {
          "kind": "revoice",
          "clips": [12],
          "sound_id": "COPY_FROM_SEARCH_INSTRUMENTS",
          "name": "Chorus guitar",
          "settings": {"reset": ["pitch"], "strum": {"spread_ms": 25}}
        }
      },
      {
        "purpose": "Lift the pad inside the chorus",
        "edit": {
          "kind": "automation",
          "target": {"kind": "track_gain", "track": 4},
          "start_tick": 92160,
          "end_tick": 122880,
          "points": [
            {"at_tick": 93120, "value": -24},
            {"at_tick": 121920, "value": -12}
          ]
        }
      }
    ]
  }
}
```

Supported edits:

| Edit | Contract |
| --- | --- |
| `breath` | Explicit phrase rests, shortened releases, named pickup omissions and optional held arrivals replacing exact approach indices. Protected/vocal notes and possible ties are refused; performed space is verified before application. |
| `performance` | Partial `set_performance` settings on an existing clip; optional exact drum writer scope. Source notes and authored curves remain intact. |
| `automation` | One existing native parameter, 1–128 unique points strictly inside a passage. Existing boundary values, interpolation and outside curves remain unchanged. |
| `revoice` | Transfer 1–16 complete clips from one instrument/drum track to a new sound. `layer:true` retains the originals and reissues copied clip IDs. |
| `transition` | `riser`, `crash` or `impact`, with explicit boundary, duration, name and gain. |
| `audio_sfx` | A frozen local source copy, aligned by start, absolute sample peak or exclusive source end, with explicit gain and source-time fades. |
| `clip_mute` | Add or remove a rest without deleting the score. |

Revoice uses fresh `search_instruments` IDs. It copies the mixer strip, effects and outputs,
with reissued effect identities, through the existing track-duplication operation.
Unselected source clips remain unchanged. Source instrument parameter lanes stay on the
original track; old parameter addresses are not copied onto the new instrument. Optional
`settings` modifies the new clips' performance only. Split clips at appropriate phrase
boundaries with `edit_clip` before preparing a section-specific proposal, then inspect
the resulting IDs and revision again. Review sustained notes, controller resets and tails
at a split or sound handover.

Automation retains the original values at both passage boundaries. Points use native
units and must lie strictly between those boundaries. Continuous/discrete interpolation
is inherited from the existing lane or descriptor. This keeps the original linear ramps
and held values outside the requested passage. Static parameter values remain unchanged.

A native riser is an editable bowed crescendo ending at its boundary. Crash and impact
are native drum attacks beginning there. Gains are -60..0 dB and native durations are
1..15,360 ticks. These sounds need no external assets. Use an actual audio source when
the desired effect is a particular recorded sound.

Audio SFX must be a nonempty local mono/stereo file up to 32 MiB and thirty seconds.
The worker copies it before decoding, finds the sample peak across channels and prepares
waveforms/resampling. The source never follows project tempo. Alignment converts through
the full tempo map and reports its tick-rounding error. Fades are in source milliseconds
and must fit the recording. Before-song alignment and edits extending the existing song
duration are refused. Preparation arguments are bounded to 128 KiB; candidate documents
and retained sources share a 64 MiB budget per plan.

For example, these two change objects place a local recording and a bowed crescendo at
the same boundary. Include them in `operation.changes` with the current revision:

```json
[
  {
    "purpose": "Place the effect's attack at the chorus",
    "edit": {
      "kind": "audio_sfx",
      "path": "/absolute/local/path/effect.wav",
      "at_tick": 92160,
      "alignment": "peak",
      "name": "Chorus effect",
      "gain_db": -12,
      "fade_in_ms": 2,
      "fade_out_ms": 5
    }
  },
  {
    "purpose": "Build toward the chorus boundary",
    "edit": {
      "kind": "transition",
      "transition": "riser",
      "at_tick": 92160,
      "duration_ticks": 1920,
      "name": "Chorus swell",
      "gain_db": -18
    }
  }
]
```

## Apply and verify

Review all prepared changes, effect measurements and warnings, then use the returned
`plan_id` and its original revision:

```json
{"operation":{"kind":"apply","plan_id":"COPY_FROM_VALIDATION","expected_revision":"SAME_ORIGINAL_REVISION"}}
```

The owner checks document identity, content revision, active gestures and expiry.
Source copies remain private until application. Sound preparation and audio work run
off the UI thread; failed, cancelled or stale preparation drops its private files.
Application commits one Undo edit. Exact retries return the original receipt without
creating tracks, notes or audio again. After subsequent edits or Undo, use ordinary
history/Redo or prepare a fresh proposal. An unchanged proposal adds no Undo history.

Plans expire after fifteen minutes, process restart or bounded-cache eviction. The cache
retains at most eight plans and 256 MiB. Saved-project calls can reuse a plan across
separate session openings within that process. A process restart requires revalidation.
Live operations remain unsaved; changed saved operations use the existing checkpointed
save adapter. Read-only/Plan modes deny application, and compound application requires
the ordinary live permission decision. Source-file/plugin preparation is separately
classified for confirmation. Existing deny rules remain enforced for this tool.

Read back the affected notes, performance and automation with independent calls. Render
the same bars before and after using `inspect_audio`/`preview`; use `listen` and its
comparison ID when the model can hear. Recheck level balance and
handover tails after revoicing. A saved setting, a nonzero event count or changed PCM
does not establish that an arrangement sounds better. Keep local repair attempts bounded
to two and report unresolved musical or resource conditions.

Rests, call-and-response, selective layering and send automation can provide contrast
without replacing every sound. Preserve deliberate repetition and phrase identity.
[Melody breathing](melody-breathing-plan.md) documents the implemented diagnosis and
explicit repair for a foreground melody that plays continuously, including clip joins,
sustain and tails.

## Validation

Session tests cover exact score/curve preservation, scoped automation, source routing,
layer identities, immutable SFX copies, alignment, cancellation, stale owners/revisions,
failed preparation, retry receipts and Undo/Redo. Catalog and policy tests cover opt-in
schemas and read-only/Plan denial.

The opt-in GPUI test
`installed_agent_arranges_sections_with_performance_automation_and_sfx` runs a real
Claude/Codex/rig worker against an isolated unsaved 32-bar fixture. Set
`AURIS_ARRANGEMENT_HARNESS=1` and `AURIS_TOOL_SURFACE_EVAL` to the configuration described
in [development](development.md#real-agent-window-tests). It checks section revoicing,
strum, scoped pad gain, copied audio SFX, a native riser, independent readback, numeric
audio inspection and one-request Undo/Redo. Evidence records actual backend outcomes;
the test's existence alone does not establish model reliability or listening quality.

The [2026-10-23 validation record](../tools/eval/references/agent-arrangement-2026-10-23.json)
contains the actual outcomes. Codex completed the numeric-only fixture in 57.45 seconds,
including independent readback, paired audio measurements and one Undo/Redo, with no tool
errors. Claude stopped before inference because its OAuth session had expired. The local
Qwen 27B trial timed out during diagnosis without applying a proposal. An earlier Codex
trial stalled in the separate audio critic; the accepted fixture explicitly limits itself
to numeric inspection. None of these measurements establishes improved musical quality.
