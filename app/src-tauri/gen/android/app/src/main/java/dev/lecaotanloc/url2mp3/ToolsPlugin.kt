package dev.lecaotanloc.url2mp3

import android.Manifest
import android.app.Activity
import android.content.Intent
import android.content.pm.PackageManager
import android.media.MediaScannerConnection
import android.os.Build
import android.webkit.WebView
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import com.yausername.ffmpeg.FFmpeg
import com.yausername.youtubedl_android.YoutubeDL
import java.io.File
import java.io.RandomAccessFile
import java.util.concurrent.atomic.AtomicReference
import kotlin.concurrent.thread

@InvokeArg
class KeepAliveArgs {
    var active: Boolean = false
    var text: String? = null
}

@InvokeArg
class ScanArgs {
    var path: String = ""
}

/**
 * What the Rust side (src/platform/android.rs) needs from Android: youtubedl-android's Python,
 * yt-dlp, ffmpeg and QuickJS, a foreground service while downloads run, and links shared from
 * other apps.
 */
@TauriPlugin
class ToolsPlugin(private val activity: Activity) : Plugin(activity) {
    private val shared = AtomicReference<String?>(null)

    /** prepare and updateYtdlp both touch the yt-dlp file: never at the same time. */
    private val tools = Any()

    /** Unpacks Python and ffmpeg on first run, then returns where everything is. */
    @Command
    fun prepare(invoke: Invoke) {
        thread {
            try { synchronized(tools) {
                val ctx = activity.applicationContext
                val base = File(ctx.noBackupFilesDir, YoutubeDL.baseName)
                val ytdlp = File(File(base, YoutubeDL.ytdlpDirName), YoutubeDL.ytdlpBin)
                // A copy or update cut short leaves a truncated yt-dlp that init() keeps forever
                // ("bad local file header"): drop it so init() copies the bundled one again.
                if (ytdlp.exists() && !looksLikeZip(ytdlp)) ytdlp.delete()
                YoutubeDL.getInstance().init(ctx)
                FFmpeg.getInstance().init(ctx)
                // Same layout and environment as YoutubeDL.execute() uses.
                val bin = File(ctx.applicationInfo.nativeLibraryDir)
                val packages = File(base, "packages")
                val python = File(packages, "python")
                val env = JSObject()
                env.put(
                    "LD_LIBRARY_PATH",
                    listOf("python", "ffmpeg", "aria2c").joinToString(":") { "${File(packages, it)}/usr/lib" },
                )
                env.put("SSL_CERT_FILE", "$python/usr/etc/tls/cert.pem")
                env.put("PYTHONHOME", "$python/usr")
                env.put("HOME", "$python/usr")
                env.put("TMPDIR", ctx.cacheDir.absolutePath)
                env.put("PATH", "${System.getenv("PATH")}:$bin")
                val ret = JSObject()
                ret.put("python", File(bin, "libpython.so").absolutePath)
                ret.put("ytdlpScript", ytdlp.absolutePath)
                ret.put("ffmpeg", File(bin, "libffmpeg.so").absolutePath)
                ret.put("qjs", File(bin, "libqjs.so").absolutePath)
                ret.put("env", env)
                ret.put("version", YoutubeDL.getInstance().version(ctx))
                invoke.resolve(ret)
            } } catch (e: Throwable) {
                invoke.reject(e.message ?: e.toString())
            }
        }
    }

    /** Fetches the latest stable yt-dlp; returns its version as `text`. */
    @Command
    fun updateYtdlp(invoke: Invoke) {
        thread {
            try { synchronized(tools) {
                val ctx = activity.applicationContext
                YoutubeDL.getInstance().updateYoutubeDL(ctx, YoutubeDL.UpdateChannel.STABLE)
                val ret = JSObject()
                ret.put("text", YoutubeDL.getInstance().version(ctx))
                invoke.resolve(ret)
            } } catch (e: Throwable) {
                invoke.reject(e.message ?: e.toString())
            }
        }
    }

    @Command
    fun keepAlive(invoke: Invoke) {
        val args = invoke.parseArgs(KeepAliveArgs::class.java)
        val intent = Intent(activity, DownloadService::class.java)
        try {
            if (args.active) {
                askForNotifications()
                intent.putExtra(DownloadService.TEXT, args.text)
                activity.startForegroundService(intent)
            } else {
                activity.stopService(intent)
            }
        } catch (e: Exception) {
            // Android 12+ refuses to start one while the app is in the background (e.g. an
            // automatic retry): downloads still run, just without the protection.
            android.util.Log.w("url2mp3", "keepAlive(${args.active}): $e")
        }
        invoke.resolve()
    }

    /** Media index entry for a finished track, so music players list it right away. */
    @Command
    fun scan(invoke: Invoke) {
        val args = invoke.parseArgs(ScanArgs::class.java)
        MediaScannerConnection.scanFile(activity.applicationContext, arrayOf(args.path), null, null)
        invoke.resolve()
    }

    /** The last link shared to the app, once; `text` is null when there is none. */
    @Command
    fun takeShared(invoke: Invoke) {
        val ret = JSObject()
        ret.put("text", shared.getAndSet(null))
        invoke.resolve(ret)
    }

    @Volatile private var askedForNotifications = false

    /**
     * Android 13+ hides the download notification unless the user allows notifications. Asked
     * once per run, on the first download.
     */
    private fun askForNotifications() {
        if (askedForNotifications) return
        askedForNotifications = true
        if (Build.VERSION.SDK_INT >= 33 &&
            activity.checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            activity.runOnUiThread {
                activity.requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
            }
        }
    }

    override fun load(webView: WebView) {
        super.load(webView)
        remember(activity.intent)
    }

    override fun onNewIntent(intent: Intent) {
        remember(intent)
    }

    /** yt-dlp is a zip app: a complete one ends with the zip end-of-central-directory record. */
    private fun looksLikeZip(file: File): Boolean = try {
        RandomAccessFile(file, "r").use { f ->
            val tail = ByteArray(minOf(f.length(), 65_557L).toInt())
            f.seek(f.length() - tail.size)
            f.readFully(tail)
            (tail.size - 22 downTo 0).any { i ->
                tail[i] == 0x50.toByte() && tail[i + 1] == 0x4b.toByte() &&
                    tail[i + 2] == 0x05.toByte() && tail[i + 3] == 0x06.toByte()
            }
        }
    } catch (e: Exception) {
        false
    }

    private fun remember(intent: Intent?) {
        if (intent?.action == Intent.ACTION_SEND) {
            intent.getStringExtra(Intent.EXTRA_TEXT)?.let { shared.set(it) }
        }
    }
}
