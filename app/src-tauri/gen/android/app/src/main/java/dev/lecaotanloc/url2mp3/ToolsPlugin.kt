package dev.lecaotanloc.url2mp3

import android.app.Activity
import android.content.Intent
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
import kotlin.concurrent.thread

@InvokeArg
class KeepAliveArgs {
    var active: Boolean = false
    var text: String? = null
}

/**
 * What the Rust side (src/platform/android.rs) needs from Android: youtubedl-android's Python,
 * yt-dlp, ffmpeg and QuickJS, a foreground service while downloads run, and links shared from
 * other apps.
 */
@TauriPlugin
class ToolsPlugin(private val activity: Activity) : Plugin(activity) {
    @Volatile private var shared: String? = null

    /** Unpacks Python and ffmpeg on first run, then returns where everything is. */
    @Command
    fun prepare(invoke: Invoke) {
        thread {
            try {
                val ctx = activity.applicationContext
                YoutubeDL.getInstance().init(ctx)
                FFmpeg.getInstance().init(ctx)
                // Same layout and environment as YoutubeDL.execute() uses.
                val bin = File(ctx.applicationInfo.nativeLibraryDir)
                val base = File(ctx.noBackupFilesDir, YoutubeDL.baseName)
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
                ret.put("ytdlpScript", File(File(base, YoutubeDL.ytdlpDirName), YoutubeDL.ytdlpBin).absolutePath)
                ret.put("ffmpeg", File(bin, "libffmpeg.so").absolutePath)
                ret.put("qjs", File(bin, "libqjs.so").absolutePath)
                ret.put("env", env)
                ret.put("version", YoutubeDL.getInstance().version(ctx))
                invoke.resolve(ret)
            } catch (e: Throwable) {
                invoke.reject(e.message ?: e.toString())
            }
        }
    }

    /** Fetches the latest stable yt-dlp; returns its version as `text`. */
    @Command
    fun updateYtdlp(invoke: Invoke) {
        thread {
            try {
                val ctx = activity.applicationContext
                YoutubeDL.getInstance().updateYoutubeDL(ctx, YoutubeDL.UpdateChannel.STABLE)
                val ret = JSObject()
                ret.put("text", YoutubeDL.getInstance().version(ctx))
                invoke.resolve(ret)
            } catch (e: Throwable) {
                invoke.reject(e.message ?: e.toString())
            }
        }
    }

    @Command
    fun keepAlive(invoke: Invoke) {
        val args = invoke.parseArgs(KeepAliveArgs::class.java)
        val intent = Intent(activity, DownloadService::class.java)
        if (args.active) {
            intent.putExtra(DownloadService.TEXT, args.text)
            activity.startForegroundService(intent)
        } else {
            activity.stopService(intent)
        }
        invoke.resolve()
    }

    /** The last link shared to the app, once; `text` is null when there is none. */
    @Command
    fun takeShared(invoke: Invoke) {
        val ret = JSObject()
        ret.put("text", shared)
        shared = null
        invoke.resolve(ret)
    }

    override fun load(webView: WebView) {
        super.load(webView)
        remember(activity.intent)
    }

    override fun onNewIntent(intent: Intent) {
        remember(intent)
    }

    private fun remember(intent: Intent?) {
        if (intent?.action == Intent.ACTION_SEND) {
            intent.getStringExtra(Intent.EXTRA_TEXT)?.let { shared = it }
        }
    }
}
