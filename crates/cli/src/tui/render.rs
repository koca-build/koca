use koca::dep::DepConstraint;
use koca_proto::{ActionKind, PlannedAction};
use ratatui::{
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

pub const SPINNERS: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
pub const BUILD_GUTTER_MAX: usize = 5;

/// All state needed to render a frame.
pub struct RenderState<'a> {
    pub phase: super::Phase,
    pub info: &'a [Line<'static>],
    pub dl_state: &'a super::DownloadState,
    pub install_summary: Option<&'a super::InstallSummary>,
    pub build_state: &'a super::BuildState,
    pub pkg_state: &'a super::BuildState,
    pub build_summary: Option<&'a str>,
    pub pkg_summary: Option<&'a str>,
    pub tick: usize,
}

pub fn format_bytes(b: u64) -> String {
    if b >= 1_000_000 {
        format!("{:.1} MB", b as f64 / 1_000_000.0)
    } else if b >= 1_000 {
        format!("{:.0} KB", b as f64 / 1_000.0)
    } else {
        format!("{} B", b)
    }
}

/// Build the styled info lines for the confirm screen from real data.
pub fn confirm_info_lines(
    actions: &[PlannedAction],
    depends: &[DepConstraint],
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    if !actions.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}", actions.len()),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Build Dependencies (makedepends):"),
        ]));

        for action in actions {
            let (icon, color) = match action.action {
                ActionKind::Install => ("+", Color::Green),
                ActionKind::Upgrade => ("^", Color::Yellow),
                ActionKind::Downgrade => ("v", Color::Yellow),
                ActionKind::Reinstall => ("=", Color::Cyan),
                ActionKind::Remove => ("-", Color::Red),
            };

            let mut spans = vec![
                Span::styled(format!("  {} ", icon), Style::default().fg(color)),
                Span::raw(action.name.clone()),
            ];

            if let Some(old_ver) = &action.old_version {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    old_ver.clone(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::CROSSED_OUT),
                ));
            }

            spans.push(Span::raw(" "));
            spans.push(Span::styled(
                action.version.clone(),
                Style::default().fg(Color::DarkGray),
            ));

            lines.push(Line::from(spans));
        }
    }

    if !depends.is_empty() {
        lines.push(Line::from(vec![
            Span::styled(
                format!("{}", depends.len()),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" Dependencies (depends):"),
        ]));

        for dep in depends {
            let spans = vec![
                Span::styled("  = ", Style::default().fg(Color::DarkGray)),
                Span::raw(dep.to_string()),
            ];

            lines.push(Line::from(spans));
        }
    }

    lines
}

/// Render the full frame. Returns the number of content lines used.
pub fn render(frame: &mut Frame, state: &RenderState) -> u16 {
    let area = frame.area();
    let mut y = area.y;

    // Render confirm info at the top (if any)
    let info_height = state.info.len() as u16;
    if info_height > 0 {
        frame.render_widget(
            Paragraph::new(state.info.to_vec()),
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: info_height,
            },
        );
        y += info_height;
    }

    // Only add a gap line if there's content above (info lines).
    let gap = if y > area.y { 1 } else { 0 };

    match state.phase {
        super::Phase::Resolve => {
            let spinner = SPINNERS[state.tick % SPINNERS.len()];
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("{} ", spinner), Style::default().fg(Color::Blue)),
                    Span::styled(
                        "Resolving dependencies...",
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ])),
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
            );
        }
        super::Phase::Confirm => {
            y += gap;
            frame.render_widget(
                Paragraph::new(Line::from("Continue? [Y/n] ")),
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
            );
            frame.set_cursor_position(Position { x: 16, y });
        }
        super::Phase::Download => {
            y += gap;
            render_download(
                frame,
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 2,
                },
                state.dl_state,
            );
        }
        super::Phase::Install => {
            y += gap;
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::raw("Downloaded "),
                    Span::styled(
                        format_bytes(state.dl_state.total_bytes),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ])),
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
            );
            y += 1;

            let spinner = SPINNERS[state.tick % SPINNERS.len()];
            let install_text = if let Some(pkg) = &state.dl_state.current_install_pkg {
                format!(
                    "Installing ({}/{}) {}...",
                    state.dl_state.install_current, state.dl_state.install_total, pkg
                )
            } else {
                "Installing...".to_string()
            };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("{} ", spinner), Style::default().fg(Color::Blue)),
                    Span::raw(install_text),
                ])),
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
            );
        }
        super::Phase::Build => {
            y += gap;
            if let Some(summary) = state.install_summary {
                render_install_summary(frame, area.x, &mut y, area.width, summary);
            }
            let gh = 1 + BUILD_GUTTER_MAX as u16;
            render_build_gutter(
                frame,
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: gh,
                },
                "Building...",
                &state.build_state.lines,
                state.tick,
            );
        }
        super::Phase::Package => {
            y += gap;
            if let Some(summary) = state.install_summary {
                render_install_summary(frame, area.x, &mut y, area.width, summary);
            }
            if let Some(bs) = state.build_summary {
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::raw("Built "),
                        Span::styled(
                            bs.to_string(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                    ])),
                    Rect {
                        x: area.x,
                        y,
                        width: area.width,
                        height: 1,
                    },
                );
                y += 1;
            }
            let gh = 1 + BUILD_GUTTER_MAX as u16;
            render_build_gutter(
                frame,
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: gh,
                },
                "Packaging...",
                &state.pkg_state.lines,
                state.tick,
            );
        }
        super::Phase::Done => {
            y += gap;
            if let Some(summary) = state.install_summary {
                render_install_summary(frame, area.x, &mut y, area.width, summary);
            }
            let mut result_spans = Vec::new();
            if let Some(bs) = state.build_summary {
                result_spans.push(Span::raw("Built "));
                result_spans.push(Span::styled(
                    bs.to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
            }
            if let Some(ps) = state.pkg_summary {
                if !result_spans.is_empty() {
                    result_spans.push(Span::raw(", "));
                }
                result_spans.push(Span::raw("Packaged into "));
                result_spans.push(Span::styled(
                    ps.to_string(),
                    Style::default().add_modifier(Modifier::BOLD),
                ));
            }
            if !result_spans.is_empty() {
                frame.render_widget(
                    Paragraph::new(Line::from(result_spans)),
                    Rect {
                        x: area.x,
                        y,
                        width: area.width,
                        height: 1,
                    },
                );
                y += 1;
            }
        }
        super::Phase::Failed => {
            y += gap;
            if let Some(summary) = state.install_summary {
                render_install_summary(frame, area.x, &mut y, area.width, summary);
            }
            let phase_label =
                if !state.build_state.lines.is_empty() && state.pkg_state.lines.is_empty() {
                    "Building..."
                } else {
                    "Packaging..."
                };
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "error: ",
                        Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(phase_label, Style::default().add_modifier(Modifier::BOLD)),
                ])),
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: 1,
                },
            );
            y += 1;

            let err_source = if !state.pkg_state.lines.is_empty() {
                &state.pkg_state.lines
            } else {
                &state.build_state.lines
            };
            let err_lines: Vec<Line> = err_source
                .iter()
                .map(|text| {
                    let mut spans = vec![Span::styled("  │ ", Style::default().fg(Color::Red))];
                    spans.extend(styled_output_line(text, false).spans);
                    Line::from(spans)
                })
                .collect();
            let el = err_lines.len() as u16;
            frame.render_widget(
                Paragraph::new(err_lines),
                Rect {
                    x: area.x,
                    y,
                    width: area.width,
                    height: el,
                },
            );
        }
    }

    y - area.y
}

fn render_install_summary(
    frame: &mut Frame,
    x: u16,
    y: &mut u16,
    width: u16,
    summary: &super::InstallSummary,
) {
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("Downloaded "),
            Span::styled(
                format_bytes(summary.total_bytes),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(", Installed "),
            Span::styled(
                format!("{}", summary.installed_count),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw(" packages"),
        ])),
        Rect {
            x,
            y: *y,
            width,
            height: 1,
        },
    );
    *y += 1;
}

pub fn render_download(frame: &mut Frame, area: Rect, state: &super::DownloadState) {
    let chunks = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);

    let total_bytes = state.total_bytes;
    let done_bytes = state.done_bytes;
    let pct = if total_bytes > 0 {
        done_bytes as f64 / total_bytes as f64
    } else {
        0.0
    };
    let total_pkgs = state.total_packages;
    let width = total_pkgs.to_string().len();
    let prefix = format!(
        "Downloading {:>width$}/{} packages ",
        state.done_count, total_pkgs,
    );
    let suffix = format!(
        " {}% ({}/{})",
        (pct * 100.0) as u32,
        format_bytes(done_bytes),
        format_bytes(total_bytes)
    );
    let bar_width = 30usize;
    let filled = (bar_width as f64 * pct) as usize;

    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw(prefix),
            Span::styled("█".repeat(filled), Style::default().fg(Color::Green)),
            Span::styled(
                "░".repeat(bar_width - filled),
                Style::default().fg(Color::DarkGray),
            ),
            Span::raw(suffix),
        ])),
        chunks[0],
    );

    let spinner = SPINNERS[state.tick as usize % SPINNERS.len()];
    let status = if !state.active_names.is_empty() {
        Line::from(vec![
            Span::raw(format!("{} ", spinner)),
            Span::raw("active: "),
            Span::styled(
                state.active_names.join(", "),
                Style::default().fg(Color::DarkGray),
            ),
        ])
    } else {
        Line::from("all downloaded")
    };
    frame.render_widget(Paragraph::new(status), chunks[1]);
}

pub fn render_build_gutter(
    frame: &mut Frame,
    area: Rect,
    header: &str,
    lines: &[String],
    tick: usize,
) {
    let header_area = Rect { height: 1, ..area };
    let body_area = Rect {
        y: area.y + 1,
        height: area.height.saturating_sub(1),
        ..area
    };

    let spinner = SPINNERS[tick % SPINNERS.len()];
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(format!("{} ", spinner), Style::default().fg(Color::Blue)),
            Span::styled(header, Style::default().add_modifier(Modifier::BOLD)),
        ])),
        header_area,
    );

    let max_visible = body_area.height as usize;
    let visible = lines;
    let start = if visible.len() > max_visible {
        visible.len() - max_visible
    } else {
        0
    };
    let output: Vec<Line> = visible[start..]
        .iter()
        .map(|text| {
            let mut spans = vec![Span::styled("  │ ", Style::default().fg(Color::DarkGray))];
            spans.extend(styled_output_line(text, true).spans);
            Line::from(spans)
        })
        .collect();
    frame.render_widget(Paragraph::new(output), body_area);
}

/// Style a build/package output line. Detects keywords like "Compiling", "error", etc.
pub fn styled_output_line(text: &str, dimmed: bool) -> Line<'static> {
    let fg = if dimmed {
        Color::DarkGray
    } else {
        Color::Reset
    };

    // Try to detect known keywords at the start of the line (after optional whitespace)
    let trimmed = text.trim_start();

    let keywords_green = [
        "Compiling",
        "Checking",
        "Linking",
        "Finished",
        "Building",
        "Downloading",
        "Downloaded",
        "Packaging",
        "Resolving",
        "Updating",
        "Fresh",
        "Running",
    ];
    let keywords_red = ["error", "Error"];

    for kw in &keywords_red {
        if trimmed.starts_with(kw) {
            let idx = text.find(kw).unwrap();
            let prefix = &text[..idx];
            let rest = &text[idx + kw.len()..];
            return Line::from(vec![
                Span::styled(prefix.to_string(), Style::default().fg(fg)),
                Span::styled(
                    kw.to_string(),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    rest.to_string(),
                    Style::default().fg(if dimmed {
                        Color::DarkGray
                    } else {
                        Color::Reset
                    }),
                ),
            ]);
        }
    }

    for kw in &keywords_green {
        if trimmed.starts_with(kw) {
            let idx = text.find(kw).unwrap();
            let prefix = &text[..idx];
            // Pad to align like cargo output
            let pad = 12usize.saturating_sub(kw.len());
            let rest = &text[idx + kw.len()..];
            return Line::from(vec![
                Span::raw(" ".repeat(pad)),
                Span::styled(
                    format!("{}{}", prefix, kw),
                    Style::default()
                        .fg(if dimmed {
                            Color::DarkGray
                        } else {
                            Color::Green
                        })
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(rest.to_string(), Style::default().fg(fg)),
            ]);
        }
    }

    // Fallback: plain text
    Line::from(Span::styled(text.to_string(), Style::default().fg(fg)))
}

/// Calculate the viewport height needed for the current state.
pub fn calc_height(state: &RenderState) -> u16 {
    let base = state.info.len() as u16;
    let gap: u16 = if base > 0 { 1 } else { 0 };
    let summary: u16 = if state.install_summary.is_some() {
        1
    } else {
        0
    };
    match state.phase {
        super::Phase::Resolve => 1,
        super::Phase::Confirm => base + gap + 1,
        super::Phase::Download => base + gap + 2,
        super::Phase::Install => base + gap + 2,
        super::Phase::Build => base + gap + summary + 1 + BUILD_GUTTER_MAX as u16,
        super::Phase::Package => {
            let built = if state.build_summary.is_some() { 1 } else { 0 };
            base + gap + summary + built + 1 + BUILD_GUTTER_MAX as u16
        }
        super::Phase::Done => base + gap + summary + 1,
        super::Phase::Failed => {
            let err_lines = if !state.pkg_state.lines.is_empty() {
                state.pkg_state.lines.len()
            } else {
                state.build_state.lines.len()
            }
            .min(200) as u16;
            base + gap + summary + 1 + err_lines
        }
    }
}
