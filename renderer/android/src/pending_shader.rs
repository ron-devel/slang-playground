//! A single-slot mailbox for the most recently received shader update,
//! written by `bridge.rs`'s connection loop (a background thread) and
//! drained by the render loop in `lib.rs` (a different thread, ticking
//! every frame) — the two otherwise share no state. Only the *latest*
//! pending update matters, not a queue of them: this app has exactly one
//! `Renderer` and one bridge connection at a time, so there's nothing to
//! gain from keeping ones a newer update has already superseded.

use std::sync::Mutex;

pub struct PendingShader {
    pub vertex_spirv: Vec<u8>,
    pub fragment_spirv: Vec<u8>,
}

static PENDING: Mutex<Option<PendingShader>> = Mutex::new(None);

pub fn set(vertex_spirv: Vec<u8>, fragment_spirv: Vec<u8>) {
    let mut pending = PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *pending = Some(PendingShader {
        vertex_spirv,
        fragment_spirv,
    });
}

pub fn take() -> Option<PendingShader> {
    PENDING
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}
