//! A card being dragged over the board, and the gap that opens where it
//! would land.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use iced::widget::Id;
use iced::{Point, Rectangle};
use taskboard_core::{Status, Ulid};

use crate::app::drop_index;

/// How long a gap takes to open or close.
const GAP_DURATION: Duration = Duration::from_millis(150);

/// Space between cards in a column.
pub const CARD_GAP: f32 = 8.0;

/// A place a card can land: in a column, before a card or at its end.
pub type Slot = (Status, Option<Ulid>);

/// A card being dragged.
pub struct Drag {
    pub id: Ulid,
    /// The dragged card's height, which the gap's box opens to.
    pub height: f32,
    /// The columns, card lists and cards measured to place the gap, again
    /// whenever the board scrolls.
    pub zones: Vec<Id>,
    /// Where the columns were when last measured, to find the one under the
    /// pointer.
    pub columns: Vec<(Status, Rectangle)>,
    /// Each expanded column's cards when last measured, leaving out the
    /// dragged one: their ids and where their middles would be with no gap
    /// open. Empty until measured.
    pub cards: HashMap<Status, Vec<(Ulid, f32)>>,
    /// Where the card would land if dropped now.
    pub target: Option<Slot>,
    /// Where the pointer was last dragged to.
    pub pointer: Point,
    /// Where the card came from, which Esc sends it back to.
    pub origin: Slot,
    /// How far each gap is open, from 0 to 1, as they move toward
    /// [`Drag::target`].
    pub gaps: Vec<(Slot, f32)>,
    /// When the gaps last moved, while they're moving.
    last_frame: Option<Instant>,
    /// Esc was pressed; the drop does nothing.
    pub cancelled: bool,
}

impl Drag {
    /// A drag of `id` from `origin`. Its gap starts fully open, so it takes
    /// the place of the card that just left.
    pub fn new(id: Ulid, height: f32, origin: Slot, zones: Vec<Id>) -> Drag {
        Drag {
            id,
            height,
            zones,
            columns: Vec::new(),
            cards: HashMap::new(),
            target: Some(origin),
            pointer: Point::ORIGIN,
            origin,
            gaps: vec![(origin, 1.0)],
            last_frame: None,
            cancelled: false,
        }
    }

    /// The column under `point`, if any.
    pub fn column_at(&self, point: Point) -> Option<Status> {
        column_at(&self.columns, point)
    }

    /// Points the gap at where the card would land with the pointer at
    /// `point`, once the board's been measured.
    pub fn hover(&mut self, point: Point) {
        self.pointer = point;
        if self.cancelled || self.columns.is_empty() {
            return;
        }
        self.target = self.column_at(point).map(|status| {
            // A collapsed column wasn't measured: drops land at its bottom.
            let before = self.cards.get(&status).and_then(|cards| {
                let mids: Vec<f32> = cards.iter().map(|(_, mid)| *mid).collect();
                cards.get(drop_index(&mids, point.y)).map(|(id, _)| *id)
            });
            (status, before)
        });
    }

    /// Esc sends the card back where it came from.
    pub fn cancel(&mut self) {
        self.cancelled = true;
        self.target = Some(self.origin);
    }

    /// How far open the gap at `slot` is now, from 0 to 1, eased so it
    /// slows as it settles.
    pub fn gap(&self, slot: Slot) -> f32 {
        let open = self
            .gaps
            .iter()
            .find(|(s, _)| *s == slot)
            .map_or(0.0, |(_, open)| *open);
        ease(open)
    }

    /// Whether any gap is still opening or closing.
    pub fn animating(&self) -> bool {
        let open_target = |slot: &Slot| Some(*slot) == self.target;
        self.target
            .is_some_and(|t| !self.gaps.iter().any(|(s, _)| *s == t))
            || self
                .gaps
                .iter()
                .any(|(s, open)| *open != if open_target(s) { 1.0 } else { 0.0 })
    }

    /// Moves the gaps on to where they should be at `now`, a frame time.
    pub fn frame(&mut self, now: Instant) {
        let elapsed = self.last_frame.map_or(Duration::ZERO, |at| now - at);
        self.step(elapsed);
        self.last_frame = self.animating().then_some(now);
    }

    /// Moves the gaps `elapsed` further toward the target: it opens while
    /// the rest close.
    pub fn step(&mut self, elapsed: Duration) {
        let delta = elapsed.as_secs_f32() / GAP_DURATION.as_secs_f32();
        if let Some(target) = self.target
            && !self.gaps.iter().any(|(s, _)| *s == target)
        {
            self.gaps.push((target, 0.0));
        }
        for (slot, open) in &mut self.gaps {
            *open = if Some(*slot) == self.target {
                (*open + delta).min(1.0)
            } else {
                (*open - delta).max(0.0)
            };
        }
        let target = self.target;
        self.gaps
            .retain(|(slot, open)| *open > 0.0 || Some(*slot) == target);
    }
}

/// The column whose bounds contain `point`, if any.
pub fn column_at(columns: &[(Status, Rectangle)], point: Point) -> Option<Status> {
    columns
        .iter()
        .find(|(_, bounds)| bounds.contains(point))
        .map(|(status, _)| *status)
}

/// Where the middles of cards `heights` tall would be if stacked from `top`
/// with no gap open.
pub fn stacked_mids(top: f32, heights: &[f32]) -> Vec<f32> {
    let mut y = top;
    heights
        .iter()
        .map(|h| {
            let mid = y + h / 2.0;
            y += h + CARD_GAP;
            mid
        })
        .collect()
}

/// Smoothstep, so gaps start and settle gently.
fn ease(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids() -> (Ulid, Ulid, Ulid) {
        (Ulid::generate(), Ulid::generate(), Ulid::generate())
    }

    #[test]
    fn stacked_mids_skip_card_gaps() {
        assert_eq!(stacked_mids(100.0, &[40.0, 20.0]), vec![120.0, 158.0]);
        assert!(stacked_mids(0.0, &[]).is_empty());
    }

    #[test]
    fn hover_targets_slot_before_card_below_pointer() {
        let (dragged, a, b) = ids();
        let mut drag = Drag::new(dragged, 40.0, (Status::Open, None), Vec::new());
        drag.columns = vec![
            (
                Status::Open,
                Rectangle::new(Point::new(0.0, 0.0), [100.0, 500.0].into()),
            ),
            (
                Status::Backlog,
                Rectangle::new(Point::new(110.0, 0.0), [34.0, 500.0].into()),
            ),
        ];
        drag.cards.insert(Status::Open, vec![(a, 50.0), (b, 100.0)]);

        drag.hover(Point::new(50.0, 20.0));
        assert_eq!(drag.target, Some((Status::Open, Some(a))));
        drag.hover(Point::new(50.0, 70.0));
        assert_eq!(drag.target, Some((Status::Open, Some(b))));
        drag.hover(Point::new(50.0, 300.0));
        assert_eq!(drag.target, Some((Status::Open, None)));
        // Backlog is collapsed, so it has no measured cards.
        drag.hover(Point::new(120.0, 20.0));
        assert_eq!(drag.target, Some((Status::Backlog, None)));
        drag.hover(Point::new(105.0, 20.0));
        assert_eq!(drag.target, None);
    }

    #[test]
    fn hover_waits_for_measurement_and_ignores_cancelled() {
        let (dragged, a, _) = ids();
        let origin = (Status::Open, Some(a));
        let mut drag = Drag::new(dragged, 40.0, origin, Vec::new());
        drag.hover(Point::new(1000.0, 1000.0));
        assert_eq!(drag.target, Some(origin));

        drag.columns = vec![(
            Status::Open,
            Rectangle::new(Point::ORIGIN, [100.0, 100.0].into()),
        )];
        drag.hover(Point::new(1000.0, 1000.0));
        assert_eq!(drag.target, None);
        drag.cancel();
        drag.hover(Point::new(50.0, 50.0));
        assert_eq!(drag.target, Some(origin));
    }

    #[test]
    fn gaps_open_toward_target_and_close_elsewhere() {
        let (dragged, a, b) = ids();
        let (from, to) = ((Status::Open, Some(a)), (Status::Open, Some(b)));
        let mut drag = Drag::new(dragged, 40.0, from, Vec::new());
        assert_eq!(drag.gap(from), 1.0);
        assert!(!drag.animating());

        drag.target = Some(to);
        assert!(drag.animating());
        drag.step(GAP_DURATION / 2);
        assert_eq!(drag.gap(from), 0.5);
        assert_eq!(drag.gap(to), 0.5);
        drag.step(GAP_DURATION);
        assert_eq!(drag.gap(from), 0.0);
        assert_eq!(drag.gap(to), 1.0);
        assert_eq!(drag.gaps, vec![(to, 1.0)]);
        assert!(!drag.animating());

        // Off the board, every gap closes.
        drag.target = None;
        drag.step(GAP_DURATION * 2);
        assert!(drag.gaps.is_empty());
        assert!(!drag.animating());
    }
}
