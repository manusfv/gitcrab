use gpui::{prelude::*, *};
use std::time::{Duration, Instant};

const SLIDE: Duration = Duration::from_millis(120);

/// Pointer-anchored reorder with fixed slots and animated neighboring items.
pub struct RailDrag<T> {
    pub dragged: T,
    pub released: bool,
    pub height: f32,
    items: Vec<(T, f32)>,
    original: Vec<T>,
    origin: f32,
    gap: f32,
    grab: f32,
    left: f32,
    shifts: Vec<(T, f32, Instant)>,
}

impl<T: Copy + Eq> RailDrag<T> {
    /// Starts a drag from measured item bounds in display order.
    /// Returns `None` if the dragged item is missing or a slot has no width.
    pub fn new(
        bounds: &[(T, Bounds<Pixels>)],
        dragged: T,
        pointer: Pixels,
        gap: f32,
    ) -> Option<Self> {
        let source = bounds.iter().find(|(id, _)| *id == dragged)?.1;
        if bounds.iter().any(|(_, b)| b.size.width <= px(0.)) {
            return None;
        }
        Some(Self {
            dragged,
            released: false,
            height: f32::from(source.size.height),
            items: bounds
                .iter()
                .map(|(id, b)| (*id, f32::from(b.size.width)))
                .collect(),
            original: bounds.iter().map(|(id, _)| *id).collect(),
            origin: f32::from(bounds.first()?.1.left()),
            gap,
            grab: f32::from(pointer - source.left()),
            left: f32::from(source.left()),
            shifts: vec![],
        })
    }

    pub fn order(&self) -> impl Iterator<Item = T> + '_ {
        self.items.iter().map(|(id, _)| *id)
    }
    pub fn original(&self) -> &[T] {
        &self.original
    }
    pub fn width(&self, id: T) -> f32 {
        self.items
            .iter()
            .find(|(key, _)| *key == id)
            .map_or(0., |(_, w)| *w)
    }
    fn slot(&self, id: T) -> f32 {
        self.origin
            + self
                .items
                .iter()
                .take_while(|(key, _)| *key != id)
                .map(|(_, w)| w + self.gap)
                .sum::<f32>()
    }

    /// Returns the item's horizontal paint offset from its layout slot.
    pub fn offset(&self, id: T, now: Instant) -> f32 {
        if id == self.dragged && !self.released {
            return self.left - self.slot(id);
        }
        self.shifts
            .iter()
            .find(|(key, _, _)| *key == id)
            .map_or(0., |(_, from, start)| {
                let t = (now.saturating_duration_since(*start).as_secs_f32() / SLIDE.as_secs_f32())
                    .min(1.);
                from * (1. - t).powi(3)
            })
    }

    /// Moves the dragged item with the pointer. Returns whether the slot order changed.
    pub fn update(&mut self, pointer: Pixels, now: Instant) -> bool {
        if self.released {
            return false;
        }
        let width = self.width(self.dragged);
        let total = self.items.iter().map(|(_, w)| *w).sum::<f32>()
            + self.gap * self.items.len().saturating_sub(1) as f32;
        self.left = (f32::from(pointer) - self.grab)
            .clamp(self.origin, self.origin + (total - width).max(0.));
        let from = self
            .items
            .iter()
            .position(|(id, _)| *id == self.dragged)
            .unwrap();
        let mut at = from;
        let mut slot = self.slot(self.dragged);
        if self.left > slot {
            while let Some((_, next_width)) = self.items.get(at + 1) {
                if self.left <= slot + self.gap + next_width / 2. {
                    break;
                }
                slot += next_width + self.gap;
                at += 1;
            }
        } else {
            while at > 0 {
                let previous_width = self.items[at - 1].1;
                if self.left >= slot - self.gap - previous_width / 2. {
                    break;
                }
                slot -= previous_width + self.gap;
                at -= 1;
            }
        }
        if at == from {
            return false;
        }
        let visual: Vec<_> = self
            .order()
            .filter(|id| *id != self.dragged)
            .map(|id| (id, self.slot(id) + self.offset(id, now)))
            .collect();
        let dragged = self.items.remove(from);
        self.items.insert(at, dragged);
        self.shifts = visual
            .into_iter()
            .map(|(id, left)| (id, left - self.slot(id), now))
            .collect();
        true
    }

    /// Starts the released item's animation into its final slot.
    pub fn release(&mut self, now: Instant) {
        if self.released {
            return;
        }
        let from = self.offset(self.dragged, now);
        self.shifts.push((self.dragged, from, now));
        self.released = true;
    }

    pub fn animating(&self, now: Instant) -> bool {
        self.shifts.iter().any(|(_, shift, start)| {
            shift.abs() > 0.1 && now.saturating_duration_since(*start) < SLIDE
        })
    }

    /// Reports whether the item was released and all slot animations are done.
    pub fn settled(&self, now: Instant) -> bool {
        self.released && !self.animating(now)
    }
}

/// Paint in the item's layout slot, lifting only the dragged item above its neighbors.
pub fn item<T: Copy + Eq>(
    element: impl IntoElement,
    rail: Option<&RailDrag<T>>,
    id: T,
    now: Instant,
) -> AnyElement {
    let Some(rail) = rail else {
        return element.into_any_element();
    };
    let width = px(rail.width(id));
    if width == px(0.) {
        return element.into_any_element();
    }
    div()
        .relative()
        .flex_none()
        .w(width)
        .h(px(rail.height))
        .child(
            deferred(
                div()
                    .absolute()
                    .top_0()
                    .left(px(rail.offset(id, now)))
                    .w(width)
                    .h_full()
                    .child(element),
            )
            .with_priority(if id == rail.dragged { 2 } else { 1 }),
        )
        .into_any_element()
}

/// Capture mouse release anywhere in the window, including outside the strip.
pub fn end_events<V: 'static>(
    view: WeakEntity<V>,
    finish: fn(&mut V, &MouseUpEvent, &mut Window, &mut Context<V>),
    cancel: fn(&mut V, &mut Window, &mut Context<V>),
) -> impl IntoElement {
    canvas(
        |_, _, _| (),
        move |_, _, window, _| {
            let keys = view.clone();
            window.on_key_event(move |event: &KeyDownEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && event.keystroke.key == "escape" {
                    let _ = keys.update(cx, |view, cx| cancel(view, window, cx));
                }
            });
            let view = view.clone();
            window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                if phase == DispatchPhase::Capture && event.button == MouseButton::Left {
                    let _ = view.update(cx, |view, cx| finish(view, event, window, cx));
                }
            });
        },
    )
    .absolute()
    .size(px(0.))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rail(widths: &[f32], source: usize, grab: f32) -> RailDrag<usize> {
        let mut x = -40.;
        let bounds: Vec<_> = widths
            .iter()
            .enumerate()
            .map(|(id, width)| {
                let b = Bounds::new(point(px(x), px(10.)), size(px(*width), px(28.)));
                x += width + 4.;
                (id, b)
            })
            .collect();
        RailDrag::new(&bounds, source, bounds[source].1.left() + px(grab), 4.).unwrap()
    }
    #[::core::prelude::v1::test]
    fn swaps_during_motion_keeps_grab_point_and_clamps_to_the_rail() {
        let now = Instant::now();
        let mut r = rail(&[100., 100., 100.], 0, 90.);
        assert!(!r.update(px(103.), now));
        assert!(r.update(px(110.), now));
        assert_eq!(r.order().collect::<Vec<_>>(), [1, 0, 2]);
        assert_eq!(r.slot(0) + r.offset(0, now) + 90., 110.);
        assert!(!r.update(px(110.), now)); // No oscillation at the swap threshold.
        r.update(px(10_000.), now);
        assert_eq!(r.order().collect::<Vec<_>>(), [1, 2, 0]);
        assert_eq!(r.left, 168.);
        r.update(px(-10_000.), now);
        assert_eq!(r.order().collect::<Vec<_>>(), [0, 1, 2]);
        assert_eq!(r.left, -40.);
    }
    #[::core::prelude::v1::test]
    fn unequal_width_neighbors_slide_continuously_and_release_settles() {
        let now = Instant::now();
        let mut r = rail(&[300., 80., 160.], 0, 290.);
        assert!(r.update(px(310.), now));
        let neighbor_before = r.slot(1) + r.offset(1, now);
        assert_eq!(neighbor_before, 264.);
        let middle = now + Duration::from_millis(60);
        assert!(r.slot(1) + r.offset(1, middle) < neighbor_before);
        r.update(px(250.), middle);
        assert_eq!(r.order().collect::<Vec<_>>(), [0, 1, 2]);
        r.release(middle);
        assert!(!r.settled(middle));
        assert!(r.settled(middle + SLIDE));
        assert_eq!(r.offset(0, middle + SLIDE), 0.);
        assert_eq!(r.original(), [0, 1, 2]);
    }
}
