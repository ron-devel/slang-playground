package dev.slangplayground.app

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.ui.Modifier
import androidx.compose.ui.viewinterop.AndroidView
import dev.slangplayground.app.renderer.RenderSurfaceView

/**
 * Hosts the Compose UI tree (controls, uniform panel, connection status —
 * none of that exists yet). The render surface itself is embedded via
 * [AndroidView] rather than drawn by Compose: it's a plain [android.view.SurfaceView]
 * driving its own dedicated render thread, kept deliberately independent
 * of Compose/HWUI's drawing and recomposition.
 */
class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        setContent {
            MaterialTheme {
                Surface(modifier = Modifier.fillMaxSize()) {
                    Box(modifier = Modifier.fillMaxSize()) {
                        AndroidView(
                            factory = { context -> RenderSurfaceView(context) },
                            modifier = Modifier.fillMaxSize(),
                        )
                    }
                }
            }
        }
    }
}
