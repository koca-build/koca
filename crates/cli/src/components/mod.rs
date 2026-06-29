//! Reusable iocraft widgets, independent of any one workflow.

mod output_gutter;
mod progress_bar;
mod spinner;

pub use output_gutter::{OutputGutter, GUTTER_WIDTH};
pub use progress_bar::ProgressBar;
pub use spinner::Spinner;
