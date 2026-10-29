# Mixer signal flow

[日本語](mixer-graph.ja.md)

The Mixer opens on **Signal flow**. Each track or bus has one node containing
its name, fader, pan, mute, solo and meters. Effects remain individual nodes in
processing order. Fader controls still act after all inserts. Bus and master nodes sum their incoming signals.
Arrows show the direction audio travels. Bypassed effects remain in the chain.

Drag a track's right port, or its last effect's right port, to a bus's left input port. **Output** changes the
strip's destinations by adding a unity post-fader branch; it keeps every existing
output. **Sends** adds a branch with its own level and pre/post-fader tap. The master
and several buses can receive the same strip at once. The console's **Output** menu
checks each connected destination; choosing one toggles that branch. Add a bus from the toolbar
to create a shared return or another processing branch.

An effect that accepts a sidechain has a port on its top edge. Drag another
strip's output port there to key the effect from that strip. Output, send and
sidechain connections all participate in cycle validation. A connection that
would lead back to its source is refused before the document or undo history
changes.

Audio wires share Atelier's audio port color. Labels and distinct line styles identify their
routing roles independently of document track colors:

| Connection | Line | Label |
| --- | --- | --- |
| Output | Solid | Out |
| Send | Dashed | Send ID, level, and pre-fader mark when applicable |
| Sidechain | Dotted | Sidechain |

The legend below the canvas shows the current colors. Wires sharing the same
source and destination use separate curved lanes, including multiple sends to
one bus. Their labels follow their lanes and identify each send independently.

During playback, wires glow and become slightly thicker in proportion to the
signal they actually carry. Outputs, sends and sidechains retain the audio
color and their line styles. Short highlights travel from each source towards its
destination, following curves and bends at a steady visual speed. Their fading
trails point back towards the source and their brightness follows the signal.
Turn off **Animate signals** in the graph toolbar to stop moving highlights
while retaining level glow and direction arrows. This preference is remembered.
Sends show their own gain and pre/post-fader tap;
insert-chain wires show the next effect's input. A bypassed effect's sidechain
is inactive. Reverb and delay tails remain visible after Stop and fade with
the audio. The display holds brief peaks and falls smoothly, like the meters.
Measurement runs only while **Signal flow** is open and does not change the
sound, project or undo history.

Click a wire or its label to select it. Selection adds a thicker accent outline
while retaining the audio color and line style. Press Delete or Backspace, or right-click the wire or label and choose
**Delete connections**. Hold Ctrl on Windows or Command on macOS while clicking
to select several wires; their deletion is one Undo step. Undo and Redo restore
the actual routing, including removed send automation. A disconnection removes
only the selected branch. Deleting every output leaves the strip disconnected;
it does not create a new master route. Thin insert-chain arrows follow effect
order; use the effect menu to change that order.

Use **Add effect** on a track node. Open an effect by its name, toggle
**Enabled** to bypass it, or use its **Effect** menu to reorder, remove or choose
a sidechain. Gain, pan, mute, solo, meters and send levels are on the track node.
A send's context menu changes its pre/post-fader tap or removes it.

Drag nodes to arrange the workspace, or drag/scroll the background to pan.
Hold Shift and drag the background to select nodes in a rectangle. Selected
nodes have highlighted borders; drag any selected node to move the group while
preserving its spacing. The entire group movement is one Undo/Redo step.
Click an unselected node to select it alone, or click empty space to clear the
selection. Ctrl-click (Command-click on macOS) toggles individual nodes.
**Arrange nodes** places dependencies from left to right. **Console** provides
the channel strips; **Signal flow** returns to the graph. Both views edit the
same project and share undo/redo. Keyboard focus moves through the controls with
Tab; routing and effect menus also provide keyboard access to connections.

The saved project contains the routing, effect order, parameters and send tap
points, plus node positions, zoom and pan. Save after arranging or navigating
the graph to restore the same view next time. This includes scales below 50%
chosen by **Fit graph**. A node drag is one Undo step; **Arrange nodes** is also
undoable. These steps share the history with routing and other project edits.
Zoom and pan stay current through Undo/Redo and do not consume steps themselves.
Layout changes and their Undo/Redo do not rebuild audio. New nodes use the automatic layout; nodes restored by Undo
keep their previous positions. Projects without a saved view start with the
automatic layout at 100%. The system's architecture is documented in
`auris_session::guide`.

## MCP and live agents

MCP exposes `mixer_graph` with a project handle/path and an `operation` object.
Start with `{"action":"read"}`. Nodes use stable `track:N`, `track:master`
and `effect:N` IDs, independent of names and effect positions. Read pages are
in processing order; `next_offset` continues the node list and each page includes
its nodes' outgoing edges. A track's `output_node` identifies its terminal insert
or the track itself. Both the track ID and its terminal effect ID can address an
outgoing connection. Send edges include a stable `send_id` and pre/post-fader tap.

For example, after reading the actual IDs:

```json
{"project":"/absolute/path/Song/Song.auris","operation":{"action":"connect","source":"track:1","target":"track:2","kind":"send"}}
```

The same `mixer_graph` operation object is used by the built-in Claude, Codex and
native API agents; omit only the saved-project `project` field for live calls.

| Task | Operation action |
| --- | --- |
| Read graph | `read` |
| Fader / pan / mute / solo | `set_strip` |
| Replace outputs / add or remove a branch | `set_outputs` / `connect` / `disconnect` |
| Create configured send / change level or tap | `add_send` / `set_send` |
| Replace or clear detector source | `set_sidechain` |
| Add / remove inserts | `add_effect` / `remove_effect` |
| Reorder / bypass | `move_effect` / `enable_effect` |
| Read / set native effect parameters | `parameters` / `set_parameter` |

Both transports use the same session operations and cycle checks. Invalid and
stale edges are refused before editing. Connecting an output adds a destination
without replacing the others. Disconnecting removes only the named branch;
an empty output list stays disconnected. Removing a send or effect also removes its automation. MCP saves
actual changes with a checkpoint; reads and unchanged connection retries do not
write. Live edits remain unsaved and undoable. Read-only and Plan modes permit
graph inspection but deny edits; ordinary Edit mode asks before disconnecting or
removing an effect, according to the existing permission policy.
