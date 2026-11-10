#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Derived from uthree/design-language, revision a3c5f69e1db8d19e7c70bf427250473c77208055 (#57).
# Source: https://github.com/uthree/design-language/blob/a3c5f69e1db8d19e7c70bf427250473c77208055/skills/atelier-gpui/tools/check-adoption.sh
# Find workarounds an application no longer needs once it builds on Atelier, so new code does
# not copy old code that predates a feature. Run it from the application's repository, or in
# its CI:
#
#     path/to/atelier-gpui/tools/check-adoption.sh [--strict] [DIR...]     # default DIR: src
#
# Each finding names the file and line and what replaces the workaround; CHANGELOG.md in the
# Atelier repository lists the release each replacement arrived in. Errors fail the run;
# warnings fail it only with --strict. To keep a line on purpose, end it with the comment
# `// atelier: allow` (or put it on the next line, where rustfmt moves a comment written after
# `fn name(`) and say why on the line above.

set -u

strict=0
dirs=()
for arg in "$@"; do
    case "$arg" in
        --strict) strict=1 ;;
        -h | --help)
            sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *) dirs+=("$arg") ;;
    esac
done
[ ${#dirs[@]} -eq 0 ] && dirs=(src)

errors=0
warnings=0

# check LEVEL PATTERN MESSAGE: report every Rust line matching the extended regex PATTERN.
check() {
    local level=$1 pattern=$2 message=$3
    local hits
    hits=$(grep -rnE --include='*.rs' -e "$pattern" "${dirs[@]}" 2>/dev/null || true)
    [ -z "$hits" ] && return
    while IFS= read -r hit; do
        local place=${hit%%:*}
        local rest=${hit#*:}
        local line=${rest%%:*}
        [[ "$hit" == *'atelier: allow'* ]] && continue
        # rustfmt moves a comment after `fn name(` onto the first parameter's line, so a marker
        # on the line straight after the match counts as well.
        sed -n "$((line + 1))p" "$place" | grep -q 'atelier: allow' && continue
        echo "$level: $place:$line: $message"
        if [ "$level" = error ]; then
            errors=$((errors + 1))
        else
            warnings=$((warnings + 1))
        fi
    done <<<"$hits"
}

# The app's mark in the title bar.
check warn 'Avatar::logo\(\s*IconSource::image' \
    "if this is the app's own icon in the title bar, use TitleBar::app_icon(IconSource::image(..)) with its title-bar.png: it takes the top-left corner at the bar's icon size. Avatar::logo is for a provider's mark."

# Wrapping and copying components.
check warn '\.debug_selector\(' \
    "if this div wraps an Atelier component, give the component the id instead (Button::new(id, ..), ListItem::new(id, ..), Plot3d::id): it uses it as its test selector. A control of your own keeps its selector with // atelier: allow."
# A div whose debug_selector wraps an Atelier component a few lines further down: the component
# takes that id itself. Reported once per wrapper, naming the component.
wrapped=$(find "${dirs[@]}" -name '*.rs' -print0 2>/dev/null | xargs -0 -r awk '
    FNR == 1 { pending = 0 }
    /atelier: allow/ { pending = 0; next }
    /\.debug_selector\(/ {
        pending = 5; place = FILENAME ":" FNR
        $0 = substr($0, index($0, ".debug_selector("))
    }
    pending > 0 {
        if (match($0, /(Button|ListItem|Switch|Checkbox|MenuItem|SegmentedControl|TextField|Slider|ReadoutStrip)::[a-z_]+\(/)) {
            name = substr($0, RSTART, RLENGTH)
            sub(/::.*/, "", name)
            print place ": " name
            pending = 0
        } else {
            pending--
        }
    }')
while IFS= read -r hit; do
    [ -z "$hit" ] && continue
    echo "error: ${hit%: *}: a debug_selector wrapped around ${hit##*: }: pass the id to ${hit##*: } itself, which uses it as its selector."
    errors=$((errors + 1))
done <<<"$wrapped"

check warn '\.tab_index\(' \
    "a hand-made tab stop: Atelier's Button, ListItem, MenuItem, Card::id and the rest are tab stops with the focus ring and Enter/Space already."
check error 'fn (nav_item|icon_button|readout|level_meter)\b' \
    "a copy of an Atelier component: use ListItem, Button::icon_only, Label::mono or LevelMeter, and the hook it now has (description_lines, IconSource, .control(..))."
check error 'titlebar::composed' \
    "use TitleBar::window_controls(width, buttons).window_drag(false)."
check error 'auris-(redo|bolt)\.svg' \
    "use IconName::Redo / IconName::Bolt; bring your own SVGs through IconSource::svg."

# Agent surfaces.
check error 'fn (user_message|assistant_message)\b' \
    "use UserMessage / AssistantMessage: .body(..) for your own Markdown view, .action(..) for Copy and Retry, AssistantMessage::anonymous() without the name line."
check warn '\b(unread_entries|follow_tail|is_near_tail)\b' \
    "a conversation that tracks its own tail and unread count: use atelier::agent::ChatView"
check warn 'fn (mode_warns|[a-z_]*approval_view)\b' \
    "a hand-built approval or permission mode menu: use atelier::agent::ApprovalDialog, ChatEntry::approval or PermissionPicker"
check warn 'fn ([a-z_]*status_row|context_gauge|phase_label|changed_files_list)\b' \
    "a hand-built activity line, plan or changed-files list: use atelier::agent::AgentActivity, PlanView or ChangedFiles"
check warn 'fn (bezier|execution_order|undo_agent_node|input_mouse_down)\b' \
    "a node graph copied from the node_editor example: use atelier::graph::NodeCanvas"
check warn 'fn ([a-z_]*choice_card|[a-z_]*queue_view|render_conversation_browser)\b' \
    "a hand-built choice card, message queue or conversation list: use atelier::agent::ChoiceCard, MessageQueue or ConversationList"
check warn 'TextView::markdown|text::TextView' \
    "Markdown in a transcript: use atelier::Markdown, which selects, copies and follows links"
check warn 'fn (render_nav|render_key_row|found_commands|[a-z_]*matches_query)\b' \
    "a hand-built preferences window or key list: use atelier::PreferencesWindow and KeybindingList"
check warn 'impl[^\n]*EntityInputHandler for' \
    "an editable text input of the application's own: use atelier::TextInput inside a TextField (caret, selection, IME, clipboard, undo; Space and Backspace never reach a shortcut while it is focused)."
check warn 'fn (inline_bounds|[a-z_]*inline_prompt)\b|InlinePlace::' \
    "a field opened over a label by hand: use atelier::InlineEdit, with SpotBook::spot on a label laid out by flex."
check warn 'struct PlayheadFollow\b|fn [a-z_]*return_to_playhead\b' \
    "a playhead follow of the application's own: use atelier::Follow (lets go on a scroll, not a zoom) and BackToPlayhead."
check warn 'fn (labelled_figure|[a-z_]*_figure)\b' \
    "a caption-number-unit figure drawn by hand: use atelier::Stat (Stat::empty for the dash before it is measured)."
check warn 'fn (level_meter|db_to_meter_position|meter_level)\b' \
    "a level meter or dB scale of the application's own: use atelier::LevelMeter::db(..).peak_db(..).clipped(..) and LevelMeter::position."
check warn 'fn [a-z_]*plot_well\b|const GUTTER: \(f32, f32\)' \
    "a plot made draggable by hand, guessing its data area: use atelier::plot::LinePlot::id(..).on_drag(..) (deltas in plot units), .on_reset, .on_wheel."
check warn 'fn keyed_tip\b|fn [a-z_]*shortcut_tooltip\b' \
    "a tooltip with keys built by hand: use Button::shortcut_for(&action) (read from the keymap) or Button::shortcut(..)."
check warn 'colou?r swatch has no component|the picker is the application.s own|mod colou?r_picker\b' \
    "a colour swatch or picker of the application's own: use atelier::ColorSwatch and ColorPicker (with presets), and atelier::{hex, parse_hex, Hsv}."
check warn 'fn [a-z_]*comment_notes\b' \
    "notes drawn onto graph nodes by hand: use GraphNode::comment(..), NodeCanvas::spots(..) and NodeCanvasEvent::CommentPressed, then an InlineEdit over NodeCanvas::comment_key."
check warn 'fn user_message_with_attachments\b' \
    "a person's message rebuilt to make its chips open: use ChatView::open_attachments() and ChatEvent::OpenAttachment."
check error 'fn selectable_text\b' \
    "use SelectableText (drag, Shift-click, double- and triple-click, Cmd/Ctrl+C), or UserMessage with an id, which selects already."
check error 'fn [a-z_]*copy_button\b' \
    "use CopyButton: it copies, says Copied for a moment and moves the label to its tooltip."

check error 'fn [a-z_]*composer_frame\b' \
    "use Composer: .tone(..) for a warning frame with its mark, .notice(..) for a row inside the frame, .attach_disabled(..), .control(..), and .busy(..).queue(true) for Stop beside a queueing Send."
check error '(punch|notes)\.svg' \
    "use IconName::Punch / IconName::Notes."
check warn 'motion\.enabled *= *false' \
    "in tests, call atelier::motion::reduce_motion(cx)."

# Keyboard.
check error 'Claim[A-Za-z]*Activation' \
    "Atelier controls claim Enter and Space themselves (key context AtelierControl). Drop the claiming action; to keep a shortcut off focused controls entirely, bind it with Some(\"!AtelierControl\")."
check warn 'KeyBinding::new\("(space|enter)",[^)]*None\)' \
    "a global single-key shortcut. It already yields to focused Atelier controls; scope it with Some(\"!AtelierControl\") if it must also stay out of your own focusable views."

# Theme.
check error 'has_global::<atelier::Theme>' \
    "atelier::init may be called again (it binds keys only once), and Theme::install switches themes; no need to check which one to call."
check warn 'fn [a-z_]*(group|cluster|category)_colou?r\b' \
    "use theme.categorical(index), which gives any number of colours that read on every theme and stay clear of the agent's hue."
check warn 'Theme::new\(Direction::(Studio|Glass|Mono), Appearance::(Light|Dark)\)' \
    "if this is a test looping over every theme, use Theme::all()."
check warn '\bselection\b.*gpui_component|gpui_component.*\bselection\b' \
    "selected text uses theme.colors.selection."
check warn 'colors\.highlight *= *' \
    "Button no longer reads colors.highlight to decide on a bezel; theme.shape.bezel_controls does, and only Studio sets it. Drop the override, or set bezel_controls instead."

# Plots and media.
check error 'scatter_batches|fn [a-z_]*project[a-z_]*\(.*azimuth' \
    "Plot3d draws every scatter in one depth order and answers Plot3d::pick / on_pick / on_orbit / project with its own projection."
check warn 'struct [A-Za-z]*Camera\b' \
    "if this re-implements Plot3d's projection, use Plot3d::project and Plot3d::pick so picking can never drift from what is drawn."

# Literals in interface code.
check warn '\b(rgb|rgba|hsla)\(0x[0-9a-fA-F]+|\bhsla\([0-9.]+, *[0-9.]+' \
    "a colour literal: read cx.theme().colors (document data such as a track's own colour is the one exception)."

echo "check-adoption: $errors error(s), $warnings warning(s)"
if [ "$errors" -gt 0 ] || { [ "$strict" -eq 1 ] && [ "$warnings" -gt 0 ]; }; then
    exit 1
fi
