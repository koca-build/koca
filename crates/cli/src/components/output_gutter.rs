use iocraft::prelude::*;

use super::Spinner;

/// Display columns taken by the `  │ ` gutter prefix.
pub const GUTTER_WIDTH: u16 = 4;

#[derive(Default, Props)]
pub struct OutputGutterProps {
    /// Bold header shown beside the spinner.
    pub header: String,
    /// The lines to render, replaced wholesale by the caller each update.
    pub lines: Vec<String>,
    /// Render only the last N lines; `None` renders all of them.
    pub max_lines: Option<usize>,
}

/// A spinner header above a block of output lines, each behind a dimmed `│`
/// gutter and truncated to the terminal width.
#[component]
pub fn OutputGutter(mut hooks: Hooks, props: &OutputGutterProps) -> impl Into<AnyElement<'static>> {
    let (width, _) = hooks.use_terminal_size();
    let avail = width.saturating_sub(GUTTER_WIDTH) as usize;
    let start = match props.max_lines {
        Some(n) => props.lines.len().saturating_sub(n),
        None => 0,
    };
    let visible: Vec<String> = props.lines[start..]
        .iter()
        .map(|line| truncate(line, avail))
        .collect();

    element! {
        View(flex_direction: FlexDirection::Column) {
            View {
                Spinner
                Text(content: format!(" {}", props.header), weight: Weight::Bold)
            }
            #(visible.into_iter().map(|line| element! {
                View {
                    Text(color: Color::DarkGrey, content: "  │ ")
                    Text(content: line)
                }
            }))
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    match s.char_indices().nth(max) {
        Some((idx, _)) => s[..idx].to_string(),
        None => s.to_string(),
    }
}
