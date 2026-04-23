use crossterm::{cursor, event, execute, queue, terminal};
use ratatui::layout::Rect;
use ratatui::{Terminal, TerminalOptions, Viewport};
use std::io::{self, Stdout, Write};
use std::time::Duration;

type Term = Terminal<ratatui::backend::CrosstermBackend<Stdout>>;

/// Create a terminal with a fixed viewport — no DSR cursor query.
fn make_fixed_terminal(top_y: u16, width: u16, height: u16) -> io::Result<Term> {
    Terminal::with_options(
        ratatui::backend::CrosstermBackend::new(io::stdout()),
        TerminalOptions {
            viewport: Viewport::Fixed(Rect::new(0, top_y, width, height)),
        },
    )
}

/// Dynamic viewport manager.
///
/// Uses `Viewport::Inline` exactly once (in `new()`) so ratatui handles
/// scrolling to make room.  Saves the resulting position and uses
/// `Viewport::Fixed` for all subsequent resizes — no more DSR queries.
pub struct DynViewport {
    terminal: Term,
    current_vh: u16,
    top_y: u16,
    width: u16,
}

impl DynViewport {
    pub fn new(height: u16) -> io::Result<Self> {
        terminal::enable_raw_mode()?;

        // Drain stale input so the initial DSR query isn't corrupted.
        while event::poll(Duration::from_millis(0))? {
            let _ = event::read()?;
        }

        // Use Viewport::Inline for the first creation — ratatui queries the
        // cursor position (one DSR) and scrolls the terminal to make room.
        let mut terminal = Terminal::with_options(
            ratatui::backend::CrosstermBackend::new(io::stdout()),
            TerminalOptions {
                viewport: Viewport::Inline(height),
            },
        )?;

        // Save the position ratatui chose so we can use Fixed from now on.
        let area = terminal.get_frame().area();
        let top_y = area.y;
        let width = area.width;

        Ok(Self {
            terminal,
            current_vh: height,
            top_y,
            width,
        })
    }

    /// Create a viewport at the current cursor position without using
    /// `Viewport::Inline` (avoids the DSR query that can flash escape chars).
    pub fn at_cursor(height: u16) -> io::Result<Self> {
        terminal::enable_raw_mode()?;

        // Drain stale input.
        while event::poll(Duration::from_millis(0))? {
            let _ = event::read()?;
        }

        let (width, term_h) = terminal::size()?;
        let (_, cursor_y) = cursor::position()?;

        // Scroll the terminal if we're near the bottom.
        let top_y = if cursor_y + height >= term_h {
            let scroll = cursor_y + height - term_h + 1;
            let mut out = io::stdout();
            execute!(out, terminal::ScrollUp(scroll))?;
            cursor_y.saturating_sub(scroll)
        } else {
            cursor_y
        };

        let terminal = make_fixed_terminal(top_y, width, height)?;

        Ok(Self {
            terminal,
            current_vh: height,
            top_y,
            width,
        })
    }

    /// Draw a frame.  Resizes the viewport first if needed.
    pub fn draw<F>(&mut self, needed: u16, draw_fn: F) -> io::Result<()>
    where
        F: FnOnce(&mut ratatui::Frame),
    {
        if needed != self.current_vh {
            self.replace_viewport(needed)?;
        }
        self.terminal.draw(draw_fn)?;
        Ok(())
    }

    /// Temporarily leave the viewport for external I/O (sudo, user input).
    /// If `at_cursor` is true the cursor stays where ratatui left it;
    /// otherwise it moves below the viewport.
    pub fn suspend(&mut self, at_cursor: bool) -> io::Result<()> {
        if !at_cursor {
            let mut out = io::stdout();
            execute!(out, cursor::MoveTo(0, self.top_y + self.current_vh))?;
        }
        let mut out = io::stdout();
        execute!(out, cursor::Show)?;
        terminal::disable_raw_mode()?;
        Ok(())
    }

    /// Clear the viewport area, position cursor after the content, and
    /// restore the terminal to normal mode.
    pub fn cleanup(&mut self) -> io::Result<()> {
        let mut out = io::stdout();
        // Best-effort clear — don't let escape sequence failures prevent
        // disable_raw_mode from running.
        for row in 0..self.current_vh {
            let _ = queue!(
                out,
                cursor::MoveTo(0, self.top_y + row),
                terminal::Clear(terminal::ClearType::CurrentLine),
            );
        }
        let _ = execute!(out, cursor::MoveTo(0, self.top_y), cursor::Show);
        terminal::disable_raw_mode()?;
        Ok(())
    }

    fn replace_viewport(&mut self, new_height: u16) -> io::Result<()> {
        // Clear old viewport lines (batched into a single flush)
        let mut out = io::stdout();
        queue!(out, cursor::MoveTo(0, self.top_y))?;
        for _ in 0..self.current_vh {
            queue!(
                out,
                terminal::Clear(terminal::ClearType::CurrentLine),
                cursor::MoveDown(1),
            )?;
        }
        queue!(out, cursor::MoveTo(0, self.top_y))?;
        out.flush()?;

        // Refresh width in case terminal was resized
        let (width, _) = terminal::size()?;
        self.width = width;

        // Fixed at the same top_y — no DSR query
        self.terminal = make_fixed_terminal(self.top_y, width, new_height)?;
        self.current_vh = new_height;
        Ok(())
    }
}
