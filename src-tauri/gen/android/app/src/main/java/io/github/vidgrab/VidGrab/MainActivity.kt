package io.github.vidgrab.VidGrab

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.os.Bundle
import android.util.Log
import android.widget.Toast
import androidx.activity.enableEdgeToEdge
import androidx.appcompat.app.AlertDialog
import io.github.vidgrab.VidGrabAndroidPlugin
import java.io.File

class MainActivity : TauriActivity() {
    private val urlRegex = Regex("""https?://\S+""")

    override fun onCreate(savedInstanceState: Bundle?) {
        installCrashHandler()
        checkPreviousCrash()
        enableEdgeToEdge()
        super.onCreate(savedInstanceState)
        extractColdShareIntent(intent)
        Thread {
            try {
                VidGrabAndroidPlugin.initEngines(applicationContext)
            } catch (t: Throwable) {
                Log.e("VidGrabCrash", "Cold engine initialization failed", t)
            }
        }.start()
    }

    @Deprecated("Deprecated in Java")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        super.onActivityResult(requestCode, resultCode, data)
        if (requestCode == SafStorageHelper.REQUEST_CODE_OPEN_TREE && resultCode == RESULT_OK) {
            data?.data?.let { treeUri ->
                SafStorageHelper.persistTreePermission(this, treeUri)
                Toast.makeText(this, "Dossier SAF autorisé", Toast.LENGTH_SHORT).show()
            }
        }
    }

    private fun installCrashHandler() {
        val defaultHandler = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, throwable ->
            try {
                Log.e("VidGrabCrash", "FATAL CRASH on ${thread.name}", throwable)
                val crashFile = File(filesDir, "vidgrab_crash.txt")
                crashFile.writeText(
                    "CRASH INFO:\n" +
                    "Thread: ${thread.name}\n" +
                    "Android SDK: ${android.os.Build.VERSION.SDK_INT}\n" +
                    "Device: ${android.os.Build.MANUFACTURER} ${android.os.Build.MODEL}\n\n" +
                    "STACK TRACE:\n" +
                    throwable.stackTraceToString()
                )
            } catch (t: Throwable) {
                Log.e("VidGrabCrash", "Failed writing crash log", t)
            }
            defaultHandler?.uncaughtException(thread, throwable)
        }
    }

    private fun checkPreviousCrash() {
        try {
            val crashFile = File(filesDir, "vidgrab_crash.txt")
            if (crashFile.exists() && crashFile.length() > 0) {
                val report = crashFile.readText()
                Log.w("VidGrabCrash", "Previous crash report detected:\n$report")
                AlertDialog.Builder(this)
                    .setTitle("Rapport de crash précédent")
                    .setMessage(report.take(1500))
                    .setPositiveButton("Copier le rapport") { _, _ ->
                        val cm = getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                        cm?.setPrimaryClip(ClipData.newPlainText("VidGrab Crash", report))
                        Toast.makeText(this, "Rapport copié dans le presse-papiers", Toast.LENGTH_SHORT).show()
                        crashFile.delete()
                    }
                    .setNegativeButton("Fermer") { _, _ ->
                        crashFile.delete()
                    }
                    .show()
            }
        } catch (t: Throwable) {
            Log.e("VidGrabCrash", "Failed checking previous crash", t)
        }
    }

    private fun extractColdShareIntent(intent: Intent?) {
        if (intent?.action != Intent.ACTION_SEND) return
        val sharedText = intent.getStringExtra(Intent.EXTRA_TEXT) ?: return
        val matched = urlRegex.find(sharedText)?.value?.trimEnd('.', ',', ')', ']') ?: return
        VidGrabAndroidPlugin.enqueueSharedUrl(matched)
    }
}
