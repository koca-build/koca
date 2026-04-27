mod ui;

use koca::dep::DepConstraint;
use koca::backend::{Event, PlannedAction};
use std::io;

pub use ui::KocaCreateUi;

const SPINNERS: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const BUILD_GUTTER_MAX: usize = 5;

// ── Trait ──

pub trait CreateUi {
    fn start_resolve(&mut self) -> io::Result<()>;
    fn finish_resolve(&mut self) -> io::Result<()>;

    fn show_confirm(
        &mut self,
        actions: &[PlannedAction],
        depends: &[DepConstraint],
        noconfirm: bool,
    ) -> io::Result<bool>;

    fn on_event(&mut self, event: &Event) -> io::Result<()>;
    fn tick(&mut self) -> io::Result<()>;

    fn finish_install(&mut self, total_bytes: u64, installed_count: u32) -> io::Result<()>;

    fn start_build(&mut self) -> io::Result<()>;
    fn on_build_line(&mut self, line: &str) -> io::Result<()>;
    fn finish_build(&mut self, pkgname: &str, version: &str) -> io::Result<()>;

    fn start_package(&mut self) -> io::Result<()>;
    fn on_package_line(&mut self, line: &str) -> io::Result<()>;
    fn finish_package(&mut self, output_file: &str) -> io::Result<()>;

    fn show_failure(&mut self, phase_name: &str) -> io::Result<()>;

    fn suspend(&mut self) -> io::Result<()>;
    fn resume(&mut self) -> io::Result<()>;
    fn cleanup(&mut self);
}
