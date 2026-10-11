use eframe::egui::{self, Key, Modifiers};
use parterre_core::text_size::{Wheel, larger, smaller};

/// Applies Ctrl+wheel and pinch in the window `ui` is in to `size`, and with `with_shortcuts`
/// Ctrl+plus, minus and 0 too. Call it once per window and frame, and not where the graph takes
/// the wheel for its own zoom.
pub fn read_input(ui: &egui::Ui, size: &mut f32, with_shortcuts: bool) {
    let (delta, now) = ui.input(|i| (i.zoom_delta(), i.time));
    let id = egui::Id::new(("text-size-wheel", ui.ctx().viewport_id()));
    let steps = ui.data_mut(|d| d.get_temp_mut_or_default::<Wheel>(id).steps(delta, now));
    for _ in 0..steps {
        *size = larger(*size);
    }
    for _ in steps..0 {
        *size = smaller(*size);
    }
    if with_shortcuts {
        ui.input_mut(|i| {
            let mut command = |k: Key| i.consume_key(Modifiers::COMMAND, k);
            if command(Key::Plus) || command(Key::Equals) {
                *size = larger(*size);
            }
            if command(Key::Minus) {
                *size = smaller(*size);
            }
            if command(Key::Num0) {
                *size = 1.0;
            }
        });
    }
}
