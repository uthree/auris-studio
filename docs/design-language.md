# Design language integration

Auris Studio uses the Atelier design language from `uthree/design-language`, pinned in
`crates/auris-gpui/Cargo.toml` to revision
`22d3a55316b22d749af91ebff918758e41c39226`. The application selects Atelier's Glass direction and
adapts its existing `Theme` through the `Theme::atelier` field, `Theme::install`, and
`Theme::sync_atelier`. Existing Auris palette identifiers remain available while shared controls,
agent surfaces, and the application shell read the corresponding Atelier tokens.

At startup, `atelier::Assets::with_fallback` serves Atelier's embedded icon set first and then
`ui::provider_logo::ProviderAssets`. The fallback contains the provider marks and Auris-specific
24px SVGs for volume, mute, punch, notes, keyboard, and knob controls. The application icon files
in `crates/auris-gpui/assets/` are generated Atelier element-cell exports for Auris Studio (`Au`),
including the editable, size-specific, symbolic, PNG, ICO, and ICNS forms. The redistributed
canonical icon files carry the upstream Apache-2.0 text in `assets/ATELIER-LICENSE`.

The editor transcript and song workshop share `ui::agent_components`: user messages, assistant
messages, status and error notices, thinking indicators, the composer frame, its wrapping control
lane, and keyboard-operable send/stop commands. The adapter keeps
the existing Auris session state, editable fields, attachments, and permission handling in their
own views while giving both workflows the same Atelier agent presentation primitives.
An agent answer is drawn without Atelier's avatar-and-name line: a conversation has exactly two
voices, the person's is the accent bubble on the trailing edge, and whatever is not in a bubble
is the agent. The agent itself is marked by Atelier's sparkle wherever it is opened, and the
composer's fast mode is a single bolt.

Glass Dark is the first-run default; Glass Light is available in Appearance. Previously saved
schemes and custom palettes retain their identifiers and document colours. Interface surfaces
follow Atelier for the two Glass presets, while custom schemes supply their own colours to the
same components. Native glass backdrops are applied when a project window opens and when its
theme changes; the editor canvas remains opaque.

The screen compositions follow the upstream `showcase` arrangement and `auris_screens` examples:
selected track headers use the accent fill, clips use rounded coloured bodies and hairline
outlines, and the trailing agent dock opens at 380 px. Both conversation entry points share the
same introductory state, message forms and inset composer. Saved dock widths remain intact.

Every window's title bar is composed by `titlebar::composed` the way Atelier's `TitleBar` is:
the title sits on the window's centre line with a short subtitle beside it, Back (or, in the
editor, Undo and Redo) leads at the left, and nothing else sits in it. The two
sides grow from bases that make up for the native traffic lights and caption buttons, so the
title stays centred on the window rather than on whatever space the controls leave.

The chooser is built from Atelier's own components, as in the upstream `auris_screens` example:
a `Sidebar` of `ListItem`s (new, recent, and the setup guide at the foot), `OptionCard`s for how
to start, an `EmptyState` when there are no recent projects, and a toolbar-height footer holding
the one primary commit. The panel takes about half the body (up to 880×480), so a larger window shows more of the metal rather than a larger panel. It floats over `MetalBackdrop`, as in the liquid-metal variant of
the upstream `ambient` example, and occludes pointer events so its controls do not also
manipulate the metal beneath it. A card selects a starting method; Create or Enter commits it,
and double-click also opens it. The setup guide is the panel's third page rather than a window
of its own; a first run opens the chooser on it, and Settings brings it forward in whichever
window shows the chooser. Inactive windows hold a still frame of the metal, and Atelier's motion
policy stills it entirely.

Choosing "Create with AI" hands the panel over to the song workshop: for one `motion.normal` the
metal stays mounted while a panel-coloured shape grows from the chooser panel to the whole body
and the workshop fades in over it; Back plays the same hand-over in reverse. The metal stays
under the workshop too, behind a veil of the surface colour that the shape turns into as it
arrives: thin while the conversation is empty, thicker once there is a transcript, and solid
once the transcript has been scrolled, so reading is never done over the moving metal. Only the shape
moves — the arriving screen is laid out at its final size from the first frame — so nothing
reflows. The workshop follows the upstream compose screen: one centred column of transcript and
candidate cards (elevated, accent-bordered when chosen, agent-washed only while being written),
the Atelier composer with the send button as its one primary, and a row of workshop options
beneath it with "Use my preferences" as a `Switch`.

Settings and the singing-backend setup are preferences windows in one shape: a leading
`Sidebar` names every page in full (the settings search sits at its head, the setup guide at its
foot), and each page opens with a `SectionHeader` saying what it covers, on a surface at a
reading width with its own actions and status underneath. Atelier has no form pattern yet, so
fields follow its `TextField` metrics with the label above the field and groups under a
section-weight heading. The rows are `ui::widgets::nav_item` rather than Atelier's `ListItem`,
which is pointer-only, so every page stays in the Tab order.

Toolbar icon buttons are Atelier ghost buttons: no fill or hairline until hovered, and filled
with their signal colour when latched.

## Pipeline and mixer graph

The DAG surfaces use this same theme adapter and shared command controls. The graph canvas
uses `background`, nodes use `surface`, selected node headings use the selected wash, and
node radii and inspector spacing/type come from Atelier tokens. Canvas coordinates remain
graph-space geometry so saved layout and graph zoom keep their meaning.

Audio outputs, sends and detector inputs share the audio port token. Solid, dashed and dotted
strokes, the legend, and connection names distinguish their routing roles without depending
on colour. Changing document track colours does not change the wire vocabulary. The adapter
retains contrast correction for custom palettes.

The pipeline inspector remains a scoped, paged list in the existing scroll region. Disclosure,
owner actions, stage menus, and connection navigation reuse the shared keyboard controls.
Intermediate parameter and connection-level readouts share a trailing numeric spine with
monospaced values; translated labels wrap within the inspector. Presentation changes do not
write project content or saved panel widths.

When integrating main, retain its theme installation, shell, shared widgets, and conversation
components. Extend the DAG-specific views against those components instead of introducing a
second palette or replacing main's screen compositions. The pipeline layout matrix includes
Glass light/dark, English/Japanese, a narrow inspector, and larger base text. Headless layout
checks verify bounds and behavior, not rendered pixels.

Windows visual verification on 2026-10-29 used an isolated configuration with
automatic updates, autosave and SoundFont fetching disabled. A new project was
opened from the chooser, then the pipeline disclosure, fader snapshot, graph Fit
and connection selection were exercised in the actual desktop window. Glass
Dark and Glass Light were checked with an approximately 220px inspector. This
caught a shrinking numeric column that headless bounds checks had missed; values
now retain their width while labels wrap and the containing rows retain height.

- [Pipeline snapshot, Glass Dark](screenshots/atelier-pipeline-dark.jpg)
- [Graph and narrow inspector, Glass Dark](screenshots/atelier-dag-narrow-dark.jpg)
- [Graph and narrow inspector, Glass Light](screenshots/atelier-dag-narrow-light.jpg)

These captures cover display and local navigation, not audio-device behavior. An
earlier command-line launch with `examples/dark-song.asong` closed before capture
and logged `window not found`; its cause was not established by this UI check.
The chooser-to-new-project route used for the captures remained open throughout.

On Windows, `gpu-allocator` 0.27 must resolve to `windows` 0.58, matching Atelier's `wgpu-hal` 25.
The committed lockfile includes that resolution; after regenerating it, check `cargo tree -p
gpu-allocator@0.27.0` and use `cargo update -p gpu-allocator@0.27.0` if it selects 0.57 instead.
