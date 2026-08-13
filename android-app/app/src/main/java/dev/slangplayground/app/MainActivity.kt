package dev.slangplayground.app

import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import dev.slangplayground.app.bridge.BridgeClient
import dev.slangplayground.app.renderer.RenderSurfaceView

/**
 * Hosts the render surface fullscreen — no Compose chrome/insets eating
 * into it, no system bars by default (swipe to reveal them temporarily).
 * Compose-based UI (controls, uniform panel, connection status) will
 * come later as an overlay on top of this, once there's something for it
 * to control; for now the [AndroidView]-wrapped [RenderSurfaceView] is
 * the entire content.
 */
class MainActivity : ComponentActivity() {
    // Independent of the render surface's own lifecycle (RenderThread) —
    // this connects for as long as the Activity is alive, regardless of
    // whether the surface has been created/destroyed in between.
    private val bridgeClient = BridgeClient()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        bridgeClient.start()

        WindowCompat.setDecorFitsSystemWindows(window, false)
        WindowInsetsControllerCompat(window, window.decorView).apply {
            hide(WindowInsetsCompat.Type.systemBars())
            systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
        }

        // Hiding system bars above isn't enough on a display with a
        // cutout (camera housing, sensor strip, ...): by default Android
        // still letterboxes around it, reserving that area even though
        // nothing here needs to avoid it. This is a genuine renderer
        // app, not one with content that could be obscured by a cutout,
        // so always draw into it rather than working around it.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            window.attributes.layoutInDisplayCutoutMode =
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_ALWAYS
                } else {
                    WindowManager.LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES
                }
        }

        setContent {
            val surfaceView = remember { RenderSurfaceView(this) }
            Box(Modifier.fillMaxSize()) {
                AndroidView(
                    factory = { surfaceView },
                    modifier = Modifier.fillMaxSize(),
                )
                PerfOverlay(
                    surfaceView = surfaceView,
                    modifier = Modifier.align(Alignment.TopStart).padding(16.dp),
                )
            }
        }
    }

    override fun onDestroy() {
        bridgeClient.shutdown()
        super.onDestroy()
    }
}

/**
 * Minimal always-on perf readout: per-frame GPU time (the first metric
 * worth surfacing — see `SwapchainRenderer::last_gpu_frame_time_ms`) and
 * the GPU name, so it's obvious at a glance which device a number came
 * from. Deliberately plain (no charting, no history) — this is meant to
 * validate the whole measurement pipeline end to end before investing in
 * a real perf UI.
 */
@Composable
private fun PerfOverlay(surfaceView: RenderSurfaceView, modifier: Modifier = Modifier) {
    val renderThread by surfaceView.currentRenderThread
    // Read directly (not via a `by` delegate) — Compose's snapshot
    // system still tracks these as recomposition triggers either way,
    // and this sidesteps the awkward "what type is a MutableState<T>?
    // elvis'd against a fallback" question a `null`-safe `by` would
    // otherwise raise when `renderThread` itself is null.
    val gpuTimeMs = renderThread?.lastGpuFrameTimeMs?.value
    val deviceInfo = renderThread?.deviceInfo?.value

    val text = buildString {
        append(if (gpuTimeMs != null) "GPU: %.2f ms".format(gpuTimeMs) else "GPU: —")
        deviceInfo?.let { append("\n${it.gpuName}") }
    }

    Text(
        text = text,
        modifier = modifier.background(Color.Black.copy(alpha = 0.5f)).padding(8.dp),
        color = Color.White,
        fontSize = 14.sp,
        fontFamily = FontFamily.Monospace,
    )
}
