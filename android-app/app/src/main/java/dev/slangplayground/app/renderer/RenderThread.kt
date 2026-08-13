package dev.slangplayground.app.renderer

import android.util.Log
import android.view.SurfaceHolder

/**
 * Owns the renderer's lifetime independent of the UI thread — started on
 * [dev.slangplayground.app.renderer.RenderSurfaceView.surfaceCreated] and
 * stopped cleanly on `surfaceDestroyed`, never sharing a thread with
 * Compose/HWUI's own drawing.
 *
 * Today this is a placeholder loop that only proves the threading model
 * (start/stop lifecycle tied to the surface, not the Activity). Native
 * Vulkan rendering against the surface's `ANativeWindow` gets wired in
 * next, once renderer-android (the JNI shim over renderer-core) exists —
 * this class is where that call will go.
 */
class RenderThread(private val surfaceHolder: SurfaceHolder) {

    @Volatile
    private var running = true

    private val thread = Thread(::runLoop, "RenderThread")

    fun start() {
        thread.start()
    }

    private fun runLoop() {
        Log.i(TAG, "render thread started, surface valid=${surfaceHolder.surface.isValid}")
        while (running) {
            // Placeholder: native rendering will be driven from here,
            // presenting to surfaceHolder.surface each frame.
            Thread.sleep(FRAME_INTERVAL_MS)
        }
        Log.i(TAG, "render thread stopped")
    }

    fun onSurfaceChanged(width: Int, height: Int) {
        Log.i(TAG, "surface changed: ${width}x$height")
    }

    fun shutdown() {
        running = false
        thread.join()
    }

    private companion object {
        const val TAG = "RenderThread"
        const val FRAME_INTERVAL_MS = 16L
    }
}
