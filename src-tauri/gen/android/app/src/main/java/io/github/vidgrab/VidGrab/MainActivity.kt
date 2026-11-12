package io.github.vidgrab.VidGrab

import android.content.Intent
import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import io.github.vidgrab.VidGrabAndroidPlugin

class MainActivity : TauriActivity() {
    private val urlRegex = Regex("""https?://\S+""")

    override fun onCreate(savedInstanceState: Bundle?) {
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        extractColdShareIntent(intent)
    }

    private fun extractColdShareIntent(intent: Intent?) {
        if (intent?.action != Intent.ACTION_SEND) return
        val sharedText = intent.getStringExtra(Intent.EXTRA_TEXT) ?: return
        val matched = urlRegex.find(sharedText)?.value?.trimEnd('.', ',', ')', ']') ?: return
        VidGrabAndroidPlugin.enqueueSharedUrl(matched)
    }
}
