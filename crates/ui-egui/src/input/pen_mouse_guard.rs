//! Canvas-only filtering of mouse motion while Windows Ink reports a pen.
//!
//! Windows egui-winit delivers a pen as a `Touch` event *and* an emulated
//! `PointerMoved`. Ordinary mouse motion is only `PointerMoved`. Compare
//! event positions before egui consumes them, retaining the pen's own pointer
//! moves and leaving mouse button/wheel events untouched. Never install a
//! system-wide hook or lock the Windows cursor.
//!
//! If the tablet stops sending hover events without an explicit leave, the
//! suppression expires: unlike a global mouse hook it can never stay stuck.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    #[default]
    Off,
    /// Ignore mouse motion briefly after a pen hover or contact.
    Hover,
    /// Ignore mouse motion only during a pen contact.
    Contact,
}

const HOVER_GRACE_SECS: f64 = 0.8;
const POSITION_TOLERANCE: f32 = 5.0;
const MAX_CONTACTS: usize = 16;

#[derive(Debug, Default)]
pub struct Filter {
    contacts_down: Vec<egui::TouchId>,
    pen_down: Vec<egui::TouchId>,
    hover_ids: Vec<egui::TouchId>,
    last_pen_event: Option<f64>,
}

fn remember(ids: &mut Vec<egui::TouchId>, id: egui::TouchId) {
    if !ids.contains(&id) {
        if ids.len() == MAX_CONTACTS {
            ids.remove(0);
        }
        ids.push(id);
    }
}

fn near(a: egui::Pos2, b: egui::Pos2) -> bool {
    (a.x - b.x).abs() <= POSITION_TOLERANCE && (a.y - b.y).abs() <= POSITION_TOLERANCE
}

impl Filter {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Filter only PointerMoved, before synthetic control-channel events are injected.
    /// Pen's native touch events and pointer moves, buttons, wheel and keyboard survive.
    pub fn filter(&mut self, events: &mut Vec<egui::Event>, mode: Mode, now: f64) {
        if mode == Mode::Off || !now.is_finite() {
            self.reset();
            return;
        }
        let mut pen_positions = Vec::new();
        let mut pen_during_frame = !self.pen_down.is_empty();
        let mut pen_event = false;
        let mut pen_left = false;
        for event in events.iter() {
            let egui::Event::Touch { id, phase, pos, force, .. } = event else {
                continue;
            };
            let known_pen = self.pen_down.contains(id);
            let known_hover = self.hover_ids.contains(id);
            let pressed = self.contacts_down.contains(id);
            match phase {
                egui::TouchPhase::Start => {
                    remember(&mut self.contacts_down, *id);
                    if known_hover || force.is_some() {
                        remember(&mut self.pen_down, *id);
                        pen_positions.push(*pos);
                        pen_during_frame = true;
                        pen_event = true;
                    }
                }
                egui::TouchPhase::Move => {
                    // A Touch Move without a preceding Start is Windows Ink pen hover.
                    // Finger contacts always have Start first.
                    if !pressed {
                        remember(&mut self.hover_ids, *id);
                    }
                    if !pressed || known_pen || force.is_some() {
                        if pressed && force.is_some() {
                            remember(&mut self.pen_down, *id);
                            pen_during_frame = true;
                        }
                        pen_positions.push(*pos);
                        pen_event = true;
                    }
                }
                egui::TouchPhase::End | egui::TouchPhase::Cancel => {
                    if known_pen || known_hover {
                        pen_positions.push(*pos);
                        pen_event = true;
                        pen_left = true;
                    }
                    self.contacts_down.retain(|c| c != id);
                    self.pen_down.retain(|c| c != id);
                    self.hover_ids.retain(|c| c != id);
                }
            }
        }
        if pen_event {
            self.last_pen_event = Some(now);
        }
        if pen_left && self.pen_down.is_empty() && self.hover_ids.is_empty() {
            // Explicit proximity/contact end: don't hold the mouse hostage.
            self.last_pen_event = None;
        }
        let active = match mode {
            Mode::Off => false,
            Mode::Contact => pen_during_frame,
            Mode::Hover => pen_during_frame || self.last_pen_event.is_some_and(|t| (0.0..=HOVER_GRACE_SECS).contains(&(now - t))),
        };
        if active {
            events.retain(|e| {
                if let egui::Event::PointerMoved(p) = e {
                    // A pen's emulated move shares the location of its Touch.
                    // An unrelated mouse move cannot change egui's canvas pointer.
                    pen_positions.iter().any(|t| near(*p, *t))
                } else {
                    true
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(id: u64, phase: egui::TouchPhase, x: f32, force: Option<f32>) -> egui::Event {
        egui::Event::Touch { device_id: egui::TouchDeviceId(1), id: egui::TouchId(id), phase, pos: egui::pos2(x, 10.0), force }
    }

    #[test]
    fn pen_hover_keeps_pen_moves_but_discards_mouse_moves() {
        let mut guard = Filter::default();
        let mut events = vec![
            egui::Event::PointerMoved(egui::pos2(500.0, 10.0)),
            touch(1, egui::TouchPhase::Move, 35.0, None),
            egui::Event::PointerMoved(egui::pos2(35.0, 10.0)),
            egui::Event::PointerButton {
                pos: egui::pos2(500.0, 10.0),
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ];
        guard.filter(&mut events, Mode::Hover, 1.0);
        assert_eq!(events.iter().filter(|e| matches!(e, egui::Event::PointerMoved(_))).count(), 1);
        assert!(events.iter().any(|e| matches!(e, egui::Event::PointerMoved(p) if p.x == 35.0)));
        assert!(events.iter().any(|e| matches!(e, egui::Event::PointerButton { .. })));
    }

    #[test]
    fn contact_only_resumes_mouse_on_lift() {
        let mut guard = Filter::default();
        let mut down = vec![touch(1, egui::TouchPhase::Start, 40.0, Some(0.5))];
        guard.filter(&mut down, Mode::Contact, 1.0);
        let mut moves = vec![egui::Event::PointerMoved(egui::pos2(500.0, 10.0))];
        guard.filter(&mut moves, Mode::Contact, 1.1);
        assert!(moves.is_empty());
        let mut up = vec![touch(1, egui::TouchPhase::End, 40.0, None)];
        guard.filter(&mut up, Mode::Contact, 1.2);
        let mut mouse = vec![egui::Event::PointerMoved(egui::pos2(500.0, 10.0))];
        guard.filter(&mut mouse, Mode::Contact, 1.3);
        assert_eq!(mouse.len(), 1);
    }

    #[test]
    fn hover_mode_has_a_fail_safe_timeout_and_off_clears_state() {
        let mut guard = Filter::default();
        let mut hover = vec![touch(1, egui::TouchPhase::Move, 40.0, None)];
        guard.filter(&mut hover, Mode::Hover, 1.0);
        let mut early = vec![egui::Event::PointerMoved(egui::pos2(500.0, 10.0))];
        guard.filter(&mut early, Mode::Hover, 1.3);
        assert!(early.is_empty());
        let mut late = vec![egui::Event::PointerMoved(egui::pos2(500.0, 10.0))];
        guard.filter(&mut late, Mode::Hover, 2.0);
        assert_eq!(late.len(), 1);
        guard.filter(&mut late, Mode::Off, 2.1);
        assert!(guard.last_pen_event.is_none());
    }

    #[test]
    fn finger_start_without_hover_or_force_is_not_a_pen() {
        let mut guard = Filter::default();
        let mut events = vec![touch(2, egui::TouchPhase::Start, 10.0, None), egui::Event::PointerMoved(egui::pos2(600.0, 10.0))];
        guard.filter(&mut events, Mode::Hover, 1.0);
        assert!(events.iter().any(|e| matches!(e, egui::Event::PointerMoved(_))));
    }
}
