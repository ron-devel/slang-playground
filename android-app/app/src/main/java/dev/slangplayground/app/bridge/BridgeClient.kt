package dev.slangplayground.app.bridge

import android.util.Log

/**
 * Connects to the bridge daemon as a live target peer and keeps
 * reconnecting for as long as the app is alive. Runs on its own thread,
 * independent of [dev.slangplayground.app.renderer.RenderThread] —
 * rendering doesn't depend on a bridge connection existing, and vice
 * versa.
 *
 * `url` points at the bridge daemon's WebSocket endpoint. The daemon
 * always runs on the developer's host machine, reachable from the device
 * over the `adb reverse` tunnel `bridge-cli` sets up, so `127.0.0.1` here
 * means "the tunnel," not this device itself.
 */
class BridgeClient(
    private val url: String = DEFAULT_URL,
    private val displayName: String = android.os.Build.MODEL,
) {

    @Volatile
    private var running = true

    private val thread = Thread(::runLoop, "BridgeClient")

    fun start() {
        thread.start()
    }

    private fun runLoop() {
        while (running) {
            Log.i(TAG, "connecting to bridge daemon at $url as \"$displayName\"")
            val connected = nativeConnectAndWait(url, displayName)
            if (!running) break
            if (connected) {
                Log.i(TAG, "bridge session ended, reconnecting in ${RECONNECT_DELAY_MS}ms")
            } else {
                Log.w(TAG, "bridge connect failed, retrying in ${RECONNECT_DELAY_MS}ms")
            }
            try {
                Thread.sleep(RECONNECT_DELAY_MS)
            } catch (_: InterruptedException) {
                break
            }
        }
        Log.i(TAG, "bridge client stopped")
    }

    fun shutdown() {
        running = false
        // Covers both places this thread can be blocked: inside the
        // native connect-and-wait call (nativeRequestShutdown cancels
        // it) or in the reconnect-delay sleep above (interrupt breaks
        // it immediately rather than joining up to RECONNECT_DELAY_MS).
        nativeRequestShutdown()
        thread.interrupt()
        thread.join()
    }

    private external fun nativeConnectAndWait(url: String, displayName: String): Boolean

    private external fun nativeRequestShutdown()

    private companion object {
        const val TAG = "BridgeClient"
        const val DEFAULT_URL = "ws://127.0.0.1:8800/ws"
        const val RECONNECT_DELAY_MS = 2000L

        init {
            System.loadLibrary("renderer_android")
        }
    }
}
