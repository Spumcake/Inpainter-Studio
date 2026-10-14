//! Stylus input beside eframe.
//!
//! egui 0.36 runs on winit 0.30, which delivers a tablet pen on Linux as a
//! plain mouse with no pressure. On Wayland, octotablet reads the tablet
//! protocol instead, which carries pressure and the eraser end. Once a client
//! binds that protocol the compositor stops sending the pen as a pointer, so
//! the pen's motion and contact are fed back into egui as pointer events
//! here. Where the protocol is unavailable, the mouse path stays as it was.

use eframe::egui::{self, Event, Modifiers, PointerButton, Pos2, RawInput};
use octotablet::events::ToolEvent;
use octotablet::tool::{Tool, Type};
use std::time::Duration;

/// How often an idle window wakes to look for a pen coming into range.
/// egui does not see tablet events, so nothing else repaints for them.
const IDLE_POLL: Duration = Duration::from_millis(100);

/// The tool events the canvas uses, in window points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum PenEvent {
    In,
    Down { eraser: bool },
    Pose { pos: Pos2, pressure: Option<f64> },
    Frame,
    Up,
    Out,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PenSample {
    pub pos: Pos2,
    pub pressure: Option<f64>,
}

/// Turns pen events into egui pointer events and pressure samples.
#[derive(Default)]
pub(crate) struct PenInput {
    near: bool,
    down: bool,
    press_pending: bool,
    pressed: bool,
    pos: Option<Pos2>,
    samples: Vec<PenSample>,
    eraser_pressed: bool,
}

impl PenInput {
    /// Drops the samples and eraser press of the previous frame.
    pub fn begin_frame(&mut self) {
        self.samples.clear();
        self.eraser_pressed = false;
    }

    /// Whether the pen is in range of the window or touching it.
    pub fn active(&self) -> bool {
        self.near || self.down
    }

    pub fn down(&self) -> bool {
        self.down
    }

    /// Samples taken while the pen touched the surface this frame.
    pub fn samples(&self) -> &[PenSample] {
        &self.samples
    }

    /// Whether the eraser end touched down this frame.
    pub fn eraser_pressed(&self) -> bool {
        self.eraser_pressed
    }

    pub fn apply(&mut self, event: PenEvent, modifiers: Modifiers, out: &mut Vec<Event>) {
        match event {
            PenEvent::In => self.near = true,
            PenEvent::Down { eraser } => {
                self.near = true;
                self.down = true;
                self.press_pending = true;
                self.eraser_pressed |= eraser;
            }
            // The contact position arrives in the pose after Down, so the
            // press waits for it.
            PenEvent::Pose { pos, pressure } => {
                self.near = true;
                self.pos = Some(pos);
                out.push(Event::PointerMoved(pos));
                self.flush_press(modifiers, out);
                if self.down {
                    self.samples.push(PenSample { pos, pressure });
                }
            }
            PenEvent::Frame => self.flush_press(modifiers, out),
            PenEvent::Up => self.release(modifiers, out),
            PenEvent::Out => {
                self.release(modifiers, out);
                if self.near {
                    out.push(Event::PointerGone);
                }
                self.near = false;
            }
        }
    }

    fn flush_press(&mut self, modifiers: Modifiers, out: &mut Vec<Event>) {
        let Some(pos) = self.pos else {
            return;
        };
        if self.press_pending {
            self.press_pending = false;
            self.pressed = true;
            out.push(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers,
            });
        }
    }

    fn release(&mut self, modifiers: Modifiers, out: &mut Vec<Event>) {
        self.flush_press(modifiers, out);
        if let (true, Some(pos)) = (self.pressed, self.pos) {
            out.push(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers,
            });
        }
        self.pressed = false;
        self.press_pending = false;
        self.down = false;
    }
}

/// The live tablet connection and the pen state it drives.
#[derive(Default)]
pub(crate) struct Tablet {
    manager: Option<octotablet::Manager>,
    input: PenInput,
}

impl Tablet {
    /// Connects to the platform tablet protocol. Unsupported platforms, and
    /// X11 sessions, leave the tablet disconnected and the mouse path in use.
    ///
    /// # Safety
    /// The window and display behind `creation` must outlive the connection.
    /// Call [`Tablet::disconnect`] before the display closes.
    pub unsafe fn connect(creation: &eframe::CreationContext<'_>) -> Self {
        let builder = octotablet::builder::Builder::new().emulate_tool_from_mouse(false);
        // Safety: forwarded to this function's contract.
        let manager = unsafe { builder.build_raw(creation) }.ok();
        Self {
            manager,
            input: PenInput::default(),
        }
    }

    pub fn disconnect(&mut self) {
        self.manager = None;
    }

    pub fn input(&self) -> &PenInput {
        &self.input
    }

    /// Reads pending tablet events into this frame's input. While the pen is
    /// in range, the pointer events it caused are the only ones egui sees.
    pub fn feed(&mut self, ctx: &egui::Context, raw: &mut RawInput) {
        self.input.begin_frame();
        let Some(manager) = self.manager.as_mut() else {
            return;
        };
        let zoom = ctx.zoom_factor();
        let events: Vec<PenEvent> = match manager.pump() {
            Ok(events) => events
                .into_iter()
                .filter_map(|event| match event {
                    octotablet::events::Event::Tool { tool, event } => translate(tool, event, zoom),
                    _ => None,
                })
                .collect(),
            Err(_) => Vec::new(),
        };

        let modifiers = ctx.input(|input| input.modifiers);
        let was_active = self.input.active();
        let mut injected = Vec::new();
        for event in events {
            self.input.apply(event, modifiers, &mut injected);
        }
        if was_active || self.input.active() {
            raw.events.retain(|event| {
                !matches!(
                    event,
                    Event::PointerMoved(_) | Event::PointerButton { .. } | Event::PointerGone
                )
            });
        }
        raw.events.extend(injected);

        if self.input.active() {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(IDLE_POLL);
        }
    }
}

/// Positions arrive in logical pixels from the window's top-left; egui points
/// are logical pixels divided by egui's own zoom.
fn translate(tool: &Tool, event: ToolEvent<'_>, zoom: f32) -> Option<PenEvent> {
    Some(match event {
        ToolEvent::In { .. } => PenEvent::In,
        ToolEvent::Down => PenEvent::Down {
            eraser: tool.tool_type == Some(Type::Eraser),
        },
        ToolEvent::Pose(pose) => PenEvent::Pose {
            pos: Pos2::new(pose.position[0] / zoom, pose.position[1] / zoom),
            pressure: pose
                .pressure
                .get()
                .map(f64::from)
                .filter(|pressure| pressure.is_finite())
                .map(|pressure| pressure.clamp(0.0, 1.0)),
        },
        ToolEvent::Frame(_) => PenEvent::Frame,
        ToolEvent::Up => PenEvent::Up,
        ToolEvent::Out => PenEvent::Out,
        ToolEvent::Added | ToolEvent::Removed | ToolEvent::Button { .. } => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(x: f32, y: f32, pressure: f64) -> PenEvent {
        PenEvent::Pose {
            pos: Pos2::new(x, y),
            pressure: Some(pressure),
        }
    }

    fn run(input: &mut PenInput, events: &[PenEvent]) -> Vec<Event> {
        let mut out = Vec::new();
        for event in events {
            input.apply(*event, Modifiers::NONE, &mut out);
        }
        out
    }

    fn button(event: &Event) -> Option<(Pos2, bool)> {
        match event {
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                ..
            } => Some((*pos, *pressed)),
            _ => None,
        }
    }

    #[test]
    fn contact_presses_at_the_contact_pose_and_samples_carry_pressure() {
        let mut input = PenInput::default();
        input.begin_frame();
        let hover = run(
            &mut input,
            &[PenEvent::In, pose(5.0, 5.0, 0.0), PenEvent::Frame],
        );
        assert!(hover.iter().all(|event| button(event).is_none()));
        assert!(input.samples().is_empty(), "hovering is not drawing");

        input.begin_frame();
        let out = run(
            &mut input,
            &[
                PenEvent::Down { eraser: false },
                pose(10.0, 20.0, 0.3),
                PenEvent::Frame,
                pose(14.0, 22.0, 0.8),
                PenEvent::Frame,
            ],
        );
        let presses: Vec<_> = out.iter().filter_map(button).collect();
        assert_eq!(presses, vec![(Pos2::new(10.0, 20.0), true)]);
        assert_eq!(
            input.samples(),
            &[
                PenSample {
                    pos: Pos2::new(10.0, 20.0),
                    pressure: Some(0.3)
                },
                PenSample {
                    pos: Pos2::new(14.0, 22.0),
                    pressure: Some(0.8)
                },
            ]
        );
        assert!(input.down());
        assert!(!input.eraser_pressed());

        input.begin_frame();
        let out = run(&mut input, &[PenEvent::Up, PenEvent::Frame]);
        assert_eq!(
            out.iter().filter_map(button).collect::<Vec<_>>(),
            vec![(Pos2::new(14.0, 22.0), false)]
        );
        assert!(!input.down());
        assert!(input.active(), "the pen is still in range after lifting");
    }

    #[test]
    fn leaving_range_while_touching_releases_once() {
        let mut input = PenInput::default();
        run(
            &mut input,
            &[
                PenEvent::In,
                PenEvent::Down { eraser: true },
                pose(3.0, 4.0, 0.5),
            ],
        );
        assert!(input.eraser_pressed());
        let out = run(&mut input, &[PenEvent::Out, PenEvent::Up]);
        assert_eq!(
            out.iter().filter_map(button).collect::<Vec<_>>(),
            vec![(Pos2::new(3.0, 4.0), false)]
        );
        assert!(out.iter().any(|event| matches!(event, Event::PointerGone)));
        assert!(!input.active());
    }

    #[test]
    fn contact_without_a_position_presses_nothing() {
        let mut input = PenInput::default();
        let out = run(
            &mut input,
            &[
                PenEvent::Down { eraser: false },
                PenEvent::Frame,
                PenEvent::Up,
            ],
        );
        assert!(out.iter().all(|event| button(event).is_none()));
        assert!(input.samples().is_empty());
    }
}
