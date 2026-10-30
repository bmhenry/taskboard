//! Finds where containers are on screen, for working out where a dragged
//! card was dropped.

use iced::advanced::widget::operation::{Outcome, Scrollable};
use iced::advanced::widget::{Id, Operation, operate};
use iced::{Rectangle, Task, Vector};

/// The on-screen bounds of every container whose id is in `ids`, in the
/// order they're laid out, accounting for any scrolling they're inside.
pub fn find(ids: Vec<Id>) -> Task<Vec<(Id, Rectangle)>> {
    operate(FindZones {
        ids,
        found: Vec::new(),
        offset: Vector::ZERO,
        pending: Vector::ZERO,
    })
}

struct FindZones {
    ids: Vec<Id>,
    found: Vec<(Id, Rectangle)>,
    /// How far everything at the current depth is scrolled.
    offset: Vector,
    /// A scrollable's offset, applied to the children it traverses next.
    pending: Vector,
}

impl Operation<Vec<(Id, Rectangle)>> for FindZones {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation<Vec<(Id, Rectangle)>>)) {
        // Widgets call `scrollable` and then `traverse` for its content, so
        // the pending offset belongs to exactly this traversal.
        let scrolled = std::mem::take(&mut self.pending);
        self.offset += scrolled;
        operate(self);
        self.offset -= scrolled;
    }

    fn container(&mut self, id: Option<&Id>, bounds: Rectangle) {
        if let Some(id) = id
            && self.ids.contains(id)
        {
            self.found.push((id.clone(), bounds - self.offset));
        }
    }

    fn scrollable(
        &mut self,
        _id: Option<&Id>,
        _bounds: Rectangle,
        _content_bounds: Rectangle,
        translation: Vector,
        _state: &mut dyn Scrollable,
    ) {
        self.pending = translation;
    }

    fn finish(&self) -> Outcome<Vec<(Id, Rectangle)>> {
        Outcome::Some(self.found.clone())
    }
}
