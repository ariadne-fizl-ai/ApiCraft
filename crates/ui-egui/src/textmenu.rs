//! The right-click menu every text field gets: Cut, Copy, Paste, Select all,
//! Undo and Redo, with the editing items disabled in read-only fields.
//!
//! egui text fields have no menu of their own, and there are dozens of them,
//! so this works from the outside: it notices a right-click that landed on
//! the focused text field, shows the menu there, and carries out the choice
//! by replaying the matching keyboard events (Copy, Cut, Paste, Ctrl+A, ...)
//! into that field on the next frame, so the field's own editing, undo and
//! clipboard handling do the work.

use egui::text::CCursorRange;
use egui::text_edit::TextEditState;
use egui::{Context, Event, Id, Key, Modifiers, PointerButton, Pos2, RichText};

use crate::icons;
use crate::theme::palette;

#[derive(Default)]
pub struct TextMenu {
    /// The focused field's selection as of the last frame without a right
    /// press: pressing moves the cursor before the menu opens.
    last: Option<(Id, Option<CCursorRange>)>,
    /// Whether the field under a right press can be edited.
    press_mutable: bool,
    open: Option<Open>,
    /// Edits to replay into a field at the start of the next frame.
    queued: Option<(Id, Option<CCursorRange>, Vec<Event>)>,
    /// Waiting for the clipboard's text to paste into this field.
    paste_into: Option<(Id, Option<CCursorRange>)>,
}

struct Open {
    id: Id,
    pos: Pos2,
    mutable: bool,
    range: Option<CCursorRange>,
    /// Frames since it opened, so the click that opened it doesn't close it.
    age: u32,
}

fn key(key: Key, modifiers: Modifiers) -> Event {
    Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }
}

fn selection(ctx: &Context, id: Id) -> Option<CCursorRange> {
    TextEditState::load(ctx, id).and_then(|s| s.cursor.char_range())
}

impl TextMenu {
    /// Call before drawing the UI: replays a chosen edit into its field.
    pub fn begin(&mut self, ctx: &Context) {
        let Some((id, range, events)) = self.queued.take() else { return };
        if let Some(mut state) = TextEditState::load(ctx, id) {
            if range.is_some() {
                state.cursor.set_char_range(range);
                state.store(ctx, id);
            }
        }
        ctx.memory_mut(|m| m.request_focus(id));
        ctx.input_mut(|i| i.events.extend(events));
        ctx.request_repaint();
    }

    /// The clipboard's text arrived for a Paste.
    pub fn paste(&mut self, ctx: &Context, text: String) {
        if let Some((id, range)) = self.paste_into.take() {
            self.queued = Some((id, range, vec![Event::Paste(text)]));
            ctx.request_repaint();
        }
    }

    /// Call after drawing the UI: opens the menu on a right-click in a text
    /// field, and draws it. Returns true when the clipboard should be read
    /// for a Paste (the host delivers it to [`Self::paste`]).
    pub fn end(&mut self, ctx: &Context) -> bool {
        let (pressed, down, released, pointer) = ctx.input(|i| {
            (
                i.pointer.button_pressed(PointerButton::Secondary),
                i.pointer.button_down(PointerButton::Secondary),
                i.pointer.button_released(PointerButton::Secondary),
                i.pointer.interact_pos(),
            )
        });
        let focused = ctx.memory(|m| m.focused()).filter(|id| TextEditState::load(ctx, *id).is_some());
        if pressed {
            self.press_mutable = ctx.output(|o| o.mutable_text_under_cursor);
        }
        if released {
            let over = |id: Id| match (ctx.read_response(id), pointer) {
                (Some(r), Some(p)) => r.rect.contains(p),
                _ => false,
            };
            if let (Some(id), Some(pos)) = (focused.filter(|id| over(*id)), pointer) {
                let range = match &self.last {
                    Some((last_id, r)) if *last_id == id => *r,
                    _ => selection(ctx, id),
                };
                let mutable = self.press_mutable || ctx.output(|o| o.mutable_text_under_cursor);
                // Put the selection back the way it was before the press.
                if let Some(mut state) = TextEditState::load(ctx, id) {
                    state.cursor.set_char_range(range);
                    state.store(ctx, id);
                }
                self.open = Some(Open { id, pos, mutable, range, age: 0 });
            }
        }
        if !pressed && !down && !released {
            self.last = focused.map(|id| (id, selection(ctx, id)));
        }
        self.show(ctx)
    }

    fn show(&mut self, ctx: &Context) -> bool {
        let Some(open) = &mut self.open else { return false };
        open.age += 1;
        let (id, pos, mutable, range) = (open.id, open.pos, open.mutable, open.range);
        let has_selection = range.is_some_and(|r| !r.is_empty());
        let p = palette(ctx.theme() == egui::Theme::Dark);
        let mut chosen: Option<Vec<Event>> = None;
        let mut want_paste = false;
        let area =
            egui::Area::new(Id::unique("apicraft-text-menu")).movable(false).fade_in(false).order(egui::Order::Foreground).fixed_pos(pos).constrain(true).show(
                ctx,
                |ui| {
                    egui::Frame::menu(ui.style()).show(ui, |ui| {
                        ui.set_min_width(190.0);
                        ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
                        let item = |ui: &mut egui::Ui, icon: &str, label: &str, shortcut: &str, enabled: bool| -> bool {
                            let text = RichText::new(format!("{icon}  {label}"));
                            ui.add_enabled(enabled, egui::Button::new(text).shortcut_text(RichText::new(shortcut).color(p.faint)).frame_when_inactive(false))
                                .clicked()
                        };
                        if item(ui, icons::SCISSORS, "Cut", "Ctrl+X", mutable && has_selection) {
                            chosen = Some(vec![Event::Cut]);
                        }
                        if item(ui, icons::COPY, "Copy", "Ctrl+C", has_selection) {
                            chosen = Some(vec![Event::Copy]);
                        }
                        if item(ui, icons::CLIPBOARD_TEXT, "Paste", "Ctrl+V", mutable) {
                            want_paste = true;
                        }
                        if item(ui, icons::TRASH, "Delete", "Del", mutable && has_selection) {
                            chosen = Some(vec![key(Key::Delete, Modifiers::NONE)]);
                        }
                        ui.separator();
                        if item(ui, icons::SELECTION_ALL, "Select all", "Ctrl+A", true) {
                            chosen = Some(vec![key(Key::A, Modifiers::COMMAND)]);
                        }
                        if mutable {
                            ui.separator();
                            if item(ui, icons::ARROW_U_UP_LEFT, "Undo", "Ctrl+Z", true) {
                                chosen = Some(vec![key(Key::Z, Modifiers::COMMAND)]);
                            }
                            if item(ui, icons::ARROW_U_UP_RIGHT, "Redo", "Ctrl+Y", true) {
                                chosen = Some(vec![key(Key::Z, Modifiers::COMMAND | Modifiers::SHIFT)]);
                            }
                        }
                    });
                },
            );
        let clicked_outside =
            ctx.input(|i| i.pointer.any_pressed()) && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| area.response.rect.contains(p)));
        let escape = ctx.input(|i| i.key_pressed(Key::Escape));
        if let Some(events) = chosen {
            self.queued = Some((id, range, events));
            self.open = None;
            ctx.request_repaint();
        } else if want_paste {
            self.paste_into = Some((id, range));
            self.open = None;
        } else if escape || (clicked_outside && open_age(&self.open) > 1) {
            self.open = None;
        }
        want_paste
    }
}

fn open_age(open: &Option<Open>) -> u32 {
    open.as_ref().map(|o| o.age).unwrap_or(0)
}

/// A right-click menu for selectable text that isn't a text field (labels,
/// table cells): copies the given text.
pub fn copy_menu(resp: &egui::Response, text: &str) {
    resp.context_menu(|ui| {
        ui.set_min_width(160.0);
        if ui.button(format!("{}  Copy", icons::COPY)).clicked() {
            ui.ctx().copy_text(text.to_string());
            ui.close();
        }
    });
}
