#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# Derived from uthree/design-language, revision 331aa75561264716729b587d1f6c90a3d0f286b8.
# Source: https://github.com/uthree/design-language/blob/331aa75561264716729b587d1f6c90a3d0f286b8/skills/atelier-gpui/tools/check-adoption.sh
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
        if (match($0, /(Button|ListItem|Switch|Checkbox|MenuItem|SegmentedControl|TextField|Slider)::[a-z_]+\(/)) {
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
