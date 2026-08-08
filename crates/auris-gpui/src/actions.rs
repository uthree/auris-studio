//! Application actions and their default key bindings.
//!
//! Actions are gpui's routed commands: the menu bar, the keymap and the buttons in the UI all
//! dispatch the same action type, so a feature is bound once and reachable three ways.

use auris_i18n::Key;
use gpui::{Action, App, KeyBinding, actions};

actions!(
    auris,
    [
        /// Quit the application.
        Quit,
        /// Create a new empty project.
        NewProject,
        /// Open a project file.
        OpenProject,
        /// Open the song sheet: a whole piece asked for with dials.
        ComposeSong,
        /// Write a piece from a song specification file, replacing the document.
        ComposeFromSpec,
        /// Read the selected clip's melody and write a band behind it.
        AccompanyMelody,
        /// Save the current project.
        SaveProject,
        /// Save the current project under a new name.
        SaveProjectAs,
        /// Import an audio file onto a new audio track.
        ImportAudio,
        /// Import a SoundFont, making its sounds available to every track.
        ImportSoundFont,
        ImportMidi,
        ExportMidi,
        /// Copy every file the project refers to into its folder.
        CollectAssets,
        /// Render the project to a WAV file.
        ExportAudio,
        /// Render only the cycle region to a WAV file.
        ExportCycle,
        /// Start or stop playback.
        TogglePlay,
        /// Stop playback and return to the start.
        StopPlayback,
        /// Move the playhead to the beginning.
        ReturnToZero,
        /// Toggle looping over the loop region.
        ToggleLoop,
        /// Turn the click on or off.
        ToggleMetronome,
        /// Add an instrument track.
        AddInstrumentTrack,
        /// Add an audio track.
        AddAudioTrack,
        /// Delete the selected track.
        DeleteTrack,
        /// Add a bus to mix other tracks through.
        AddBusTrack,
        /// Duplicate the selected track, its sound and its clips.
        DuplicateTrack,
        /// Mute or unmute the selected track.
        ToggleTrackMute,
        /// Solo or unsolo the selected track.
        ToggleTrackSolo,
        /// Delete the current selection.
        DeleteSelection,
        /// Select every note in the clip being edited.
        SelectAllNotes,
        /// Lay a copy of the selected notes down after them.
        DuplicateNotes,
        /// Put the selected notes on the clipboard and remove them.
        CutNotes,
        /// Put the selected notes on the clipboard.
        CopyNotes,
        /// Lay the clipboard's notes into the clip being edited, at the playhead.
        PasteNotes,
        /// Raise the selected notes by a semitone.
        TransposeUp,
        /// Lower the selected notes by a semitone.
        TransposeDown,
        /// Raise the selected notes by an octave.
        OctaveUp,
        /// Lower the selected notes by an octave.
        OctaveDown,
        /// Select every clip in the song.
        SelectAllClips,
        /// Lay a copy of the selected clips down after them.
        DuplicateClip,
        /// Put the selected clips on the clipboard and remove them.
        CutClips,
        /// Put the selected clips on the clipboard.
        CopyClips,
        /// Lay the clipboard's clips onto the selected track, at the playhead.
        PasteClips,
        /// Cut the selected clip in two where the playhead is.
        SplitClip,
        /// Mute or unmute the selected clip.
        ToggleClipMute,
        /// Repeat the selected clip out to the next one, or stop it repeating.
        ToggleClipLoop,
        /// Snap the selected notes' starts onto the editing grid.
        QuantizeNoteStarts,
        /// Snap the selected notes' lengths onto the editing grid.
        QuantizeNoteLengths,
        /// Snap both of the selected notes' numbers onto the editing grid.
        QuantizeNotes,
        /// Put the next of the piano roll's tools in hand.
        NextTool,
        /// Type the tempo of the stretch the playhead is in.
        SetTempo,
        /// Type the time signature of the stretch the playhead is in.
        SetTimeSignature,
        /// Step the editing grid to the next division.
        CycleGrid,
        /// Type a bar and beat to move the playhead to.
        GoToPosition,
        /// Undo the last edit.
        Undo,
        /// Redo the last undone edit.
        Redo,
        /// Silence every voice and reset the engine.
        PanicStop,
        /// Zoom the timeline in.
        ZoomIn,
        /// Zoom the timeline out.
        ZoomOut,
        /// Show or hide the library panel.
        ToggleLibrary,
        /// Show or hide the inspector panel.
        ToggleInspector,
        /// Show or hide the piano roll.
        TogglePianoRoll,
        /// Show or hide the mixer.
        ToggleMixer,
        /// Show or hide the log.
        ToggleLog,
        /// Show or hide the strip of section names above the arrangement.
        ToggleStructureLane,
        /// Show or hide the key and chord strip above the arrangement.
        ToggleHarmonyLane,
        /// Show or hide the tempo changes marked along the ruler.
        ToggleTempoMarks,
        /// Show or hide the pitch bend strip under the piano roll.
        ToggleBendLane,
        /// Show or hide the modulation strip under the piano roll.
        ToggleModulationLane,
        /// Open the settings window.
        OpenSettings,
        /// Open the command palette.
        OpenCommandPalette,
        /// Drop open the menu bar this window draws for itself.
        OpenMenuBar,
        /// Move keyboard focus to the next panel.
        FocusNextPane,
        /// Move keyboard focus to the previous panel.
        FocusPreviousPane,
    ]
);

/// The key contexts a binding can be scoped to.
///
/// gpui dispatches an action from whatever holds focus up through its ancestors, matching each
/// binding's context against the names it passes on the way. [`context::WINDOW`] sits at the root and is
/// therefore always on that path; a pane's name is only on it while that pane holds focus. That
/// is what lets `t` mean one thing in the piano roll without meaning it everywhere.
pub mod context {
    /// The window. A binding here fires wherever focus is.
    pub const WINDOW: &str = "Auris";
    /// A sheet or the palette is up. Nothing is bound here, which is how a text field gets its
    /// keystrokes: `i` has to type an `i` rather than toggle the inspector.
    pub const PROMPT: &str = "AurisPrompt";
    /// The sound library.
    pub const LIBRARY: &str = "AurisLibrary";
    /// The track lanes and the ruler above them.
    pub const ARRANGEMENT: &str = "AurisArrangement";
    /// The piano roll, whichever dock is showing it.
    pub const ROLL: &str = "AurisRoll";
    /// The mixer, whichever dock is showing it.
    pub const MIXER: &str = "AurisMixer";
    /// The inspector.
    pub const INSPECTOR: &str = "AurisInspector";
    /// The log.
    pub const LOG: &str = "AurisLog";
}

/// One command the user can rebind.
///
/// `bind` exists because [`KeyBinding::new`] needs a concrete action type, so a table of
/// `Box<dyn Action>` would not do — each entry carries its own constructor instead. `make` is the
/// same problem from the other end: the command palette has to *dispatch* a command it picked out
/// of this table, and dispatching takes an owned action.
#[derive(Debug)]
pub struct Bindable {
    /// Stable identifier written to the settings file. Never change a released one.
    pub id: &'static str,
    /// Section heading in the settings window.
    pub group: Key,
    /// Name shown to the user.
    pub label: Key,
    /// Where the command can be reached from. One of the names in [`context`].
    pub context: &'static str,
    /// Keystroke used when the user has not chosen one, or `None` for a command that ships with
    /// no key.
    ///
    /// `None` is not "we forgot". A command reachable from a menu or a context menu is already
    /// usable, and a keystroke is only worth spending on one somebody reaches for often enough
    /// to want it under a finger. The keyboard has far fewer chords than this table has rows, and
    /// squatting on `⌥⇧K` because a command needed *some* default is worse than leaving the row
    /// blank: it makes the chord unavailable to the person who did have a use for it, and buries
    /// the commands that earned their key among ones nobody asked for. The row is still in the
    /// settings window, still in the palette, and one click from a key of the user's choosing.
    pub default: Option<&'static str>,
    bind: fn(&str) -> KeyBinding,
    make: fn() -> Box<dyn Action>,
}

/// Two entries are the same command when they have the same id.
///
/// Written out rather than derived: a derive would compare the two function pointers as well, and
/// comparing those is meaningless — the same function can have different addresses in different
/// codegen units, and two different ones can be merged into the same address.
impl PartialEq for Bindable {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for Bindable {}

impl Bindable {
    /// Builds the binding for `keystroke`.
    pub fn binding(&self, keystroke: &str) -> KeyBinding {
        (self.bind)(keystroke)
    }

    /// The action this command dispatches.
    pub fn action(&self) -> Box<dyn Action> {
        (self.make)()
    }

    /// Whether both commands can be reached from the same place, and so could clash on a key.
    ///
    /// Two panes are never focused at once, so the same keystroke may mean one thing in the piano
    /// roll and another in the mixer without either being wrong — reporting that as a conflict
    /// would be reporting the point of having contexts at all. A pane's context sits *inside*
    /// the window's, though, so anything bound at the window level is reachable from every pane
    /// and does clash with all of them.
    pub fn shares_reach_with(&self, other: &Bindable) -> bool {
        self.context == other.context
            || self.context == context::WINDOW
            || other.context == context::WINDOW
    }
}

/// Key context the window itself is in.
///
/// Kept as its own name because the root element names it directly. See [`context`] for the rest.
pub const KEY_CONTEXT: &str = context::WINDOW;

macro_rules! bindable {
    ($($context:path => { $($id:literal, $group:ident, $label:ident, $default:literal => $action:ident;)* })*) => {
        /// Every command the settings window offers to rebind, in display order.
        pub const BINDABLE: &[Bindable] = &[
            $($(Bindable {
                id: $id,
                group: Key::$group,
                label: Key::$label,
                context: $context,
                // `""` in the table rather than `None`, so every row stays one line of the same
                // shape and the column of keystrokes reads down the page.
                default: if $default.is_empty() { None } else { Some($default) },
                bind: |keys| KeyBinding::new(keys, $action, Some($context)),
                make: || Box::new($action),
            },)*)*
        ];
    };
}

// `secondary` rather than `cmd`: gpui resolves it to ⌘ on macOS and to Ctrl everywhere else,
// which is what each platform's users already have in their fingers. Writing `cmd` would bind
// the Windows key off a Mac, and the shell takes most of those combinations first.
bindable! {
    context::WINDOW => {
        "transport.play",       GroupTransport, CmdPlayStop,           "space"       => TogglePlay;
        "transport.return",     GroupTransport, CmdReturnToZero,       "enter"       => ReturnToZero;
        "transport.loop",       GroupTransport, CmdToggleCycle,        "secondary-l" => ToggleLoop;
        // Logic's own key for the click, and free here. A bare letter at the window's context is
        // the same bargain the panel toggles above already take: nothing types into the window,
        // and a sheet or a prompt is a context of its own where nothing at all is bound.
        "transport.metronome",  GroupTransport, CmdToggleMetronome,    "k"           => ToggleMetronome;
        "transport.panic",      GroupTransport, CmdPanic,              "escape"      => PanicStop;
        // The readouts in the middle of the transport bar answer to the mouse and, until now, to
        // nothing else. `g` for go, which is what every editor calls this.
        "transport.go_to",      GroupTransport, CmdGoToPosition,       "secondary-g" => GoToPosition;

        "file.new",             GroupFile,      CmdNewProject,         "secondary-n" => NewProject;
        "file.open",            GroupFile,      CmdOpenProject,        "secondary-o" => OpenProject;
        "file.save",            GroupFile,      CmdSave,               "secondary-s" => SaveProject;
        "file.save_as",         GroupFile,      CmdSaveAs,             "secondary-shift-s" => SaveProjectAs;
        "file.import",          GroupFile,      CmdImportAudio,        "secondary-i" => ImportAudio;
        "file.import_soundfont", GroupFile,     CmdImportSoundFont,    "secondary-shift-i" => ImportSoundFont;
        // M for MIDI, in and out. The table wants a keystroke per command rather than an empty
        // one, and these are the letters left that mean anything — the I and E pairs both went to
        // audio, which is imported and exported far more often.
        "file.import_midi",     GroupFile,      CmdImportMidi,         "secondary-m" => ImportMidi;
        "file.export_midi",     GroupFile,      CmdExportMidi,         "secondary-alt-m" => ExportMidi;
        "file.collect",         GroupFile,      CmdCollectAssets,      "secondary-shift-a" => CollectAssets;
        "file.export",          GroupFile,      CmdExportWav,          "secondary-e" => ExportAudio;
        "file.export_cycle",    GroupFile,      CmdExportCycle,        "secondary-shift-e" => ExportCycle;
        "file.quit",            GroupFile,      CmdQuit,               "secondary-q" => Quit;

        // Their ids still begin `file.` because an id is written into settings files and never
        // changes once released. The *group* is what a person reads, and these two are not file
        // operations — one of them opens a form with no file in sight.
        "file.compose",         GroupCompose,   CmdComposeSong,        "secondary-shift-c" => ComposeSong;
        // The sheet is the primary way in and keeps the plain shift; the file picker is what an
        // agent-written or hand-edited document goes through, one modifier further out.
        "file.compose_spec",    GroupCompose,   CmdComposeFromSpec,    "secondary-alt-c" => ComposeFromSpec;
        // No default. The two above take the chord that is free and the mnemonic that is obvious;
        // this one has neither, and squatting on a third combination would take it from whoever
        // wanted it more. The row is here so it can be given one.
        "file.accompany",       GroupCompose,   CmdAccompanyMelody,    ""            => AccompanyMelody;

        "edit.undo",            GroupEdit,      CmdUndo,               "secondary-z" => Undo;
        "edit.redo",            GroupEdit,      CmdRedo,               "secondary-shift-z" => Redo;
        "edit.delete",          GroupEdit,      CmdDeleteSelection,    "backspace"   => DeleteSelection;
        // The three things about the song a person changes by reaching for a readout with the
        // mouse. B for beats per minute, M for meter, G for grid.
        "edit.tempo",           GroupEdit,      CmdSetTempo,           "secondary-shift-b" => SetTempo;
        "edit.signature",       GroupEdit,      CmdSetSignature,       "secondary-shift-m" => SetTimeSignature;
        "edit.grid",            GroupEdit,      CmdCycleGrid,          "secondary-shift-g" => CycleGrid;

        "track.add_instrument", GroupTrack,     CmdAddInstrumentTrack, "secondary-t" => AddInstrumentTrack;
        "track.add_audio",      GroupTrack,     CmdAddAudioTrack,      "secondary-shift-t" => AddAudioTrack;
        // B for bus, and it is the one plain letter of the three that was still free.
        "track.add_bus",        GroupTrack,     CmdAddBusTrack,        "secondary-b" => AddBusTrack;
        "track.delete",         GroupTrack,     CmdDeleteTrack,        "secondary-backspace" => DeleteTrack;
        // The four below reached the track under the pointer through its context menu and
        // nothing else, so a person working from the keyboard could not mute a track at all.
        // They ship with no key: mute and solo want M and S, which the mixer and the structure
        // lane hold, and inventing a chord nobody would guess is worse than an empty row a
        // person can fill in with the one they do want.
        "track.duplicate",      GroupTrack,     CmdDuplicateTrack,     ""            => DuplicateTrack;
        "track.mute",           GroupTrack,     CmdToggleTrackMute,    ""            => ToggleTrackMute;
        "track.solo",           GroupTrack,     CmdToggleTrackSolo,    ""            => ToggleTrackSolo;

        // `y` is Logic's own Library key, and it is free here.
        "view.library",         GroupView,      CmdShowLibrary,        "y"           => ToggleLibrary;
        "view.inspector",       GroupView,      CmdShowInspector,      "i"           => ToggleInspector;
        "view.piano_roll",      GroupView,      CmdShowPianoRoll,      "p"           => TogglePianoRoll;
        "view.mixer",           GroupView,      CmdShowMixer,          "m"           => ToggleMixer;
        // Out on `secondary-alt-` with the arrangement furniture rather than on a bare letter
        // like its three sibling panels: the log is opened on the day something is wrong and left
        // alone every other day, and a plain letter is worth more to something reached mid-take.
        "view.log",             GroupView,      CmdShowLog,            "secondary-alt-l" => ToggleLog;
        // The three strips over the arrangement. Out on `secondary-alt-` because they are
        // arrangement furniture rather than things reached mid-take, and because the plain and
        // shifted forms of these letters are all spoken for.
        "view.structure_lane",  GroupView,      CmdShowStructureLane,  "secondary-alt-s" => ToggleStructureLane;
        "view.harmony_lane",    GroupView,      CmdShowHarmonyLane,    "secondary-alt-h" => ToggleHarmonyLane;
        "view.tempo_marks",     GroupView,      CmdShowTempoMarks,     "secondary-alt-t" => ToggleTempoMarks;
        "view.bend_lane",       GroupView,      CmdShowBendLane,       "secondary-alt-b" => ToggleBendLane;
        "view.modulation_lane", GroupView,      CmdShowModulationLane, "secondary-alt-w" => ToggleModulationLane;
        "view.zoom_in",         GroupView,      CmdZoomIn,             "secondary-=" => ZoomIn;
        "view.zoom_out",        GroupView,      CmdZoomOut,            "secondary--" => ZoomOut;
        "view.settings",        GroupView,      CmdSettings,           "secondary-," => OpenSettings;
        // What VS Code and Zed both use, and free here — `p` alone already shows the piano roll.
        "view.palette",         GroupView,      CmdCommandPalette,     "secondary-shift-p" => OpenCommandPalette;
        // F10 is what Windows has reached the menu bar with since there was one. Not the Alt key
        // it also uses: a modifier on its own is not a keystroke gpui can bind, and one that was
        // would fire on every ⌥-click the roll uses to delete a note.
        "view.menu_bar",        GroupView,      CmdOpenMenuBar,        "f10"         => OpenMenuBar;
        "view.focus_next",      GroupView,      CmdFocusNextPane,      "tab"         => FocusNextPane;
        "view.focus_previous",  GroupView,      CmdFocusPreviousPane,  "shift-tab"   => FocusPreviousPane;
    }

    // Scoped to the roll, which is what having contexts buys: `t` is a bare letter, and a bare
    // letter that fired everywhere would change a mode the user cannot see while they are looking
    // at the mixer. It is Logic's own tool key, where pressing it twice swaps back to the tool
    // before — with two tools that is exactly a cycle, so it is one command rather than one per
    // tool.
    //
    // The note commands are scoped here for the other reason contexts exist: ⌘A and ⌘D mean the
    // notes in the roll and the clips in the arrangement, and which one a user meant is answered
    // by where they are looking. Both spellings are below, on the same keys, and neither
    // shadows the other — see `Bindable::shares_reach_with`.
    //
    // Grouped as "Notes" rather than under Edit, which the window's own block already heads: the
    // settings page walks this table in order and starts a section wherever the group changes, so
    // a second run of `GroupEdit` down here would print a second Edit heading with no way to tell
    // the two apart.
    context::ROLL => {
        "edit.next_tool",       GroupNotes,     CmdNextTool,           "t"           => NextTool;
        "edit.select_all",      GroupNotes,     CmdSelectAllNotes,     "secondary-a" => SelectAllNotes;
        "edit.duplicate",       GroupNotes,     CmdDuplicateNotes,     "secondary-d" => DuplicateNotes;
        // The three keystrokes nobody has to be told. Scoped to the roll, and paired with the
        // clip row of the same name below, for the reason `edit.select_all` is: two panes are
        // never focused at once, so ⌘C means "these notes" or "these clips" depending on where
        // the eye already is — which is the only reading either could have.
        "edit.cut",             GroupNotes,     CmdCutNotes,           "secondary-x" => CutNotes;
        "edit.copy",            GroupNotes,     CmdCopyNotes,          "secondary-c" => CopyNotes;
        "edit.paste",           GroupNotes,     CmdPasteNotes,         "secondary-v" => PasteNotes;
        // Logic's own four, and the arrow keys are the one part of the keyboard where ⌥ and a
        // letter cannot collide with the character that letter would have typed.
        "edit.transpose_up",    GroupNotes,     CmdTransposeUp,        "alt-up"      => TransposeUp;
        "edit.transpose_down",  GroupNotes,     CmdTransposeDown,      "alt-down"    => TransposeDown;
        "edit.octave_up",       GroupNotes,     CmdOctaveUp,           "alt-shift-up" => OctaveUp;
        "edit.octave_down",     GroupNotes,     CmdOctaveDown,         "alt-shift-down" => OctaveDown;
        // Q is what every sequencer since the first one has quantised with, and it means the
        // starts — which is what "quantise" means when nobody says which. The other two are the
        // same command with a different half of the note in it, and take no key: the table's
        // policy is that a chord nobody would guess is worth less than a row somebody can fill
        // in, and both are a right-click away in the roll.
        "edit.quantize",        GroupNotes,     CmdQuantize,           "q"           => QuantizeNoteStarts;
        "edit.quantize_lengths", GroupNotes,    CmdQuantizeLengths,    ""            => QuantizeNoteLengths;
        "edit.quantize_both",   GroupNotes,     CmdQuantizeBoth,       ""            => QuantizeNotes;
    }

    context::ARRANGEMENT => {
        "clip.select_all",      GroupClip,      CmdSelectAllClips,     "secondary-a" => SelectAllClips;
        "clip.duplicate",       GroupClip,      CmdDuplicateClip,      "secondary-d" => DuplicateClip;
        "clip.cut",             GroupClip,      CmdCutClips,           "secondary-x" => CutClips;
        "clip.copy",            GroupClip,      CmdCopyClips,          "secondary-c" => CopyClips;
        "clip.paste",           GroupClip,      CmdPasteClips,         "secondary-v" => PasteClips;
        // X is the scissors everywhere that has a pair. Not ⌘T, which Logic splits with and
        // which is the instrument track here.
        "clip.split",           GroupClip,      CmdSplitClip,          "alt-x"       => SplitClip;
        "clip.mute",            GroupClip,      CmdToggleClipMute,     ""            => ToggleClipMute;
        // Logic's own key for looping a region, and a bare letter is free here for the reason
        // `edit.next_tool` is: the arrangement is not a place anything types.
        "clip.loop",            GroupClip,      CmdToggleClipLoop,     "l"           => ToggleClipLoop;
    }
}

/// The bindable command with this id.
pub fn bindable(id: &str) -> Option<&'static Bindable> {
    BINDABLE.iter().find(|entry| entry.id == id)
}

/// What a key context is called, for the settings window to say where a command lives.
///
/// `None` for the window's own context, which is where nearly every command is: a chip on nearly
/// every row would be a column of noise, and its absence says "everywhere" perfectly well.
pub fn context_label(context: &str) -> Option<Key> {
    match context {
        context::LIBRARY => Some(Key::ScopeLibrary),
        context::ARRANGEMENT => Some(Key::ScopeArrangement),
        context::ROLL => Some(Key::ScopeRoll),
        context::MIXER => Some(Key::ScopeMixer),
        context::INSPECTOR => Some(Key::ScopeInspector),
        _ => None,
    }
}

/// `keystroke` written the way this platform writes it.
///
/// The defaults above say `secondary-`, which gpui resolves to ⌘ on macOS and to Ctrl
/// everywhere else; a keystroke captured from the keyboard arrives already resolved. The two
/// forms name the same key and compare unequal as text, so a ⌘L the user pressed would neither
/// count as "back to the default" nor be reported as clashing with one. Everything that
/// compares or displays a keystroke goes through here first.
///
/// A chunk gpui cannot parse is passed through untouched rather than dropped: this is used on
/// the display path, and showing the user what is actually in their settings file beats showing
/// them nothing.
pub fn normalise_keystroke(keystroke: &str) -> String {
    keystroke
        .split_whitespace()
        .map(|chunk| {
            gpui::Keystroke::parse(chunk)
                .map(|parsed| parsed.unparse())
                .unwrap_or_else(|_| chunk.to_string())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `keystroke` as a menu would print it.
///
/// `ctrl-shift-s` is the form gpui parses; it is not a form anyone would put in a menu. macOS
/// stacks glyphs in a fixed order and leaves out the separators, and Windows spells the
/// modifiers out and joins them with plus signs, so each platform gets its own.
pub fn menu_keystroke(keystroke: &str) -> String {
    normalise_keystroke(keystroke)
        .split_whitespace()
        .map(pretty_chunk)
        .collect::<Vec<_>>()
        .join(" ")
}

/// `keystroke` written the way the keymap file stores one.
///
/// gpui reports what the *keyboard* did — `cmd-s` on a Mac, `ctrl-s` on Windows — and the file
/// is shared between machines and checked into dotfiles. `secondary-` is the spelling that means
/// "whichever modifier this platform uses for a command", so a binding captured on either one
/// works on both. Without this a keymap.json captured on a Mac asked Windows for the Windows
/// key, which the shell claims long before the application sees it.
pub fn portable_keystroke(keystroke: &str) -> String {
    keystroke
        .split_whitespace()
        .map(portable_chunk)
        .collect::<Vec<_>>()
        .join(" ")
}

/// One keystroke of a binding, with the platform's command modifier written as `secondary-`.
fn portable_chunk(chunk: &str) -> String {
    let Ok(parsed) = gpui::Keystroke::parse(chunk) else {
        return chunk.to_string();
    };
    let modifiers = parsed.modifiers;
    let mac = cfg!(target_os = "macos");
    let mut out = String::new();
    // The fn key is nobody's command modifier and has no portable stand-in, so it is written out
    // as itself. gpui reports it only for a key that is not already an arrow or an F-key, which
    // is what keeps a plain ← from being stored as `fn-left`.
    if modifiers.function {
        out.push_str("fn-");
    }
    if modifiers.secondary() {
        out.push_str("secondary-");
    }
    // Control and the platform key are one pair seen from two sides: on macOS `secondary` is ⌘,
    // which leaves control a modifier in its own right, and everywhere else `secondary` *is*
    // control, which leaves the Windows or Super key as the odd one out. Each is written unless
    // the `secondary-` above has already said it. Writing the one it stands for twice would
    // produce `secondary-ctrl-s`; leaving the other out cost macOS the whole control-⌘ space,
    // which came back stored as the plain ⌘ chord the user had not pressed.
    if modifiers.control && mac {
        out.push_str("ctrl-");
    }
    if modifiers.platform && !mac {
        // Never `cmd-`: gpui would parse it back to the same key, but the file is read by people
        // as well, and there is no command key on the keyboard this was captured from.
        out.push_str(if cfg!(target_os = "windows") {
            "win-"
        } else {
            "super-"
        });
    }
    if modifiers.alt {
        out.push_str("alt-");
    }
    if modifiers.shift {
        out.push_str("shift-");
    }
    out.push_str(&parsed.key);
    out
}

/// One keystroke of a possibly multi-stroke binding, prettified.
fn pretty_chunk(chunk: &str) -> String {
    let Ok(parsed) = gpui::Keystroke::parse(chunk) else {
        return chunk.to_string();
    };
    let modifiers = parsed.modifiers;
    let key = pretty_key(&parsed.key);

    if cfg!(target_os = "macos") {
        // ⌃⌥⇧⌘ then the key, which is the order Apple prints them in and the order people
        // read them in without thinking about it.
        let mut out = String::new();
        for (held, glyph) in [
            (modifiers.control, '⌃'),
            (modifiers.alt, '⌥'),
            (modifiers.shift, '⇧'),
            (modifiers.platform, '⌘'),
        ] {
            if held {
                out.push(glyph);
            }
        }
        out.push_str(&key);
        return out;
    }

    let mut parts: Vec<&str> = Vec::new();
    for (held, name) in [
        (modifiers.control, "Ctrl"),
        (modifiers.alt, "Alt"),
        (modifiers.shift, "Shift"),
        // Never produced by a default, but a user can bind it and an empty gap would be worse.
        (modifiers.platform, "Win"),
    ] {
        if held {
            parts.push(name);
        }
    }
    parts.push(&key);
    parts.join("+")
}

/// A key's name as a menu prints it.
fn pretty_key(key: &str) -> String {
    let mac = cfg!(target_os = "macos");
    let named = match key {
        "backspace" if mac => "⌫",
        "backspace" => "Backspace",
        "delete" if mac => "⌦",
        "delete" => "Del",
        "enter" if mac => "⏎",
        "enter" => "Enter",
        "escape" if mac => "⎋",
        "escape" => "Esc",
        "tab" if mac => "⇥",
        "tab" => "Tab",
        "space" => "Space",
        "left" => "←",
        "right" => "→",
        "up" => "↑",
        "down" => "↓",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    // A letter is shown in upper case — `⌘S`, never `⌘s` — and punctuation is left alone, so
    // `=` does not become something unrecognisable. Anything longer keeps its shape with a
    // capital, which is what turns `f1` into `F1`.
    let mut chars = key.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Whether `keystroke` is something the user could actually press.
///
/// Checked before anything is bound, because [`KeyBinding::new`] panics on a malformed
/// keystroke and a settings file is user-editable text. Modifiers alone are rejected too —
/// gpui parses `cmd-` happily, but a binding with no key can never fire.
pub fn is_valid_keystroke(keystroke: &str) -> bool {
    let mut chunks = keystroke.split_whitespace().peekable();
    if chunks.peek().is_none() {
        return false;
    }
    chunks
        .all(|chunk| gpui::Keystroke::parse(chunk).is_ok_and(|keystroke| !keystroke.key.is_empty()))
}

/// Installs `bindings`, replacing whatever was bound before.
///
/// Rebinding replaces rather than adds: `cx.bind_keys` appends, so without the clear an old
/// keystroke would keep working alongside the new one.
pub fn install_bindings(cx: &mut App, bindings: impl IntoIterator<Item = KeyBinding>) {
    cx.clear_key_bindings();
    cx.bind_keys(bindings);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Every command that ships with a keystroke, as its id and that keystroke.
    ///
    /// The sweeps below are all about what a default *says*, and a command with no default says
    /// nothing — skipping them here rather than in each loop keeps the assertions about the
    /// keystroke rather than about whether there is one.
    fn defaults() -> impl Iterator<Item = (&'static str, &'static str)> {
        BINDABLE
            .iter()
            .filter_map(|entry| Some((entry.id, entry.default?)))
    }

    #[test]
    fn every_bindable_id_and_default_is_usable() {
        let mut ids = BTreeSet::new();
        for entry in BINDABLE {
            assert!(ids.insert(entry.id), "duplicate id `{}`", entry.id);
            if let Some(default) = entry.default {
                assert!(
                    is_valid_keystroke(default),
                    "`{}` has an unparseable default `{default}`",
                    entry.id,
                );
            }
            assert_eq!(bindable(entry.id).map(|e| e.id), Some(entry.id));
        }
    }

    #[test]
    fn a_blank_default_reads_back_as_no_key_rather_than_as_a_keystroke() {
        // The table spells "no key" as `""` so every row stays one line of the same shape. If
        // that ever stopped becoming `None`, `KeyBinding::new` would be handed an empty string
        // and panic on the way up.
        let unbound = BINDABLE
            .iter()
            .find(|entry| entry.default.is_none())
            .expect("some commands ship with no key");
        assert!(!is_valid_keystroke(""));
        assert_eq!(unbound.default, None);
    }

    #[test]
    fn no_two_commands_that_can_be_reached_together_share_a_default() {
        // Compared as this platform writes them: two defaults spelled differently could still
        // resolve to the same physical keys. Scoped as well, because two panes are never focused
        // at once — the same key meaning one thing in the roll and another in the mixer is the
        // point of having contexts, not a collision. A command with no default collides with
        // nothing, which is one of the things having no default is for.
        for (index, entry) in BINDABLE.iter().enumerate() {
            for other in &BINDABLE[index + 1..] {
                if !entry.shares_reach_with(other) {
                    continue;
                }
                let (Some(one), Some(two)) = (entry.default, other.default) else {
                    continue;
                };
                assert_ne!(
                    normalise_keystroke(one),
                    normalise_keystroke(two),
                    "`{}` collides with `{}` on `{one}`",
                    entry.id,
                    other.id,
                );
            }
        }
    }

    #[test]
    fn the_note_and_clip_pairs_are_deliberately_on_the_same_keys() {
        // Select All and Duplicate mean the notes in the roll and the clips in the arrangement,
        // and share a key on purpose — which is a claim about the *contexts*, not a coincidence
        // of the table. If either moved to the window's context the pair would become a genuine
        // conflict, and the sibling test above would catch it. This one catches the other
        // direction: somebody "fixing" the duplicate keystroke by moving one of them off.
        for (notes, clips) in [
            ("edit.select_all", "clip.select_all"),
            ("edit.duplicate", "clip.duplicate"),
            ("edit.cut", "clip.cut"),
            ("edit.copy", "clip.copy"),
            ("edit.paste", "clip.paste"),
        ] {
            let notes = bindable(notes).expect("a real command");
            let clips = bindable(clips).expect("a real command");
            assert_eq!(notes.context, context::ROLL);
            assert_eq!(clips.context, context::ARRANGEMENT);
            assert_eq!(
                notes.default, clips.default,
                "`{}` and `{}` are meant to be the same key in two panes",
                notes.id, clips.id
            );
            assert!(!notes.shares_reach_with(clips));
        }
    }

    #[test]
    fn a_pane_binding_is_shadowed_by_nothing_and_shadows_nothing_across_panes() {
        let window = BINDABLE
            .iter()
            .find(|entry| entry.context == context::WINDOW)
            .expect("the window holds most of the commands");
        let roll = BINDABLE
            .iter()
            .find(|entry| entry.context == context::ROLL)
            .expect("the roll has at least the tool key");

        let reason = "the window's context is on the dispatch path to every pane";
        assert!(window.shares_reach_with(roll), "{reason}");
        assert!(roll.shares_reach_with(window), "{reason}");
        assert!(roll.shares_reach_with(roll));

        // The relation is what conflict detection rests on, so it has to be symmetric.
        for a in BINDABLE {
            for b in BINDABLE {
                assert_eq!(a.shares_reach_with(b), b.shares_reach_with(a));
            }
        }
    }

    #[test]
    fn every_command_names_a_context_that_exists() {
        // A typo would parse as an identifier gpui never sees on the dispatch path, and the
        // binding would simply never fire — with nothing anywhere to say why.
        const CONTEXTS: &[&str] = &[
            context::WINDOW,
            context::LIBRARY,
            context::ARRANGEMENT,
            context::ROLL,
            context::MIXER,
            context::INSPECTOR,
        ];
        for entry in BINDABLE {
            assert!(
                CONTEXTS.contains(&entry.context),
                "`{}` is scoped to `{}`, which is not a context",
                entry.id,
                entry.context
            );
            assert_ne!(
                entry.context,
                context::PROMPT,
                "`{}` is bound where the whole point is that nothing is",
                entry.id
            );
        }
    }

    #[test]
    fn no_default_names_a_key_this_platform_lacks() {
        // `cmd` is gpui's *platform* modifier, which off a Mac is the Windows or Super key —
        // reserved by the shell, so the binding would simply never fire.
        for (id, default) in defaults() {
            assert!(
                !default.contains("cmd-")
                    && !default.contains("super-")
                    && !default.contains("win-"),
                "`{id}` binds a platform key directly; use `secondary-`",
            );
        }
    }

    #[test]
    fn normalising_makes_the_two_spellings_of_a_default_agree() {
        // What the table says and what the keyboard reports must land on the same string, or
        // conflict detection and "is this the default?" both quietly stop working.
        let native = if cfg!(target_os = "macos") {
            "cmd-l"
        } else {
            "ctrl-l"
        };
        assert_eq!(normalise_keystroke("secondary-l"), native);
        assert_eq!(normalise_keystroke(native), native);
        assert_eq!(normalise_keystroke("g g"), "g g");
        // Every default survives the round trip, punctuation and all.
        for (id, default) in defaults() {
            let once = normalise_keystroke(default);
            assert_eq!(
                normalise_keystroke(&once),
                once,
                "`{id}` does not normalise to a fixed point",
            );
            assert!(
                is_valid_keystroke(&once),
                "`{id}` normalises to an unbindable `{once}`",
            );
        }
    }

    #[test]
    fn the_stored_spelling_keeps_every_modifier_the_user_held() {
        // The other direction from the test above: what the keyboard reported, written the way
        // the file spells it. Control and the platform key swap roles across platforms —
        // whichever of the two is `secondary` is written as that, and the *other* one still has
        // to survive, or what comes back is a chord nobody pressed.
        let control_command = portable_chunk("ctrl-cmd-l");
        if cfg!(target_os = "macos") {
            assert_eq!(control_command, "secondary-ctrl-l");
            assert_eq!(portable_chunk("ctrl-l"), "ctrl-l");
            assert_eq!(portable_chunk("cmd-l"), "secondary-l");
        } else if cfg!(target_os = "windows") {
            assert_eq!(control_command, "secondary-win-l");
            assert_eq!(portable_chunk("ctrl-l"), "secondary-l");
            assert_eq!(portable_chunk("cmd-l"), "win-l");
        } else {
            assert_eq!(control_command, "secondary-super-l");
            assert_eq!(portable_chunk("ctrl-l"), "secondary-l");
            assert_eq!(portable_chunk("cmd-l"), "super-l");
        }
        assert_ne!(
            control_command,
            portable_chunk("secondary-l"),
            "a chord stored as the plain secondary one is bound to whatever holds that key"
        );
        // The fn key stands for nothing else and has no platform to argue about.
        assert_eq!(portable_chunk("fn-a"), "fn-a");
        assert_eq!(portable_chunk("fn-shift-a"), "fn-shift-a");
        // And every one of them survives the trip out to the keyboard's spelling and back.
        for stored in [control_command.as_str(), "fn-a", "fn-shift-a"] {
            assert!(is_valid_keystroke(stored), "`{stored}` cannot be bound");
            assert_eq!(
                portable_chunk(&normalise_keystroke(stored)),
                stored,
                "`{stored}` does not survive the round trip"
            );
        }
    }

    #[test]
    fn a_menu_prints_keystrokes_the_way_the_platform_does() {
        if cfg!(target_os = "macos") {
            assert_eq!(menu_keystroke("secondary-s"), "⌘S");
            assert_eq!(menu_keystroke("secondary-shift-s"), "⇧⌘S");
            assert_eq!(menu_keystroke("secondary-backspace"), "⌘⌫");
            assert_eq!(menu_keystroke("escape"), "⎋");
        } else {
            assert_eq!(menu_keystroke("secondary-s"), "Ctrl+S");
            assert_eq!(menu_keystroke("secondary-shift-s"), "Ctrl+Shift+S");
            assert_eq!(menu_keystroke("secondary-backspace"), "Ctrl+Backspace");
            assert_eq!(menu_keystroke("escape"), "Esc");
        }
        assert_eq!(menu_keystroke("space"), "Space");
        assert_eq!(menu_keystroke("g g"), "G G");
    }

    #[test]
    fn every_default_prints_as_something_a_person_can_read() {
        for (id, default) in defaults() {
            let printed = menu_keystroke(default);
            assert!(!printed.is_empty(), "`{id}` prints as nothing at all");
            assert!(
                !printed.contains('-') || printed.contains("Ctrl+-") || printed.ends_with('-'),
                "`{id}` still prints in gpui's own syntax: `{printed}`",
            );
            assert!(
                !printed.contains("secondary"),
                "`{id}` leaks the portable spelling into the menu: `{printed}`",
            );
        }
    }

    #[test]
    fn malformed_keystrokes_are_rejected_before_they_can_panic() {
        assert!(is_valid_keystroke("cmd-shift-s"));
        assert!(is_valid_keystroke("g g"));
        assert!(!is_valid_keystroke(""));
        assert!(!is_valid_keystroke("   "));
        assert!(!is_valid_keystroke("notakey-x"));
        // gpui parses this, but a binding with no key can never fire.
        assert!(!is_valid_keystroke("cmd-"));
    }
}
