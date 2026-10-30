//! A dropdown: content floating just below (or above) the widget it hangs
//! from, kept inside the window, that asks to close when the mouse is
//! pressed anywhere else.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{self, Tree, Widget};
use iced::advanced::{Clipboard, Shell, overlay, renderer};
use iced::{Element, Event, Length, Point, Rectangle, Renderer, Size, Theme, Vector, mouse};

/// Space between the anchor and the popup, and kept clear at window edges.
const GAP: f32 = 4.0;

pub struct Popup<'a, Message> {
    anchor: Element<'a, Message>,
    content: Option<Element<'a, Message>>,
    on_dismiss: Option<Message>,
}

/// Hangs `content` from `anchor` while it's `Some`.
pub fn popup<'a, Message>(
    anchor: impl Into<Element<'a, Message>>,
    content: Option<Element<'a, Message>>,
) -> Popup<'a, Message> {
    Popup {
        anchor: anchor.into(),
        content,
        on_dismiss: None,
    }
}

impl<Message> Popup<'_, Message> {
    /// Sent when the mouse is pressed outside both the popup and its anchor.
    /// The anchor is left out so its own click handler can toggle the popup.
    pub fn on_dismiss(mut self, message: Message) -> Self {
        self.on_dismiss = Some(message);
        self
    }

    fn elements(&self) -> Vec<&dyn Widget<Message, Theme, Renderer>> {
        std::iter::once(self.anchor.as_widget())
            .chain(self.content.as_ref().map(|c| c.as_widget()))
            .collect()
    }
}

impl<Message: Clone> Widget<Message, Theme, Renderer> for Popup<'_, Message> {
    fn size(&self) -> Size<Length> {
        self.anchor.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.anchor.as_widget().size_hint()
    }

    fn children(&self) -> Vec<Tree> {
        self.elements().into_iter().map(Tree::new).collect()
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(&self.elements());
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.anchor
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.anchor.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.anchor
            .as_widget_mut()
            .operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.anchor.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.anchor.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let mut children = tree.children.iter_mut();
        let anchor = self.anchor.as_widget_mut().overlay(
            children.next()?,
            layout,
            renderer,
            viewport,
            translation,
        );
        let popup = match (&mut self.content, children.next()) {
            (Some(content), Some(tree)) => Some(overlay::Element::new(Box::new(Overlay {
                content,
                tree,
                anchor: layout.bounds() + translation,
                on_dismiss: self.on_dismiss.clone(),
            }))),
            _ => None,
        };
        let all: Vec<_> = anchor.into_iter().chain(popup).collect();
        (!all.is_empty()).then(|| overlay::Group::with_children(all).overlay())
    }
}

impl<'a, Message: Clone + 'a> From<Popup<'a, Message>> for Element<'a, Message> {
    fn from(popup: Popup<'a, Message>) -> Self {
        Element::new(popup)
    }
}

struct Overlay<'a, 'b, Message> {
    content: &'b mut Element<'a, Message>,
    tree: &'b mut Tree,
    anchor: Rectangle,
    on_dismiss: Option<Message>,
}

impl<Message: Clone> overlay::Overlay<Message, Theme, Renderer> for Overlay<'_, '_, Message> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let node = self.content.as_widget_mut().layout(
            self.tree,
            renderer,
            &layout::Limits::new(Size::ZERO, bounds),
        );
        let at = popup_position(self.anchor, node.size(), bounds);
        node.move_to(at)
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.content.as_widget().draw(
            self.tree,
            renderer,
            theme,
            style,
            layout,
            cursor,
            &layout.bounds(),
        );
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn widget::Operation,
    ) {
        self.content
            .as_widget_mut()
            .operate(self.tree, layout, renderer, operation);
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        if let Event::Mouse(mouse::Event::ButtonPressed(_)) = event
            && !cursor.is_over(layout.bounds())
            && !cursor.is_over(self.anchor)
            && let Some(message) = &self.on_dismiss
        {
            shell.publish(message.clone());
        }
        self.content.as_widget_mut().update(
            self.tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &layout.bounds(),
        );
        // Keep presses on the popup from reaching whatever is beneath it.
        if let Event::Mouse(mouse::Event::ButtonPressed(_)) = event
            && cursor.is_over(layout.bounds())
        {
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            self.tree,
            layout,
            cursor,
            &layout.bounds(),
            renderer,
        )
    }
}

/// Where a popup of `size` goes: below `anchor`, left-aligned with it, unless
/// it would run off the bottom and there's more room above. It's shifted left
/// to stay inside a window of `window` size.
pub fn popup_position(anchor: Rectangle, size: Size, window: Size) -> Point {
    let below = window.height - (anchor.y + anchor.height);
    let y = if below < size.height + 2.0 * GAP && anchor.y > below {
        anchor.y - GAP - size.height
    } else {
        anchor.y + anchor.height + GAP
    };
    let x = anchor.x.min(window.width - GAP - size.width).max(GAP);
    Point::new(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Size = Size::new(800.0, 600.0);

    #[test]
    fn opens_below_and_shifts_left_at_the_right_edge() {
        let size = Size::new(300.0, 200.0);
        let anchor = Rectangle::new(Point::new(100.0, 50.0), Size::new(80.0, 28.0));
        assert_eq!(
            popup_position(anchor, size, WINDOW),
            Point::new(100.0, 82.0)
        );
        let anchor = Rectangle::new(Point::new(700.0, 50.0), Size::new(80.0, 28.0));
        assert_eq!(popup_position(anchor, size, WINDOW).x, 800.0 - GAP - 300.0);
    }

    #[test]
    fn opens_upward_only_when_there_is_more_room_above() {
        let size = Size::new(160.0, 200.0);
        let low = Rectangle::new(Point::new(10.0, 500.0), Size::new(80.0, 28.0));
        assert_eq!(popup_position(low, size, WINDOW).y, 500.0 - GAP - 200.0);
        let cramped = Rectangle::new(Point::new(10.0, 100.0), Size::new(80.0, 28.0));
        assert_eq!(
            popup_position(cramped, size, Size::new(800.0, 300.0)).y,
            132.0
        );
    }
}
