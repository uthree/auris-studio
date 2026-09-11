//! Content-sized Markdown blocks for a transcript, with vertical list continuations.
//!
//! TextView 0.5 puts successive paragraphs of a list item in the same horizontal
//! flex row and drops non-paragraph children. Own the block structure here while
//! retaining its selectable rich text, links, tables and syntax highlighting.

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
        Node::Definition(_) => Vec::new(),
        _ => vec![Block::Text(
            format!("{}{definitions}", fragment(node, source)).into(),
        )],
    }
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
    let mut column = div()
        .flex()
        .flex_col()
        .w_full()
        .min_w_0()
        .h_auto()
        .flex_shrink_0()
        .gap_2();
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
        column = column.child(div().w_full().min_w_0().flex_shrink_0().child(child));
    }
    column.into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(code.as_ref(), "```rust\nlet x = 1;\n```");
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
        assert_eq!(code.as_ref(), "```rust\nfn main() {\n    run();\n}\n```");
    }
}
