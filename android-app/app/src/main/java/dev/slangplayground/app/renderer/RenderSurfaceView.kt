package dev.slangplayground.app.renderer

import android.content.Context
import android.util.AttributeSet
import android.view.MotionEvent
import android.view.SurfaceHolder
import android.view.SurfaceView
import androidx.compose.runtime.mutableStateOf

/**
 * A [SurfaceView] dedicated to the native Vulkan renderer, deliberately
 * kept outside Compose/HWUI's own drawing: rendering runs on its own
 * [RenderThread], driven only by this surface's lifecycle callbacks, not
 * by the Activity or the Compose recomposition loop.
 */
class RenderSurfaceView @JvmOverloads constructor(
    context: Context,
    attrs: AttributeSet? = null,
) : SurfaceView(context, attrs), SurfaceHolder.Callback {

    private var renderThread: RenderThread? = null

    /**
     * Exposed as Compose `State` (rather than a plain nullable property)
     * so a Compose overlay drawn alongside this view — see
     * `MainActivity`'s `PerfOverlay` — can observe the surface's
     * create/destroy lifecycle and, through the [RenderThread] it holds,
     * per-frame perf data, without this view needing to know Compose UI
     * exists at all beyond publishing this one field.
     */
    val currentRenderThread = mutableStateOf<RenderThread?>(null)

    init {
        holder.addCallback(this)
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        val thread = RenderThread(holder)
        renderThread = thread
        currentRenderThread.value = thread
        thread.start()
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        renderThread?.onSurfaceChanged(width, height)
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        renderThread?.shutdown()
        renderThread = null
        currentRenderThread.value = null
    }

    /**
     * Forwards touch events to the native renderer for shaders using the
     * MOUSE_POSITION uniform (e.g. ocean.slang) — see
     * [RenderThread.onTouchEvent]. `actionMasked` (rather than `action`)
     * strips out the pointer-index bits multi-touch events pack into
     * `action`, since only the primary pointer's gesture matters here.
     */
    override fun onTouchEvent(event: MotionEvent): Boolean {
        renderThread?.onTouchEvent(event.actionMasked, event.x, event.y)
        return true
    }
}
