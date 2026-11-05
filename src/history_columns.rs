//! Persistent column layout, independent of the native renderer.
use serde::{Deserialize, Serialize};

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
    pub const ALL: [Self; 6] = [
        Self::References,
        Self::Graph,
        Self::Message,
        Self::Sha,
        Self::Author,
        Self::Date,
    ];

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

    pub fn minimum_width(self) -> f32 {
        match self {
            Self::References => 80.,
            Self::Graph => 92.,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColumnLayout {
    pub column: HistoryColumn,
    pub width: f32,
    pub visible: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HistoryColumns {
    pub columns: Vec<ColumnLayout>,
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
        }
    }
}

impl HistoryColumns {
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

    pub fn visible(&self) -> impl Iterator<Item = &ColumnLayout> {
        self.columns.iter().filter(|entry| entry.visible)
    }

    pub fn toggle(&mut self, column: HistoryColumn) {
        if column != HistoryColumn::Message {
            if let Some(entry) = self.columns.iter_mut().find(|entry| entry.column == column) {
                entry.visible = !entry.visible;
            }
        }
    }

    pub fn resize(&mut self, column: HistoryColumn, width: f32) {
        if width.is_finite() {
            if let Some(entry) = self.columns.iter_mut().find(|entry| entry.column == column) {
                entry.width = width.clamp(column.minimum_width(), 1600.);
            }
        }
    }

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
    fn layout_survives_round_trip_with_hidden_reordered_and_resized_columns() {
        let mut layout = HistoryColumns::default();
        layout.toggle(HistoryColumn::Author);
        layout.toggle(HistoryColumn::Graph);
        layout.resize(HistoryColumn::Message, 480.);
        layout.move_to(HistoryColumn::Author, HistoryColumn::References, false);
        layout.move_to(HistoryColumn::References, HistoryColumn::Sha, true);
        let restored: HistoryColumns =
            serde_json::from_str(&serde_json::to_string(&layout).unwrap()).unwrap();
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
