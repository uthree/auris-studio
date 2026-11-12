# Mixer signal flow

[日本語](mixer-graph.ja.md)

The Mixer opens on **Signal flow**. Every track, every bus and the master is one node, and
nothing else is: a node holds its name, its effects, fader, pan, mute, solo and meters. Effects
are rows inside their strip's node, in processing order, and act before the fader; a bypassed
effect stays in its row. An effect always belongs to a track or a bus — to process something in
parallel, add a bus and put the effect there. Bus and master nodes sum their incoming signals.
Arrows show the direction audio travels.

Drag a track's right port to a bus's left input port. That adds an output: a unity post-fader
branch beside every existing one. There is no separate send. Every output has its own level and
its own tap point, after the fader or before it, so a reverb feed is simply a second output at a
lower level. The master and several buses can receive the same strip at once. The console's
**Output** menu checks each connected destination; choosing one toggles that branch. Add a bus
from the toolbar to create a shared return or another processing branch.

A strip with an effect that accepts a sidechain has a key port on its top edge. Drag another
strip's right port there to key that effect from it; when several effects on the strip accept a
key, the first one that can take it is chosen, and each effect's menu chooses another. The effect's
row then names the strip keying it. Output and sidechain connections both participate in cycle
validation. A connection that would lead back to its source is refused before the document or
undo history changes.

Wire roles use themed colors, labels and distinct line styles:

| Connection | Line | Label |
| --- | --- | --- |
| Output after the fader | Solid | Out, and its level once it is not unity |
| Output before the fader | Dashed | Out, its level, and the pre-fader mark |
| Sidechain | Dotted | Sidechain |

The legend below the canvas shows the current colors. Wires sharing the same source and
destination use separate curved lanes, such as a pre- and a post-fader output to one bus, or an
output and a sidechain. Their labels follow their lanes and identify each one; a sidechain's
tooltip names the effect it keys.

During playback, wires glow and become slightly thicker in proportion to the signal they actually
carry. Outputs and sidechains retain their own colors and line styles. Short highlights travel
from each source towards its destination, following curves and bends at a steady visual speed.
Their fading trails point back towards the source and their brightness follows the signal. Turn
off **Animate signals** in the graph toolbar to stop moving highlights while retaining level glow
and direction arrows. This preference is remembered. Outputs show their own gain and tap. A
bypassed effect's sidechain is inactive. Reverb and delay tails remain visible after Stop and fade
with the audio. The display holds brief peaks and falls smoothly, like the meters. Measurement
runs only while **Signal flow** is open and does not change the sound, project or undo history.

Click a wire or its label to select it. Selection adds a thicker accent outline while retaining
the role color and line style. Press Delete or Backspace, or right-click the wire or label and
choose **Delete connections**. Hold Ctrl on Windows or Command on macOS while clicking to select
several wires; their deletion is one Undo step. Undo and Redo restore the actual routing,
including removed output automation. A disconnection removes only the selected branch. Deleting
every output leaves the strip disconnected; it does not create a new master route.

Use **Add effect** on a node. Click an effect's name to open its editor, the dot beside it to
bypass it, or **…** — or right-click the row — for its menu: reorder, remove or choose a
sidechain. Gain, pan, mute, solo and meters are below the effects. Once a track has more than its
one unity post-fader output, every output also gets a level row there; its context menu moves it
before or after the fader or removes it.

Drag nodes to arrange the workspace, or drag/scroll the background to pan. Hold Shift and drag
the background to select nodes in a rectangle. Selected nodes have highlighted borders; drag any
selected node to move the group while preserving its spacing. The entire group movement is one
Undo/Redo step. Click an unselected node to select it alone, or click empty space to clear the
selection. Ctrl-click (Command-click on macOS) toggles individual nodes. **Arrange nodes** places
dependencies from left to right. **Console** provides the channel strips; **Signal flow** returns
to the graph. Both views edit the same project and share undo/redo. Keyboard focus moves through
the controls with Tab; routing and effect menus also provide keyboard access to connections.

The saved project contains the routing, effect order, parameters and output levels and tap
points, plus node positions, zoom and pan. Save after arranging or navigating the graph to restore
the same view next time. This includes scales below 50% chosen by **Fit graph**. A node drag is
one Undo step; **Arrange nodes** is also undoable. These steps share the history with routing and
other project edits. Zoom and pan stay current through Undo/Redo and do not consume steps
themselves. Layout changes and their Undo/Redo do not rebuild audio. New nodes use the automatic
layout; nodes restored by Undo keep their previous positions. Projects without a saved view start
with the automatic layout at 100%. The system's architecture is documented in
`auris_session::guide`.

## MCP and live agents

MCP exposes `mixer_graph` with a project handle/path and an `operation` object. Start with
`{"action":"read"}`. Nodes use stable `track:N` and `track:master` IDs, independent of names. Each
node lists its `effects` in processing order with stable `effect:N` IDs, their position, bypass
state, whether they accept a key and which track keys them. An effect ID addresses the effect's
own operations and is never the end of a connection. Read pages are in processing order;
`next_offset` continues the node list and each page includes its nodes' outgoing edges. Output
edges include their `gain_db` and `pre_fader` tap. An output is named by its source, its target
and its tap; a track holds at most one per pair. Sidechain edges run from the keying track to the
keyed effect's track and name the effect.

For example, after reading the actual IDs:

```json
{"project":"/absolute/path/Song/Song.auris","operation":{"action":"connect","source":"track:1","target":"track:2"}}
```

The same `mixer_graph` operation object is used by the built-in Claude, Codex and native API
agents; omit only the saved-project `project` field for live calls.

| Task | Operation action |
| --- | --- |
| Read graph | `read` |
| Fader / pan / mute / solo | `set_strip` |
| Replace outputs / add or remove an output | `set_outputs` / `connect` / `disconnect` |
| Add an output with a level and tap / change level or tap | `add_output` / `set_output` |
| Key an effect from a track, or clear it | `set_sidechain` |
| Add / remove inserts | `add_effect` / `remove_effect` |
| Reorder / bypass | `move_effect` / `enable_effect` |
| Read / set native effect parameters | `parameters` / `set_parameter` |

Both transports use the same session operations and cycle checks. Invalid and stale edges are
refused before editing. Connecting an output adds a destination without replacing the others.
Disconnecting removes only the named output; an empty output list stays disconnected. When a pre-
and a post-fader output share a source and target, `disconnect` takes `pre_fader` to say which.
Removing an output or effect also removes its automation. MCP saves actual changes with a
checkpoint; reads and unchanged connection retries do not write. Live edits remain unsaved and
undoable. Read-only and Plan modes permit graph inspection but deny edits; ordinary Edit mode asks
before disconnecting or removing an effect, according to the existing permission policy.
