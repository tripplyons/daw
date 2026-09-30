//! The note canvas: drawing keys and notes, and turning mouse input into
//! piano roll messages.

use daw_engine::song::PlayMode;
use daw_model::Note;
use daw_model::time::Ticks;
use iced::keyboard::Modifiers;
use iced::widget::canvas::{self, Frame, Geometry};
use iced::{Color, Point, Rectangle, Renderer, Size, Theme, mouse};

use super::{Message, State, notes};
use crate::app::{App, Message as AppMessage};
use crate::panels::timeline::{self, Clicks, RULER_HEIGHT, Wheel};
use crate::theme;

const KEYS_WIDTH: f32 = 40.0;
const EDGE: f32 = 5.0;

pub(super) struct Roll<'a> {
    pub(super) app: &'a App,
    #[allow(dead_code)]
    pub(super) focused: bool,
}

#[derive(Debug, Clone, Copy)]
enum Drag {
    Notes { tick: f64, key: i32 },
    Box { from: Point, to: Point, additive: bool },
    /// Moving the pattern end in the ruler.
    End,
    /// Moving the start marker in the ruler; it is set on release.
    Seek { tick: f64 },
}

#[derive(Debug, Default)]
pub struct CanvasState {
    drag: Option<Drag>,
    modifiers: Modifiers,
    clicks: Clicks,
}

fn is_black(key: i32) -> bool {
    matches!(key.rem_euclid(12), 1 | 3 | 6 | 8 | 10)
}

fn key_name(key: i32) -> String {
    format!("C{}", key / 12 - 1)
}

impl Roll<'_> {
    fn state(&self) -> &State {
        &self.app.piano_roll
    }

    fn key_at(&self, y: f32) -> i32 {
        self.state().top_key - ((y - RULER_HEIGHT) / self.state().key_height).floor() as i32
    }

    fn key_y(&self, key: i32) -> f32 {
        RULER_HEIGHT + (self.state().top_key - key) as f32 * self.state().key_height
    }

    fn tick_at(&self, x: f32) -> f64 {
        self.state().time.tick(x - KEYS_WIDTH)
    }

    fn marker_tick(&self, tick: f64) -> Ticks {
        timeline::marker_tick(tick, self.app.project.grid, self.app.project.signature)
    }

    fn note_rect(&self, note: &Note) -> Rectangle {
        let view = self.state().time;
        let x = KEYS_WIDTH + view.x(note.start as f64);
        let width = (view.x(note.end() as f64) - view.x(note.start as f64)).max(3.0);
        Rectangle { x, y: self.key_y(i32::from(note.key)), width, height: self.state().key_height }
    }

    /// Whether `x` is on the pattern end handle in the ruler.
    fn near_end(&self, x: f32) -> bool {
        let Some(pattern) = self.app.project.pattern(self.app.selected_pattern) else { return false };
        (KEYS_WIDTH + self.state().time.x(pattern.length as f64) - x).abs() <= EDGE + 1.0
    }

    fn hit(&self, p: Point) -> Option<(usize, bool)> {
        notes(self.app).iter().enumerate().rev().find_map(|(i, n)| {
            let r = self.note_rect(n);
            r.contains(p).then_some((i, p.x > r.x + r.width - EDGE && r.width > EDGE * 2.0))
        })
    }
}

impl canvas::Program<AppMessage> for Roll<'_> {
    type State = CanvasState;

    fn update(&self, state: &mut CanvasState, event: &canvas::Event, bounds: Rectangle, cursor: mouse::Cursor) -> Option<canvas::Action<AppMessage>> {
        use iced::keyboard::Event as KeyEvent;
        use iced::mouse::{Button, Event as MouseEvent};
        let publish = |m: Message| Some(canvas::Action::publish(AppMessage::from(m)).and_capture());
        match event {
            canvas::Event::Keyboard(KeyEvent::ModifiersChanged(m)) => {
                state.modifiers = *m;
                None
            }
            canvas::Event::Mouse(MouseEvent::ButtonPressed(button)) => {
                let p = cursor.position_in(bounds)?;
                if p.y < RULER_HEIGHT {
                    if p.x < KEYS_WIDTH || *button != Button::Left {
                        return None;
                    }
                    if self.near_end(p.x) {
                        state.drag = Some(Drag::End);
                        return publish(Message::PatternEnd(self.tick_at(p.x)));
                    }
                    state.drag = Some(Drag::Seek { tick: self.tick_at(p.x) });
                    return Some(canvas::Action::request_redraw().and_capture());
                }
                let key = self.key_at(p.y).clamp(0, 127);
                if p.x < KEYS_WIDTH {
                    return publish(Message::Preview(key as u8));
                }
                let tick = self.tick_at(p.x);
                let double = state.clicks.press(p);
                match (button, self.hit(p)) {
                    (Button::Left, Some((index, _))) if double => publish(Message::Delete(index)),
                    (Button::Left, Some((index, resize))) => {
                        state.drag = Some(Drag::Notes { tick, key });
                        publish(Message::Begin { index, resize, additive: state.modifiers.shift() })
                    }
                    (Button::Left, None) if !timeline::box_select_modifier(state.modifiers) => {
                        let app = self.app;
                        let start = if state.modifiers.command() {
                            tick.max(0.0) as Ticks
                        } else {
                            app.project.grid.snap_floor(tick.max(0.0) as Ticks, app.project.signature)
                        };
                        state.drag = Some(Drag::Notes { tick: start as f64, key });
                        publish(Message::Add { start, key: key as u8 })
                    }
                    (Button::Right, Some((index, _))) => publish(Message::Delete(index)),
                    (Button::Left | Button::Right, _) => {
                        state.drag = Some(Drag::Box { from: p, to: p, additive: state.modifiers.shift() });
                        Some(canvas::Action::request_redraw().and_capture())
                    }
                    _ => None,
                }
            }
            canvas::Event::Mouse(MouseEvent::CursorMoved { .. }) => {
                let p = cursor.position_from(bounds.position())?;
                match state.drag.as_mut()? {
                    Drag::Notes { tick, key } => publish(Message::Drag {
                        ticks: self.tick_at(p.x) - *tick,
                        keys: self.key_at(p.y) - *key,
                        bypass: state.modifiers.command(),
                    }),
                    Drag::Box { to, .. } => {
                        *to = p;
                        Some(canvas::Action::request_redraw())
                    }
                    Drag::End => publish(Message::PatternEnd(self.tick_at(p.x))),
                    Drag::Seek { tick } => {
                        *tick = self.tick_at(p.x);
                        Some(canvas::Action::request_redraw())
                    }
                }
            }
            canvas::Event::Mouse(MouseEvent::ButtonReleased(_)) => match state.drag.take()? {
                Drag::Notes { .. } => publish(Message::End),
                Drag::End => Some(canvas::Action::publish(AppMessage::EndEdit).and_capture()),
                Drag::Seek { tick } => publish(Message::Seek(tick)),
                Drag::Box { from, to, additive } => publish(Message::BoxSelect {
                    from: (self.tick_at(from.x), self.key_at(from.y)),
                    to: (self.tick_at(to.x), self.key_at(to.y)),
                    additive,
                }),
            },
            canvas::Event::Mouse(MouseEvent::WheelScrolled { delta }) => {
                let p = cursor.position_in(bounds)?;
                match Wheel::from_event(self.state().time, *delta, state.modifiers, p.x - KEYS_WIDTH) {
                    Wheel::Time(view) => publish(Message::View(view)),
                    Wheel::Vertical(steps) => publish(Message::ScrollKeys((steps * 3.0).round() as i32)),
                    Wheel::Height(steps) => publish(Message::ZoomKeys { steps, y: p.y }),
                    Wheel::Alternate(steps) => {
                        let (index, _) = self.hit(p)?;
                        publish(Message::Velocity(index, steps * 0.05))
                    }
                }
            }
            _ => None,
        }
    }

    fn draw(&self, state: &CanvasState, renderer: &Renderer, _theme: &Theme, bounds: Rectangle, _cursor: mouse::Cursor) -> Vec<Geometry> {
        let app = self.app;
        let roll = self.state();
        let mut frame = Frame::new(renderer, bounds.size());
        let size = bounds.size();
        let area = Size::new(size.width - KEYS_WIDTH, size.height - RULER_HEIGHT);
        let kh = roll.key_height;
        let bottom_key = self.key_at(size.height) - 1;

        // Key rows.
        for key in bottom_key.max(0)..=roll.top_key.min(127) {
            let y = self.key_y(key);
            if is_black(key) {
                frame.fill_rectangle(Point::new(KEYS_WIDTH, y), Size::new(area.width, kh), Color::from_rgb8(0x10, 0x10, 0x10));
            }
            if key % 12 == 0 {
                frame.fill_rectangle(Point::new(KEYS_WIDTH, y + kh - 1.0), Size::new(area.width, 1.0), theme::GRID_STRONG);
            }
        }
        timeline::draw_grid(&mut frame, roll.time, KEYS_WIDTH, RULER_HEIGHT, area, app.project.signature, app.project.grid);

        // Pattern end.
        if let Some(pattern) = app.project.pattern(app.selected_pattern) {
            let x = (KEYS_WIDTH + roll.time.x(pattern.length as f64)).max(KEYS_WIDTH);
            if x < size.width {
                frame.fill_rectangle(Point::new(x, RULER_HEIGHT), Size::new(size.width - x, area.height), Color { a: 0.45, ..Color::BLACK });
            }
        }

        // Other channels as outlines for context.
        if let Some(pattern) = app.project.pattern(app.selected_pattern) {
            for lane in pattern.lanes.iter().filter(|l| Some(l.channel) != app.selected_channel) {
                for note in &lane.notes {
                    let r = self.note_rect(note);
                    frame.fill_rectangle(Point::new(r.x, r.y + 1.0), Size::new(r.width - 1.0, r.height - 2.0), theme::GRID_STRONG);
                }
            }
        }

        for (index, note) in notes(app).iter().enumerate() {
            let r = self.note_rect(note);
            if r.x + r.width < KEYS_WIDTH {
                continue;
            }
            // Brightness follows velocity.
            let color = if roll.selected.contains(&index) {
                theme::SELECTED
            } else {
                let level = 0.25 + 0.35 * note.velocity;
                Color::from_rgb(level, level, level)
            };
            frame.fill_rectangle(Point::new(r.x, r.y + 1.0), Size::new(r.width - 1.0, r.height - 1.0), color);
        }

        if let Some(Drag::Box { from, to, .. }) = state.drag {
            let top_left = Point::new(from.x.min(to.x), from.y.min(to.y));
            let box_size = Size::new((from.x - to.x).abs(), (from.y - to.y).abs());
            frame.fill_rectangle(top_left, box_size, Color { a: 0.08, ..theme::BRIGHT });
            frame.stroke_rectangle(top_left, box_size, canvas::Stroke::default().with_color(theme::TEXT_DIM).with_width(1.0));
        }

        // Keyboard.
        frame.fill_rectangle(Point::new(0.0, RULER_HEIGHT), Size::new(KEYS_WIDTH, area.height), Color::from_rgb8(0x2c, 0x2c, 0x2c));
        for key in bottom_key.max(0)..=roll.top_key.min(127) {
            let y = self.key_y(key);
            if is_black(key) {
                frame.fill_rectangle(Point::new(0.0, y), Size::new(KEYS_WIDTH * 0.62, kh), theme::HEADER);
            } else if key % 12 == 0 || key % 12 == 5 {
                frame.fill_rectangle(Point::new(0.0, y + kh - 1.0), Size::new(KEYS_WIDTH, 1.0), theme::HEADER);
            }
            if key % 12 == 0 {
                timeline::label(&mut frame, key_name(key), Point::new(KEYS_WIDTH - 18.0, y), theme::TEXT_DIM);
            }
        }

        timeline::draw_ruler(&mut frame, roll.time, KEYS_WIDTH, area.width, app.project.signature);
        // Pattern end handle in the ruler; drag it to change the length.
        if let Some(pattern) = app.project.pattern(app.selected_pattern) {
            let x = KEYS_WIDTH + roll.time.x(pattern.length as f64).round();
            if x >= KEYS_WIDTH && x < size.width {
                frame.fill_rectangle(Point::new(x - 1.0, 0.0), Size::new(2.0, RULER_HEIGHT), theme::TEXT_DIM);
            }
        }
        frame.fill_rectangle(Point::ORIGIN, Size::new(KEYS_WIDTH, RULER_HEIGHT), theme::HEADER);
        // While dragging in the ruler, the marker follows the pointer.
        let seeking = match state.drag {
            Some(Drag::Seek { tick }) => Some(self.marker_tick(tick) as f64),
            _ => None,
        };
        if let Some(tick) = seeking {
            timeline::draw_start_marker(&mut frame, roll.time, KEYS_WIDTH, size.height, tick);
        }
        if app.mode == PlayMode::Pattern(app.selected_pattern) {
            // The start marker stays put; the playhead only moves away from it while playing.
            if seeking.is_none() {
                timeline::draw_start_marker(&mut frame, roll.time, KEYS_WIDTH, size.height, app.pattern_start);
            }
            if app.playing {
                timeline::draw_playhead(&mut frame, roll.time, KEYS_WIDTH, size.height, app.position);
            }
        }
        vec![frame.into_geometry()]
    }

    fn mouse_interaction(&self, state: &CanvasState, bounds: Rectangle, cursor: mouse::Cursor) -> mouse::Interaction {
        let Some(p) = cursor.position_in(bounds) else { return mouse::Interaction::default() };
        match (state.drag, self.hit(p)) {
            (Some(Drag::Notes { .. }), _) => mouse::Interaction::Grabbing,
            (Some(Drag::End), _) => mouse::Interaction::ResizingHorizontally,
            _ if p.y < RULER_HEIGHT && p.x >= KEYS_WIDTH && self.near_end(p.x) => mouse::Interaction::ResizingHorizontally,
            (_, Some((_, true))) => mouse::Interaction::ResizingHorizontally,
            (_, Some(_)) => mouse::Interaction::Grab,
            _ if p.x >= KEYS_WIDTH && p.y >= RULER_HEIGHT => mouse::Interaction::Crosshair,
            _ => mouse::Interaction::default(),
        }
    }
}
