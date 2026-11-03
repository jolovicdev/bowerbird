package io.github.jolovicdev.bowerbird

import android.os.Bundle
import androidx.activity.enableEdgeToEdge
import androidx.core.graphics.Insets
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // The window runs behind the system bars, and WebView older than 136 doesn't
    // tell the page where they are, so the view holding the page keeps clear of
    // them. The zeroed insets stop newer WebViews from padding the page again.
    ViewCompat.setOnApplyWindowInsetsListener(findViewById(android.R.id.content)) { view, windowInsets ->
      val bars = WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
      val insets = windowInsets.getInsets(bars)
      view.setPadding(insets.left, insets.top, insets.right, insets.bottom)
      WindowInsetsCompat.Builder(windowInsets).setInsets(bars, Insets.NONE).build()
    }
  }
}
