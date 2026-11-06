//! Content-sized Markdown blocks for a transcript, with vertical list continuations.
//!
//! TextView 0.5 puts successive paragraphs of a list item in the same horizontal
//! flex row, can collapse its clipped text body, and drops non-paragraph children.
//! Own the block structure here while
//! retaining its selectable rich text, links and tables.

use std::sync::Arc;

use gpui::{AnyElement, App, SharedString, Window, div, prelude::*, rems};
use gpui_component::text::{TextView, TextViewStyle};
use markdown::mdast::Node;

use crate::theme::Theme;

#[derive(Debug)]
enum Block {
    Text(SharedString),
    List(Vec<(String, Vec<Block>)>),
    Quote(Vec<Block>),
}

struct Parsed {
    source: SharedString,
    blocks: Arc<Vec<Block>>,
}

/// Renders a message with natural block heights inside the panel's single scroll area.
pub(super) fn render(
    id: SharedString,
    source: SharedString,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let state = window.use_keyed_state(id.clone(), cx, |_, _| Parsed {
        source: source.clone(),
        blocks: Arc::new(parse(&source)),
    });
    let blocks = state.update(cx, |state, _| {
        if state.source != source {
            state.blocks = Arc::new(parse(&source));
            state.source = source;
        }
        state.blocks.clone()
    });
    render_blocks(&blocks, &id, theme, window, cx)
}

fn parse(source: &str) -> Vec<Block> {
    let Ok(root) = markdown::to_mdast(source, &markdown::ParseOptions::gfm()) else {
        return vec![Block::Text(source.to_string().into())];
    };
    let mut definitions = String::new();
    collect_definitions(&root, source, &mut definitions);
    blocks(&root, source, &definitions)
}

fn collect_definitions(node: &Node, source: &str, result: &mut String) {
    if matches!(node, Node::Definition(_)) {
        result.push_str("\n\n");
        result.push_str(&fragment(node, source));
    }
    if let Some(children) = node.children() {
        for child in children {
            collect_definitions(child, source, result);
        }
    }
}

fn blocks(node: &Node, source: &str, definitions: &str) -> Vec<Block> {
    match node {
        Node::Root(_) | Node::ListItem(_) => node
            .children()
            .into_iter()
            .flatten()
            .flat_map(|child| blocks(child, source, definitions))
            .collect(),
        Node::List(list) => vec![Block::List(
            list.children
                .iter()
                .enumerate()
                .map(|(index, item)| {
                    let marker = match item {
                        Node::ListItem(item) if item.checked == Some(true) => "☑".to_string(),
                        Node::ListItem(item) if item.checked == Some(false) => "☐".to_string(),
                        _ if list.ordered => {
                            format!("{}.", u64::from(list.start.unwrap_or(1)) + index as u64)
                        }
                        _ => "•".to_string(),
                    };
                    (marker, blocks(item, source, definitions))
                })
                .collect(),
        )],
        Node::Blockquote(quote) => vec![Block::Quote(
            quote
                .children
                .iter()
                .flat_map(|child| blocks(child, source, definitions))
                .collect(),
        )],
        Node::Code(code) => vec![Block::Text(plain_code_block(&code.value).into())],
        Node::Definition(_) => Vec::new(),
        _ => vec![Block::Text(
            format!("{}{definitions}", fragment(node, source)).into(),
        )],
    }
}

// A language-free fence keeps TextView from constructing a syntax highlighter.
// Use a longer fence than any run in the body so literal Markdown stays intact.
fn plain_code_block(code: &str) -> String {
    let longest = code.split(|ch| ch != '`').map(str::len).max().unwrap_or(0);
    let fence = "`".repeat((longest + 1).max(3));
    format!("{fence}\n{code}\n{fence}")
}

/// Positions exclude the first line's container prefix; remove that prefix from
/// continuation lines too, preserving code indentation beyond the container.
fn fragment(node: &Node, source: &str) -> String {
    let Some(position) = node.position() else {
        return String::new();
    };
    let raw = &source[position.start.offset..position.end.offset];
    let indent = position.start.column.saturating_sub(1);
    raw.lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                return line;
            }
            let prefix = line
                .bytes()
                .take(indent)
                .take_while(|byte| matches!(byte, b' ' | b'\t' | b'>'))
                .count();
            &line[prefix..]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn render_blocks(
    blocks: &[Block],
    id: &str,
    theme: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    // A block container, not a flex column. As a flex column its height came from a measurement
    // its parent took before the width was known, when text wraps at every break opportunity:
    // Japanese breaks between any two characters, so a three-line reply measured one character
    // per line and kept that height, thousands of pixels, at its real width. Everything after it
    // in the transcript ended past the scroll range. Block layout sums the blocks as drawn.
    let root_id = format!("{id}-root");
    let mut column = div()
        .debug_selector(move || root_id.clone())
        .w_full()
        .min_w_0()
        .h_auto()
        .flex_shrink_0();
    for (index, block) in blocks.iter().enumerate() {
        let child_id = format!("{id}-{index}");
        let child = match block {
            Block::Text(text) => {
                TextView::markdown(SharedString::from(child_id), text.clone(), window, cx)
                    .style(
                        TextViewStyle {
                            heading_base_font_size: window.rem_size(),
                            ..Default::default()
                        }
                        .heading_font_size(|level, base| {
                            base * match level {
                                1 => 1.25,
                                2 => 1.125,
                                _ => 1.0,
                            }
                        }),
                    )
                    .w_full()
                    .min_w_0()
                    .h_auto()
                    .selectable(true)
                    .into_any_element()
            }
            Block::Quote(children) => div()
                .w_full()
                .min_w_0()
                .border_l_2()
                .border_color(theme.border)
                .pl_2()
                .text_color(theme.text_muted)
                .child(render_blocks(children, &child_id, theme, window, cx))
                .into_any_element(),
            Block::List(items) => {
                let mut list = div().flex().flex_col().w_full().min_w_0().gap_1();
                for (index, (marker, children)) in items.iter().enumerate() {
                    list = list.child(
                        div()
                            .flex()
                            .items_start()
                            .w_full()
                            .min_w_0()
                            .flex_shrink_0()
                            .gap_1()
                            .child(
                                div()
                                    .min_w(rems(1.5))
                                    .flex_shrink_0()
                                    .text_right()
                                    .child(marker.clone()),
                            )
                            .child(div().flex_1().min_w_0().child(render_blocks(
                                children,
                                &format!("{child_id}-{index}"),
                                theme,
                                window,
                                cx,
                            ))),
                    );
                }
                list.into_any_element()
            }
        };
        let debug_id = format!("{id}-{index}-block");
        column = column.child(
            div()
                .debug_selector(move || debug_id.clone())
                .when(index > 0, |block| block.mt_2())
                .w_full()
                .min_w_0()
                .flex_shrink_0()
                .child(child),
        );
    }
    column.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_japanese_list_keeps_rich_text_bodies() {
        let parsed = parse("- **テンポ:** 150 BPM\n- **拍子:** `4/4`");
        let Block::List(items) = &parsed[0] else {
            panic!("expected list")
        };
        let bodies: Vec<_> = items
            .iter()
            .map(|(_, children)| match &children[..] {
                [Block::Text(text)] => text.as_ref(),
                _ => panic!("expected one paragraph per item"),
            })
            .collect();
        assert_eq!(bodies, ["**テンポ:** 150 BPM", "**拍子:** `4/4`"]);
    }

    #[test]
    fn list_items_keep_paragraphs_code_and_nested_lists() {
        let parsed =
            parse("3. first\n\n   next\n\n   ```rust\n   let x = 1;\n   ```\n\n   - nested");
        let Block::List(items) = &parsed[0] else {
            panic!("expected list")
        };
        assert_eq!(items[0].0, "3.");
        assert_eq!(items[0].1.len(), 4);
        let Block::Text(code) = &items[0].1[2] else {
            panic!("expected code")
        };
        assert_eq!(code.as_ref(), "```\nlet x = 1;\n```");
        assert!(matches!(items[0].1[3], Block::List(_)));
    }

    #[test]
    fn reference_definitions_follow_split_blocks() {
        let parsed = parse("- [manual][docs]\n\n[docs]: https://example.com");
        let Block::List(items) = &parsed[0] else {
            panic!("expected list")
        };
        let Block::Text(text) = &items[0].1[0] else {
            panic!("expected text")
        };
        assert_eq!(
            text.as_ref(),
            "[manual][docs]\n\n[docs]: https://example.com"
        );
    }

    #[test]
    fn quoted_japanese_and_code_indentation_survive_block_splitting() {
        let parsed = parse(
            "> 日本語の説明\n> 続きです。\n>\n> - 項目\n>\n>   ```rust\n>   fn main() {\n>       run();\n>   }\n>   ```",
        );
        let Block::Quote(children) = &parsed[0] else {
            panic!("expected quote")
        };
        let Block::Text(text) = &children[0] else {
            panic!("expected paragraph")
        };
        assert_eq!(text.as_ref(), "日本語の説明\n続きです。");
        let Block::List(items) = &children[1] else {
            panic!("expected list")
        };
        let Block::Text(code) = &items[0].1[1] else {
            panic!("expected code")
        };
        assert_eq!(code.as_ref(), "```\nfn main() {\n    run();\n}\n```");
    }

    #[test]
    fn code_languages_and_metadata_do_not_reach_the_highlighter() {
        for language in ["rust", "json", "yaml", "unknown title=score"] {
            let source = format!("```{language}\n  音符: C4\n\n    D4\n```");
            assert_plain_code(&source, "  音符: C4\n\n    D4");
        }
    }

    #[test]
    fn literal_fences_and_indented_code_keep_their_contents() {
        assert_plain_code(
            "~~~~~~markdown\n```rust\nC4\n```\n~~~~\n~~~~~~",
            "```rust\nC4\n```\n~~~~",
        );
        assert_plain_code("    C4\n        D4", "C4\n    D4");
    }

    fn assert_plain_code(source: &str, expected: &str) {
        let parsed = parse(source);
        let Block::Text(text) = &parsed[0] else {
            panic!("expected code block")
        };
        let root = markdown::to_mdast(text, &markdown::ParseOptions::gfm()).unwrap();
        let [Node::Code(code)] = root.children().unwrap().as_slice() else {
            panic!("expected one code block")
        };
        assert_eq!(code.value, expected);
        assert_eq!(code.lang, None);
        assert_eq!(code.meta, None);
    }
}
