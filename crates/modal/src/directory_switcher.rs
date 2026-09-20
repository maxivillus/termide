//! Directory switcher modal dialog.
//!
//! Provides a modal for quickly switching the current panel's working directory
//! by selecting from a deduplicated list of paths from all open panels.

use anyhow::Result;
use crossterm::event::{KeyCode, MouseButton, MouseEvent, MouseEventKind};
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{List, ListItem, Widget},
};

use crate::base::render_modal_block;
use std::path::PathBuf;
use unicode_width::UnicodeWidthStr;

use termide_theme::Theme;

use crate::{calculate_modal_width, centered_rect_with_size, Modal, ModalResult, ModalWidthConfig};

/// Item representing a directory or a drive in the list
#[derive(Debug, Clone)]
pub struct DirectoryItem {
    /// Full path
    pub path: PathBuf,
    /// Display path (potentially shortened)
    pub display: String,
    /// Whether this is the current panel's directory
    pub is_current: bool,
    /// Whether this is a bookmarked directory
    pub is_bookmark: bool,
    /// Whether this entry is a mounted drive rather than a directory in use.
    /// Drives are listed first, so this drives styling only.
    pub is_drive: bool,
    /// Filesystem type or volume name, shown dimmed after the path. `None`
    /// for directories and for platforms whose drives carry no label.
    pub label: Option<String>,
}

/// Directory switcher modal window
#[derive(Debug)]
pub struct DirectorySwitcherModal {
    title: String,
    items: Vec<DirectoryItem>,
    cursor: usize,
    scroll_offset: usize,
    last_list_area: Option<Rect>,
}

/// Maximum number of items visible at once (single-line items)
const MAX_VISIBLE_ITEMS: usize = 10;

impl DirectoryItem {
    /// Text appended after the path. Drives carry their filesystem type so two
    /// similar mount points stay distinguishable; directories carry none.
    fn suffix(&self) -> Option<String> {
        self.label.as_deref().map(|label| format!("  [{label}]"))
    }
}

impl DirectorySwitcherModal {
    /// Create a new directory switcher modal
    pub fn new(title: impl Into<String>, items: Vec<DirectoryItem>) -> Self {
        Self {
            title: title.into(),
            items,
            cursor: 0,
            scroll_offset: 0,
            last_list_area: None,
        }
    }

    /// Set initial cursor position (for selecting current directory)
    pub fn with_cursor(mut self, index: usize) -> Self {
        self.cursor = index.min(self.items.len().saturating_sub(1));
        self.adjust_scroll();
        self
    }

    /// Calculate dynamic modal width
    fn calculate_modal_width(&self, screen_width: u16) -> u16 {
        let title_width = self.title.len() as u16 + 4;

        // Find max path width
        let max_path_width = self
            .items
            .iter()
            .map(|item| {
                item.display.len() as u16 // "▶ " + path + suffix + " (*)"
                    + item.suffix().map_or(0, |suffix| suffix.len() as u16)
                    + 6
            })
            .max()
            .unwrap_or(40);

        calculate_modal_width(
            [title_width, max_path_width].into_iter(),
            screen_width,
            ModalWidthConfig::wide(),
        )
    }

    /// Move cursor up
    fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
            self.adjust_scroll();
        }
    }

    /// Move cursor down
    fn cursor_down(&mut self) {
        if self.cursor < self.items.len().saturating_sub(1) {
            self.cursor += 1;
            self.adjust_scroll();
        }
    }

    /// Go to first item
    fn cursor_home(&mut self) {
        self.cursor = 0;
        self.adjust_scroll();
    }

    /// Go to last item
    fn cursor_end(&mut self) {
        self.cursor = self.items.len().saturating_sub(1);
        self.adjust_scroll();
    }

    /// Adjust scroll to keep cursor visible
    fn adjust_scroll(&mut self) {
        self.scroll_offset =
            termide_ui::ensure_offset_visible(self.scroll_offset, self.cursor, MAX_VISIBLE_ITEMS);
    }

    /// Get the selected directory
    fn get_selected(&self) -> Option<&DirectoryItem> {
        self.items.get(self.cursor)
    }
}

impl Modal for DirectorySwitcherModal {
    type Result = PathBuf;

    fn render(&mut self, area: Rect, buf: &mut Buffer, theme: &Theme) {
        let modal_width = self.calculate_modal_width(area.width);

        // Each item takes 1 line
        let visible_items = self.items.len().min(MAX_VISIBLE_ITEMS);
        let list_height = visible_items as u16;

        // Height: 1 (top border) + list_height + 1 (bottom border)
        let modal_height = 2 + list_height;

        let modal_area = centered_rect_with_size(modal_width, modal_height, area);

        let inner = render_modal_block(modal_area, buf, &self.title, theme);

        // Build list items (1 line per directory)
        let mut list_items: Vec<ListItem> = Vec::new();

        for (idx, item) in self
            .items
            .iter()
            .enumerate()
            .skip(self.scroll_offset)
            .take(MAX_VISIBLE_ITEMS)
        {
            let is_selected = idx == self.cursor;

            // Line: Path with selection indicator
            let prefix = if is_selected { "▶ " } else { "  " };

            let suffix = item.suffix().unwrap_or_default();

            let path_style = if is_selected {
                Style::default()
                    .fg(theme.fg)
                    .bg(theme.bg)
                    .add_modifier(Modifier::BOLD)
            } else if item.is_current {
                // Current directory highlighted with accent color
                Style::default().fg(theme.accented_fg)
            } else if item.is_drive {
                // Drives lead the list; accent marks them as roots rather than
                // places the session currently sits in.
                Style::default().fg(theme.accented_fg)
            } else {
                Style::default().fg(theme.fg)
            };

            // The suffix stays dimmed so it never competes with the path, and
            // keeps the row background when the row is the selected one.
            let suffix_style = if is_selected {
                Style::default()
                    .fg(theme.disabled)
                    .bg(theme.bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme.disabled)
            };

            // Pad line to full width (use unicode width for correct calculation)
            let line_width = prefix.width() + item.display.width() + suffix.width();
            let padding = " ".repeat((inner.width as usize).saturating_sub(line_width));

            let line = Line::from(vec![
                Span::styled(prefix, path_style),
                Span::styled(&item.display, path_style),
                Span::styled(suffix, suffix_style),
                Span::styled(padding, path_style),
            ]);

            list_items.push(ListItem::new(vec![line]));
        }

        let list = List::new(list_items).style(Style::default().bg(theme.bg));
        list.render(inner, buf);

        self.last_list_area = Some(inner);
    }

    fn handle_key(
        &mut self,
        chord: termide_core::KeyChord,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        let key = chord.raw;
        match key.code {
            KeyCode::Esc => Ok(Some(ModalResult::Cancelled)),
            KeyCode::Up | KeyCode::Char('k') => {
                self.cursor_up();
                Ok(None)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.cursor_down();
                Ok(None)
            }
            KeyCode::Home => {
                self.cursor_home();
                Ok(None)
            }
            KeyCode::End => {
                self.cursor_end();
                Ok(None)
            }
            KeyCode::Enter => {
                if let Some(item) = self.get_selected() {
                    if item.is_current {
                        // Current directory selected - just close modal
                        Ok(Some(ModalResult::Cancelled))
                    } else {
                        Ok(Some(ModalResult::Confirmed(item.path.clone())))
                    }
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn handle_mouse(
        &mut self,
        mouse: MouseEvent,
        _modal_area: Rect,
    ) -> Result<Option<ModalResult<Self::Result>>> {
        use crate::{check_mouse_click, MouseClickResult};

        match mouse.kind {
            MouseEventKind::ScrollUp => {
                for _ in 0..3 {
                    self.cursor_up();
                }
                return Ok(None);
            }
            MouseEventKind::ScrollDown => {
                for _ in 0..3 {
                    self.cursor_down();
                }
                return Ok(None);
            }
            _ => {}
        }

        // Only handle left button press
        if mouse.kind != MouseEventKind::Down(MouseButton::Left) {
            return Ok(None);
        }

        match check_mouse_click(
            mouse.column,
            mouse.row,
            None, // No modal area check for directory switcher modal
            self.last_list_area,
            self.scroll_offset,
        ) {
            MouseClickResult::OutsideModal | MouseClickResult::OutsideList => Ok(None),
            MouseClickResult::OnListItem(clicked_index) => {
                if clicked_index < self.items.len() {
                    let item = &self.items[clicked_index];
                    self.cursor = clicked_index;

                    if item.is_current {
                        // Current directory clicked - just close modal
                        return Ok(Some(ModalResult::Cancelled));
                    } else {
                        return Ok(Some(ModalResult::Confirmed(item.path.clone())));
                    }
                }
                Ok(None)
            }
        }
    }
}
