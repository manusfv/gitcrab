//! Persistent layout settings for the history table.
use serde::{Deserialize, Serialize};

/// A configurable column in the commit history table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryColumn {
    References,
    Graph,
    Message,
    Sha,
    Author,
    Date,
}

impl HistoryColumn {
    /// All columns in their default layout order.
    pub const ALL: [Self; 6] = [
        Self::References,
        Self::Graph,
        Self::Message,
        Self::Sha,
        Self::Author,
        Self::Date,
    ];

    /// Returns the heading shown in the history table.
    pub fn label(self) -> &'static str {
        match self {
            Self::References => "Branch / tag",
            Self::Graph => "Graph",
            Self::Message => "Commit message",
            Self::Sha => "SHA",
            Self::Author => "Author",
            Self::Date => "Date / Time",
        }
    }

    /// Returns the smallest supported width in logical pixels.
    pub const fn minimum_width(self) -> f32 {
        match self {
            Self::References => 80.,
            Self::Graph => 54.,
            Self::Message => 200.,
            Self::Sha => 80.,
            Self::Author => 100.,
            Self::Date => 170.,
        }
    }

    fn default_width(self) -> f32 {
        match self {
            Self::References => 130.,
            Self::Graph => 100.,
            Self::Message => 320.,
            Self::Sha => 90.,
            Self::Author => 160.,
            Self::Date => 190.,
        }
    }
}

/// Saved width and visibility for a single history column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnLayout {
    pub column: HistoryColumn,
    /// Width in logical pixels, retained even when the column is hidden.
    pub width: f32,
    pub visible: bool,
}

/// Ordered layout of the history table, including hidden columns.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryColumns {
    pub columns: Vec<ColumnLayout>,
    /// Set once the user resizes, reorders or toggles a column. Until then,
    /// [`HistoryColumns::adapted`] fits the defaults to the available width.
    pub customized: bool,
}

impl Default for HistoryColumns {
    fn default() -> Self {
        Self {
            columns: HistoryColumn::ALL
                .into_iter()
                .map(|column| ColumnLayout {
                    column,
                    width: column.default_width(),
                    visible: !matches!(column, HistoryColumn::Author | HistoryColumn::Date),
                })
                .collect(),
            customized: false,
        }
    }
}

impl HistoryColumns {
    /// Returns the layout to display in `available` logical pixels at a text `scale`.
    ///
    /// A customized layout is shown as saved. Otherwise text columns grow with
    /// the text size, Author and Date appear when they fit, SHA yields its
    /// space first, and the message shrinks toward its minimum. The message
    /// takes any remaining space when displayed.
    pub fn adapted(&self, available: f32, scale: f32) -> Self {
        if self.customized || !available.is_finite() || available <= 0. {
            return self.clone();
        }
        let scale = if scale.is_finite() {
            scale.clamp(1., 2.5)
        } else {
            1.
        };
        let mut layout = self.clone();
        for entry in &mut layout.columns {
            if entry.column != HistoryColumn::Graph {
                entry.width = (entry.width * scale).min(1600.);
            }
        }
        let total = |l: &Self| l.visible().map(|e| e.width).sum::<f32>();
        for column in [HistoryColumn::Author, HistoryColumn::Date] {
            if let Some(index) = layout.columns.iter().position(|e| e.column == column) {
                let width = layout.columns[index].width;
                if !layout.columns[index].visible && total(&layout) + width <= available {
                    layout.columns[index].visible = true;
                }
            }
        }
        if total(&layout) > available {
            if let Some(sha) = layout
                .columns
                .iter_mut()
                .find(|e| e.column == HistoryColumn::Sha)
            {
                sha.visible = false;
            }
        }
        let overflow = total(&layout) - available;
        if overflow > 0. {
            if let Some(message) = layout
                .columns
                .iter_mut()
                .find(|e| e.column == HistoryColumn::Message)
            {
                message.width =
                    (message.width - overflow).max(HistoryColumn::Message.minimum_width());
            }
        }
        layout
    }

    /// Moves a visible column one visible position left (`-1`) or right (`1`).
    /// Returns `false` at either end or for a hidden column.
    pub fn move_visible(&mut self, column: HistoryColumn, delta: isize) -> bool {
        let order: Vec<_> = self.visible().map(|e| e.column).collect();
        let Some(index) = order.iter().position(|c| *c == column) else {
            return false;
        };
        let Some(target) = index
            .checked_add_signed(delta)
            .and_then(|i| order.get(i))
            .copied()
        else {
            return false;
        };
        if target == column {
            return false;
        }
        self.move_to(column, target, delta > 0);
        self.customized = true;
        true
    }

    /// Changes a column's width by `delta` pixels, within its limits.
    pub fn resize_by(&mut self, column: HistoryColumn, delta: f32) {
        if let Some(width) = self
            .columns
            .iter()
            .find(|e| e.column == column)
            .map(|e| e.width)
        {
            self.resize(column, width + delta);
            self.customized = true;
        }
    }

    /// Repairs a saved layout before use.
    ///
    /// Keeps the first occurrence of each column, appends missing columns with
    /// their defaults, repairs invalid widths and ensures the message is visible.
    pub fn normalize(&mut self) {
        let mut seen = Vec::new();
        self.columns.retain(|entry| {
            if seen.contains(&entry.column) {
                return false;
            }
            seen.push(entry.column);
            true
        });
        for entry in &mut self.columns {
            entry.width = if entry.width.is_finite() {
                entry.width.clamp(entry.column.minimum_width(), 1600.)
            } else {
                entry.column.default_width()
            };
            if entry.column == HistoryColumn::Message {
                entry.visible = true;
            }
        }
        for entry in Self::default().columns {
            if !seen.contains(&entry.column) {
                self.columns.push(entry);
            }
        }
    }

    /// Iterates over visible columns in display order.
    pub fn visible(&self) -> impl Iterator<Item = &ColumnLayout> {
        self.columns.iter().filter(|entry| entry.visible)
    }

    /// Width of the message cell that is on screen, in logical pixels, or `None`
    /// before the history viewport has been measured.
    ///
    /// The table is `max(sum of visible widths, viewport)` wide, with the message
    /// taking any slack. A layout wider than the `viewport` scrolls horizontally;
    /// `scrolled` is how far it is scrolled right. The result is cut at the
    /// viewport's right edge but not its left, so it can exceed the visible part
    /// when the message is scrolled partly out on the left.
    pub fn visible_message_width(&self, viewport: f32, scrolled: f32) -> Option<f32> {
        if !(viewport.is_finite() && viewport > 0.) {
            return None;
        }
        let scrolled = if scrolled.is_finite() {
            scrolled.max(0.)
        } else {
            0.
        };
        let mut seen = false;
        let (before, after) =
            self.visible()
                .fold((0., 0.), |(before, after), entry| match entry.column {
                    HistoryColumn::Message => {
                        seen = true;
                        (before, after)
                    }
                    _ if seen => (before, after + entry.width),
                    _ => (before + entry.width, after),
                });
        let table = self.visible().map(|entry| entry.width).sum::<f32>();
        let message_right = table.max(viewport) - after;
        let visible_right = viewport + scrolled;
        Some((message_right.min(visible_right) - before).max(0.))
    }

    /// Toggles an existing column's visibility. The message column stays visible.
    pub fn toggle(&mut self, column: HistoryColumn) {
        if column != HistoryColumn::Message {
            if let Some(entry) = self.columns.iter_mut().find(|entry| entry.column == column) {
                entry.visible = !entry.visible;
                self.customized = true;
            }
        }
    }

    /// Sets the width of an existing column, clamped to its minimum and 1600 pixels.
    /// Non-finite widths are ignored.
    pub fn resize(&mut self, column: HistoryColumn, width: f32) {
        if width.is_finite() {
            if let Some(entry) = self.columns.iter_mut().find(|entry| entry.column == column) {
                entry.width = width.clamp(column.minimum_width(), 1600.);
            }
        }
    }

    /// Moves a column before `target`, or after it when `after` is `true`.
    /// Either column may be hidden. Both must be present for the move to take effect.
    pub fn move_to(&mut self, column: HistoryColumn, target: HistoryColumn, after: bool) {
        if column == target {
            return;
        }
        let Some(from) = self.columns.iter().position(|entry| entry.column == column) else {
            return;
        };
        if !self.columns.iter().any(|entry| entry.column == target) {
            return;
        }
        let entry = self.columns.remove(from);
        let slot = self
            .columns
            .iter()
            .position(|entry| entry.column == target)
            .unwrap();
        self.columns.insert(slot + usize::from(after), entry);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_message_width_is_cut_at_the_viewport_edge() {
        // References 124, Graph 148, Message 475: a 747px table.
        let mut layout = HistoryColumns::default();
        for entry in &mut layout.columns {
            (entry.width, entry.visible) = match entry.column {
                HistoryColumn::References => (124., true),
                HistoryColumn::Graph => (148., true),
                HistoryColumn::Message => (475., true),
                _ => (entry.width, false),
            };
        }
        assert_eq!(layout.visible_message_width(0., 0.), None);
        // A wider viewport gives the message the slack.
        assert_eq!(layout.visible_message_width(900., 0.), Some(628.));
        // An overflowing layout loses what is cut off, until scrolled into view.
        assert_eq!(layout.visible_message_width(709., 0.), Some(437.));
        assert_eq!(layout.visible_message_width(709., 38.), Some(475.));
        // A column to the right of the message keeps its space.
        layout.toggle(HistoryColumn::Sha);
        layout.resize(HistoryColumn::Sha, 90.);
        assert_eq!(layout.visible_message_width(1000., 0.), Some(638.));
    }

    #[test]
    fn layout_survives_round_trip_with_hidden_reordered_and_resized_columns() {
        let mut layout = HistoryColumns::default();
        layout.toggle(HistoryColumn::Author);
        layout.toggle(HistoryColumn::Graph);
        layout.resize(HistoryColumn::Graph, 60.);
        layout.resize(HistoryColumn::Message, 480.);
        layout.move_to(HistoryColumn::Author, HistoryColumn::References, false);
        layout.move_to(HistoryColumn::References, HistoryColumn::Sha, true);
        let mut restored: HistoryColumns =
            serde_json::from_str(&serde_json::to_string(&layout).unwrap()).unwrap();
        restored.normalize();
        assert_eq!(
            restored
                .columns
                .iter()
                .find(|entry| entry.column == HistoryColumn::Graph)
                .unwrap()
                .width,
            60.
        );
        assert_eq!(layout, restored);
        assert_eq!(
            restored.visible().map(|e| e.column).collect::<Vec<_>>(),
            [
                HistoryColumn::Author,
                HistoryColumn::Message,
                HistoryColumn::Sha,
                HistoryColumn::References
            ]
        );
        assert_eq!(restored.columns[2].width, 480.);
    }

    #[test]
    fn repairs_partial_duplicate_and_invalid_settings() {
        let mut layout = HistoryColumns {
            columns: vec![
                ColumnLayout {
                    column: HistoryColumn::Message,
                    width: f32::NAN,
                    visible: false,
                },
                ColumnLayout {
                    column: HistoryColumn::Message,
                    width: 900.,
                    visible: true,
                },
                ColumnLayout {
                    column: HistoryColumn::Sha,
                    width: -1.,
                    visible: false,
                },
            ],
            customized: false,
        };
        layout.normalize();
        assert_eq!(layout.columns.len(), 6);
        assert_eq!(layout.columns[0].width, 320.);
        assert!(layout.columns[0].visible);
        assert_eq!(layout.columns[1].width, 80.);
        layout.toggle(HistoryColumn::Message);
        assert!(layout.columns[0].visible);
        layout.resize(HistoryColumn::Sha, f32::INFINITY);
        assert_eq!(layout.columns[1].width, 80.);
    }

    #[test]
    fn defaults_adapt_to_width_and_text_size_until_customized() {
        let layout = HistoryColumns::default();
        let wide = layout.adapted(1400., 1.);
        assert!(wide.visible().any(|e| e.column == HistoryColumn::Author));
        assert!(wide.visible().any(|e| e.column == HistoryColumn::Date));
        let narrow = layout.adapted(560., 1.);
        assert!(!narrow.visible().any(|e| e.column == HistoryColumn::Author));
        assert!(!narrow.visible().any(|e| e.column == HistoryColumn::Sha));
        let message = |l: &HistoryColumns| {
            l.columns
                .iter()
                .find(|e| e.column == HistoryColumn::Message)
                .unwrap()
                .width
        };
        assert!(message(&narrow) >= HistoryColumn::Message.minimum_width());
        let large = layout.adapted(2000., 2.);
        let refs = large
            .columns
            .iter()
            .find(|e| e.column == HistoryColumn::References)
            .unwrap();
        assert_eq!(refs.width, 260.);
        let mut custom = layout.clone();
        custom.toggle(HistoryColumn::Sha);
        assert!(custom.customized);
        assert_eq!(custom.adapted(2000., 2.), custom);
    }

    #[test]
    fn keyboard_moves_follow_visible_order() {
        let mut layout = HistoryColumns::default();
        assert!(!layout.move_visible(HistoryColumn::References, -1));
        assert!(layout.move_visible(HistoryColumn::References, 1));
        assert_eq!(
            layout.visible().map(|e| e.column).collect::<Vec<_>>(),
            [
                HistoryColumn::Graph,
                HistoryColumn::References,
                HistoryColumn::Message,
                HistoryColumn::Sha
            ]
        );
        assert!(!layout.move_visible(HistoryColumn::Author, 1));
        assert!(!layout.move_visible(HistoryColumn::Sha, 1));
        layout.resize_by(HistoryColumn::Sha, -100.);
        assert_eq!(
            layout
                .columns
                .iter()
                .find(|e| e.column == HistoryColumn::Sha)
                .unwrap()
                .width,
            HistoryColumn::Sha.minimum_width()
        );
        assert!(layout.customized);
    }

    #[test]
    fn moves_in_both_directions_and_preserves_hidden_column_settings() {
        let mut layout = HistoryColumns::default();
        layout.move_to(HistoryColumn::References, HistoryColumn::Date, true);
        assert_eq!(
            layout.columns.last().unwrap().column,
            HistoryColumn::References
        );
        layout.move_to(HistoryColumn::References, HistoryColumn::Graph, false);
        assert_eq!(layout, HistoryColumns::default());
        layout.resize(HistoryColumn::Author, 240.);
        layout.toggle(HistoryColumn::Author);
        assert_eq!(
            layout
                .visible()
                .find(|e| e.column == HistoryColumn::Author)
                .unwrap()
                .width,
            240.
        );
    }
}
