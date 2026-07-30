use std::time::Duration;

use iocraft::prelude::*;

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// A braille spinner that animates on its own.
#[component]
pub fn Spinner(mut hooks: Hooks) -> impl Into<AnyElement<'static>> {
    let mut frame = hooks.use_state(|| 0usize);
    hooks.use_future(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(80)).await;
            frame.set((frame.get() + 1) % FRAMES.len());
        }
    });
    element! {
        Text(color: Color::Blue, content: FRAMES[frame.get()])
    }
}
