//! A single-slot mailbox for the most recently received shader update,
//! written by `bridge.rs`'s connection loop (a background thread) and
//! drained by the render loop in `lib.rs` (a different thread, ticking
//! every frame) — the two otherwise share no state. Only the *latest*
//! pending update matters, not a queue of them: this app has exactly one
//! `Renderer` and one bridge connection at a time, so there's nothing to
//! gain from keeping ones a newer update has already superseded.

use std::sync::Mutex;

pub struct PendingShader {
    pub compute_spirv: Vec<u8>,
    pub entry_point: String,
    pub thread_group_size: [u32; 3],
    pub output_texture_binding: u32,
    pub uniform_buffer_size: u32,
    pub time_offset: Option<u32>,
    pub frame_id_offset: Option<u32>,
}

static PENDING: Mutex<Option<PendingShader>> = Mutex::new(None);

pub fn set(update: PendingShader) {
    let mut pending = PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *pending = Some(update);
}

pub fn take() -> Option<PendingShader> {
    PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}
