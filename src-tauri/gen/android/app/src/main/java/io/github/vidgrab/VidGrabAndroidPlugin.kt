package io.github.vidgrab

import android.Manifest
import android.app.Activity
import android.app.DownloadManager
import android.app.NotificationChannel
import android.app.NotificationManager
import android.content.ClipData
import android.content.ClipboardManager
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.webkit.MimeTypeMap
import android.webkit.WebView
import androidx.core.app.ActivityCompat
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.yausername.ffmpeg.FFmpeg
import com.yausername.youtubedl_android.YoutubeDL
import com.yausername.youtubedl_android.YoutubeDLRequest
import org.json.JSONObject
import java.io.File
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicInteger

@InvokeArg
class ArgsArgs {
    var args: List<String> = emptyList()
}

@InvokeArg
class DownloadArgs {
    var taskId: String = ""
    var args: List<String> = emptyList()
    var onLine: Channel? = null
}

@InvokeArg
class TaskArgs {
    var taskId: String = ""
}

@InvokeArg
class PathArgs {
    var path: String = ""
    var revealParent: Boolean = false
}

@InvokeArg
class TextArgs {
    var text: String = ""
}

@InvokeArg
class NotificationArgs {
    var title: String = ""
    var body: String = ""
}

@TauriPlugin
class VidGrabAndroidPlugin(private val activity: Activity) : Plugin(activity) {
    private val workerPool = Executors.newCachedThreadPool()
    private val notifIdCounter = AtomicInteger(1000)
    private var tauriWebView: WebView? = null

    override fun load(webView: WebView) {
        super.load(webView)
        tauriWebView = webView
        workerPool.execute {
            runCatching { ensureInitialized() }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        if (intent.action == Intent.ACTION_SEND) {
            val shared = intent.getStringExtra(Intent.EXTRA_TEXT) ?: return
            val url = Regex("""https?://\S+""").find(shared)?.value?.trimEnd('.', ',', ')', ']') ?: return
            enqueueSharedUrl(url)
            tauriWebView?.post {
                val quoted = JSONObject.quote(url)
                tauriWebView?.evaluateJavascript(
                    "window.dispatchEvent(new CustomEvent('vidgrab-android-share', { detail: $quoted }));",
                    null
                )
            }
        }
    }

    private fun ensureInitialized() {
        val err = initError
        if (err != null) {
            initEngines(activity.applicationContext)
            return
        }
        if (!initDone) {
            initEngines(activity.applicationContext)
        }
        createNotificationChannel(activity.applicationContext)
    }

    private fun createNotificationChannel(ctx: Context) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val mgr = ctx.getSystemService(Context.NOTIFICATION_SERVICE) as? NotificationManager ?: return
            val channel = NotificationChannel(
                CHANNEL_ID,
                "Téléchargements",
                NotificationManager.IMPORTANCE_DEFAULT
            ).apply {
                description = "Alertes de fin de téléchargement et d'abonnements"
            }
            mgr.createNotificationChannel(channel)
        }
    }

    private fun canWriteDirectoryDirectly(dir: File): Boolean {
        return try {
            if (!dir.exists() && !dir.mkdirs()) {
                return false
            }
            val probe = File(dir, ".vidgrab_probe_${System.nanoTime()}")
            val ok = probe.createNewFile()
            if (ok) probe.delete()
            ok
        } catch (_: Throwable) {
            false
        }
    }

    /**
     * Extracts target `-o` output path and stages inside internal storage when Android Scoped Storage
     * blocks direct `/storage/emulated/0/...` writes, then publishes outputs cleanly to MediaStore.
     */
    private fun prepareScopedArgs(taskId: String, rawArgs: List<String>): Triple<List<String>, File?, File?> {
        var originalOutDir: File? = null
        val mut = rawArgs.toMutableList()
        for (i in 0 until mut.size - 1) {
            if (mut[i] == "-o" || mut[i] == "--output") {
                val template = mut[i + 1]
                val parent = File(template).parentFile
                if (parent != null && !canWriteDirectoryDirectly(parent)) {
                    originalOutDir = parent
                    val stageDir = File(activity.filesDir, "ytdl_stage/$taskId").apply { mkdirs() }
                    val fileName = File(template).name
                    mut[i + 1] = File(stageDir, fileName).absolutePath
                    return Triple(mut, stageDir, originalOutDir)
                }
            }
        }
        return Triple(mut, null, null)
    }

    private fun publishStagedFiles(stageDir: File, originalDir: File?): File? {
        var lastPublished: File? = null
        val isAudio = originalDir?.absolutePath?.contains("/Music", ignoreCase = true) == true
        val relDir = if (isAudio) {
            "${Environment.DIRECTORY_MUSIC}/4kFreeDownloader"
        } else {
            "${Environment.DIRECTORY_DOWNLOADS}/4kFreeDownloader"
        }

        val files = stageDir.listFiles()?.filter { it.isFile && !it.name.endsWith(".part") && !it.name.endsWith(".ytdl") }
            ?: emptyList()

        for (file in files) {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                val resolver = activity.contentResolver
                val mime = guessMimeType(file.name)
                val collection = when {
                    mime.startsWith("audio/") -> MediaStore.Audio.Media.EXTERNAL_CONTENT_URI
                    mime.startsWith("video/") -> MediaStore.Video.Media.EXTERNAL_CONTENT_URI
                    else -> MediaStore.Downloads.EXTERNAL_CONTENT_URI
                }
                val values = ContentValues().apply {
                    put(MediaStore.MediaColumns.DISPLAY_NAME, file.name)
                    put(MediaStore.MediaColumns.MIME_TYPE, mime)
                    put(MediaStore.MediaColumns.RELATIVE_PATH, relDir)
                    put(MediaStore.MediaColumns.IS_PENDING, 1)
                }
                val uri = resolver.insert(collection, values)
                if (uri != null) {
                    runCatching {
                        resolver.openOutputStream(uri)?.use { out ->
                            file.inputStream().use { input -> input.copyTo(out) }
                        }
                        values.clear()
                        values.put(MediaStore.MediaColumns.IS_PENDING, 0)
                        resolver.update(uri, values, null, null)
                    }
                    lastPublished = File(
                        Environment.getExternalStoragePublicDirectory(
                            if (isAudio) Environment.DIRECTORY_MUSIC else Environment.DIRECTORY_DOWNLOADS
                        ),
                        "4kFreeDownloader/${file.name}"
                    )
                }
            }
        }
        stageDir.deleteRecursively()
        return lastPublished
    }

    @Command
    fun runYtDlpJson(invoke: Invoke) {
        val parsed = invoke.parseArgs(ArgsArgs::class.java)
        workerPool.execute {
            try {
                ensureInitialized()
                val req = YoutubeDLRequest(emptyList<String>()).apply {
                    addCommands(parsed.args)
                }
                val resp = YoutubeDL.getInstance().execute(req)
                val out = JSObject().apply {
                    put("value", resp.out ?: "")
                }
                invoke.resolve(out)
            } catch (e: Throwable) {
                android.util.Log.e("VidGrabPlugin", "runYtDlpJson error", e)
                invoke.reject(e.message ?: "yt-dlp JSON error")
            }
        }
    }

    @Command
    fun runYtDlpDownload(invoke: Invoke) {
        val parsed = invoke.parseArgs(DownloadArgs::class.java)
        workerPool.execute {
            try {
                ensureInitialized()
                val (finalArgs, stageDir, originalDir) = prepareScopedArgs(parsed.taskId, parsed.args)
                val req = YoutubeDLRequest(emptyList<String>()).apply {
                    addCommands(finalArgs)
                }
                val resp = YoutubeDL.getInstance().execute(req, parsed.taskId) { _, _, line ->
                    val payload = JSObject().apply {
                        put("line", line)
                    }
                    parsed.onLine?.send(payload)
                }
                if (stageDir != null) {
                    val published = publishStagedFiles(stageDir, originalDir)
                    if (published != null) {
                        parsed.onLine?.send(JSObject().apply {
                            put("line", "VIDGRAB_FINAL_PATH:${published.absolutePath}")
                        })
                    }
                }
                invoke.resolve(JSObject().apply {
                    put("exitCode", resp.exitCode)
                })
            } catch (e: Throwable) {
                android.util.Log.e("VidGrabPlugin", "runYtDlpDownload error", e)
                invoke.reject(e.message ?: "yt-dlp download failed")
            }
        }
    }

    @Command
    fun cancelTask(invoke: Invoke) {
        val parsed = invoke.parseArgs(TaskArgs::class.java)
        workerPool.execute {
            runCatching {
                YoutubeDL.getInstance().destroyProcessById(parsed.taskId)
            }
            invoke.resolve()
        }
    }

    @Command
    fun runFfmpeg(invoke: Invoke) {
        val parsed = invoke.parseArgs(ArgsArgs::class.java)
        workerPool.execute {
            try {
                ensureInitialized()
                val ctx = activity.applicationContext
                val ffmpegBin = File(ctx.applicationInfo.nativeLibraryDir, "libffmpeg.so")
                if (!ffmpegBin.exists()) {
                    invoke.reject("libffmpeg.so introuvable dans l'APK")
                    return@execute
                }
                val pkgRoot = File(ctx.noBackupFilesDir, "youtubedl-android/packages")
                val ffLibDir = File(pkgRoot, "ffmpeg/usr/lib").absolutePath
                val pyLibDir = File(pkgRoot, "python/usr/lib").absolutePath
                val envLib = listOf(ffLibDir, pyLibDir, ctx.applicationInfo.nativeLibraryDir)
                    .joinToString(":")

                val cmd = mutableListOf(ffmpegBin.absolutePath).apply { addAll(parsed.args) }
                val pb = ProcessBuilder(cmd).redirectErrorStream(true)
                pb.environment()["LD_LIBRARY_PATH"] = envLib
                val proc = pb.start()
                val output = proc.inputStream.bufferedReader().readText()
                val code = proc.waitFor()
                if (code == 0) {
                    invoke.resolve()
                } else {
                    invoke.reject("ffmpeg a échoué (code $code) : ${output.takeLast(500)}")
                }
            } catch (e: Throwable) {
                android.util.Log.e("VidGrabPlugin", "runFfmpeg error", e)
                invoke.reject(e.message ?: "Erreur ffmpeg Android")
            }
        }
    }

    @Command
    fun updateYtDlp(invoke: Invoke) {
        workerPool.execute {
            try {
                ensureInitialized()
                val ctx = activity.applicationContext
                val status = YoutubeDL.getInstance().updateYoutubeDL(ctx, YoutubeDL.UpdateChannel.STABLE)
                val ver = YoutubeDL.getInstance().version(ctx) ?: status?.name ?: "STABLE"
                invoke.resolve(JSObject().apply {
                    put("value", ver)
                })
            } catch (e: Throwable) {
                android.util.Log.e("VidGrabPlugin", "updateYtDlp error", e)
                invoke.reject(e.message ?: "Mise à jour yt-dlp Android impossible")
            }
        }
    }

    @Command
    fun getYtDlpVersion(invoke: Invoke) {
        workerPool.execute {
            try {
                ensureInitialized()
                val ver = YoutubeDL.getInstance().version(activity.applicationContext)
                    ?: "youtubedl-android-0.18.1"
                invoke.resolve(JSObject().apply {
                    put("value", ver)
                })
            } catch (e: Throwable) {
                android.util.Log.e("VidGrabPlugin", "getYtDlpVersion error", e)
                invoke.reject(e.message ?: "Version yt-dlp indisponible")
            }
        }
    }

    @Command
    fun scanMediaFile(invoke: Invoke) {
        val parsed = invoke.parseArgs(PathArgs::class.java)
        if (parsed.path.isNotBlank()) {
            MediaScannerConnection.scanFile(
                activity.applicationContext,
                arrayOf(parsed.path),
                null,
                null
            )
        }
        invoke.resolve()
    }

    @Command
    fun readClipboard(invoke: Invoke) {
        var clipText = ""
        val latch = CountDownLatch(1)
        activity.runOnUiThread {
            try {
                val cm = activity.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
                val item = cm?.primaryClip?.getItemAt(0)
                clipText = item?.coerceToText(activity)?.toString()?.trim() ?: ""
            } catch (_: Throwable) {
            } finally {
                latch.countDown()
            }
        }
        latch.await(2, TimeUnit.SECONDS)
        invoke.resolve(JSObject().apply {
            put("value", clipText)
        })
    }

    @Command
    fun writeClipboard(invoke: Invoke) {
        val parsed = invoke.parseArgs(TextArgs::class.java)
        activity.runOnUiThread {
            val cm = activity.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
            cm?.setPrimaryClip(ClipData.newPlainText("4k free video dowloader", parsed.text))
        }
        invoke.resolve()
    }

    @Command
    fun openPath(invoke: Invoke) {
        val parsed = invoke.parseArgs(PathArgs::class.java)
        val raw = parsed.path.trim()
        try {
            if (raw.startsWith("http://") || raw.startsWith("https://")) {
                val intent = Intent(Intent.ACTION_VIEW, Uri.parse(raw)).apply {
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
                activity.startActivity(intent)
                invoke.resolve()
                return
            }

            val target = File(raw)
            if (parsed.revealParent || target.isDirectory) {
                val intent = Intent(DownloadManager.ACTION_VIEW_DOWNLOADS).apply {
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
                activity.startActivity(intent)
                invoke.resolve()
                return
            }

            if (!target.exists()) {
                val dlIntent = Intent(DownloadManager.ACTION_VIEW_DOWNLOADS).apply {
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
                activity.startActivity(dlIntent)
                invoke.resolve()
                return
            }

            val uri = FileProvider.getUriForFile(
                activity,
                "${activity.packageName}.fileprovider",
                target
            )
            val mime = guessMimeType(target.name)
            val viewIntent = Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(uri, mime)
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            activity.startActivity(Intent.createChooser(viewIntent, "Ouvrir avec"))
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject("Impossible d'ouvrir $raw : ${e.message}")
        }
    }

    @Command
    fun sendNotification(invoke: Invoke) {
        val parsed = invoke.parseArgs(NotificationArgs::class.java)
        try {
            val ctx = activity.applicationContext
            createNotificationChannel(ctx)
            if (Build.VERSION.SDK_INT >= 33 &&
                ContextCompat.checkSelfPermission(ctx, Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
            ) {
                invoke.resolve()
                return
            }
            val builder = NotificationCompat.Builder(ctx, CHANNEL_ID)
                .setSmallIcon(android.R.drawable.stat_sys_download_done)
                .setContentTitle(parsed.title)
                .setContentText(parsed.body)
                .setPriority(NotificationCompat.PRIORITY_DEFAULT)
                .setAutoCancel(true)
            NotificationManagerCompat.from(ctx).notify(notifIdCounter.incrementAndGet(), builder.build())
            invoke.resolve()
        } catch (_: Throwable) {
            invoke.resolve()
        }
    }

    @Command
    fun takePendingSharedUrl(invoke: Invoke) {
        val url = synchronized(shareLock) {
            val cur = pendingSharedUrl
            pendingSharedUrl = null
            cur
        }
        invoke.resolve(JSObject().apply {
            if (url != null) put("url", url) else put("url", JSONObject.NULL)
        })
    }

    private fun guessMimeType(fileName: String): String {
        val ext = fileName.substringAfterLast('.', "").lowercase()
        return MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: when (ext) {
            "mkv" -> "video/x-matroska"
            "webm" -> "video/webm"
            "mp4" -> "video/mp4"
            "mp3" -> "audio/mpeg"
            "m4a" -> "audio/mp4"
            "opus", "ogg" -> "audio/ogg"
            "flac" -> "audio/flac"
            "wav" -> "audio/wav"
            else -> "*/*"
        }
    }

    companion object {
        private const val CHANNEL_ID = "vidgrab_downloads"
        private val shareLock = Any()
        private val initLock = Any()

        @Volatile
        private var pendingSharedUrl: String? = null

        @Volatile
        private var initDone = false

        @Volatile
        private var initError: String? = null

        fun initEngines(ctx: Context) {
            if (initDone && initError == null) return
            synchronized(initLock) {
                if (initDone && initError == null) return
                try {
                    android.util.Log.i("VidGrabPlugin", "Initializing YoutubeDL...")
                    YoutubeDL.getInstance().init(ctx)
                    android.util.Log.i("VidGrabPlugin", "YoutubeDL initialized successfully")
                } catch (t: Throwable) {
                    android.util.Log.e("VidGrabPlugin", "YoutubeDL.init failed", t)
                    val msg = "YoutubeDL init failed: ${t.javaClass.simpleName}: ${t.message}"
                    initError = msg
                    throw RuntimeException(msg, t)
                }
                try {
                    android.util.Log.i("VidGrabPlugin", "Initializing FFmpeg...")
                    FFmpeg.getInstance().init(ctx)
                    android.util.Log.i("VidGrabPlugin", "FFmpeg initialized successfully")
                } catch (t: Throwable) {
                    android.util.Log.e("VidGrabPlugin", "FFmpeg.init failed", t)
                }
                initDone = true
                initError = null
            }
        }

        fun enqueueSharedUrl(url: String) {
            synchronized(shareLock) {
                pendingSharedUrl = url
            }
        }
    }
}
