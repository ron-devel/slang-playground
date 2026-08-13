package dev.slangplayground.app.renderer

import android.content.Context
import android.util.AttributeSet
import android.view.SurfaceHolder
import android.view.SurfaceView

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

    init {
        holder.addCallback(this)
    }

    override fun surfaceCreated(holder: SurfaceHolder) {
        val thread = RenderThread(holder)
        renderThread = thread
        thread.start()
    }

    override fun surfaceChanged(holder: SurfaceHolder, format: Int, width: Int, height: Int) {
        renderThread?.onSurfaceChanged(width, height)
    }

    override fun surfaceDestroyed(holder: SurfaceHolder) {
        renderThread?.shutdown()
        renderThread = null
    }
}
