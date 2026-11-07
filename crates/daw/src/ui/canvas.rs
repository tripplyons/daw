//! Interactive canvases: the piano roll, playlist, and automation editor.
//!
//! A canvas is a `Program` that turns pointer events into messages and
//! paints a `Frame`. Its state (the drag in progress, the last click) lives
//! in the app's UI state, keyed by the canvas. The frame becomes one Rune
//! shape for each run of fills and strokes, with text and rounded boxes
//! between them, so later drawing stays on top.

use std::any::Any;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use rune::{Color, Drag, El, LineCap, Phase, PointerButton, Shape, Tx, canvas as shape_el, el, text};

use crate::app::{App, Message};
use crate::input::{Modifiers, Point, Rectangle, ScrollDelta, Size};
use crate::theme;

pub mod mouse {
    pub use crate::input::ScrollDelta;
    use crate::input::{Point, Rectangle};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum Button {
        Left,
        Right,
        Middle,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Event {
        ButtonPressed(Button),
        ButtonReleased(Button),
        CursorMoved { position: Point },
        WheelScrolled { delta: ScrollDelta },
    }

    /// Where the pointer is, in the canvas's points.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Cursor {
        Available(Point),
        Unavailable,
    }

    impl Cursor {
        pub fn position(self) -> Option<Point> {
            match self {
                Cursor::Available(p) => Some(p),
                Cursor::Unavailable => None,
            }
        }

        /// The position relative to `bounds`, when inside it.
        pub fn position_in(self, bounds: Rectangle) -> Option<Point> {
            let p = self.position()?;
            bounds.contains(p).then(|| Point::new(p.x - bounds.x, p.y - bounds.y))
        }

        /// The position relative to `origin`, inside or not.
        pub fn position_from(self, origin: Point) -> Option<Point> {
            let p = self.position()?;
            Some(Point::new(p.x - origin.x, p.y - origin.y))
        }

    }

    /// The pointer shape over a canvas.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub enum Interaction {
        #[default]
        Idle,
        Pointer,
        Grab,
        Grabbing,
        Crosshair,
        ResizingHorizontally,
        ResizingVertically,
    }

    #[allow(non_upper_case_globals)]
    impl Interaction {
        pub fn default() -> Interaction {
            Interaction::Idle
        }
    }
}

use mouse::{Button, Cursor, Interaction};

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Mouse(mouse::Event),
    Keyboard(crate::input::Event),
}

/// Whether a canvas used an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Captured,
    Ignored,
}

/// What a canvas does with an event: send a message, redraw, or both.
#[derive(Debug, Clone)]
pub struct Action {
    message: Option<Message>,
    status: Status,
}

impl Action {
    pub fn publish(message: impl Into<Message>) -> Action {
        Action { message: Some(message.into()), status: Status::Ignored }
    }

    pub fn request_redraw() -> Action {
        Action { message: None, status: Status::Ignored }
    }

    pub fn capture() -> Action {
        Action { message: None, status: Status::Captured }
    }

    pub fn and_capture(mut self) -> Action {
        self.status = Status::Captured;
        self
    }

    #[cfg(test)]
    pub fn into_inner(self) -> (Option<Message>, (), Status) {
        (self.message, (), self.status)
    }

}

pub trait Program {
    type State: Default + 'static;

    fn update(&self, state: &mut Self::State, event: &Event, bounds: Rectangle, cursor: Cursor) -> Option<Action>;

    fn draw(&self, state: &Self::State, frame: &mut Frame, bounds: Rectangle, cursor: Cursor);

    fn mouse_interaction(&self, _state: &Self::State, _bounds: Rectangle, _cursor: Cursor) -> Interaction {
        Interaction::Idle
    }
}

/// `Program` with its state as `Any`, so programs that borrow the app can
/// be built and boxed on demand.
pub trait Dyn {
    fn update(&self, state: &mut dyn Any, event: &Event, bounds: Rectangle, cursor: Cursor) -> Option<Action>;
    fn draw(&self, state: &dyn Any, frame: &mut Frame, bounds: Rectangle, cursor: Cursor);
    fn mouse_interaction(&self, state: &dyn Any, bounds: Rectangle, cursor: Cursor) -> Interaction;
    fn new_state(&self) -> Box<dyn Any>;
}

impl<P: Program> Dyn for P {
    fn update(&self, state: &mut dyn Any, event: &Event, bounds: Rectangle, cursor: Cursor) -> Option<Action> {
        Program::update(self, state.downcast_mut().expect("canvas state"), event, bounds, cursor)
    }

    fn draw(&self, state: &dyn Any, frame: &mut Frame, bounds: Rectangle, cursor: Cursor) {
        Program::draw(self, state.downcast_ref().expect("canvas state"), frame, bounds, cursor)
    }

    fn mouse_interaction(&self, state: &dyn Any, bounds: Rectangle, cursor: Cursor) -> Interaction {
        Program::mouse_interaction(self, state.downcast_ref().expect("canvas state"), bounds, cursor)
    }

    fn new_state(&self) -> Box<dyn Any> {
        Box::new(P::State::default())
    }
}

/// Builds a canvas's program from the app, for drawing and for each event.
pub type Build = Rc<dyn for<'a> Fn(&'a App) -> Box<dyn Dyn + 'a>>;

/// The canvas's size from the last layout.
fn bounds(app: &App, key: &'static str) -> Rectangle {
    let size = app.ui.sizes.borrow().get(key).copied().unwrap_or_default();
    Rectangle::with_size(size)
}

fn cursor(app: &App, key: &'static str) -> Cursor {
    match app.ui.pointer {
        Some((k, p)) if k == key => Cursor::Available(p),
        _ => Cursor::Unavailable,
    }
}

/// Run an event through the canvas's program and send what it publishes.
fn dispatch(app: &mut App, tx: &mut Tx, key: &'static str, build: &Build, event: Event, at: Option<Point>) -> bool {
    let action = {
        let app_ref: &App = app;
        let program = build(app_ref);
        let mut states = app_ref.ui.canvases.borrow_mut();
        let state = states.entry(key).or_insert_with(|| program.new_state());
        let cursor = at.map_or(Cursor::Unavailable, Cursor::Available);
        program.update(state.as_mut(), &event, bounds(app_ref, key), cursor)
    };
    let Some(action) = action else { return false };
    let captured = action.status == Status::Captured || action.message.is_some();
    if let Some(message) = action.message {
        super::send(app, tx, message);
    }
    captured
}

fn modifiers(app: &mut App, tx: &Tx) -> Modifiers {
    let m = super::modifiers(tx.mods());
    app.ui.modifiers = m;
    m
}

/// A canvas filling its parent, drawn and driven by the program `build` makes.
pub fn view(app: &App, key: &'static str, build: impl for<'a> Fn(&'a App) -> Box<dyn Dyn + 'a> + 'static) -> El {
    let build: Build = Rc::new(build);
    // Read the size during the build, so Rune builds again when it changes
    // and the first frame already draws at the right size.
    if let Some([w, h]) = rune::memo::layout_size(key) {
        app.ui.sizes.borrow_mut().insert(key, Size::new(w, h));
    }
    let bounds = bounds(app, key);
    let cursor = cursor(app, key);
    let program = build(app);
    let mut frame = Frame::new(bounds.size());
    let interaction = {
        let mut states = app.ui.canvases.borrow_mut();
        let state = states.entry(key).or_insert_with(|| program.new_state());
        if bounds.width >= 1.0 && bounds.height >= 1.0 {
            program.draw(state.as_ref(), &mut frame, bounds, cursor);
        }
        program.mouse_interaction(state.as_ref(), bounds, cursor)
    };
    let pointer = match interaction {
        Interaction::Idle => rune::Cursor::Default,
        Interaction::Pointer => rune::Cursor::Pointer,
        Interaction::Grab => rune::Cursor::Grab,
        Interaction::Grabbing => rune::Cursor::Grabbing,
        Interaction::Crosshair => rune::Cursor::Crosshair,
        Interaction::ResizingHorizontally => rune::Cursor::ResizeColumn,
        Interaction::ResizingVertically => rune::Cursor::ResizeRow,
    };
    let (on_drag, on_move, on_wheel, on_right) = (build.clone(), build.clone(), build.clone(), build);
    // Drawings jump to their new place each frame; they never glide.
    frame.into_el()
        .instant_changes()
        .key(key)
        .w_full()
        .h_full()
        .cursor(pointer)
        .on_layout(move |app: &mut App, [_, _, w, h]| {
            app.ui.sizes.borrow_mut().insert(key, Size::new(w, h));
        })
        .on_hover_change(move |app: &mut App, inside, _| {
            if !inside && app.ui.pointer.is_some_and(|(k, _)| k == key) && app.ui.dragging != Some(key) {
                app.ui.pointer = None;
            }
        })
        .on_pointer_move_local(move |app: &mut App, p, tx| {
            if app.ui.dragging == Some(key) { return; }
            let point = Point::new(p.x, p.y);
            app.ui.pointer = Some((key, point));
            modifiers(app, tx);
            dispatch(app, tx, key, &on_move, Event::Mouse(mouse::Event::CursorMoved { position: point }), Some(point));
        })
        .on_drag(move |app: &mut App, d: &mut Drag, tx: &mut Tx| {
            let point = Point::new(d.x, d.y);
            app.ui.pointer = Some((key, point));
            let button = match tx.button() {
                PointerButton::Middle => Button::Middle,
                PointerButton::Secondary => Button::Right,
                _ => Button::Left,
            };
            match d.phase {
                Phase::Down => {
                    super::touch(app);
                    app.ui.dragging = Some(key);
                    let m = modifiers(app, tx);
                    dispatch(app, tx, key, &on_drag, Event::Keyboard(crate::input::Event::ModifiersChanged(m)), Some(point));
                    dispatch(app, tx, key, &on_drag, Event::Mouse(mouse::Event::ButtonPressed(button)), Some(point));
                }
                Phase::Move => {
                    let m = modifiers(app, tx);
                    dispatch(app, tx, key, &on_drag, Event::Keyboard(crate::input::Event::ModifiersChanged(m)), Some(point));
                    dispatch(app, tx, key, &on_drag, Event::Mouse(mouse::Event::CursorMoved { position: point }), Some(point));
                }
                Phase::Up => {
                    app.ui.dragging = None;
                    dispatch(app, tx, key, &on_drag, Event::Mouse(mouse::Event::ButtonReleased(button)), Some(point));
                    super::send(app, tx, Message::MouseReleased);
                }
            }
        })
        .on_secondary_press_local(move |app: &mut App, p, tx| {
            super::touch(app);
            let point = Point::new(p.x, p.y);
            app.ui.pointer = Some((key, point));
            let m = modifiers(app, tx);
            dispatch(app, tx, key, &on_right, Event::Keyboard(crate::input::Event::ModifiersChanged(m)), Some(point));
            dispatch(app, tx, key, &on_right, Event::Mouse(mouse::Event::ButtonPressed(Button::Right)), Some(point));
            dispatch(app, tx, key, &on_right, Event::Mouse(mouse::Event::ButtonReleased(Button::Right)), Some(point));
        })
        .on_wheel(move |app: &mut App, w, tx| {
            let point = Point::new(w.x, w.y);
            let mut m = super::modifiers(w.mods);
            // Shift turns a mouse wheel sideways on macOS before it gets here.
            if w.mods.shift { m.insert(Modifiers::SHIFT); }
            app.ui.modifiers = m;
            dispatch(app, tx, key, &on_wheel, Event::Keyboard(crate::input::Event::ModifiersChanged(m)), Some(point));
            let delta = if w.precise { ScrollDelta::Pixels { x: w.dx, y: w.dy } } else { ScrollDelta::Lines { x: w.dx / 20.0, y: w.dy / 20.0 } };
            let used = dispatch(app, tx, key, &on_wheel, Event::Mouse(mouse::Event::WheelScrolled { delta }), Some(point));
            // Wheel steps end continuous edits such as a velocity change.
            super::send(app, tx, Message::EndEdit);
            used
        })
}

/// A line's look.
#[derive(Debug, Clone, Copy)]
pub struct Stroke {
    pub color: Color,
    pub width: f32,
}

impl Default for Stroke {
    fn default() -> Self {
        Stroke { color: theme::TEXT, width: 1.0 }
    }
}

impl Stroke {
    pub fn with_color(mut self, color: Color) -> Self {
        self.color = color;
        self
    }

    pub fn with_width(mut self, width: f32) -> Self {
        self.width = width;
        self
    }
}

/// Text drawn at a point, its top-left corner.
#[derive(Debug, Clone)]
pub struct Text {
    pub content: String,
    pub position: Point,
    pub color: Color,
    pub size: f32,
}

impl Default for Text {
    fn default() -> Self {
        Text { content: String::new(), position: Point::ORIGIN, color: theme::TEXT, size: theme::SMALL }
    }
}

/// An outline of lines, open or closed.
#[derive(Debug, Clone, Default)]
pub struct Path {
    subpaths: Vec<(Vec<[f32; 2]>, bool)>,
}

#[derive(Default)]
pub struct Builder {
    path: Path,
}

impl Builder {
    pub fn move_to(&mut self, p: Point) {
        self.path.subpaths.push((vec![[p.x, p.y]], false));
    }

    pub fn line_to(&mut self, p: Point) {
        match self.path.subpaths.last_mut() {
            Some((points, false)) => points.push([p.x, p.y]),
            _ => self.move_to(p),
        }
    }

    pub fn close(&mut self) {
        if let Some((_, closed)) = self.path.subpaths.last_mut() {
            *closed = true;
        }
    }
}

impl Path {
    pub fn new(f: impl FnOnce(&mut Builder)) -> Path {
        let mut builder = Builder::default();
        f(&mut builder);
        builder.path
    }

    pub fn line(a: Point, b: Point) -> Path {
        Path { subpaths: vec![(vec![[a.x, a.y], [b.x, b.y]], false)] }
    }

    pub fn rectangle(p: Point, size: Size) -> Path {
        let (x, y, w, h) = (p.x, p.y, size.width, size.height);
        Path { subpaths: vec![(vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]], true)] }
    }

    pub fn circle(center: Point, radius: f32) -> Path {
        let n = 20;
        let points = (0..n)
            .map(|i| {
                let a = i as f32 / n as f32 * std::f32::consts::TAU;
                [center.x + radius * a.cos(), center.y + radius * a.sin()]
            })
            .collect();
        Path { subpaths: vec![(points, true)] }
    }
}

/// A layer and a fingerprint of what it draws. The fingerprint keys the
/// layer's element, so a layer keeps its element only while it draws the same
/// thing. Keyed by position instead, removing a note would hand its slot (and
/// its fill) to the layer after it, and Rune would fade the colour across.
enum Layer {
    Shape(Shape, DefaultHasher),
    El(El, u64),
}

fn fingerprint(parts: &[f32], color: Color, extra: &str) -> u64 {
    let mut h = DefaultHasher::new();
    for v in parts.iter().chain(&[color.r, color.g, color.b, color.a]) { v.to_bits().hash(&mut h); }
    extra.hash(&mut h);
    h.finish()
}

/// What a canvas paints, in order.
pub struct Frame {
    size: Size,
    layers: Vec<Layer>,
}

impl Frame {
    pub fn new(size: Size) -> Frame {
        Frame { size, layers: Vec::new() }
    }

    fn shape(&mut self, points: &[[f32; 2]], color: Color, f: impl FnOnce(Shape) -> Shape) {
        if !matches!(self.layers.last(), Some(Layer::Shape(..))) {
            self.layers.push(Layer::Shape(Shape::new(), DefaultHasher::new()));
        }
        if let Some(Layer::Shape(shape, hash)) = self.layers.last_mut() {
            for [x, y] in points { x.to_bits().hash(hash); y.to_bits().hash(hash); }
            for v in [color.r, color.g, color.b, color.a] { v.to_bits().hash(hash); }
            *shape = f(std::mem::take(shape));
        }
    }

    pub fn fill_rectangle(&mut self, p: Point, size: Size, color: Color) {
        if size.width <= 0.0 || size.height <= 0.0 || color.a <= 0.0 {
            return;
        }
        let (x, y, w, h) = (p.x, p.y, size.width, size.height);
        let points = vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
        self.shape(&points, color, |s| s.fill(points.clone(), color));
    }

    pub fn stroke_rectangle(&mut self, p: Point, size: Size, stroke: Stroke) {
        self.stroke(&Path::rectangle(Point::new(p.x + 0.5, p.y + 0.5), Size::new(size.width - 1.0, size.height - 1.0)), stroke);
    }

    pub fn stroke(&mut self, path: &Path, stroke: Stroke) {
        for (points, closed) in &path.subpaths {
            let points = points.clone();
            let closed = *closed;
            self.shape(&points.clone(), stroke.color, |s| s.stroke_with(points, closed, stroke.width, LineCap::Butt, stroke.color));
        }
    }

    pub fn fill(&mut self, path: &Path, color: Color) {
        for (points, _) in &path.subpaths {
            let points = points.clone();
            self.shape(&points.clone(), color, |s| s.fill(points, color));
        }
    }

    pub fn fill_text(&mut self, t: Text) {
        if t.content.is_empty() {
            return;
        }
        let style = if (t.size - theme::SMALL).abs() < 0.01 { theme::CANVAS } else { rune::TextStyle { size: t.size, ..theme::CANVAS } };
        let id = fingerprint(&[t.position.x, t.position.y, t.size], t.color, &t.content);
        self.layers.push(Layer::El(text(t.content).font(style).color(t.color).single_line().absolute(t.position.x, t.position.y), id));
    }

    /// A rounded box, such as a note or clip body.
    pub fn round_rectangle(&mut self, p: Point, size: Size, radius: f32, color: Color) {
        if size.width <= 0.0 || size.height <= 0.0 {
            return;
        }
        let radius = radius.min(size.width / 2.0).min(size.height / 2.0);
        let id = fingerprint(&[p.x, p.y, size.width, size.height, radius], color, "");
        self.layers.push(Layer::El(el().absolute(p.x, p.y).size(size.width, size.height).radius(radius).fill(color), id));
    }

    pub fn into_el(self) -> El {
        let (w, h) = (self.size.width, self.size.height);
        let mut root = el().clip_rounded(0.0);
        // Two layers can draw the same thing; the count keeps their keys apart.
        let mut seen: HashMap<u64, u32> = HashMap::new();
        for layer in self.layers {
            let (element, id) = match layer {
                Layer::Shape(shape, hash) => (shape_el(shape).absolute(0.0, 0.0).size(w, h), hash.finish()),
                Layer::El(element, id) => (element, id),
            };
            let n = seen.entry(id).or_default();
            root = root.child(element.key(format!("{id:x}-{n}")));
            *n += 1;
        }
        root
    }
}
