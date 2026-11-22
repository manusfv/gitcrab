use super::*;
use gitcrab_core::history_columns::{HistoryColumn, HistoryColumns};
mod columns;
mod graph_scroll;
pub(super) use graph_scroll::GraphScroll;

const HISTORY_CELL_INSET: Rems = Rems(0.5);
const HISTORY_HEADER_HEIGHT: f32 = 28.;
/// Commit row heights by density. Larger fonts grow rows past these to fit a
/// line of [`COMMIT_ROW_LINE_HEIGHT`] plus [`COMMIT_ROW_PADDING`].
const COMMIT_ROW_HEIGHT_COMPACT: f32 = 26.;
const COMMIT_ROW_HEIGHT: f32 = geometry::SIZE_ROW_REGULAR;
/// Line height of commit-row text at [`BASE_FONT_SIZE`].
const COMMIT_ROW_LINE_HEIGHT: f32 = 20.;
const COMMIT_ROW_PADDING: f32 = 8.;
/// Vertical inset of the branch band from a row's top and bottom edges.
const BAND_INSET_Y: f32 = 3.;
/// Expanded reference labels per row in element ids, so ids stay unique per row.
const REF_IDS_PER_ROW: usize = 64;
const ANIMATION_FRAME: Duration = Duration::from_millis(16);
/// Rows left below the viewport when the next history page is requested,
/// so it usually arrives before the user reaches the end.
const NEXT_PAGE_PREFETCH_ROWS: usize = 100;

/// Narrowest the WIP summary editor gets when the history pane is cramped.
const WIP_EDITOR_MIN_WIDTH: f32 = 48.;
/// Width of the WIP summary editor while its text is short or empty.
const WIP_EDITOR_DEFAULT_WIDTH: f32 = 120.;
/// Right padding of the WIP editor. The input also keeps a 10px margin right
/// of the caret (gpui-component's `RIGHT_MARGIN`), so together they match the
/// 12px left padding.
const WIP_EDITOR_PAD_RIGHT: f32 = 2.;
const WIP_EDITOR_PAD_LEFT: f32 = 12.;
const WIP_EDITOR_CARET_MARGIN: f32 = 10.;
/// Left and right border of the input.
const WIP_EDITOR_BORDER: f32 = 2.;
/// The editor is this much shorter than its row, down to [`WIP_EDITOR_MIN_HEIGHT`].
const WIP_EDITOR_ROW_INSET: f32 = 10.;
const WIP_EDITOR_MIN_HEIGHT: f32 = 18.;

/// Width the WIP editor needs to show `text_width` of text without the input
/// scrolling it: padding, border and caret margin, plus 1px against rounding.
/// Short text uses [`WIP_EDITOR_DEFAULT_WIDTH`]; the cell's cap and the badges
/// can still shrink it to [`WIP_EDITOR_MIN_WIDTH`].
fn wip_editor_width(text_width: f32) -> f32 {
    let text = if text_width.is_finite() {
        text_width.max(0.).ceil()
    } else {
        0.
    };
    (text
        + WIP_EDITOR_PAD_LEFT
        + WIP_EDITOR_PAD_RIGHT
        + WIP_EDITOR_BORDER
        + WIP_EDITOR_CARET_MARGIN
        + 1.)
        .max(WIP_EDITOR_DEFAULT_WIDTH)
}

use gitcrab_core::presentation::{resolve_decorations, Decoration, DecorationKind};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// How long a hovered reference waits before the rows off its history dim,
/// and how long they then take to fade.
const REF_DIM_DELAY: Duration = Duration::from_millis(1000);
const REF_DIM_FADE: Duration = Duration::from_millis(500);
/// A ghost reference label, the nearest branch containing an undecorated
/// commit, is drawn at half opacity, and a little stronger while hovered.
const GHOST_OPACITY: f32 = 0.5;
const GHOST_HOVER_OPACITY: f32 = 0.65;

/// Opacity of the branch band that runs from a commit's node to the text
/// columns, and of a selected row's band, which stands in for the selection
/// fill there: as strong as the checked-out branch's label.
const BAND_TINT: u8 = 0x24;
const SELECTED_BAND_TINT: u8 = REF_HEAD_TINT;

/// Text opacities of the dark theme for the commit list, applied to
/// white so they blend over the hover and selection fills the same way.
/// Titles use `--text-normal`; SHA, author and date `--text-disabled`.
const TITLE_ALPHA: f32 = 0.75;
const META_ALPHA: f32 = 0.4;
/// The selected row uses `--text-selected-row` for every column.
const SELECTED_ALPHA: f32 = 1.;
/// The message body after the title is drawn at 60% of the title.
const BODY_FACTOR: f32 = 0.6;
/// Rows dimmed by a hovered reference or left out by a search use
/// `--text-dimmed`; selected or hovered ones `--text-dimmed-selected`.
const DIMMED_ALPHA: f32 = 0.2;
const DIMMED_SELECTED_ALPHA: f32 = 0.5;

/// Commit-list text at `alpha`: white on the dark palette, black on the
/// light validation palette.
fn graph_text(alpha: f32) -> Rgba {
    let canvas = role_color(Role::SurfaceCanvas);
    let ink = if canvas.r + canvas.g + canvas.b < 1.5 {
        1.
    } else {
        0.
    };
    Rgba {
        r: ink,
        g: ink,
        b: ink,
        a: alpha,
    }
}

/// Text colours of one commit row.
struct RowText {
    title: Rgba,
    meta: Rgba,
}

impl RowText {
    /// `dim` is how far the row has faded, 0 to 1: off a hovered reference's
    /// history (animated), or 1 when a search leaves it out.
    fn new(selected: bool, hovered: bool, dim: f32) -> Self {
        let dimmed = if selected || hovered {
            DIMMED_SELECTED_ALPHA
        } else {
            DIMMED_ALPHA
        };
        let fade = |base: f32| base + (dimmed - base) * dim;
        let (title, meta) = if selected {
            (fade(SELECTED_ALPHA), fade(SELECTED_ALPHA))
        } else {
            (fade(TITLE_ALPHA), fade(META_ALPHA))
        };
        Self {
            title: graph_text(title),
            meta: graph_text(meta),
        }
    }
}

/// Pointer state of the history graph.
#[derive(Default)]
pub(super) struct HistoryHover {
    /// Commit row (0 is the newest commit) under the pointer.
    row: Option<usize>,
    /// The pointer is on the working-changes row.
    wip: bool,
    /// Row whose references are expanded into a list of every label.
    refs_row: Option<usize>,
    /// Parts of that row's references under the pointer (see [`RefsPart`]).
    /// Buttons take the hover from what lies beneath them, so each part
    /// reports its own; the list stays open while any of them is hovered.
    refs_over: Vec<RefsPart>,
    highlight: Option<RefHighlight>,
    /// Waits out [`REF_DIM_DELAY`] and runs the fade; dropping it cancels both.
    dim_task: Option<Task<()>>,
}

/// The rows on a hovered reference's history.
struct RefHighlight {
    /// The decorated row the reference points at.
    row: usize,
    reachable: Rc<Vec<bool>>,
    /// How far the other rows have faded, from 0 to 1.
    dim: f32,
}

/// A hoverable part of a row's references.
#[derive(Clone, Copy, PartialEq, Eq)]
enum RefsPart {
    Cell,
    InlineLabel,
    List,
    ListLabel(usize),
}

impl HistoryHover {
    fn refs_open(&self, row: usize) -> bool {
        self.refs_row == Some(row) && !self.refs_over.is_empty()
    }

    /// The commit row counts as hovered: the pointer is on it, but not on
    /// its references. The references column (and the
    /// list it expands into) is a zone of its own that does not hover the row.
    fn row_hovered(&self, row: usize) -> bool {
        self.row == Some(row) && !self.refs_open(row)
    }

    /// How far `row` has faded for being off the hovered reference's history.
    fn dim(&self, row: usize) -> f32 {
        self.highlight
            .as_ref()
            .filter(|h| !h.reachable.get(row).copied().unwrap_or(true))
            .map_or(0., |h| h.dim)
    }
}

/// The decoration shown inline: the checked-out branch, else the first that
/// is not a detached `HEAD`, else the first.
fn primary_decoration(decorations: &[Decoration]) -> Option<usize> {
    decorations
        .iter()
        .position(|d| d.kind == DecorationKind::HeadBranch)
        .or_else(|| {
            decorations
                .iter()
                .position(|d| d.kind != DecorationKind::Head)
        })
        .or((!decorations.is_empty()).then_some(0))
}

/// A type marker drawn before a decoration's name so tags, remotes and HEAD
/// stay distinct.
fn decoration_marker(decoration: &Decoration) -> Option<AnyElement> {
    let glyph = |name: &str| {
        icon(name)
            .size(scaled(REF_BADGE_ICON_SIZE))
            .flex_none()
            .text_role(Role::TextSecondary)
            .into_any_element()
    };
    match (decoration.kind, decoration.target.as_ref().map(|t| t.0)) {
        // A checkmark: the branch name already says where
        // HEAD is, so no "HEAD" text. The tooltip and label still say it.
        (DecorationKind::HeadBranch, _) => Some(
            icon("check")
                .size(scaled(REF_BADGE_ICON_SIZE))
                .flex_none()
                .text_role(Role::TextDefault)
                .into_any_element(),
        ),
        (DecorationKind::Tag, _) => Some(glyph("tag")),
        _ => None,
    }
}

/// Where a branch lives, drawn after its name: a monitor for
/// a local branch, a cloud for a remote-tracking one.
fn decoration_location(decoration: &Decoration) -> Option<AnyElement> {
    let name = match (decoration.kind, decoration.target.as_ref().map(|t| t.0)) {
        (DecorationKind::Tag | DecorationKind::Head, _) => return None,
        (_, Some(ReferenceKind::Remote)) => "remote",
        (DecorationKind::HeadBranch, _) | (_, Some(ReferenceKind::LocalBranch)) => "local",
        _ => return None,
    };
    Some(
        icon(name)
            .size(scaled(REF_BADGE_ICON_SIZE))
            .flex_none()
            .text_role(Role::TextSecondary)
            .into_any_element(),
    )
}

#[derive(Clone, Copy)]
enum ReferenceSide {
    Left,
    Right,
}

fn paint_reference_link(
    w: &mut Window,
    start: Pixels,
    end: Pixels,
    y: Pixels,
    color: usize,
    checked_out: bool,
) {
    let scale = w.scale_factor();
    // The checked-out branch gets a thick, stronger link; others stay hairline.
    // It matches the lane lines exactly: same stroke width, opaque lane colour.
    let (thickness, fill_color) = if checked_out {
        (
            graph_stroke(scale),
            rgba(lanes()[color % lanes().len()] << 8 | 0xFF),
        )
    } else {
        (px(1. / scale), branch_tint(color, 0x70))
    };
    let top = snap_graph_stroke_center(y, scale) - thickness / 2.;
    let top = if checked_out {
        top
    } else {
        px((f32::from(y) * scale - 0.5).round() / scale)
    };
    w.paint_quad(fill(
        Bounds::new(
            point(start.min(end), top),
            size((end - start).abs(), thickness),
        ),
        fill_color,
    ));
}

/// Fills the space between a reference label and the graph with its link line.
fn reference_link_filler(color: usize, checked_out: bool) -> AnyElement {
    div()
        .flex_1()
        .min_w_0()
        .self_stretch()
        .child(
            canvas(
                |_, _, _| (),
                move |bounds, _, w, _| {
                    paint_reference_link(
                        w,
                        bounds.left(),
                        bounds.right(),
                        bounds.center().y,
                        color,
                        checked_out,
                    )
                },
            )
            .size_full(),
        )
        .into_any_element()
}

/// Radius of the dashed working-changes marker, matching the commit avatars.
const WIP_RADIUS: f32 = 11.;

/// The dashed ring that stands for uncommitted work, drawn with the same
/// stroke width as the lane lines.
fn paint_dashed_circle(w: &mut Window, center: Point<Pixels>, radius: Pixels, color: u32) {
    const DASHES: usize = 12;
    const STEPS: usize = 6;
    let stroke = graph_stroke(w.scale_factor());
    let r = f32::from(radius - stroke / 2.);
    let step = std::f32::consts::TAU / DASHES as f32;
    for dash in 0..DASHES {
        // Each dash covers 55% of its slot and is centred on the slot's angle,
        // so there is one dash dead-centre at the top and bottom (even count).
        let start = dash as f32 * step - std::f32::consts::FRAC_PI_2 - step * 0.275;
        let mut path = PathBuilder::stroke(stroke);
        for i in 0..=STEPS {
            let a = start + step * 0.55 * i as f32 / STEPS as f32;
            let p = point(center.x + px(r * a.cos()), center.y + px(r * a.sin()));
            if i == 0 {
                path.move_to(p);
            } else {
                path.line_to(p);
            }
        }
        if let Ok(path) = path.build() {
            w.paint_path(path, rgb(color));
        }
    }
}

/// A stash entry's node: a dashed square, filled so the lanes stop at its
/// edge, holding the stash icon.
fn paint_stash_node(w: &mut Window, center: Point<Pixels>, half: Pixels, color: u32, cx: &App) {
    const DASHES: usize = 3;
    let stroke = graph_stroke(w.scale_factor());
    let box_bounds = Bounds::new(
        point(center.x - half, center.y - half),
        size(half * 2., half * 2.),
    );
    w.paint_quad(fill(box_bounds, rgb(semantic(Role::SurfaceCanvas))).corner_radii(px(3.)));
    let edge = half - stroke / 2.;
    let corners = [
        point(center.x - edge, center.y - edge),
        point(center.x + edge, center.y - edge),
        point(center.x + edge, center.y + edge),
        point(center.x - edge, center.y + edge),
    ];
    // Dashes are centred on both corners and evenly between them; the corner
    // halves of adjacent sides meet as an L, so the corners read as closed.
    let side = f32::from(edge * 2.);
    let slot = side / (DASHES - 1) as f32;
    let dash = slot * 0.55;
    for (i, start) in corners.iter().enumerate() {
        let end = corners[(i + 1) % corners.len()];
        let along = |t: f32| {
            let t = (t / side).clamp(0., 1.);
            point(
                start.x + (end.x - start.x) * t,
                start.y + (end.y - start.y) * t,
            )
        };
        for d in 0..DASHES {
            let from = d as f32 * slot - dash / 2.;
            let mut path = PathBuilder::stroke(stroke);
            path.move_to(along(from));
            path.line_to(along(from + dash));
            if let Ok(path) = path.build() {
                w.paint_path(path, rgb(color));
            }
        }
    }
    let icon = half * 1.1;
    let _ = w.paint_svg(
        Bounds::new(
            point(center.x - icon / 2., center.y - icon / 2.),
            size(icon, icon),
        ),
        "icons/stash.svg".into(),
        None,
        TransformationMatrix::unit(),
        rgb(color).into(),
        cx,
    );
}

/// The branch band behind the working-changes marker: it runs from the
/// circle's centre to `right`, with a cut-out so its left edge follows the
/// circle's right half and the circle itself stays unfilled.
fn paint_hugging_band(
    w: &mut Window,
    center: Point<Pixels>,
    radius: Pixels,
    top: Pixels,
    bottom: Pixels,
    right: Pixels,
    color: Rgba,
) {
    const STEPS: usize = 24;
    let r = f32::from(radius);
    // Half the band's height measured from the circle's centre; when the band
    // is shorter than the circle, the arc is cut where it meets the edges.
    let reach = f32::from((center.y - top).min(bottom - center.y)).max(0.);
    let limit = (reach / r).min(1.).asin();
    let on_arc = |a: f32| point(center.x + px(r * a.cos()), center.y + px(r * a.sin()));
    let mut path = PathBuilder::fill();
    path.move_to(point(right, top));
    path.line_to(point(on_arc(-limit).x, top));
    for i in 0..=STEPS {
        path.line_to(on_arc(-limit + 2. * limit * i as f32 / STEPS as f32));
    }
    path.line_to(point(on_arc(limit).x, bottom));
    path.line_to(point(right, bottom));
    path.close();
    if let Ok(path) = path.build() {
        w.paint_path(path, color);
    }
}

/// A dotted lane line, used from the working-changes marker down to the first
/// commit. `travelled` is how far the pattern already ran above `top`, so the
/// dots continue unbroken across the row boundary.
fn paint_dotted_lane(
    w: &mut Window,
    x: Pixels,
    top: Pixels,
    bottom: Pixels,
    color: u32,
    travelled: Pixels,
) {
    let stroke = graph_stroke(w.scale_factor());
    let (dash, gap) = (stroke, stroke * 1.5);
    let period = dash + gap;
    let phase = px(f32::from(travelled).rem_euclid(f32::from(period)));
    let mut y = top - phase;
    while y < bottom {
        let (from, to) = (y.max(top), (y + dash).min(bottom));
        if to > from {
            w.paint_quad(fill(
                Bounds::new(point(x - stroke / 2., from), size(stroke, to - from)),
                rgb(color),
            ));
        }
        y += period;
    }
}

/// How round a lane's turn is; capped by the lane gap and half a row.
const LANE_CORNER_RADIUS: f32 = 10.;

fn paint_lane_corner(
    w: &mut Window,
    junction: Point<Pixels>,
    lane_x: Pixels,
    row_edge: Pixels,
    color: u32,
) {
    let run = lane_x - junction.x;
    let rise = row_edge - junction.y;
    let radius = px(LANE_CORNER_RADIUS).min(run.abs()).min(rise.abs());
    let bend_x = lane_x - if run < px(0.) { -radius } else { radius };
    let bend_y = junction.y + if rise < px(0.) { -radius } else { radius };
    let mut path = PathBuilder::stroke(graph_stroke(w.scale_factor()));
    path.move_to(junction);
    path.line_to(point(bend_x, junction.y));
    path.curve_to(point(lane_x, bend_y), point(lane_x, junction.y));
    path.line_to(point(lane_x, row_edge));
    if let Ok(path) = path.build() {
        w.paint_path(path, rgb(color));
    }
}

impl GitCrab {
    pub(super) fn column_drag_events(&self, cx: &Context<Self>) -> impl IntoElement {
        components::rail::end_events(
            cx.weak_entity(),
            Self::finish_column_drag,
            Self::cancel_column_drag,
        )
    }

    /// Previews a decoration's reference, or selects its commit for a detached HEAD.
    fn activate_decoration(
        &mut self,
        decoration: &Decoration,
        id: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &decoration.target {
            Some((kind, name)) => {
                if self.single_pane {
                    self.constrained_inspector = true;
                }
                self.preview_ref(*kind, name, window, cx);
            }
            None => self.activate_commit(id, window, cx),
        }
    }

    fn hover_history_row(&mut self, row: usize, hovered: bool, cx: &mut Context<Self>) {
        let hover = &mut self.history_hover;
        let before = hover.row;
        if hovered {
            hover.row = Some(row);
        } else if hover.row == Some(row) {
            hover.row = None;
        }
        if hover.row != before {
            // On another row, no part of an expanded list is under the
            // pointer, even one whose leave was lost when it scrolled away.
            if hovered && hover.refs_row.is_some_and(|refs| refs != row) {
                hover.refs_over.clear();
                hover.refs_row = None;
                self.update_ref_highlight(cx);
            }
            cx.notify();
        }
    }

    /// Records the pointer entering or leaving a part of `row`'s references.
    /// Closes the expanded list once it has left every part, and highlights
    /// the hovered reference's history.
    fn hover_refs(&mut self, row: usize, part: RefsPart, hovered: bool, cx: &mut Context<Self>) {
        let hover = &mut self.history_hover;
        if hovered {
            if hover.refs_row != Some(row) {
                hover.refs_over.clear();
                hover.refs_row = Some(row);
            }
            if !hover.refs_over.contains(&part) {
                hover.refs_over.push(part);
            }
        } else if hover.refs_row == Some(row) {
            hover.refs_over.retain(|over| *over != part);
        }
        if hover.refs_over.is_empty() {
            hover.refs_row = None;
        }
        self.update_ref_highlight(cx);
    }

    /// Highlights the history of the reference under the pointer, dimming
    /// the other rows after [`REF_DIM_DELAY`].
    fn update_ref_highlight(&mut self, cx: &mut Context<Self>) {
        let hover = &self.history_hover;
        let source = hover
            .refs_row
            .and_then(|row| self.ref_highlight_source(row));
        if self.history_hover.highlight.as_ref().map(|h| h.row) != source {
            self.history_hover.dim_task = None;
            self.history_hover.highlight = source.map(|row| RefHighlight {
                row,
                reachable: Rc::new(self.state.graph.reachable_from(row)),
                dim: 0.,
            });
            if source.is_some() {
                self.history_hover.dim_task = Some(cx.spawn(async move |view, cx| {
                    cx.background_executor().timer(REF_DIM_DELAY).await;
                    let start = Instant::now();
                    loop {
                        let t =
                            (start.elapsed().as_secs_f32() / REF_DIM_FADE.as_secs_f32()).min(1.);
                        let updated = view.update(cx, |v, cx| {
                            if let Some(h) = v.history_hover.highlight.as_mut() {
                                h.dim = t;
                            }
                            cx.notify();
                        });
                        if updated.is_err() || t >= 1. {
                            break;
                        }
                        cx.background_executor().timer(ANIMATION_FRAME).await;
                    }
                }));
            }
        }
        cx.notify();
    }

    /// The decorated row whose history hovering `row`'s references highlights.
    /// A ghost label highlights only on the selected row.
    fn ref_highlight_source(&self, row: usize) -> Option<usize> {
        let snapshot = self.state.snapshot.as_ref()?;
        let commit = snapshot.commits.get(row)?;
        if !commit.refs.is_empty() {
            Some(row)
        } else if self.state.selected.as_deref() == Some(commit.id.as_str()) {
            self.state.graph.ghost_source(row)
        } else {
            None
        }
    }

    /// A reference label for `decoration`, tinted with its row's lane colour.
    #[allow(clippy::too_many_arguments)]
    fn ref_pill(
        &self,
        id: impl Into<ElementId>,
        decoration: Decoration,
        commit: String,
        color: usize,
        truncate: bool,
        cx: &mut Context<Self>,
    ) -> Button {
        let checked_out = decoration.kind == DecorationKind::HeadBranch;
        let marker = decoration_marker(&decoration);
        let location = decoration_location(&decoration);
        let name = div()
            // The font sits low in its line box; lift it so it lines up with the icons.
            .relative()
            .top(px(-1.))
            // Set here: the button's inner content overrides a size set on the button.
            .text_size(scaled(REF_BADGE_FONT_SIZE))
            .text_role(Role::TextDefault)
            .when(truncate, |name| name.min_w_0().truncate())
            .when(!truncate, |name| name.flex_none().whitespace_nowrap())
            .child(decoration.name.clone());
        components::tinted_fill(
            self.button(id, ""),
            // The checked-out branch reads as highlighted.
            branch_tint(color, if checked_out { REF_HEAD_TINT } else { REF_TINT }),
            branch_tint(
                color,
                if checked_out {
                    REF_HEAD_HOVER_TINT
                } else {
                    REF_HOVER_TINT
                },
            ),
            Role::TextDefault,
            cx,
        )
        .children(marker)
        .child(name)
        .children(location)
        .gap_1()
        .justify_center()
        .items_center()
        .px(px(7.))
        .h(scaled(REF_BADGE_HEIGHT))
        // No tooltip: a button's tooltip takes its hover listener, which the
        // expanded list needs, and the list already shows the full name.
        .on_click(
            cx.listener(move |v, e: &ClickEvent, w, cx| match &decoration.target {
                Some((ReferenceKind::LocalBranch, name))
                    if e.click_count() == 2 && decoration.kind != DecorationKind::HeadBranch =>
                {
                    v.run_operation(Operation::Checkout, name.clone(), w, cx);
                }
                _ => v.activate_decoration(&decoration, commit.clone(), w, cx),
            }),
        )
    }

    /// Every label of a row, stacked over its references cell while the cell
    /// is hovered, so names cut short or hidden behind `+N` can be read and
    /// picked. The primary label comes first, in its inline place.
    #[allow(clippy::too_many_arguments)]
    fn expanded_refs(
        &self,
        row: usize,
        decorations: &[Decoration],
        primary: usize,
        commit: &str,
        color: usize,
        ghost: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let order =
            std::iter::once(primary).chain((0..decorations.len()).filter(|i| *i != primary));
        let pills = order
            .map(|index| {
                div()
                    .flex_none()
                    // An opaque backing, so rows below don't show through the tint.
                    .bg_role(Role::SurfaceCanvas)
                    .rounded(px(4.))
                    .child({
                        let view = cx.weak_entity();
                        self.ref_pill(
                            (
                                "ref-expanded",
                                row * REF_IDS_PER_ROW + index.min(REF_IDS_PER_ROW - 1),
                            ),
                            decorations[index].clone(),
                            commit.to_owned(),
                            color,
                            false,
                            cx,
                        )
                        .when(ghost, |pill| {
                            pill.cursor_default().opacity(
                                if self
                                    .history_hover
                                    .refs_over
                                    .contains(&RefsPart::ListLabel(index))
                                {
                                    GHOST_HOVER_OPACITY
                                } else {
                                    GHOST_OPACITY
                                },
                            )
                        })
                        .on_hover(move |hovered, _, cx| {
                            let _ = view.update(cx, |v, cx| {
                                v.hover_refs(row, RefsPart::ListLabel(index), *hovered, cx)
                            });
                        })
                    })
            })
            .collect::<Vec<_>>();
        let lift = window.rem_size() * (REF_BADGE_HEIGHT / BASE_FONT_SIZE / 2.);
        div()
            .size_0()
            .flex_none()
            .child(
                deferred(
                    anchored()
                        .anchor(Anchor::TopLeft)
                        .offset(point(px(0.), -lift))
                        .snap_to_window_with_margin(px(4.))
                        .child(
                            div()
                                .id(("ref-expanded-list", row))
                                .occlude()
                                .flex()
                                .flex_col()
                                .items_start()
                                .gap(px(2.))
                                .children(pills)
                                .on_hover(cx.listener(move |v, hovered: &bool, _, cx| {
                                    v.hover_refs(row, RefsPart::List, *hovered, cx)
                                })),
                        ),
                )
                .with_priority(1),
            )
            .into_any_element()
    }

    pub(super) fn history(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.prepare_column_drag(window, cx);
        // Row numbers from before a refresh may name other commits now.
        let rows = self.state.graph.rows.len();
        if self
            .history_hover
            .highlight
            .as_ref()
            .is_some_and(|h| h.reachable.len() != rows)
            || self.history_hover.row.is_some_and(|row| row >= rows)
        {
            self.history_hover = HistoryHover::default();
        }
        self.history_layout = self.history_columns.adapted(
            self.history_available - self.history_gutter() - f32::from(SCROLLBAR_GUTTER),
            self.preferences.font_size / BASE_FONT_SIZE,
        );
        // Lanes keep their pitch; the graph column scrolls when they don't fit.
        let graph_width = self
            .history_layout
            .visible()
            .find(|entry| entry.column == HistoryColumn::Graph)
            .map_or(0., |entry| entry.width);
        self.graph_scroll.set_extent(
            graph_width,
            graph_content_width(graph_width, self.state.graph.lanes),
        );
        let count = self.state.graph.rows.len();
        let wip_rows = usize::from(self.has_wip_row());
        let weak = cx.weak_entity();
        let unborn = self
            .state
            .snapshot
            .as_ref()
            .filter(|s| s.commits.is_empty() && s.head_id.is_empty())
            .map(|s| s.head.clone());
        let row_height = self.commit_row_height();
        panel()
            .bg_role(Role::SurfaceCanvas)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    // A fixed strip beside the table for the vertical scrollbar, so it
                    // never sits over the last column.
                    .pr(SCROLLBAR_GUTTER)
                    .overflow_hidden()
                    .child(
                                div()
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .w_full()
                                    .min_w(px(self.history_table_width()))
                                    .h_full()
                                    .child(self.history_header(cx))
                                    .child({
                                        // Measures the table, so the WIP editor fits its message cell.
                                        let viewport = self.history_viewport.clone();
                                        canvas(
                                            move |bounds, _, _| viewport.set(f32::from(bounds.size.width)),
                                            |_, _, _, _| (),
                                        )
                                        .absolute()
                                        .inset_0()
                                    })
                                    .child(
                                        uniform_list("history", count + wip_rows, move |range, w, cx| {
                                            weak.update(cx, |v, cx| {
                                                if range.end + NEXT_PAGE_PREFETCH_ROWS
                                                    >= count + wip_rows
                                                    && v.state.wants_next_page()
                                                {
                                                    cx.defer_in(w, |v, w, cx| {
                                                        if v.state.wants_next_page() {
                                                            let plan = v.state.load_more();
                                                            v.apply_refresh(plan, w, cx);
                                                        }
                                                    });
                                                }
                                                range
                                                    .map(|i| v.history_row(i, w, cx))
                                                    .collect::<Vec<_>>()
                                            })
                                            .unwrap_or_default()
                                        })
                                        .track_scroll(&self.history_scroll)
                                        // Size from a commit row, not the WIP row: its
                                        // text input reports an unbounded content width.
                                        .with_width_from_item(Some(wip_rows))
                                        .map(|mut el| {
                                            el.style().restrict_scroll_to_axis = Some(true);
                                            el
                                        })
                                        .map(|list| {
                                            if unborn.is_some() {
                                                list.flex_none().h(px(row_height))
                                            } else {
                                                list.flex_1().min_h_0()
                                            }
                                        }),
                                    )
                                    .when_some(unborn, |table, branch| {
                                        table.child(
                                            div()
                                                .flex_1()
                                                .flex()
                                                .flex_col()
                                                .items_center()
                                                .justify_center()
                                                .gap_2()
                                                .p_4()
                                                .child(
                                                    div()
                                                        .text_size(scaled(TEXT_SIZE_HEADING))
                                                        .child("No commits yet"),
                                                )
                                                .child(
                                                    div()
                                                        .text_role(Role::TextMuted)
                                                        .child(format!(
                                                            "{branch} has no commits. Stage files in Working changes and commit them to start its history."
                                                        )),
                                                ),
                                        )
                                    })
                                    // Last, so it sits above the rows it overlays.
                                    .when(self.graph_scroll.max() > 0., |table| {
                                        table.child(self.graph_scrollbar())
                                    }),
                    )
                    .child(pinned_scrollbar_overlay("history", &self.history_scroll, scaled(HISTORY_HEADER_HEIGHT)))
                    .child(
                        // Continues the header band across the scrollbar strip.
                        div()
                            .absolute()
                            .top_0()
                            .right_0()
                            .w(SCROLLBAR_GUTTER)
                            .h(scaled(HISTORY_HEADER_HEIGHT))
                            .bg_role(Role::SurfaceChrome),
                    )
                    .child({
                        // A visible, keyboard-reachable route to column settings.
                        let weak = cx.weak_entity();
                        div()
                            .absolute()
                            .top_0()
                            .right(SCROLLBAR_GUTTER + px(4.))
                            .h(scaled(HISTORY_HEADER_HEIGHT))
                            .flex()
                            .items_center()
                            .bg_role(Role::SurfaceChrome)
                            .child(
                                self.icon_button(
                                    "history-columns",
                                    "columns",
                                    "Columns",
                                )
                                .icon(icon("columns").size(scaled(14.)))
                                .size(scaled(geometry::SIZE_TARGET_INLINE))
                                .dropdown_menu(move |menu, w, cx| {
                                    Self::columns_menu(weak.clone(), menu, w, cx)
                                }),
                            )
                    }),
            )
            .when(self.state.limit == HISTORY_MAX && count == HISTORY_MAX, |e| {
                e.child(
                    div()
                        .h(px(geometry::SIZE_ROW_REGULAR))
                        .px_3()
                        .text_role(Role::TextMuted)
                        .child(format!("Showing the newest {count} commits")),
                )
            })
            .into_any_element()
    }
    fn commit_row_height(&self) -> f32 {
        (if self.compact {
            COMMIT_ROW_HEIGHT_COMPACT
        } else {
            COMMIT_ROW_HEIGHT
        })
        .max(
            self.preferences.font_size * COMMIT_ROW_LINE_HEIGHT / BASE_FONT_SIZE
                + COMMIT_ROW_PADDING,
        )
    }
    /// The Working changes row only exists while there is something in it.
    pub(crate) fn has_wip_row(&self) -> bool {
        self.state
            .snapshot
            .as_ref()
            .is_some_and(|s| !s.status.is_empty())
    }
    fn history_row(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let wip_rows = usize::from(self.has_wip_row());
        let height = self.commit_row_height();
        let graph_scrolled = self.graph_scroll.scrolled();
        let graph_max = self.graph_scroll.max();
        if wip_rows == 1 && index == 0 {
            // Cap the editor to the message cell so it shrinks before the badges
            // are cut off. Measured from the table's last painted bounds, which
            // track window and pane resizes exactly (unlike the computed pane widths).
            let viewport = self.history_viewport.get();
            let inset = f32::from(HISTORY_CELL_INSET.to_pixels(window.rem_size()));
            let wip_visible_cap = self
                .history_layout
                .visible_message_width(viewport)
                .map(|width| (width - 2. * inset).max(WIP_EDITOR_MIN_WIDTH));
            // The editor starts at the width of its text (or placeholder) and grows
            // as the summary is typed; the cap above and the badges bound it.
            let wip_width = {
                let input = self.wip_summary.read(cx);
                let text = match input.value() {
                    value if value.is_empty() => SharedString::from("// WIP"),
                    value => value,
                };
                let font_size = type_size(cx, TEXT_SIZE_SMALL);
                let run = TextRun {
                    len: text.len(),
                    // The input renders in the theme font, not the window default.
                    font: font(gpui::component::Theme::global(cx).font_family.clone()),
                    color: Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let shaped = window
                    .text_system()
                    .shape_line(text, font_size, &[run], None);
                wip_editor_width(f32::from(shaped.width))
            };
            // The colour of the checked-out commit's branch, which need not be
            // the newest row (another branch can have later commits).
            let wip_color = self
                .state
                .snapshot
                .as_ref()
                .and_then(|s| s.commits.iter().position(|c| c.id == s.head_id))
                .and_then(|i| self.state.graph.rows.get(i))
                .map_or(0, |row| row.color);
            let has_head = self
                .state
                .snapshot
                .as_ref()
                .is_some_and(|s| !s.head_id.is_empty());
            // As on commit rows, the hover fill covers the text columns only.
            let wip_selected = self.state.selected.is_none();
            let wip_hover_fill = self.history_hover.wip && !wip_selected;
            let band_tint = if wip_selected {
                SELECTED_BAND_TINT
            } else {
                BAND_TINT
            };
            return div()
                .id("wip-row")
                .cursor_pointer()
                .w_full()
                .h(px(height))
                .flex()
                .items_center()
                .min_w(px(self.history_table_width()))
                .pr(px(self.history_gutter()))
                .text_role(Role::ActionPrimary)
                .bg(rgb(if self.state.selected.is_none() {
                    semantic(Role::SurfaceWipSelected)
                } else if wip_hover_fill {
                    semantic(Role::InteractionHover)
                } else {
                    semantic(Role::SurfaceCanvas)
                }))
                .on_hover(cx.listener(|v, hovered: &bool, _, cx| {
                    if v.history_hover.wip != *hovered {
                        v.history_hover.wip = *hovered;
                        cx.notify();
                    }
                }))
                .children(self.history_layout.visible().map(|entry| {
                    let cell = match entry.column {
                        HistoryColumn::Graph => (div()
                            .w_full()
                            .h(px(height))
                            .flex_none()
                            .on_scroll(cx.listener(Self::scroll_graph))
                            .child(
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, w, _| {
                                        let scale = w.scale_factor();
                                        let (x, _) = graph_node_x(
                                            bounds,
                                            graph_lane_positions(bounds, scale, graph_scrolled)(0),
                                            scale,
                                        );
                                        let y = bounds.origin.y + px(height / 2.);
                                        let radius = px(WIP_RADIUS);
                                        // Same branch band as the commit rows, in the
                                        // colour of the branch being worked on. Its left
                                        // edge hugs the circle, which stays hollow.
                                        let band_y = bounds.origin.y + px(BAND_INSET_Y);
                                        let band_height =
                                            bounds.size.height - px(2. * BAND_INSET_Y);
                                        let band_right = bounds.right();
                                        paint_hugging_band(
                                            w,
                                            point(x, y),
                                            radius,
                                            band_y,
                                            band_y + band_height,
                                            band_right,
                                            branch_tint(wip_color, band_tint),
                                        );
                                        w.paint_quad(fill(
                                            Bounds::new(
                                                point(band_right - px(1.), band_y),
                                                size(px(1.), band_height),
                                            ),
                                            branch_tint(wip_color, 0x70),
                                        ));
                                        if has_head {
                                            paint_dotted_lane(
                                                w,
                                                x,
                                                y + radius,
                                                bounds.bottom(),
                                                lanes()[wip_color % lanes().len()],
                                                px(0.),
                                            );
                                        }
                                        paint_graph_sticky_strips(
                                            w,
                                            bounds,
                                            graph_scrolled,
                                            graph_max,
                                            band_y,
                                            band_y + band_height,
                                            branch_tint(wip_color, band_tint),
                                            branch_tint(wip_color, 0x70),
                                        );
                                        paint_dashed_circle(
                                            w,
                                            point(x, y),
                                            radius,
                                            lanes()[wip_color % lanes().len()],
                                        );
                                        paint_graph_edge_shades(
                                            w,
                                            bounds,
                                            graph_scrolled,
                                            graph_max,
                                        );
                                    },
                                )
                                .size_full(),
                            ))
                        .into_any_element(),
                        HistoryColumn::Message => div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .w_full()
                            .min_w_0()
                            .overflow_hidden()
                            .when_some(wip_visible_cap, |el, cap| el.max_w(px(cap)))
                            // The cap uses the previous frame's viewport. If that
                            // changed (a resize), draw once more with the new size.
                            .child({
                                let measured = self.history_viewport.clone();
                                let used = viewport;
                                canvas(
                                    |_, _, _| (),
                                    move |_, _, w, _| {
                                        if measured.get() != used {
                                            w.refresh();
                                        }
                                    },
                                )
                                .absolute()
                                .size_0()
                            })
                            // Inline summary editor, mirrored with the composer's
                            // Commit summary field (see the draft observers in view.rs).
                            .child(
                                div()
                                    .id("wip-summary")
                                    // Sized to its text; shrinks before the
                                    // flex_none badges when space runs out.
                                    .flex_shrink(1.)
                                    .w(px(wip_width))
                                    .min_w(px(WIP_EDITOR_MIN_WIDTH))
                                    .overflow_hidden()
                                    .child(
                                        Input::new(&self.wip_summary)
                                            .disabled(self.settings_open)
                                            .py_0()
                                            // The input's 10px caret margin supplies
                                            // the rest, matching the 12px left inset.
                                            .pr(px(WIP_EDITOR_PAD_RIGHT))
                                            .h(px((height - WIP_EDITOR_ROW_INSET)
                                                .max(WIP_EDITOR_MIN_HEIGHT)))
                                            .text_size(scaled(TEXT_SIZE_SMALL)),
                                    ),
                            )
                            .when(
                                self.state
                                    .snapshot
                                    .as_ref()
                                    .is_some_and(|s| s.status.is_empty()),
                                |row| {
                                    row.child(
                                        div()
                                            .text_size(scaled(TEXT_SIZE_SMALL))
                                            .text_role(Role::TextMuted)
                                            .child("No changes"),
                                    )
                                },
                            )
                            .when_some(
                                self.state
                                    .snapshot
                                    .as_ref()
                                    .map(|s| s.conflict_count())
                                    .filter(|count| *count > 0),
                                |row, count| {
                                    row.child(Badge::role(
                                        format!(
                                            "{count} {}",
                                            if count == 1 { "conflict" } else { "conflicts" }
                                        ),
                                        Role::StatusConflictFg,
                                    ))
                                },
                            )
                            .children(self.state.snapshot.as_ref().into_iter().flat_map(|s| {
                                s.change_counts()
                                    .into_iter()
                                    .zip([
                                        ("M", "Modified", Role::StatusWarningFg),
                                        ("+", "Added", Role::StatusSuccessFg),
                                        ("−", "Deleted", Role::StatusErrorFg),
                                        ("R", "Renamed", Role::StatusInfoFg),
                                    ])
                                    .filter(|(count, _)| *count > 0)
                                    .map(|(count, (mark, name, role))| {
                                        Badge::role(format!("{mark} {count}"), role)
                                            .fixed_width(44.)
                                            .tooltip(
                                                SharedString::from(format!("wip-{name}")),
                                                format!("{count} {}", name.to_lowercase()),
                                            )
                                    })
                            }))
                            .into_any_element(),
                        _ => div().into_any_element(),
                    };
                    self.history_cell(entry)
                        .when(
                            (wip_hover_fill || wip_selected)
                                && matches!(
                                    entry.column,
                                    HistoryColumn::Graph | HistoryColumn::References
                                ),
                            |cell| cell.bg_role(Role::SurfaceCanvas),
                        )
                        .child(cell)
                        .into_any_element()
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|v, _, w, cx| {
                        if v.state.selected.is_some() {
                            let job = v.state.select(None);
                            v.dispatch(job, w, cx);
                        }
                    }),
                )
                .into_any_element();
        }
        let i = index - wip_rows;
        let Some(snap) = &self.state.snapshot else {
            return div().into_any_element();
        };
        let c = &snap.commits[i];
        let id = c.id.clone();
        let target = c.id.clone();
        let selected = self.state.selected.as_ref() == Some(&c.id);
        let matched = c.matches_search(&self.search_query);
        let row = self.state.graph.rows[i].clone();
        let outgoing = self.state.graph.edges_at(i);
        let next_lane = self.state.graph.rows.get(i + 1).map(|row| row.lane);
        let incoming = i
            .checked_sub(1)
            .map(|p| self.state.graph.edges_at(p))
            .unwrap_or_default();
        let (link_above, link_below) = if wip_rows == 1 && !snap.head_id.is_empty() {
            let head_row = snap.commits.iter().position(|c| c.id == snap.head_id);
            gitcrab_core::graph::head_link_at(head_row, i)
        } else {
            (false, false)
        };

        let decorations = resolve_decorations(snap, &c.refs);
        let hover = &self.history_hover;
        // An undecorated commit that is hovered or selected shows the nearest
        // branch containing it as a faded "ghost" label.
        let row_hovered = hover.row_hovered(i);
        let ghost = decorations.is_empty() && (row_hovered || selected);
        let (labels, ghost) = match ghost.then(|| self.state.graph.ghost_source(i)).flatten() {
            Some(source) => (resolve_decorations(snap, &snap.commits[source].refs), true),
            None => (decorations.clone(), false),
        };
        let primary_index = primary_decoration(&labels);
        let others = labels.len().saturating_sub(1);
        let refs_open = primary_index.is_some() && hover.refs_open(i);
        let mut expanded = primary_index.filter(|_| refs_open).map(|primary| {
            self.expanded_refs(i, &labels, primary, &c.id, row.color, ghost, window, cx)
        });
        let primary = primary_index.map(|index| labels[index].clone());
        let checked_out = !ghost
            && primary
                .as_ref()
                .is_some_and(|d| d.kind == DecorationKind::HeadBranch);
        let dim = hover.dim(i);
        // Hover fills only the text columns of a row; the graph
        // and references columns keep the canvas behind them.
        let hover_fill = row_hovered && !selected;
        let band_tint = if selected {
            SELECTED_BAND_TINT
        } else {
            BAND_TINT
        };
        // Text colours; a search dims the rows it leaves out.
        let ink = RowText::new(selected, row_hovered, if matched { dim } else { 1. });
        let columns = &self.history_layout;
        let reference_side = if decorations.is_empty() {
            None
        } else {
            columns
                .visible()
                .zip(columns.visible().skip(1))
                .find_map(|(left, right)| match (left.column, right.column) {
                    (HistoryColumn::References, HistoryColumn::Graph) => Some(ReferenceSide::Left),
                    (HistoryColumn::Graph, HistoryColumn::References) => Some(ReferenceSide::Right),
                    _ => None,
                })
        };
        let color = row.color;
        let stash = c.stash;
        let junction = if link_above && row.lane == 0 {
            let mut joined = incoming.clone();
            joined.push(gitcrab_core::graph::Edge {
                from_lane: 0,
                to_lane: 0,
                color: 0,
                merge: false,
            });
            row.is_junction(&joined)
        } else {
            row.is_junction(&incoming)
        };
        let mut el = div()
            .id(("commit", i))
            .w_full()
            .h(px(height))
            .flex()
            .items_center()
            .min_w(px(self.history_table_width()))
            .pr(px(self.history_gutter()))
            .cursor_pointer()
            .overflow_hidden()
            .when(selected, |e| e.bg_role(Role::InteractionSelected))
            .when(hover_fill, |e| e.bg_role(Role::InteractionHover))
            .text_color(ink.title)
            .on_hover(
                cx.listener(move |v, hovered: &bool, _, cx| v.hover_history_row(i, *hovered, cx)),
            )
            .children(self.history_layout.visible().map(|entry| {
                let row = row.clone();
                let incoming = incoming.clone();
                let cell = match entry.column {
                    HistoryColumn::References => (div()
                        .id(("refs", i))
                        .w_full()
                        .flex_none()
                        .flex()
                        .items_center()
                        .overflow_hidden()
                        .text_size(scaled(TEXT_SIZE_SMALL))
                        .text_role(Role::TextDefault)
                        .on_hover(cx.listener(move |v, hovered: &bool, _, cx| {
                            v.hover_refs(i, RefsPart::Cell, *hovered, cx)
                        }))
                        // The link runs from the label's edge to the graph,
                        // so the label can stay the size of its content.
                        .when(matches!(reference_side, Some(ReferenceSide::Right)), |e| {
                            e.child(reference_link_filler(color, checked_out))
                        })
                        // The expanded list starts where the inline label does.
                        .children(expanded.take())
                        .when_some(primary.clone(), |e, primary| {
                            e.child(
                                self.ref_pill(
                                    ("ref-detail", i),
                                    primary,
                                    c.id.clone(),
                                    color,
                                    true,
                                    cx,
                                )
                                .flex_shrink(1.)
                                .min_w_0()
                                .overflow_hidden()
                                .when(ghost, |pill| pill.opacity(GHOST_OPACITY).cursor_default())
                                // The expanded list shows it in its place. Still
                                // painted, so it reports the pointer leaving.
                                .when(refs_open, |pill| pill.opacity(0.))
                                .on_hover(cx.listener(
                                    move |v, hovered: &bool, _, cx| {
                                        v.hover_refs(i, RefsPart::InlineLabel, *hovered, cx)
                                    },
                                )),
                            )
                        })
                        .when(others > 0, |e| {
                            e.child(
                                div()
                                    .flex_none()
                                    .flex()
                                    .items_center()
                                    .ml_1()
                                    .h(scaled(REF_BADGE_HEIGHT))
                                    .px(px(4.))
                                    .rounded(px(4.))
                                    .bg(branch_tint(color, REF_TINT))
                                    .when(ghost, |count| count.opacity(GHOST_OPACITY))
                                    // Hide the count while the list is open.
                                    .when(refs_open, |count| count.invisible())
                                    .child(
                                        div()
                                            .relative()
                                            .top(px(-1.))
                                            .text_size(scaled(REF_BADGE_FONT_SIZE))
                                            .child(format!("+{others}")),
                                    ),
                            )
                        })
                        .when(matches!(reference_side, Some(ReferenceSide::Left)), |e| {
                            e.child(reference_link_filler(color, checked_out))
                        }))
                    .into_any_element(),
                    HistoryColumn::Graph => {
                        let outgoing = outgoing.clone();
                        (div()
                            .relative()
                            .w_full()
                            .h(px(height))
                            .flex_none()
                            .on_scroll(cx.listener(Self::scroll_graph))
                            .child(
                                canvas(
                                    move |_, _, _| (),
                                    move |bounds, _, w, cx| {
                                        let origin = bounds.origin;
                                        let scale = w.scale_factor();
                                        let x = graph_lane_positions(bounds, scale, graph_scrolled);
                                        let center_y = bounds.center().y;
                                        let y = snap_graph_stroke_center(center_y, scale);
                                        // The node, its band and its label link
                                        // stay in view; the lanes are cut off.
                                        let (center_x, pinned) =
                                            graph_node_x(bounds, x(row.lane), scale);
                                        let node_color = if pinned {
                                            tint(
                                                lanes()[color % lanes().len()],
                                                GRAPH_PINNED_NODE_ALPHA,
                                            )
                                        } else {
                                            rgb(lanes()[color % lanes().len()])
                                        };
                                        let band_y = origin.y + px(BAND_INSET_Y);
                                        let band_height =
                                            bounds.size.height - px(2. * BAND_INSET_Y);
                                        let band_right = bounds.right();
                                        let band_left = center_x;
                                        w.paint_quad(fill(
                                            Bounds::new(
                                                point(band_left, band_y),
                                                size(band_right - band_left, band_height),
                                            ),
                                            branch_tint(color, band_tint),
                                        ));
                                        w.paint_quad(fill(
                                            Bounds::new(
                                                point(band_right - px(1.), band_y),
                                                size(px(1.), band_height),
                                            ),
                                            branch_tint(color, 0x70),
                                        ));
                                        if let Some(side) = reference_side {
                                            let edge = match side {
                                                ReferenceSide::Left => bounds.left(),
                                                ReferenceSide::Right => bounds.right(),
                                            };
                                            paint_reference_link(
                                                w,
                                                edge,
                                                center_x,
                                                center_y,
                                                color,
                                                checked_out,
                                            );
                                        }
                                        let edges = incoming
                                            .iter()
                                            .map(|edge| (edge, true, Some(row.lane)))
                                            .chain(
                                                outgoing
                                                    .iter()
                                                    .map(|edge| (edge, false, next_lane)),
                                            );
                                        let mut corners = Vec::new();
                                        for (edge, above, lower_lane) in edges {
                                            let from = x(edge.from_lane);
                                            let to = x(edge.to_lane);
                                            let edge_color = lanes()[edge.color % lanes().len()];
                                            let turns_below =
                                                !edge.merge && lower_lane == Some(edge.to_lane);
                                            let (top, bottom) = if above {
                                                (bounds.top(), y)
                                            } else {
                                                (y, bounds.bottom())
                                            };
                                            if from != to && above == turns_below {
                                                let (junction_x, lane_x, row_edge) = if above {
                                                    (to, from, top)
                                                } else {
                                                    (from, to, bottom)
                                                };
                                                let span = edge.from_lane.abs_diff(edge.to_lane);
                                                corners.push((
                                                    span, junction_x, lane_x, row_edge, edge_color,
                                                ));
                                            } else {
                                                let lane_x = if turns_below { from } else { to };
                                                paint_vertical_lane(
                                                    w, lane_x, top, bottom, edge_color,
                                                );
                                            }
                                        }
                                        // Dashes continue from the WIP circle's edge.
                                        let travelled =
                                            height / 2. - WIP_RADIUS + i as f32 * height;
                                        let link_color = lanes()[0];
                                        if link_above {
                                            paint_dotted_lane(
                                                w,
                                                x(0),
                                                bounds.top(),
                                                y,
                                                link_color,
                                                px(travelled),
                                            );
                                        }
                                        if link_below {
                                            paint_dotted_lane(
                                                w,
                                                x(0),
                                                y,
                                                bounds.bottom(),
                                                link_color,
                                                px(travelled) + (y - bounds.top()),
                                            );
                                        }
                                        // Paint longer runs first so each nearer branch keeps its color at the turn.
                                        corners.sort_by_key(|corner| std::cmp::Reverse(corner.0));
                                        for (_, junction_x, lane_x, row_edge, edge_color) in corners
                                        {
                                            paint_lane_corner(
                                                w,
                                                point(junction_x, y),
                                                lane_x,
                                                row_edge,
                                                edge_color,
                                            );
                                        }
                                        // The edge strips sit between the lanes and the
                                        // node, so lanes pass under them and nodes stay on top.
                                        paint_graph_sticky_strips(
                                            w,
                                            bounds,
                                            graph_scrolled,
                                            graph_max,
                                            band_y,
                                            band_y + band_height,
                                            branch_tint(color, band_tint),
                                            branch_tint(color, 0x70),
                                        );
                                        if stash {
                                            paint_stash_node(
                                                w,
                                                point(center_x, y),
                                                px(WIP_RADIUS),
                                                lanes()[color % lanes().len()],
                                                cx,
                                            );
                                        } else if junction {
                                            let radius = px(6.);
                                            let mut diamond = PathBuilder::fill();
                                            diamond.move_to(point(center_x, y - radius));
                                            diamond.line_to(point(center_x + radius, y));
                                            diamond.line_to(point(center_x, y + radius));
                                            diamond.line_to(point(center_x - radius, y));
                                            diamond.close();
                                            if let Ok(path) = diamond.build() {
                                                w.paint_path(path, node_color);
                                            }
                                        } else {
                                            let radius = 4.;
                                            w.paint_quad(
                                                fill(
                                                    Bounds::new(
                                                        point(
                                                            center_x - px(radius),
                                                            y - px(radius),
                                                        ),
                                                        size(px(radius * 2.), px(radius * 2.)),
                                                    ),
                                                    node_color,
                                                )
                                                .corner_radii(px(radius)),
                                            );
                                        }
                                        paint_graph_edge_shades(
                                            w,
                                            bounds,
                                            graph_scrolled,
                                            graph_max,
                                        );
                                    },
                                )
                                .size_full(),
                            ))
                        .into_any_element()
                    }
                    HistoryColumn::Message => {
                        let subject = c.subject.clone();
                        // The body follows the subject in a quieter color, as a
                        // hint of what the rest of the message says.
                        let text = if c.body_preview.is_empty() {
                            SharedString::from(c.subject.clone())
                        } else {
                            SharedString::from(format!("{}  {}", c.subject, c.body_preview))
                        };
                        let preview = HighlightStyle {
                            // Faded, not recoloured: a highlight colour blends over the
                            // title colour and keeps its alpha, so it would not dim.
                            fade_out: Some(1. - BODY_FACTOR),
                            ..Default::default()
                        };
                        let highlights = (!c.body_preview.is_empty())
                            .then(|| (c.subject.len()..text.len(), preview));
                        (div()
                            .id(("subject", i))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .py(px(3.))
                            .child(StyledText::new(text).with_highlights(highlights))
                            // The full subject stays available when the column clips it.
                            .tooltip(move |w, cx| Tooltip::new(subject.clone()).build(w, cx)))
                        .into_any_element()
                    }
                    HistoryColumn::Sha => (div()
                        .w_full()
                        .flex_none()
                        .whitespace_nowrap()
                        .truncate()
                        .text_size(scaled(TEXT_SIZE_SMALL))
                        .font_family(CODE_FONT)
                        .text_color(ink.meta)
                        .child(c.id[..8.min(c.id.len())].to_string()))
                    .into_any_element(),
                    HistoryColumn::Author => div()
                        .truncate()
                        .text_color(ink.meta)
                        .child(c.author.clone())
                        .into_any_element(),
                    HistoryColumn::Date => div()
                        .truncate()
                        .text_size(scaled(TEXT_SIZE_SMALL))
                        .text_color(ink.meta)
                        .child(display_date(&c.date))
                        .into_any_element(),
                };
                self.history_cell(entry)
                    // Hover and selection fill only the text columns; a selected row's graph band shows it instead.
                    .when(
                        (hover_fill || selected)
                            && matches!(
                                entry.column,
                                HistoryColumn::Graph | HistoryColumn::References
                            ),
                        |cell| cell.bg_role(Role::SurfaceCanvas),
                    )
                    .when(
                        entry.column == HistoryColumn::References
                            && decorations.is_empty()
                            && !selected,
                        |cell| cell.block_mouse_except_scroll().cursor_default(),
                    )
                    .when_some(
                        reference_side.filter(|_| entry.column == HistoryColumn::References),
                        |frame, side| {
                            // This segment spans the reference cell's padding and
                            // meets the graph segment at their shared boundary.
                            frame.relative().child(
                                canvas(
                                    |_, _, _| (),
                                    move |bounds, _, w, _| {
                                        let inset = w.rem_size() * HISTORY_CELL_INSET.0;
                                        let (edge, badge) = match side {
                                            ReferenceSide::Left => {
                                                (bounds.right(), bounds.right() - inset)
                                            }
                                            ReferenceSide::Right => {
                                                (bounds.left(), bounds.left() + inset)
                                            }
                                        };
                                        paint_reference_link(
                                            w,
                                            badge,
                                            edge,
                                            bounds.center().y,
                                            color,
                                            checked_out,
                                        );
                                    },
                                )
                                .absolute()
                                .inset_0(),
                            )
                        },
                    )
                    .child(cell)
                    .into_any_element()
            }))
            .on_click(cx.listener(move |v, _, w, cx| v.activate_commit(id.clone(), w, cx)));
        if c.id == snap.head_id {
            // Paint the marker over the row so it cannot shift the lane origin.
            el = el.relative().child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left_0()
                    .w(px(2.))
                    .bg_role(Role::ActionPrimary),
            );
        }
        let view = cx.entity();
        let parents = c.parents.len();
        let stash = c.stash;
        el.context_menu(move |mut menu, w, cx| {
            let sha = target.clone();
            menu = menu.item(components::menu_item("Copy commit SHA", true).on_click(
                move |_, _, cx| cx.write_to_clipboard(ClipboardItem::new_string(sha.clone())),
            ));
            if stash {
                return menu;
            }
            let commands = view.update(cx, |v, cx| v.command_context(w, cx));
            menu = menu.separator();
            let create_view = view.clone();
            let start = target.clone();
            menu = menu
                .item(components::menu_item("Create branch here…", true).on_click(
                    move |_, w, cx| {
                        create_view.update(cx, |v, cx| v.prompt_branch(Some(start.clone()), w, cx))
                    },
                ))
                .separator();
            // The shared command model decides availability, including merge commits.
            for command in [
                Command::Checkout,
                Command::Merge,
                Command::CherryPick,
                Command::Revert,
            ] {
                let view = view.clone();
                let target = target.clone();
                let availability = command.availability_for_commit(&commands, parents);
                let short = short_id(&target);
                let label = match availability.reason() {
                    Some(reason) => format!("{} {short} — {reason}", command.label()),
                    None => format!("{} {short}", command.label()),
                };
                menu = menu.item(
                    components::menu_item(label, availability.enabled()).on_click(
                        move |_, w, cx| {
                            view.update(cx, |v, cx| v.run_command(command, target.clone(), w, cx))
                        },
                    ),
                );
            }
            menu
        })
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{wip_editor_width, WIP_EDITOR_DEFAULT_WIDTH};

    #[test]
    fn wip_editor_fits_its_text_without_scrolling() {
        assert_eq!(wip_editor_width(40.), WIP_EDITOR_DEFAULT_WIDTH);
        // 12px left + 2px right padding, 2px border, the input's 10px caret
        // margin and 1px slack; any less and the first glyph gets clipped.
        assert_eq!(wip_editor_width(300.2), 301. + 27.);
    }
}
