//! The line cursor in no-step mode.
//!
//! The cursor is the add-comment row (`review_line_add_row`), the same row the mouse
//! sets by hovering, so `m` comments on it and the mouse and keyboard share one cursor.
//! Keyboard moves (`j`/`k` by line, `l`/`h` by hunk) set a target line; after the next
//! frame the view scrolls so the target sits in the middle row, or as close to it as
//! the start and end of the file allow. Scrolling keeps the cursor's screen row, so the
//! content moves under a cursor that stays in the middle.

use super::*;

/// The line a keyboard move asked the cursor to show, resolved after the next frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineCursorTarget {
    /// A display line, as indexed by `review_visible_lines_with_idx`.
    Line(usize),
    /// The active change, after a hunk jump or when a file opens.
    ActiveChange,
}

impl App {
    /// Screen rows that start a diff line in the last frame, top to bottom, with each
    /// row's display line. Wrapped continuation rows repeat their line's index.
    fn line_cursor_rows(&self) -> Vec<(u16, usize)> {
        let Some((_, y, _, _)) = self.diff_view_area else {
            return Vec::new();
        };
        let mut rows: Vec<(u16, usize)> = if self.view_mode == ViewMode::Split {
            self.review_split_line_rows
                .iter()
                .map(|(row, _, idx)| (*row, *idx))
                .collect()
        } else {
            self.review_unified_line_rows
                .iter()
                .map(|(row, idx)| (y.saturating_add(*row as u16), *idx))
                .collect()
        };
        rows.sort_unstable();
        rows.dedup_by_key(|(row, _)| *row);
        rows
    }

    /// The display line under the cursor in the last frame, if the line cursor is on and
    /// on a line.
    pub(crate) fn line_cursor_display_idx(&self) -> Option<usize> {
        if !self.line_cursor_applies() {
            return None;
        }
        let row = self.review_line_add_row?;
        self.line_cursor_rows()
            .into_iter()
            .find_map(|(line_row, idx)| (line_row == row).then_some(idx))
    }

    pub(crate) fn line_cursor_applies(&self) -> bool {
        self.line_cursor
            && !self.stepping
            && self.view_mode != ViewMode::Preview
            && !self.current_file_is_binary()
    }

    /// Move the cursor one diff line down (`delta > 0`) or up and centre it. Returns
    /// false when the frame has no diff lines, so the caller can fall back to scrolling.
    pub(crate) fn move_line_cursor(&mut self, delta: isize) -> bool {
        if !self.line_cursor_applies() {
            return false;
        }
        let rows = self.line_cursor_rows();
        if rows.is_empty() {
            return false;
        }
        let down = delta > 0;
        let current = self
            .line_cursor_target
            .and_then(|target| match target {
                LineCursorTarget::Line(idx) => Some(idx),
                LineCursorTarget::ActiveChange => None,
            })
            .or_else(|| self.line_cursor_display_idx());
        let target = match current {
            // No cursor yet: start on the line in the middle row.
            None => {
                let middle = self.line_cursor_middle_row().unwrap_or(rows[0].0);
                rows.iter()
                    .find(|(row, _)| *row >= middle)
                    .or(rows.last())
                    .map(|(_, idx)| *idx)
            }
            // The next line drawn on screen skips folded lines; past the last drawn line,
            // the next display line.
            Some(idx) => {
                let drawn = if down {
                    rows.iter()
                        .map(|(_, other)| *other)
                        .find(|other| *other > idx)
                } else {
                    rows.iter()
                        .rev()
                        .map(|(_, other)| *other)
                        .find(|other| *other < idx)
                };
                drawn.or_else(|| {
                    if down {
                        let len = self.review_visible_lines_with_idx().len();
                        (idx + 1 < len).then_some(idx + 1)
                    } else {
                        idx.checked_sub(1)
                    }
                })
            }
        };
        if let Some(idx) = target {
            self.set_line_cursor_target(LineCursorTarget::Line(idx));
            // The next hunk jump starts from the cursor, not from the last active hunk.
            self.multi_diff.current_navigator().set_hunk_scope(false);
        }
        true
    }

    pub(crate) fn set_line_cursor_target(&mut self, target: LineCursorTarget) {
        self.line_cursor_target = Some(target);
        self.line_cursor_last_scroll = None;
        self.review_line_add_side = None;
        self.review_line_add_hover = false;
    }

    fn line_cursor_middle_row(&self) -> Option<u16> {
        let (_, y, _, height) = self.diff_view_area?;
        (height > 0).then(|| y + height / 2)
    }

    /// Called after each frame. Scrolls toward the cursor target and moves the cursor
    /// onto it. Returns true when the frame must be drawn again, because the view
    /// scrolled or the cursor moved to a new row.
    pub(crate) fn settle_line_cursor(&mut self) -> bool {
        let file = self.multi_diff.selected_index;
        if self.line_cursor_applies() && self.line_cursor_file != Some(file) {
            self.line_cursor_file = Some(file);
            self.line_cursor_target = Some(LineCursorTarget::ActiveChange);
            self.line_cursor_last_scroll = None;
        }
        let Some(target) = self.line_cursor_target else {
            return self.line_cursor_follow_scroll();
        };
        if !self.line_cursor_applies() {
            self.line_cursor_target = None;
            return false;
        }
        let rows = self.line_cursor_rows();
        let Some(middle) = self.line_cursor_middle_row() else {
            return false;
        };
        if rows.is_empty() {
            self.line_cursor_target = None;
            return false;
        }
        let target_idx = match target {
            LineCursorTarget::Line(idx) => idx,
            LineCursorTarget::ActiveChange => {
                let active = self
                    .review_visible_lines_with_idx()
                    .iter()
                    .find_map(|(idx, line)| line.is_primary_active.then_some(*idx));
                match active {
                    Some(idx) => {
                        self.line_cursor_target = Some(LineCursorTarget::Line(idx));
                        idx
                    }
                    None => {
                        self.line_cursor_target = None;
                        return false;
                    }
                }
            }
        };
        let found = rows
            .iter()
            .find(|(_, idx)| *idx == target_idx)
            .map(|(row, _)| *row);
        // Rows to scroll so the target reaches the middle row. Scroll units are screen
        // rows, so this is exact for a drawn target and an estimate for one off screen.
        let shift: isize = match found {
            Some(row) => row as isize - middle as isize,
            None => {
                let first = rows.first().map(|(_, idx)| *idx).unwrap_or(0);
                let last = rows.last().map(|(_, idx)| *idx).unwrap_or(0);
                if target_idx > last {
                    (target_idx - last) as isize
                        + (rows.last().unwrap().0 as isize - middle as isize)
                } else {
                    -((first.saturating_sub(target_idx)) as isize)
                        - (middle as isize - rows[0].0 as isize)
                }
            }
        };
        let wanted = (self.scroll_offset as isize + shift).max(0) as usize;
        // Stop when the view is centred, or when the previous attempt could not scroll
        // any further (the start or end of the file).
        let stuck = self.line_cursor_last_scroll == Some(wanted);
        if shift == 0 || stuck {
            self.line_cursor_target = None;
            self.line_cursor_last_scroll = None;
            self.line_cursor_seen_scroll = Some(self.scroll_offset);
            let row = found.or_else(|| {
                // The target is hidden (for example inside a fold): use the nearest line.
                rows.iter()
                    .min_by_key(|(_, idx)| idx.abs_diff(target_idx))
                    .map(|(row, _)| *row)
            });
            let moved = row != self.review_line_add_row;
            self.review_line_add_row = row;
            return moved;
        }
        self.line_cursor_last_scroll = Some(wanted);
        self.scroll_offset = wanted;
        self.centered_once = false;
        true
    }

    /// After the view scrolls (half page, `J`/`K`, the trackpad), put the cursor on the
    /// line in the middle row, so the next `j`/`k` continues from there.
    fn line_cursor_follow_scroll(&mut self) -> bool {
        if !self.line_cursor_applies() {
            return false;
        }
        let scroll = self.scroll_offset;
        let previous = self.line_cursor_seen_scroll.replace(scroll);
        if previous.is_none() || previous == Some(scroll) {
            return false;
        }
        let Some(middle) = self.line_cursor_middle_row() else {
            return false;
        };
        let row = self
            .line_cursor_rows()
            .into_iter()
            .map(|(row, _)| row)
            .min_by_key(|row| row.abs_diff(middle));
        let moved = row.is_some() && row != self.review_line_add_row;
        if moved {
            self.review_line_add_row = row;
            self.review_line_add_side = None;
            self.review_line_add_hover = false;
        }
        moved
    }

    /// The first selectable cell on the cursor row, as a selection start point relative
    /// to the diff area, so `v`/`V` start where the cursor is.
    pub(crate) fn line_cursor_selection_cell(&self) -> Option<((u16, u16), (u16, u16))> {
        if !self.line_cursor_applies() {
            return None;
        }
        let (_, y, _, height) = self.diff_view_area?;
        let row = self.review_line_add_row?;
        if row < y || row >= y.saturating_add(height) {
            return None;
        }
        let local = row - y;
        let cells = self.diff_selection_cells.get(local as usize)?;
        for (col_start, col_end) in self.diff_selection_content_ranges() {
            let end = (col_end as usize).min(cells.len());
            for col in col_start as usize..end {
                if cells
                    .get(col)
                    .is_some_and(|symbol| !symbol.is_empty() && symbol != " ")
                {
                    return Some(((col as u16, local), (col_start, col_end)));
                }
            }
        }
        None
    }

    /// The active-change marker is drawn only in step mode; in no-step mode the line
    /// cursor marks the current line.
    pub(crate) fn shows_active_marker(&self) -> bool {
        !self.line_cursor_applies()
    }
}
