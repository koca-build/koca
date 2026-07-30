use iocraft::prelude::*;

#[derive(Default, Props)]
pub struct ProgressBarProps {
    /// Completion in `0.0..=1.0`.
    pub fraction: f64,
    /// Total cell width.
    pub width: u16,
}

/// Green `█` for the filled portion, dim `░` for the remainder.
#[component]
pub fn ProgressBar(props: &ProgressBarProps) -> impl Into<AnyElement<'static>> {
    let width = props.width.max(1) as usize;
    let filled = (((width as f64) * props.fraction.clamp(0.0, 1.0)).round() as usize).min(width);
    element! {
        MixedText(contents: vec![
            MixedTextContent::new("█".repeat(filled)).color(Color::Green),
            MixedTextContent::new("░".repeat(width - filled)).color(Color::DarkGrey),
        ])
    }
}
