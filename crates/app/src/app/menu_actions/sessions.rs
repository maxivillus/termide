//! Sessions menu actions — session switching, directory switcher.

use anyhow::Result;
use std::path::PathBuf;

use super::super::drives::mounted_drives;
use super::super::App;
use crate::state::{ActiveModal, PendingAction};
use crate::PanelExt;
use termide_app_core::Panel;
use termide_i18n as i18n;
use termide_ui_render::{
    SESSIONS_SUBMENU_CHANGE_ROOT, SESSIONS_SUBMENU_NEW, SESSIONS_SUBMENU_SWITCH,
};

/// Which panel the directory switcher acts on.
///
/// `Alt+F1` / `Alt+F2` pick a column by its place on screen, the way Far's
/// drive menu does; `Ctrl+\` keeps addressing the focused panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum DirectoryTarget {
    /// The focused panel — the historical `Ctrl+\` behaviour.
    Focused,
    /// The panel in the leftmost column that still holds one.
    LeftmostColumn,
    /// The panel in the rightmost column that still holds one.
    RightmostColumn,
}

impl App {
    /// Open sessions modal to switch between projects
    pub(in crate::app) fn handle_open_sessions_modal(&mut self) -> Result<()> {
        use termide_modal::{SessionItem, SessionsModal};
        use termide_session::{format_relative_time, list_all_sessions};

        let t = i18n::t();

        // Get all sessions
        let sessions = list_all_sessions().unwrap_or_default();

        // Get current project path
        let current_project = std::env::current_dir().unwrap_or_default();

        // Convert to SessionItems
        let items: Vec<SessionItem> = sessions
            .into_iter()
            .map(|info| {
                let is_current = info.project_path == current_project;
                let display_path =
                    termide_core::util::shorten_home_path(&info.project_path.display().to_string());
                let relative_time = format_relative_time(info.modified);

                SessionItem {
                    project_path: info.project_path,
                    display_path,
                    relative_time,
                    is_current,
                }
            })
            .collect();

        // Only show modal if there are other sessions
        if items.iter().any(|item| !item.is_current) {
            // Find index of current session to position cursor there
            let current_idx = items.iter().position(|item| item.is_current).unwrap_or(0);
            let modal = SessionsModal::new(t.sessions_title(), items).with_cursor(current_idx);
            self.state.set_pending_action(
                PendingAction::SwitchSession,
                ActiveModal::Sessions(Box::new(modal)),
            );
        }

        Ok(())
    }

    /// Open directory switcher modal for the focused panel.
    pub(in crate::app) fn handle_open_directory_switcher(&mut self) -> Result<()> {
        self.open_directory_switcher(DirectoryTarget::Focused)
    }

    /// Column index the target addresses, if the layout still has that column.
    ///
    /// Columns are resolved before any panel is touched so a layout with no
    /// panel never opens a modal that has nowhere to navigate.
    fn directory_switcher_group(&self, target: DirectoryTarget) -> Option<usize> {
        match target {
            DirectoryTarget::Focused => self
                .layout_manager
                .panel_groups
                .get(self.layout_manager.focus)
                .map(|_| self.layout_manager.focus),
            DirectoryTarget::LeftmostColumn => self.layout_manager.first_populated_group_index(),
            DirectoryTarget::RightmostColumn => self.layout_manager.last_populated_group_index(),
        }
    }

    /// Open the directory switcher for a panel chosen by column.
    ///
    /// The target panel decides what happens: a file manager navigates, a
    /// terminal receives `cd`. Focus is never moved — `Alt+F1` on a two-column
    /// layout changes the left panel and leaves the caret where it was, as in
    /// Far.
    pub(in crate::app) fn open_directory_switcher(
        &mut self,
        target: DirectoryTarget,
    ) -> Result<()> {
        use termide_modal::{DirectoryItem, DirectorySwitcherModal};

        let t = i18n::t();

        let Some(group_index) = self.directory_switcher_group(target) else {
            self.state
                .set_info(t.directory_switcher_unsupported().to_string());
            return Ok(());
        };

        // Check if the target panel supports directory switching (Terminal or FileManager)
        let panel_supported = self
            .layout_manager
            .get_group_mut(group_index)
            .and_then(|group| group.expanded_panel_mut())
            .map(|p| p.as_terminal_mut().is_some() || p.as_file_manager_mut().is_some())
            .unwrap_or(false);

        if !panel_supported {
            self.state
                .set_info(t.directory_switcher_unsupported().to_string());
            return Ok(());
        }

        // For terminal panels, check if there's a running process (cd won't work)
        let has_running_process = self
            .layout_manager
            .get_group_mut(group_index)
            .and_then(|group| group.expanded_panel_mut())
            .and_then(|p| p.as_terminal_mut())
            .map(|t| t.has_running_processes())
            .unwrap_or(false);

        if has_running_process {
            self.state
                .set_info(t.directory_switcher_process_running().to_string());
            return Ok(());
        }

        // Get current panel's working directory
        let current_dir = self
            .layout_manager
            .get_group_mut(group_index)
            .and_then(|group| group.expanded_panel_mut())
            .and_then(|p| p.get_working_directory());

        // Get all unique paths from all panels
        let panel_paths = self.collect_panel_paths();

        // Get bookmarked directories
        let bookmark_dirs = self.state.bookmarks.directories();

        // Build combined items list
        let mut items: Vec<DirectoryItem> = Vec::new();
        let mut seen_paths = std::collections::HashSet::new();

        // Mounted drives lead the list, mirroring Far's drive menu. Listing them
        // first is also what keeps the drive block contiguous, since only the
        // directories below are sorted.
        for drive in mounted_drives() {
            let is_current = current_dir.as_ref() == Some(&drive.path);
            let display = termide_core::util::shorten_home_path(&drive.path.display().to_string());
            seen_paths.insert(drive.path.clone());
            items.push(DirectoryItem {
                path: drive.path,
                display,
                is_current,
                is_bookmark: false,
                is_drive: true,
                label: drive.label,
            });
        }
        let drive_count = items.len();

        // Add panel paths after the drives
        for path in panel_paths {
            let is_current = current_dir.as_ref() == Some(&path);
            // A panel sitting on a drive is the drive entry marked current, not
            // a second row for the same directory.
            if let Some(existing) = items.iter_mut().find(|item| item.path == path) {
                existing.is_current |= is_current;
                continue;
            }
            let display = termide_core::util::shorten_home_path(&path.display().to_string());
            seen_paths.insert(path.clone());
            items.push(DirectoryItem {
                path,
                display,
                is_current,
                is_bookmark: false,
                is_drive: false,
                label: None,
            });
        }

        // Add bookmarked directories (if not already in list)
        for bookmark in bookmark_dirs {
            let path = PathBuf::from(&bookmark.path);
            if !seen_paths.contains(&path) {
                // Show path instead of display name for consistency
                let display = termide_core::util::shorten_home_path(&bookmark.path);
                let is_current = current_dir.as_ref() == Some(&path);
                items.push(DirectoryItem {
                    path,
                    display,
                    is_current,
                    is_bookmark: true,
                    is_drive: false,
                    label: None,
                });
            }
        }

        // Sort the directories alphabetically, leaving the drive block alone
        items[drive_count..].sort_by(|a, b| a.display.cmp(&b.display));

        // If no paths available, show info message
        if items.is_empty() {
            self.state
                .set_info(t.directory_switcher_no_paths().to_string());
            return Ok(());
        }

        // Find index of current directory to position cursor there
        let current_idx = items.iter().position(|item| item.is_current).unwrap_or(0);
        let modal = DirectorySwitcherModal::new(t.directory_switcher_title(), items)
            .with_cursor(current_idx);
        self.state.set_pending_action(
            PendingAction::SwitchDirectory { group_index },
            ActiveModal::DirectorySwitcher(Box::new(modal)),
        );

        Ok(())
    }

    // =========================================================================
    // Sessions submenu handling
    // =========================================================================

    /// Handle keyboard event in Sessions submenu
    pub(in crate::app) fn handle_sessions_submenu_key(
        &mut self,
        key: crossterm::event::KeyEvent,
    ) -> Result<()> {
        use super::navigate_submenu;
        use super::SubmenuNavAction;
        use termide_ui_render::SESSIONS_SUBMENU_ITEM_COUNT;

        match navigate_submenu(
            &key,
            &mut self.state.ui.sessions_submenu,
            SESSIONS_SUBMENU_ITEM_COUNT,
            &[],
        ) {
            SubmenuNavAction::Close => self.state.close_menu(),
            SubmenuNavAction::Execute => self.execute_sessions_submenu_action()?,
            SubmenuNavAction::Right => self.switch_to_next_menu()?,
            SubmenuNavAction::Left => self.switch_to_prev_menu()?,
            SubmenuNavAction::Rename
            | SubmenuNavAction::Edit
            | SubmenuNavAction::Delete
            | SubmenuNavAction::None => {}
        }
        Ok(())
    }

    /// Execute action for selected Sessions submenu item
    pub(in crate::app) fn execute_sessions_submenu_action(&mut self) -> Result<()> {
        match self.state.ui.sessions_submenu.selected {
            SESSIONS_SUBMENU_NEW => {
                self.state.close_menu();
                self.handle_new_session()?;
            }
            SESSIONS_SUBMENU_SWITCH => {
                self.state.close_menu();
                self.handle_open_sessions_modal()?;
            }
            SESSIONS_SUBMENU_CHANGE_ROOT => {
                self.state.close_menu();
                self.handle_change_root_path()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Open directory picker for creating new session
    pub(in crate::app) fn handle_new_session(&mut self) -> Result<()> {
        use termide_modal::DirectoryPickerModal;

        let t = i18n::t();
        // Get current project root as starting directory
        let initial_dir = self.project_root.clone();

        let modal = DirectoryPickerModal::new(
            initial_dir,
            t.sessions_new().to_string(),
            t.directory_picker_create().to_string(),
        );
        self.state.set_pending_action(
            PendingAction::NewSession,
            ActiveModal::DirectoryPicker(Box::new(modal)),
        );

        Ok(())
    }

    /// Open directory picker for changing root path of current session
    pub(in crate::app) fn handle_change_root_path(&mut self) -> Result<()> {
        use termide_modal::DirectoryPickerModal;

        let t = i18n::t();
        // Get current project root as starting directory
        let initial_dir = self.project_root.clone();

        let modal = DirectoryPickerModal::new(
            initial_dir,
            t.sessions_change_root().to_string(),
            t.directory_picker_move().to_string(),
        );
        self.state.set_pending_action(
            PendingAction::ChangeRootPath,
            ActiveModal::DirectoryPicker(Box::new(modal)),
        );

        Ok(())
    }
}
