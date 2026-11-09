# Design language integration

Auris Studio uses the Atelier design language from `uthree/design-language`, pinned in
`crates/auris-gpui/Cargo.toml` to revision
`b584b128658858fdd7917e003271dd35dadfd279`. The application selects Atelier's Glass direction and
adapts its existing `Theme` through the `Theme::atelier` field, `Theme::install`, and
`Theme::sync_atelier`. Existing Auris palette identifiers remain available while shared controls,
agent surfaces, and the application shell read the corresponding Atelier tokens.

At startup, `atelier::Assets::with_fallback` serves Atelier's embedded icon set first and then
`ui::provider_logo::ProviderAssets`. The fallback contains the provider marks, the caption glyphs and
the Auris-specific 24px SVGs for the [library's musical-family presets](timbre-map.md).
These share Atelier's grid and stroke geometry and reach its `Icon` as an `IconSource::svg`;
shared controls use Atelier's own marks. The application icon files
in `crates/auris-gpui/assets/` are generated Atelier element-cell exports for Auris Studio (`Au`),
including the editable, size-specific, symbolic, PNG, ICO, and ICNS forms. The redistributed
canonical icon files carry the upstream Apache-2.0 text in `assets/ATELIER-LICENSE`.

The editor transcript and song workshop share Atelier's native `UserMessage`, `AssistantMessage`,
`SelectableText`, `CopyButton`, status notices, thinking indicators and `AudioPlayer` controls.
The adapter keeps
the existing Auris session state, editable fields, attachments, and permission handling in their
own views while giving both workflows the same Atelier agent presentation primitives.
An agent answer is Atelier's `AssistantMessage::anonymous()`, without the avatar-and-name line:
a conversation has exactly two voices, the person's is the accent bubble on the trailing edge, and whatever is not in a bubble
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

Every window's title bar follows Atelier's native `TitleBar`: one slim row with the title on the
window centre line, no subtitle, and only the frequent leading/trailing commands. Back (or, in the
editor, Undo and Redo) leads at the left while the empty title-bar region remains a drag target.
Native window controls keep platform caption and traffic-light behavior intact.

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

Choosing "AI workshop" hands the panel over to the song workshop: for one `motion.normal` the
metal stays mounted while Atelier's `Morph` grows a panel-coloured shape from the chooser panel to
the whole body
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
reading width with its own actions and status underneath. The navigation rows are Atelier's
`ListItem`s, each a tab stop. The General page is built from `FormGroup`s of `FormField`s, with
yes-or-no settings as a `Switch` beside its label, and the search sits in Atelier's `TextField`
frame around Auris's own editable text.

The form column uses Atelier's `density.form_width`, and each page reports the last operation in
a native `StatusBar`; inline field errors remain beside the field that needs recovery.
Agent connection fields form a draft: Apply and Revert are enabled only after an edit, and
Revert restores those fields without changing the model or permission mode of a live conversation.

The editor and song workshop keep a small application composer adapter built from Atelier tokens
and components because those workflows combine queued-send state, simultaneous Send/Stop actions,
MIDI editing, attachment management and workflow-specific completion controls. It is intentionally
not a direct use of native `Composer`: the native slots cannot express those simultaneous domain
states. The adapter preserves the same tone, notice, disabled and keyboard treatments while domain
controls remain owned by their views.

Toolbar icon buttons are Atelier ghost buttons: no fill or hairline until hovered, and filled
with their signal colour when latched.

On Windows, `gpu-allocator` 0.27 must resolve to `windows` 0.58, matching Atelier's `wgpu-hal` 25.
The committed lockfile includes that resolution; after regenerating it, check `cargo tree -p
gpu-allocator@0.27.0` and use `cargo update -p gpu-allocator@0.27.0` if it selects 0.57 instead.
