package dev.lecaotanloc.url2mp3

import android.content.res.Configuration
import android.graphics.Color
import android.os.Bundle
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    super.onCreate(savedInstanceState)
    // Android 15+ draws apps edge to edge, and the WebView doesn't hand the insets to CSS:
    // keep the page clear of the status bar, the gesture bar and the keyboard here instead,
    // on the page's own background (--bg in app/src/style.css).
    val dark = (resources.configuration.uiMode and Configuration.UI_MODE_NIGHT_MASK) ==
      Configuration.UI_MODE_NIGHT_YES
    val content = findViewById<android.view.View>(android.R.id.content)
    content.setBackgroundColor(Color.parseColor(if (dark) "#15110e" else "#efe6d8"))
    WindowCompat.getInsetsController(window, content).apply {
      isAppearanceLightStatusBars = !dark
      isAppearanceLightNavigationBars = !dark
    }
    ViewCompat.setOnApplyWindowInsetsListener(content) { view, insets ->
      val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.ime())
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
  }
}
