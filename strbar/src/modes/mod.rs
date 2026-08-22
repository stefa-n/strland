#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IslandMode {
    Clock,
    Volume,
    Notification,
}

// Shared math helpers used by the drawing functions.
pub(crate) fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub(crate) fn ease_out_cubic(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

pub(crate) fn smoothstep01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

pub mod clock;
pub mod control;
pub mod notification;
pub mod status;
pub mod volume;

pub use clock::*;
pub use control::*;
pub use notification::*;
pub use status::*;
pub use volume::*;
