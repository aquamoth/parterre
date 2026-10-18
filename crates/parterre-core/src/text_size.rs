/// The sizes Ctrl+plus and minus and the wheel step through, as in browsers.
pub const STEPS: [f32; 13] = [
    0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0, 2.5, 3.0,
];
pub const MIN: f32 = STEPS[0];
pub const MAX: f32 = STEPS[STEPS.len() - 1];
const DEFAULT: f32 = 1.0;
const STEP_TOLERANCE: f32 = 0.001;

pub fn larger(size: f32) -> f32 {
    STEPS
        .into_iter()
        .find(|&s| s > size + STEP_TOLERANCE)
        .unwrap_or(MAX)
}

pub fn smaller(size: f32) -> f32 {
    STEPS
        .into_iter()
        .rev()
        .find(|&s| s < size - STEP_TOLERANCE)
        .unwrap_or(MIN)
}

/// For a size as saved, or given on the command line.
pub fn sanitize(size: f32) -> f32 {
    if size.is_finite() {
        size.clamp(MIN, MAX)
    } else {
        DEFAULT
    }
}

/// Turns Ctrl+wheel and pinch into steps, however egui spreads a wheel notch over frames: a
/// notch is one step, and so is pinching by a fifth.
#[derive(Clone, Copy, Debug, Default)]
pub struct Wheel {
    /// Gathered towards the next step.
    log_zoom: f32,
    last_zoom_secs: f64,
}

/// A pause this long ends a gesture, and its remainder is dropped. Pinch events don't come
/// every frame, so frames without zoom don't end one.
const GESTURE_PAUSE_SECS: f64 = 0.5;
const ZOOM_PER_STEP: f32 = 1.2;

impl Wheel {
    /// Adds one frame's zoom (egui's `zoom_delta`); returns how many steps to go up
    /// (positive) or down (negative).
    pub fn steps(&mut self, delta: f32, now_secs: f64) -> i32 {
        if delta == 1.0 {
            return 0;
        }
        if now_secs - self.last_zoom_secs > GESTURE_PAUSE_SECS {
            self.log_zoom = 0.0;
        }
        self.last_zoom_secs = now_secs;
        let step = ZOOM_PER_STEP.ln();
        self.log_zoom += delta.ln();
        let steps = (self.log_zoom / step).trunc();
        self.log_zoom -= steps * step;
        steps as i32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_up_and_down() {
        assert_eq!(larger(1.0), 1.1);
        assert_eq!(smaller(1.0), 0.9);
        assert_eq!(larger(1.25), 1.5);
        assert_eq!(smaller(1.25), 1.1);
    }

    #[test]
    fn stops_at_the_ends() {
        assert_eq!(larger(MAX), MAX);
        assert_eq!(smaller(MIN), MIN);
    }

    #[test]
    fn a_size_between_steps_goes_to_the_neighbouring_steps() {
        assert_eq!(larger(1.2), 1.25);
        assert_eq!(smaller(1.2), 1.1);
    }

    #[test]
    fn saved_sizes_are_kept_within_range() {
        assert_eq!(sanitize(1.2), 1.2);
        assert_eq!(sanitize(0.1), MIN);
        assert_eq!(sanitize(9.0), MAX);
        assert_eq!(sanitize(f32::NAN), 1.0);
        assert_eq!(sanitize(f32::INFINITY), 1.0);
    }

    /// A wheel notch as egui smooths it, starting at `t`: exp(40 / 200) in all (a line of 40
    /// points), over frames 16 ms apart, with a tail.
    fn notch(wheel: &mut Wheel, sign: f32, t: f64) -> i32 {
        let total = sign * 0.2;
        let parts = [0.5, 0.2, 0.1, 0.1, 0.05, 0.05];
        let mut steps = 0;
        for (i, part) in parts.into_iter().enumerate() {
            steps += wheel.steps((total * part).exp(), t + i as f64 * 0.016);
        }
        steps
    }

    #[test]
    fn a_wheel_notch_is_one_step() {
        let mut wheel = Wheel::default();
        assert_eq!(notch(&mut wheel, 1.0, 10.0), 1);
        assert_eq!(notch(&mut wheel, 1.0, 11.0), 1);
        assert_eq!(notch(&mut wheel, -1.0, 12.0), -1);
    }

    #[test]
    fn notches_in_quick_succession_are_a_step_each() {
        let mut wheel = Wheel::default();
        let steps: i32 = (0..5).map(|i| notch(&mut wheel, 1.0, i as f64 * 0.1)).sum();
        assert_eq!(steps, 5);
    }

    #[test]
    fn a_slow_pinch_adds_up_across_frames_without_zoom() {
        // Events every few frames, with frames of no zoom (1.0) between them.
        let mut wheel = Wheel::default();
        let mut steps = 0;
        for i in 0..10 {
            let t = f64::from(i) * 0.05;
            steps += wheel.steps(1.02, t);
            steps += wheel.steps(1.0, t + 0.016);
        }
        // 1.02^10 = 1.22: one step.
        assert_eq!(steps, 1);
    }

    #[test]
    fn a_small_pinch_is_forgotten_after_a_pause() {
        let mut wheel = Wheel::default();
        assert_eq!(wheel.steps(1.1, 1.0), 0);
        assert_eq!(wheel.steps(1.1, 2.0), 0);
    }

    #[test]
    fn a_long_pinch_steps_again_and_again() {
        let mut wheel = Wheel::default();
        let steps: i32 = (0..20)
            .map(|i| wheel.steps(1.05, f64::from(i) * 0.016))
            .sum();
        // 1.05^20 = 2.65, about five fifths.
        assert_eq!(steps, 5);
        assert_eq!(wheel.steps(0.5, 0.4), -3);
    }
}
