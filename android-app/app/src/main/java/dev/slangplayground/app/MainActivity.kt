package dev.slangplayground.app

import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
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
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

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
            AndroidView(
                factory = { context -> RenderSurfaceView(context) },
                modifier = Modifier.fillMaxSize(),
            )
        }
    }
}
