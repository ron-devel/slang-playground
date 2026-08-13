//! A queue of touch events, written by the JNI touch callback (the
//! Android UI thread, via `RenderSurfaceView.onTouchEvent`) and drained
//! by the render loop in `lib.rs` (a different thread, ticking every
//! frame) — the two otherwise share no state, same shape as
//! `pending_shader`'s mailbox. Unlike `pending_shader`, this keeps every
//! queued event rather than just the latest: a quick tap (down then up
//! within the same frame) still needs both transitions applied in
//! order, since `SwapchainRenderer::touch_down`/`touch_up` are a state
//! machine, not just a snapshot to overwrite.

use std::sync::Mutex;

pub enum TouchEvent {
    Down { x: f32, y: f32 },
    Move { x: f32, y: f32 },
    Up,
}

static QUEUE: Mutex<Vec<TouchEvent>> = Mutex::new(Vec::new());

pub fn push(event: TouchEvent) {
    QUEUE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(event);
}

pub fn drain() -> Vec<TouchEvent> {
    std::mem::take(
        &mut *QUEUE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    )
}
