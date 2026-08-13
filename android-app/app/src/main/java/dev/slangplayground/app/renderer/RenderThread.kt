package dev.slangplayground.app.renderer

import android.util.Log
import android.view.SurfaceHolder

/**
 * Owns the renderer's lifetime independent of the UI thread — started on
 * [dev.slangplayground.app.renderer.RenderSurfaceView.surfaceCreated] and
 * stopped cleanly on `surfaceDestroyed`, never sharing a thread with
 * Compose/HWUI's own drawing.
 *
 * Creates/destroys a native Vulkan instance + device (via renderer-android,
 * the JNI shim over renderer-core) for the lifetime of this thread — a
 * first proof that the whole native toolchain path works end to end on a
 * real device. Actual rendering against the surface's `ANativeWindow` is
 * the next increment, once this is confirmed working.
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

        val renderer = nativeCreateRenderer()
        if (renderer == 0L) {
            Log.e(TAG, "failed to create native renderer (Vulkan instance/device)")
        } else {
            Log.i(TAG, "native renderer created (Vulkan instance + device)")
        }

        while (running) {
            // Placeholder: native rendering will be driven from here,
            // presenting to surfaceHolder.surface each frame.
            Thread.sleep(FRAME_INTERVAL_MS)
        }

        if (renderer != 0L) {
            nativeDestroyRenderer(renderer)
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

    private external fun nativeCreateRenderer(): Long

    private external fun nativeDestroyRenderer(handle: Long)

    private companion object {
        const val TAG = "RenderThread"
        const val FRAME_INTERVAL_MS = 16L

        init {
            System.loadLibrary("renderer_android")
        }
    }
}
