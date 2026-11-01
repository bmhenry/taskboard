//! Soft keyboard for phones (task WRJVWT28).
//!
//! iced draws to a `<canvas>`, and a phone only opens its on-screen keyboard
//! for a focused editable DOM element. So while an iced `text_input` or
//! `text_editor` has focus after a touch, a hidden `<textarea>` placed over it
//! takes the DOM focus, and what the keyboard types into the textarea is
//! replayed to the canvas as `keydown`/`keyup` events, which winit and iced
//! handle like a physical keyboard.
//!
//! - [`bridge`] wraps the root element. After each event it asks the widget
//!   tree (and its overlay, for popups) whether a focusable widget is focused.
//! - The textarea is focused from a `pointerup` listener on the document,
//!   which runs after winit's own listener on the canvas has fed the lift to
//!   iced (and `ui/touch.rs` has turned it into the click that focuses the
//!   widget, or a button's focus task has run). That's inside the tap's user
//!   gesture, which mobile browsers require before they show the keyboard; a
//!   touch `pointerdown` doesn't count as user activation. Focus that arrives
//!   later after a touch is applied at the next redraw instead, while the
//!   tap's activation is still fresh.
//! - Input is mirrored by diffing the textarea's value before and after each
//!   `input` event, so composition (Android keyboards compose each word and
//!   send keyCode 229 rather than normal keydowns), autocorrect and
//!   `deleteContentBackward` all become backspaces and characters. The value
//!   starts as one space, so a backspace in an otherwise empty textarea still
//!   produces an `input` event. Keys that aren't text (Enter, Backspace with a
//!   real key code, arrows, Tab, Escape, shortcuts) are forwarded as they are.
//!
//! Mouse input never focuses the textarea, so desktop browsers behave as
//! before, and the native app doesn't compile this module at all.

use std::cell::{Cell, RefCell};

use crate::text_diff::diff;
use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::operation::Focusable;
use iced::advanced::widget::{self, Operation, Tree, Widget, tree};
use iced::advanced::{Clipboard, Shell, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Renderer, Size, Theme, Vector, mouse, window};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::{
    AddEventListenerOptions, FocusOptions, HtmlElement, HtmlTextAreaElement, InputEvent,
    KeyboardEvent, KeyboardEventInit, PointerEvent,
};

/// The textarea's resting value: see the module docs.
const SENTINEL: &str = " ";
/// Past this many characters (outside a composition) the value is reset.
const MAX_VALUE: usize = 200;
const INPUT_ID: &str = "tb-soft-keyboard";

/// Wraps the application's root element with the soft-keyboard bridge.
pub fn bridge<'a, Message: 'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    Element::new(Bridge { content })
}

struct Bridge<'a, Message> {
    content: Element<'a, Message>,
}

impl<Message> Widget<Message, Theme, Renderer> for Bridge<'_, Message> {
    fn size(&self) -> Size<Length> {
        self.content.as_widget().size()
    }

    fn size_hint(&self) -> Size<Length> {
        self.content.as_widget().size_hint()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        let node = self.content.as_widget_mut().layout(tree, renderer, limits);
        with_state(|s| s.root_width = node.size().width);
        node
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
        self.content
            .as_widget()
            .draw(tree, renderer, theme, style, layout, cursor, viewport);
    }

    // The bridge is transparent: the tree is the content's own.
    fn tag(&self) -> tree::Tag {
        self.content.as_widget().tag()
    }

    fn state(&self) -> tree::State {
        self.content.as_widget().state()
    }

    fn children(&self) -> Vec<Tree> {
        self.content.as_widget().children()
    }

    fn diff(&self, tree: &mut Tree) {
        self.content.as_widget().diff(tree);
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
            .operate(tree, layout, renderer, operation);
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
        if hides_caret(event) {
            return;
        }
        self.content.as_widget_mut().update(
            tree, event, layout, cursor, renderer, clipboard, shell, viewport,
        );
        let mut find = FindFocused(None);
        self.content
            .as_widget_mut()
            .operate(tree, layout, renderer, &mut find);
        sync(Layer::Base, find.0, event);
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.content
            .as_widget()
            .mouse_interaction(tree, layout, cursor, viewport, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let inner =
            self.content
                .as_widget_mut()
                .overlay(tree, layout, renderer, viewport, translation);
        match inner {
            Some(inner) => Some(overlay::Element::new(Box::new(OverlayBridge { inner }))),
            None => {
                with_state(|s| s.overlay = None);
                None
            }
        }
    }
}

/// The same check for text widgets in popups, which iced updates separately
/// (and first: an event the overlay captures never reaches the base layer).
struct OverlayBridge<'a, Message> {
    inner: overlay::Element<'a, Message, Theme, Renderer>,
}

impl<Message> overlay::Overlay<Message, Theme, Renderer> for OverlayBridge<'_, Message> {
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        self.inner.as_overlay_mut().layout(renderer, bounds)
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.inner
            .as_overlay()
            .draw(renderer, theme, style, layout, cursor);
    }

    fn operate(&mut self, layout: Layout<'_>, renderer: &Renderer, operation: &mut dyn Operation) {
        self.inner
            .as_overlay_mut()
            .operate(layout, renderer, operation);
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
        if hides_caret(event) {
            return;
        }
        self.inner
            .as_overlay_mut()
            .update(event, layout, cursor, renderer, clipboard, shell);
        let mut find = FindFocused(None);
        self.inner
            .as_overlay_mut()
            .operate(layout, renderer, &mut find);
        sync(Layer::Overlay, find.0, event);
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.inner
            .as_overlay()
            .mouse_interaction(layout, cursor, renderer)
    }

    fn overlay<'b>(
        &'b mut self,
        layout: Layout<'b>,
        renderer: &Renderer,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        self.inner.as_overlay_mut().overlay(layout, renderer)
    }

    fn index(&self) -> f32 {
        self.inner.as_overlay().index()
    }
}

/// Finds the bounds of the focused focusable widget, if any.
struct FindFocused(Option<Rectangle>);

impl Operation for FindFocused {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&widget::Id>,
        bounds: Rectangle,
        state: &mut dyn Focusable,
    ) {
        if state.is_focused() {
            self.0 = Some(bounds);
        }
    }
}

/// The canvas losing focus to the textarea mustn't count as the window losing
/// focus, or iced's text widgets stop drawing their caret.
/// While the textarea is being focused, the canvas's blur arrives before
/// `document.activeElement` names the textarea, hence `FOCUSING`.
fn hides_caret(event: &Event) -> bool {
    matches!(event, Event::Window(window::Event::Unfocused))
        && (FOCUSING.get() || with_state(|s| s.is_active()))
}

#[derive(Clone, Copy)]
enum Layer {
    Base,
    Overlay,
}

struct State {
    canvas: HtmlElement,
    input: HtmlTextAreaElement,
    /// Focused text widget bounds, per layer, in iced's logical coordinates.
    base: Option<Rectangle>,
    overlay: Option<Rectangle>,
    /// `overlay.or(base)` as of the last sync.
    focused: Option<Rectangle>,
    /// Width of the root layout, to convert to CSS pixels.
    root_width: f32,
    /// A widget gained focus after a touch; the textarea should follow.
    pending: bool,
    /// A touch or pen pointer is down.
    touch_down: bool,
    /// The last pointer pressed was a touch or pen, not a mouse.
    touch_seen: bool,
    /// The textarea's value as of the last mirrored `input` event.
    last_value: String,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
    /// Whether `install` has run, so a failure isn't retried on every event.
    static INSTALLED: Cell<bool> = const { Cell::new(false) };
    /// The textarea is taking the focus right now.
    static FOCUSING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `f` on the bridge state, setting it up on the first call.
/// DOM calls that can fire events back into winit (focus, blur, dispatching
/// keys) must happen outside `f`, since they re-enter the bridge.
fn with_state<R: Default>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|cell| {
        let mut slot = cell.borrow_mut();
        if !INSTALLED.replace(true) {
            *slot = install();
            log::info!("keyboard: install -> {}", slot.is_some());
        }
        slot.as_mut().map(f).unwrap_or_default()
    })
}

impl State {
    fn is_active(&self) -> bool {
        let active = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.active_element());
        active.is_some_and(|a| a.id() == INPUT_ID)
    }

    fn reset_value(&mut self) {
        self.input.set_value(SENTINEL);
        self.last_value = SENTINEL.to_string();
    }

    /// Lays the textarea over the focused widget, so the browser scrolls the
    /// right place into view above the keyboard.
    fn place(&self, bounds: Rectangle) {
        let rect = self.canvas.get_bounding_client_rect();
        let scale = if self.root_width > 0.0 {
            rect.width() / f64::from(self.root_width)
        } else {
            1.0
        };
        let window = web_sys::window();
        let sx = window
            .as_ref()
            .and_then(|w| w.scroll_x().ok())
            .unwrap_or(0.0);
        let sy = window
            .as_ref()
            .and_then(|w| w.scroll_y().ok())
            .unwrap_or(0.0);
        let width = (f64::from(bounds.width) * scale).clamp(1.0, rect.width().max(1.0));
        let height = (f64::from(bounds.height) * scale).clamp(20.0, rect.height().max(20.0));
        let left = (f64::from(bounds.x) * scale).clamp(0.0, (rect.width() - width).max(0.0));
        let top = (f64::from(bounds.y) * scale).clamp(0.0, (rect.height() - height).max(0.0));
        let style = self.input.style();
        let _ = style.set_property("left", &format!("{}px", rect.left() + sx + left));
        let _ = style.set_property("top", &format!("{}px", rect.top() + sy + top));
        let _ = style.set_property("width", &format!("{width}px"));
        let _ = style.set_property("height", &format!("{height}px"));
    }
}

/// What a sync asks to do with the DOM once the state is released.
#[derive(Default)]
enum Action {
    #[default]
    None,
    Focus(HtmlTextAreaElement),
    Release(HtmlTextAreaElement, HtmlElement),
}

/// Records which text widget `layer` has focused after `event` and moves the
/// DOM focus to match. Runs inside winit's event handling.
fn sync(layer: Layer, bounds: Option<Rectangle>, event: &Event) {
    let redraw = matches!(event, Event::Window(window::Event::RedrawRequested(_)));
    let action = with_state(|s| {
        match layer {
            Layer::Base => s.base = bounds,
            Layer::Overlay => s.overlay = bounds,
        }
        let now = s.overlay.or(s.base);
        let was = std::mem::replace(&mut s.focused, now);
        let active = s.is_active();
        match (was, now) {
            (None, Some(_)) => s.pending = s.touch_seen,
            (Some(_), None) => {
                s.pending = false;
                if active {
                    return Action::Release(s.input.clone(), s.canvas.clone());
                }
            }
            (Some(a), Some(b)) if a != b && active => s.place(b),
            _ => {}
        }
        // Focus that didn't come from the tap itself (a focus task, say).
        // A tap in progress is left to `on_pointer_up`.
        if let Some(bounds) = now
            && s.pending
            && redraw
            && !s.touch_down
        {
            s.pending = false;
            if !active {
                s.reset_value();
                s.place(bounds);
                return Action::Focus(s.input.clone());
            }
        }
        Action::None
    });
    run(action);
}

fn run(action: Action) {
    match action {
        Action::None => {}
        Action::Focus(input) => {
            let options = FocusOptions::new();
            options.set_prevent_scroll(true);
            FOCUSING.set(true);
            let _ = input.focus_with_options(&options);
            FOCUSING.set(false);
        }
        Action::Release(input, canvas) => {
            let _ = input.blur();
            // Give winit's keyboard listeners their focus back.
            let _ = canvas.focus();
        }
    }
}

/// Inside the tap's user gesture: focus the textarea if the tap focused a
/// text widget. Runs after winit's `pointerup` listener on the canvas.
fn on_pointer_up() {
    let action = with_state(|s| {
        s.touch_down = false;
        s.pending = false;
        let Some(bounds) = s.focused else {
            return Action::None;
        };
        if s.is_active() {
            if keyboard_visible() {
                return Action::None;
            }
            // Focused, but the keyboard was dismissed (Android's back
            // button): blur so that focusing again brings it back.
            let _ = s.input.blur();
        }
        s.reset_value();
        s.place(bounds);
        Action::Focus(s.input.clone())
    });
    run(action);
}

/// Whether the on-screen keyboard looks open: it shrinks the visual viewport.
fn keyboard_visible() -> bool {
    let Some(window) = web_sys::window() else {
        return false;
    };
    let inner = window
        .inner_height()
        .ok()
        .and_then(|h| h.as_f64())
        .unwrap_or(0.0);
    window
        .visual_viewport()
        .is_some_and(|v| v.height() * v.scale() < inner * 0.85)
}

/// Mirrors the textarea's new value into iced as key presses.
fn on_input(composing: bool) {
    let replay = with_state(|s| {
        let value = s.input.value();
        let edit = diff(&s.last_value, &value);
        s.last_value = value;
        if !composing && (s.last_value.is_empty() || s.last_value.chars().count() > MAX_VALUE) {
            s.reset_value();
        }
        Some((s.canvas.clone(), edit))
    });
    let Some((canvas, edit)) = replay else { return };
    for _ in 0..edit.after {
        press(&canvas, "ArrowLeft");
    }
    for _ in 0..edit.deleted {
        press(&canvas, "Backspace");
    }
    for c in edit.inserted.chars() {
        match c {
            '\n' | '\r' => press(&canvas, "Enter"),
            '\t' => press(&canvas, "Tab"),
            c => press(&canvas, c.encode_utf8(&mut [0; 4])),
        }
    }
    for _ in 0..edit.after {
        press(&canvas, "ArrowRight");
    }
}

/// Keys that aren't text input, which go to iced as they are.
fn forwarded(event: &KeyboardEvent) -> bool {
    if event.is_composing() || event.key_code() == 229 {
        return false;
    }
    let key = event.key();
    let named =
        key.chars().count() > 1 && !matches!(key.as_str(), "Unidentified" | "Process" | "Dead");
    named || event.ctrl_key() || event.meta_key()
}

fn forward(canvas: &HtmlElement, kind: &str, from: &KeyboardEvent) {
    let init = KeyboardEventInit::new();
    init.set_key(&from.key());
    init.set_code(&from.code());
    init.set_ctrl_key(from.ctrl_key());
    init.set_shift_key(from.shift_key());
    init.set_alt_key(from.alt_key());
    init.set_meta_key(from.meta_key());
    init.set_repeat(from.repeat());
    dispatch(canvas, kind, &init);
}

/// Presses and releases `key` (a `KeyboardEvent.key` value) on the canvas.
fn press(canvas: &HtmlElement, key: &str) {
    let init = KeyboardEventInit::new();
    init.set_key(key);
    dispatch(canvas, "keydown", &init);
    dispatch(canvas, "keyup", &init);
}

fn dispatch(canvas: &HtmlElement, kind: &str, init: &KeyboardEventInit) {
    init.set_bubbles(true);
    init.set_cancelable(true);
    if let Ok(event) = KeyboardEvent::new_with_keyboard_event_init_dict(kind, init) {
        let _ = canvas.dispatch_event(&event);
    }
}

/// Creates the textarea and the listeners once iced's canvas exists.
fn install() -> Option<State> {
    let document = web_sys::window()?.document()?;
    let canvas: HtmlElement = document.query_selector("canvas").ok()??.dyn_into().ok()?;
    let input: HtmlTextAreaElement = document.create_element("textarea").ok()?.dyn_into().ok()?;
    input.set_id(INPUT_ID);
    for (name, value) in [
        ("autocomplete", "off"),
        ("spellcheck", "false"),
        ("aria-hidden", "true"),
        ("tabindex", "-1"),
    ] {
        let _ = input.set_attribute(name, value);
    }
    // Invisible but focusable (not `display: none`), and never hit by taps.
    // 16px text stops iOS zooming in on focus.
    let _ = input.set_attribute(
        "style",
        "position: absolute; left: 0; top: 0; width: 1px; height: 20px; margin: 0; padding: 0; \
         border: 0; outline: 0; resize: none; overflow: hidden; opacity: 0; color: transparent; \
         background: transparent; caret-color: transparent; font-size: 16px; \
         pointer-events: none; z-index: -1;",
    );
    input.set_value(SENTINEL);
    document.body()?.append_child(&input).ok()?;

    // winit focuses the canvas on every pointerdown. While the textarea has
    // focus, skip that, or each tap would close and reopen the keyboard.
    let guard = js_sys::Function::new_no_args(&format!(
        "if (document.activeElement && document.activeElement.id === '{INPUT_ID}') return; \
         return HTMLElement.prototype.focus.apply(this, arguments);"
    ));
    let _ = js_sys::Reflect::set(&canvas, &"focus".into(), &guard);

    let capture = AddEventListenerOptions::new();
    capture.set_capture(true);
    listen(
        &document,
        "pointerdown",
        Some(&capture),
        |e: PointerEvent| {
            let touch = e.pointer_type() != "mouse";
            with_state(|s| {
                s.touch_seen = touch;
                s.touch_down = touch;
                // A tap moves iced's caret (or focus), so whatever the keyboard
                // was composing belongs to the old position: start afresh.
                if touch && s.is_active() {
                    s.reset_value();
                }
            });
        },
    );
    listen(&document, "pointerup", None, |e: PointerEvent| {
        if e.pointer_type() != "mouse" {
            on_pointer_up();
        }
    });
    listen(&document, "pointercancel", None, |_: PointerEvent| {
        with_state(|s| s.touch_down = false);
    });
    listen(&input, "input", None, |e: InputEvent| {
        on_input(e.is_composing())
    });
    listen(&input, "compositionend", None, |_: web_sys::Event| {
        on_input(false)
    });
    for kind in ["keydown", "keyup"] {
        listen(&input, kind, None, move |e: KeyboardEvent| {
            if forwarded(&e) {
                e.prevent_default();
                let canvas = with_state(|s| Some(s.canvas.clone()));
                if let Some(canvas) = canvas {
                    forward(&canvas, kind, &e);
                }
            }
        });
    }

    Some(State {
        canvas,
        input,
        base: None,
        overlay: None,
        focused: None,
        root_width: 0.0,
        pending: false,
        touch_down: false,
        touch_seen: false,
        last_value: SENTINEL.to_string(),
    })
}

/// Adds a listener for the page's lifetime.
fn listen<E: JsCast + 'static>(
    target: &web_sys::EventTarget,
    kind: &str,
    options: Option<&AddEventListenerOptions>,
    mut handler: impl FnMut(E) + 'static,
) {
    let closure = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        if let Ok(event) = event.dyn_into::<E>() {
            handler(event);
        }
    });
    let function = closure.as_ref().unchecked_ref();
    let _ = match options {
        Some(options) => target.add_event_listener_with_callback_and_add_event_listener_options(
            kind, function, options,
        ),
        None => target.add_event_listener_with_callback(kind, function),
    };
    closure.forget();
}
