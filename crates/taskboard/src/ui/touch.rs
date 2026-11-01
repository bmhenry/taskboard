//! Touch input, turned into the mouse input the rest of the UI is built for.
//!
//! On the web, winit hands iced a browser's touch pointers as touch events,
//! not mouse ones. Iced's scrollable only scrolls on touch when nothing inside
//! it takes the press first, but buttons, mouse areas, droppable cards and
//! nested scrollables all do. So a drag on a card, a list row or a column
//! (whose own scrollable takes it) never reached the scrollable meant to move.
//!
//! [`TouchInput`] wraps the whole view and keeps touch events from it,
//! sending what they mean instead:
//! - a tap is a left click where the finger lifted;
//! - a drag is pixel wheel scrolling, locked to the axis it started along, so
//!   the innermost scrollable that can move that way does, with a fling that
//!   carries on after the finger lifts;
//! - a long press is a left button held down, so dragging on from there drags
//!   the card or selects the text under the finger, like a mouse would.
//!
//! Mouse input passes through untouched.

use std::time::Duration;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Shell, Widget, overlay, renderer};
use iced::time::Instant;
use iced::window::RedrawRequest;
use iced::{Element, Event, Length, Point, Rectangle, Size, Vector, mouse, touch, window};

/// How far a finger moves before a press becomes a drag, not a tap.
const SLOP: f32 = 10.0;

/// How long a finger rests before a press becomes a held button.
const LONG_PRESS: Duration = Duration::from_millis(500);

/// A fling slower than this, in pixels a second, doesn't start or stops.
const MIN_FLING: f32 = 60.0;

/// The fastest a drag is taken to move, in pixels a second, so one odd
/// sample can't send a fling flying.
const MAX_SPEED: f32 = 8000.0;

/// Moves closer together than this are added up before measuring speed, as
/// events handled back to back would make any move look instant.
const MIN_SAMPLE: Duration = Duration::from_millis(8);

/// How quickly a fling slows: its speed falls by e every this long.
const FLING_DECAY: f32 = 0.33;

/// A drag that rests this long before lifting doesn't fling.
const FLING_REST: Duration = Duration::from_millis(80);

/// A finger heard nothing from for this long may have lost its lift, so a
/// new finger takes over from it.
const STALE: Duration = Duration::from_secs(1);

/// The axis a drag scrolls along.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Axis {
    X,
    Y,
}

impl Axis {
    /// Which way a move of `delta` goes, by its larger part.
    fn of(delta: Vector) -> Axis {
        if delta.x.abs() > delta.y.abs() {
            Axis::X
        } else {
            Axis::Y
        }
    }

    /// `delta` along just this axis.
    fn along(self, delta: Vector) -> Vector {
        match self {
            Axis::X => Vector::new(delta.x, 0.0),
            Axis::Y => Vector::new(0.0, delta.y),
        }
    }
}

/// `distance` covered in `elapsed`, in pixels a second, no faster than
/// [`MAX_SPEED`].
fn speed(distance: Vector, elapsed: Duration) -> Vector {
    let v = distance * (1.0 / elapsed.as_secs_f32().max(1e-3));
    let len = v.x.hypot(v.y);
    if len > MAX_SPEED {
        v * (MAX_SPEED / len)
    } else {
        v
    }
}

/// What the finger being followed is doing.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    /// No finger is down, and nothing's moving.
    Idle,
    /// Down, but not yet moved far or held long enough to say what it is.
    Pending { start: Point, at: Instant },
    /// Dragging, scrolling whatever's under `start`. `velocity` is in pixels
    /// a second, along `axis`, last measured at `sampled_at`, with
    /// `unsampled` moved since.
    Scrolling {
        start: Point,
        axis: Axis,
        last: Point,
        moved_at: Instant,
        velocity: Vector,
        sampled_at: Instant,
        unsampled: Vector,
    },
    /// Held long enough to be a pressed mouse button, which it stays until
    /// lifted.
    Holding { last: Point },
    /// Lifted mid-drag; what's under `at` keeps scrolling as it slows.
    Flinging {
        at: Point,
        velocity: Vector,
        last_frame: Instant,
    },
}

/// Follows one finger at a time and says what mouse events it stands for.
#[derive(Debug, Clone, Copy)]
struct Gesture {
    finger: Option<touch::Finger>,
    /// When the followed finger was last heard from.
    heard_at: Option<Instant>,
    phase: Phase,
}

impl Default for Gesture {
    fn default() -> Self {
        Gesture {
            finger: None,
            heard_at: None,
            phase: Phase::Idle,
        }
    }
}

/// A mouse event to send in a touch's place, with where the cursor is for it.
type Synthetic = (mouse::Event, Point);

fn wheel(delta: Vector) -> mouse::Event {
    mouse::Event::WheelScrolled {
        delta: mouse::ScrollDelta::Pixels {
            x: delta.x,
            y: delta.y,
        },
    }
}

impl Gesture {
    /// The mouse events `event`, arriving at `now`, stands for.
    fn touch(&mut self, event: touch::Event, now: Instant) -> Vec<Synthetic> {
        use mouse::{Button, Event as M};
        match event {
            touch::Event::FingerPressed { id, position } => {
                let mut out = Vec::new();
                if self.finger.is_some() {
                    let stale = self
                        .heard_at
                        .is_none_or(|at| now.saturating_duration_since(at) >= STALE);
                    if self.phase != Phase::Idle && !stale {
                        // A second finger is ignored; pinches aren't handled.
                        return out;
                    }
                    // The followed finger's lift went missing: let go of it.
                    out = self.reset();
                }
                self.finger = Some(id);
                self.heard_at = Some(now);
                // Any fling stops. Leaving and re-entering ends the wheel
                // "transaction" a scrollable keeps after scrolling, which
                // would otherwise send the next drag to it whichever way it
                // goes; moving there also shows what's under the finger.
                self.phase = Phase::Pending {
                    start: position,
                    at: now,
                };
                out.push((M::CursorLeft, position));
                out.push((M::CursorMoved { position }, position));
                out
            }
            touch::Event::FingerMoved { id, position } if self.finger == Some(id) => {
                self.heard_at = Some(now);
                match self.phase {
                    Phase::Pending { start, at } if now >= at + LONG_PRESS => {
                        // Held long enough, though no frame has said so yet:
                        // it's a press, and this move drags it.
                        self.phase = Phase::Holding { last: position };
                        vec![
                            (M::ButtonPressed(Button::Left), start),
                            (M::CursorMoved { position }, position),
                        ]
                    }
                    Phase::Pending { start, at } => {
                        let delta = position - start;
                        if delta.x.hypot(delta.y) < SLOP {
                            return Vec::new();
                        }
                        let axis = Axis::of(delta);
                        // Scrolling: the content follows the finger from
                        // where it went down.
                        self.phase = Phase::Scrolling {
                            start,
                            axis,
                            last: position,
                            moved_at: now,
                            velocity: speed(
                                axis.along(delta),
                                now.saturating_duration_since(at).max(MIN_SAMPLE),
                            ),
                            sampled_at: now,
                            unsampled: Vector::ZERO,
                        };
                        vec![(wheel(axis.along(delta)), start)]
                    }
                    Phase::Scrolling {
                        start,
                        axis,
                        last,
                        velocity,
                        sampled_at,
                        unsampled,
                        ..
                    } => {
                        let delta = axis.along(position - last);
                        let unsampled = unsampled + delta;
                        let elapsed = now.saturating_duration_since(sampled_at);
                        // Smoothed, as moves arrive unevenly.
                        let (velocity, sampled_at, unsampled) = if elapsed >= MIN_SAMPLE {
                            (
                                speed(unsampled, elapsed) * 0.7 + velocity * 0.3,
                                now,
                                Vector::ZERO,
                            )
                        } else {
                            (velocity, sampled_at, unsampled)
                        };
                        self.phase = Phase::Scrolling {
                            start,
                            axis,
                            last: position,
                            moved_at: now,
                            velocity,
                            sampled_at,
                            unsampled,
                        };
                        vec![(wheel(delta), start)]
                    }
                    Phase::Holding { .. } => {
                        self.phase = Phase::Holding { last: position };
                        vec![(M::CursorMoved { position }, position)]
                    }
                    Phase::Idle | Phase::Flinging { .. } => Vec::new(),
                }
            }
            touch::Event::FingerLifted { id, position } if self.finger == Some(id) => {
                self.finger = None;
                self.heard_at = None;
                let phase = std::mem::replace(&mut self.phase, Phase::Idle);
                match phase {
                    Phase::Pending { .. } => vec![
                        (M::CursorMoved { position }, position),
                        (M::ButtonPressed(Button::Left), position),
                        (M::ButtonReleased(Button::Left), position),
                    ],
                    Phase::Scrolling {
                        start,
                        moved_at,
                        velocity,
                        ..
                    } => {
                        let speed = velocity.x.hypot(velocity.y);
                        if now.saturating_duration_since(moved_at) < FLING_REST
                            && speed >= MIN_FLING
                        {
                            self.phase = Phase::Flinging {
                                at: start,
                                velocity,
                                last_frame: now,
                            };
                        }
                        Vec::new()
                    }
                    Phase::Holding { .. } => vec![
                        (M::CursorMoved { position }, position),
                        (M::ButtonReleased(Button::Left), position),
                    ],
                    Phase::Idle | Phase::Flinging { .. } => Vec::new(),
                }
            }
            touch::Event::FingerLost { id, .. } if self.finger == Some(id) => self.reset(),
            _ => Vec::new(),
        }
    }

    /// Forgets the followed finger, as its touch was cancelled, its lift went
    /// missing or the window lost focus. A held button can't stay down;
    /// anything else just stops.
    fn reset(&mut self) -> Vec<Synthetic> {
        self.finger = None;
        self.heard_at = None;
        match std::mem::replace(&mut self.phase, Phase::Idle) {
            Phase::Holding { last } => {
                vec![(mouse::Event::ButtonReleased(mouse::Button::Left), last)]
            }
            _ => Vec::new(),
        }
    }

    /// The mouse events due by `now`, a frame's time: a held press becoming
    /// a pressed button, or a fling's next step.
    fn frame(&mut self, now: Instant) -> Vec<Synthetic> {
        match self.phase {
            Phase::Pending { start, at } if now >= at + LONG_PRESS => {
                self.phase = Phase::Holding { last: start };
                vec![(mouse::Event::ButtonPressed(mouse::Button::Left), start)]
            }
            Phase::Flinging {
                at,
                velocity,
                last_frame,
            } => {
                let elapsed = now.saturating_duration_since(last_frame).as_secs_f32();
                let slowed = velocity * (-elapsed / FLING_DECAY).exp();
                if slowed.x.hypot(slowed.y) < MIN_FLING {
                    self.phase = Phase::Idle;
                    return Vec::new();
                }
                self.phase = Phase::Flinging {
                    at,
                    velocity: slowed,
                    last_frame: now,
                };
                vec![(wheel(slowed * elapsed), at)]
            }
            _ => Vec::new(),
        }
    }

    /// Stops a fling, as what it scrolls has reached its end.
    fn stop_fling(&mut self) {
        if matches!(self.phase, Phase::Flinging { .. }) {
            self.phase = Phase::Idle;
        }
    }

    /// When the next frame's needed: when a press would become held, or
    /// right away while flinging.
    fn redraw(&self) -> Option<RedrawRequest> {
        match self.phase {
            Phase::Pending { at, .. } => Some(RedrawRequest::At(at + LONG_PRESS)),
            Phase::Flinging { .. } => Some(RedrawRequest::NextFrame),
            _ => None,
        }
    }
}

/// Wraps `content` so touch acts like the mouse it's built for. See the
/// module docs.
pub struct TouchInput<'a, Message, Theme, Renderer> {
    content: Element<'a, Message, Theme, Renderer>,
}

pub fn touch_input<'a, Message, Theme, Renderer>(
    content: impl Into<Element<'a, Message, Theme, Renderer>>,
) -> TouchInput<'a, Message, Theme, Renderer> {
    TouchInput {
        content: content.into(),
    }
}

/// `cursor` moved to `position`, keeping whether it's over an overlay.
fn cursor_at(cursor: mouse::Cursor, position: Point) -> mouse::Cursor {
    match cursor {
        mouse::Cursor::Levitating(_) => mouse::Cursor::Levitating(position),
        _ => mouse::Cursor::Available(position),
    }
}

impl<Message, Theme, Renderer> TouchInput<'_, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    /// Sends `synthetic` events to the content, each through a shell of its
    /// own so one being captured doesn't hide the next from widgets that
    /// check. Returns whether the last was captured.
    #[allow(clippy::too_many_arguments)]
    fn send(
        &mut self,
        tree: &mut Tree,
        synthetic: Vec<Synthetic>,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> bool {
        let mut captured = false;
        for (event, position) in synthetic {
            let mut messages = Vec::new();
            let mut inner = Shell::new(&mut messages);
            self.content.as_widget_mut().update(
                &mut tree.children[0],
                &Event::Mouse(event),
                layout,
                cursor_at(cursor, position),
                renderer,
                clipboard,
                &mut inner,
                viewport,
            );
            captured = inner.is_event_captured();
            shell.merge(inner, |m| m);
        }
        captured
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for TouchInput<'_, Message, Theme, Renderer>
where
    Renderer: renderer::Renderer,
{
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Gesture>()
    }

    fn state(&self) -> tree::State {
        tree::State::new(Gesture::default())
    }

    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }

    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
    }

    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.content
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
        match event {
            Event::Touch(touch) => {
                let synthetic = tree
                    .state
                    .downcast_mut::<Gesture>()
                    .touch(*touch, Instant::now());
                self.send(
                    tree, synthetic, layout, cursor, renderer, clipboard, shell, viewport,
                );
                shell.capture_event();
            }
            Event::Window(window::Event::RedrawRequested(now)) => {
                self.content.as_widget_mut().update(
                    &mut tree.children[0],
                    event,
                    layout,
                    cursor,
                    renderer,
                    clipboard,
                    shell,
                    viewport,
                );
                let gesture = tree.state.downcast_mut::<Gesture>();
                let flinging = matches!(gesture.phase, Phase::Flinging { .. });
                let synthetic = gesture.frame(*now);
                if !synthetic.is_empty() {
                    let scrolled = self.send(
                        tree, synthetic, layout, cursor, renderer, clipboard, shell, viewport,
                    );
                    if flinging && !scrolled {
                        tree.state.downcast_mut::<Gesture>().stop_fling();
                    }
                }
            }
            _ => {
                self.content.as_widget_mut().update(
                    &mut tree.children[0],
                    event,
                    layout,
                    cursor,
                    renderer,
                    clipboard,
                    shell,
                    viewport,
                );
                // Touches can end unseen while the window's in the
                // background, so a held one lets go.
                if let Event::Window(window::Event::Unfocused) = event {
                    let synthetic = tree.state.downcast_mut::<Gesture>().reset();
                    self.send(
                        tree, synthetic, layout, cursor, renderer, clipboard, shell, viewport,
                    );
                }
            }
        }
        if let Some(request) = tree.state.downcast_ref::<Gesture>().redraw() {
            shell.request_redraw_at(request);
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
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
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

impl<'a, Message, Theme, Renderer> From<TouchInput<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: 'a + renderer::Renderer,
{
    fn from(input: TouchInput<'a, Message, Theme, Renderer>) -> Self {
        Element::new(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mouse::{Button, Event as M};

    const F: touch::Finger = touch::Finger(1);

    fn press(g: &mut Gesture, x: f32, y: f32, now: Instant) -> Vec<Synthetic> {
        g.touch(
            touch::Event::FingerPressed {
                id: F,
                position: Point::new(x, y),
            },
            now,
        )
    }

    fn moved(g: &mut Gesture, x: f32, y: f32, now: Instant) -> Vec<Synthetic> {
        g.touch(
            touch::Event::FingerMoved {
                id: F,
                position: Point::new(x, y),
            },
            now,
        )
    }

    fn lift(g: &mut Gesture, x: f32, y: f32, now: Instant) -> Vec<Synthetic> {
        g.touch(
            touch::Event::FingerLifted {
                id: F,
                position: Point::new(x, y),
            },
            now,
        )
    }

    fn events(s: Vec<Synthetic>) -> Vec<M> {
        s.into_iter().map(|(e, _)| e).collect()
    }

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn tap_clicks_where_lifted() {
        let (mut g, t) = (Gesture::default(), Instant::now());
        let down = events(press(&mut g, 10.0, 10.0, t));
        assert!(matches!(down[..], [M::CursorLeft, M::CursorMoved { .. }]));
        // A jiggle within the slop is still a tap.
        assert!(moved(&mut g, 14.0, 12.0, t + ms(30)).is_empty());
        let up = lift(&mut g, 14.0, 12.0, t + ms(60));
        assert_eq!(
            events(up.clone()),
            vec![
                M::CursorMoved {
                    position: Point::new(14.0, 12.0)
                },
                M::ButtonPressed(Button::Left),
                M::ButtonReleased(Button::Left),
            ]
        );
        assert!(up.iter().all(|(_, at)| *at == Point::new(14.0, 12.0)));
        assert_eq!(g.phase, Phase::Idle);
        assert_eq!(g.redraw(), None);
    }

    #[test]
    fn drag_scrolls_along_its_axis_under_where_it_started() {
        let (mut g, t) = (Gesture::default(), Instant::now());
        press(&mut g, 100.0, 100.0, t);
        // Mostly sideways, so it scrolls sideways only.
        let first = moved(&mut g, 80.0, 104.0, t + ms(20));
        assert_eq!(
            first,
            vec![(wheel(Vector::new(-20.0, 0.0)), Point::new(100.0, 100.0))]
        );
        let next = moved(&mut g, 50.0, 140.0, t + ms(40));
        assert_eq!(
            next,
            vec![(wheel(Vector::new(-30.0, 0.0)), Point::new(100.0, 100.0))]
        );
        // Lifting a drag isn't a click.
        let up = lift(&mut g, 50.0, 140.0, t + ms(50));
        assert!(up.is_empty());
    }

    #[test]
    fn quick_lift_flings_and_slows_to_a_stop() {
        let (mut g, t) = (Gesture::default(), Instant::now());
        press(&mut g, 100.0, 300.0, t);
        moved(&mut g, 100.0, 250.0, t + ms(16));
        moved(&mut g, 100.0, 200.0, t + ms(32));
        lift(&mut g, 100.0, 200.0, t + ms(40));
        assert!(matches!(g.phase, Phase::Flinging { .. }));
        assert_eq!(g.redraw(), Some(RedrawRequest::NextFrame));

        let step = g.frame(t + ms(56));
        let [(M::WheelScrolled { delta }, at)] = step[..] else {
            panic!("expected a wheel step, got {step:?}");
        };
        assert_eq!(at, Point::new(100.0, 300.0));
        let mouse::ScrollDelta::Pixels { x, y } = delta else {
            panic!("expected pixels");
        };
        assert_eq!(x, 0.0);
        assert!(y < 0.0, "keeps scrolling the way it was dragged");

        let mut frames = 0;
        let mut now = t + ms(56);
        while matches!(g.phase, Phase::Flinging { .. }) {
            now += ms(16);
            g.frame(now);
            frames += 1;
            assert!(frames < 1000, "fling never stops");
        }
        assert_eq!(g.redraw(), None);
    }

    #[test]
    fn resting_before_lift_does_not_fling() {
        let (mut g, t) = (Gesture::default(), Instant::now());
        press(&mut g, 100.0, 300.0, t);
        moved(&mut g, 100.0, 200.0, t + ms(16));
        lift(&mut g, 100.0, 200.0, t + ms(300));
        assert_eq!(g.phase, Phase::Idle);
    }

    #[test]
    fn long_press_holds_the_button_until_lifted() {
        let (mut g, t) = (Gesture::default(), Instant::now());
        press(&mut g, 10.0, 10.0, t);
        assert_eq!(g.redraw(), Some(RedrawRequest::At(t + LONG_PRESS)));
        // An early frame changes nothing.
        assert!(g.frame(t + ms(100)).is_empty());
        let held = g.frame(t + LONG_PRESS);
        assert_eq!(
            held,
            vec![(M::ButtonPressed(Button::Left), Point::new(10.0, 10.0))]
        );
        // Moves now move the cursor with the button down, which drags.
        assert_eq!(
            events(moved(&mut g, 60.0, 90.0, t + ms(600))),
            vec![M::CursorMoved {
                position: Point::new(60.0, 90.0)
            }]
        );
        assert_eq!(
            events(lift(&mut g, 60.0, 90.0, t + ms(700))),
            vec![
                M::CursorMoved {
                    position: Point::new(60.0, 90.0)
                },
                M::ButtonReleased(Button::Left)
            ]
        );
    }

    #[test]
    fn a_new_press_stops_a_fling_and_other_fingers_are_ignored() {
        let (mut g, t) = (Gesture::default(), Instant::now());
        g.phase = Phase::Flinging {
            at: Point::ORIGIN,
            velocity: Vector::new(0.0, 1000.0),
            last_frame: t,
        };
        press(&mut g, 5.0, 5.0, t);
        assert!(matches!(g.phase, Phase::Pending { .. }));
        let other = touch::Event::FingerPressed {
            id: touch::Finger(2),
            position: Point::new(50.0, 50.0),
        };
        assert!(g.touch(other, t).is_empty());
        let other_moved = touch::Event::FingerMoved {
            id: touch::Finger(2),
            position: Point::new(90.0, 50.0),
        };
        assert!(g.touch(other_moved, t).is_empty());
        assert!(matches!(g.phase, Phase::Pending { .. }));
    }

    fn hold(g: &mut Gesture, t: Instant) {
        press(g, 10.0, 10.0, t);
        g.frame(t + LONG_PRESS);
        moved(g, 40.0, 60.0, t + ms(600));
        assert_eq!(
            g.phase,
            Phase::Holding {
                last: Point::new(40.0, 60.0)
            }
        );
    }

    #[test]
    fn lost_finger_releases_a_held_button() {
        let (mut g, t) = (Gesture::default(), Instant::now());
        hold(&mut g, t);
        let lost = g.touch(
            touch::Event::FingerLost {
                id: F,
                position: Point::new(40.0, 60.0),
            },
            t + ms(700),
        );
        assert_eq!(
            lost,
            vec![(M::ButtonReleased(Button::Left), Point::new(40.0, 60.0))]
        );
        assert_eq!((g.finger, g.phase), (None, Phase::Idle));
    }

    #[test]
    fn reset_releases_a_held_button() {
        let (mut g, t) = (Gesture::default(), Instant::now());
        hold(&mut g, t);
        assert_eq!(events(g.reset()), vec![M::ButtonReleased(Button::Left)]);
        assert!(g.reset().is_empty());
        // A new touch works as usual afterwards.
        press(&mut g, 5.0, 5.0, t + ms(800));
        assert_eq!(g.finger, Some(F));
    }

    #[test]
    fn a_stale_finger_gives_way_to_a_new_one() {
        let (mut g, t) = (Gesture::default(), Instant::now());
        hold(&mut g, t);
        let other = |position| touch::Event::FingerPressed {
            id: touch::Finger(2),
            position,
        };
        // While the held finger was just heard from, a new one is ignored.
        assert!(
            g.touch(other(Point::new(90.0, 90.0)), t + ms(900))
                .is_empty()
        );
        assert_eq!(g.finger, Some(F));
        // Its lift must have gone missing: let go of it, then follow the
        // new finger.
        let taken = g.touch(other(Point::new(90.0, 90.0)), t + ms(600) + STALE);
        assert_eq!(
            events(taken),
            vec![
                M::ButtonReleased(Button::Left),
                M::CursorLeft,
                M::CursorMoved {
                    position: Point::new(90.0, 90.0)
                },
            ]
        );
        assert_eq!(g.finger, Some(touch::Finger(2)));
        assert!(matches!(g.phase, Phase::Pending { .. }));
        // The old finger's events no longer count.
        assert!(lift(&mut g, 40.0, 60.0, t + ms(1700)).is_empty());
        assert!(matches!(g.phase, Phase::Pending { .. }));
    }

    #[test]
    fn moves_handled_back_to_back_do_not_spike_the_speed() {
        let (mut g, t) = (Gesture::default(), Instant::now());
        press(&mut g, 100.0, 500.0, t);
        moved(&mut g, 100.0, 480.0, t + ms(16));
        let before = match g.phase {
            Phase::Scrolling { velocity, .. } => velocity,
            p => panic!("not scrolling: {p:?}"),
        };
        // Two moves handled a microsecond apart still scroll by their full
        // distance, but don't count toward the speed until 8 ms have passed.
        let t2 = t + ms(16) + Duration::from_micros(1);
        assert_eq!(
            moved(&mut g, 100.0, 380.0, t2),
            vec![(wheel(Vector::new(0.0, -100.0)), Point::new(100.0, 500.0))]
        );
        let Phase::Scrolling { velocity, .. } = g.phase else {
            panic!("not scrolling");
        };
        assert_eq!(velocity, before);
        // Once measured, it's no faster than the cap.
        moved(&mut g, 100.0, 0.0, t + ms(30));
        let Phase::Scrolling { velocity, .. } = g.phase else {
            panic!("not scrolling");
        };
        assert!(velocity.y < 0.0 && velocity.y.abs() <= MAX_SPEED);
        // Even the first move after the press is capped.
        let mut g = Gesture::default();
        press(&mut g, 100.0, 500.0, t);
        moved(&mut g, 100.0, 0.0, t + Duration::from_micros(1));
        let Phase::Scrolling { velocity, .. } = g.phase else {
            panic!("not scrolling");
        };
        assert!(velocity.y.abs() <= MAX_SPEED);
    }

    #[test]
    fn a_move_after_the_long_press_deadline_holds_and_drags() {
        let (mut g, t) = (Gesture::default(), Instant::now());
        press(&mut g, 10.0, 10.0, t);
        // No frame came at the deadline; the first move after it decides.
        let out = moved(&mut g, 80.0, 10.0, t + LONG_PRESS + ms(20));
        assert_eq!(
            out,
            vec![
                (M::ButtonPressed(Button::Left), Point::new(10.0, 10.0)),
                (
                    M::CursorMoved {
                        position: Point::new(80.0, 10.0)
                    },
                    Point::new(80.0, 10.0)
                ),
            ]
        );
        assert!(matches!(g.phase, Phase::Holding { .. }));
    }
}
