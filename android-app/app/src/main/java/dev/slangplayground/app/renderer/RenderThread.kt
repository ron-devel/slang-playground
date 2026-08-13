package dev.slangplayground.app.renderer

import android.util.Log
import android.view.SurfaceHolder

/**
 * Owns the renderer's lifetime independent of the UI thread — started on
 * [dev.slangplayground.app.renderer.RenderSurfaceView.surfaceCreated] and
 * stopped cleanly on `surfaceDestroyed`, never sharing a thread with
 * Compose/HWUI's own drawing.
 *
 * Creates a native Vulkan instance/device/swapchain (via renderer-android,
 * the JNI shim over renderer-core) for this surface, then continuously
 * renders and presents frames until the surface is destroyed. The
 * rendered content is a fixed test triangle for now — receiving a real
 * shader over the bridge is future work, once the app actually connects
 * to the bridge daemon.
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

        val renderer = nativeCreateRenderer(surfaceHolder.surface)
        if (renderer == 0L) {
            Log.e(TAG, "failed to create native renderer (Vulkan instance/device/swapchain)")
        } else {
            Log.i(TAG, "native renderer created (Vulkan instance + device + swapchain)")
        }

        while (running && renderer != 0L) {
            nativeRenderFrame(renderer)
            // Placeholder pacing until there's a reason for anything
            // fancier (e.g. driving off the swapchain's own present
            // timing) — FIFO present mode already blocks on vsync, so
            // this just keeps the loop from spinning faster than that.
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

    private external fun nativeCreateRenderer(surface: android.view.Surface): Long

    private external fun nativeRenderFrame(handle: Long): Boolean

    private external fun nativeDestroyRenderer(handle: Long)

    private companion object {
        const val TAG = "RenderThread"
        const val FRAME_INTERVAL_MS = 16L

        init {
            System.loadLibrary("renderer_android")
        }
    }
}
