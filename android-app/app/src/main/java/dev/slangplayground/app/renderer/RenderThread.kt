package dev.slangplayground.app.renderer

import android.util.Log
import android.view.SurfaceHolder
import androidx.compose.runtime.mutableStateOf

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

    /**
     * Written only from this class's own render thread; read by Compose
     * on the UI thread via [PerfOverlay] — a plain Compose `State`
     * (rather than e.g. a `StateFlow`) is enough since there's exactly
     * one writer and Compose already knows how to observe state reads
     * cross-thread for recomposition.
     */
    val lastGpuFrameTimeMs = mutableStateOf<Float?>(null)

    /**
     * Set once, right after the native renderer is created — see
     * [DeviceInfo]. `null` until then (or if renderer creation failed).
     */
    val deviceInfo = mutableStateOf<DeviceInfo?>(null)

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
            deviceInfo.value = DeviceInfo.fromJson(nativeGetDeviceInfoJson(renderer))
        }

        while (running && renderer != 0L) {
            nativeRenderFrame(renderer)
            // -1 means no frame has finished yet (the very first
            // iteration) — see nativeGetLastGpuFrameTimeMs's docs.
            val gpuTimeMs = nativeGetLastGpuFrameTimeMs(renderer)
            if (gpuTimeMs >= 0f) {
                lastGpuFrameTimeMs.value = gpuTimeMs
            }
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

    /**
     * Forwards one touch event from [RenderSurfaceView.onTouchEvent] (the
     * UI thread) into the native touch-input queue, applied on this
     * render thread's next frame — matches the web playground's own
     * mouse handling for the MOUSE_POSITION uniform (see
     * `renderer-core`'s `SwapchainRenderer::touch_down`).
     */
    fun onTouchEvent(action: Int, x: Float, y: Float) {
        nativeTouchEvent(action, x, y)
    }

    fun shutdown() {
        running = false
        thread.join()
    }

    private external fun nativeCreateRenderer(surface: android.view.Surface): Long

    private external fun nativeRenderFrame(handle: Long): Boolean

    private external fun nativeGetLastGpuFrameTimeMs(handle: Long): Float

    private external fun nativeGetDeviceInfoJson(handle: Long): String?

    private external fun nativeTouchEvent(action: Int, x: Float, y: Float)

    private external fun nativeDestroyRenderer(handle: Long)

    private companion object {
        const val TAG = "RenderThread"
        const val FRAME_INTERVAL_MS = 16L

        init {
            System.loadLibrary("renderer_android")
        }
    }
}

/**
 * GPU/driver + Android build identity for one perf-measurement session —
 * the fixed context every [RenderThread.lastGpuFrameTimeMs] sample
 * should be interpreted against, since the same shader's GPU time isn't
 * comparable across devices/drivers without it. Shaped to match what's
 * expected to become the bridge protocol's `DeviceInfo` message once
 * perf samples start flowing to the web playground over it — the GPU
 * fields below come from `renderer-android`'s `nativeGetDeviceInfoJson`
 * (which in turn mirrors `renderer_core::DeviceInfo`); the Android
 * fields have no native equivalent (they're JVM statics), so they're
 * read here directly rather than round-tripped through JNI.
 */
data class DeviceInfo(
    val gpuName: String,
    val driverVersion: Long,
    val vendorId: Long,
    val deviceId: Long,
    val apiVersion: Long,
    val androidModel: String = android.os.Build.MODEL,
    val androidManufacturer: String = android.os.Build.MANUFACTURER,
    val androidRelease: String = android.os.Build.VERSION.RELEASE,
    val androidSdkInt: Int = android.os.Build.VERSION.SDK_INT,
    val androidFingerprint: String = android.os.Build.FINGERPRINT,
) {
    fun toJson(): org.json.JSONObject =
        org.json.JSONObject()
            .put("gpuName", gpuName)
            .put("driverVersion", driverVersion)
            .put("vendorId", vendorId)
            .put("deviceId", deviceId)
            .put("apiVersion", apiVersion)
            .put("androidModel", androidModel)
            .put("androidManufacturer", androidManufacturer)
            .put("androidRelease", androidRelease)
            .put("androidSdkInt", androidSdkInt)
            .put("androidFingerprint", androidFingerprint)

    companion object {
        /** `null` if `json` is `null` or doesn't parse. */
        fun fromJson(json: String?): DeviceInfo? {
            if (json == null) return null
            return try {
                val parsed = org.json.JSONObject(json)
                DeviceInfo(
                    gpuName = parsed.getString("gpuName"),
                    driverVersion = parsed.getLong("driverVersion"),
                    vendorId = parsed.getLong("vendorId"),
                    deviceId = parsed.getLong("deviceId"),
                    apiVersion = parsed.getLong("apiVersion"),
                )
            } catch (e: org.json.JSONException) {
                Log.e("RenderThread", "failed to parse native device info JSON: $json", e)
                null
            }
        }
    }
}
