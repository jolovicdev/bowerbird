package io.github.jolovicdev.bowerbird

import android.content.ActivityNotFoundException
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.webkit.JavascriptInterface
import android.webkit.WebView
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

  // The page's way to the system, as window.BowerbirdAndroid.
  override fun onWebViewCreate(webView: WebView) {
    webView.addJavascriptInterface(object {
      // Android has no program to hand a link to, as desktops do. A page's
      // links can come from someone else's notebook, so only web and mail
      // links leave the app.
      @JavascriptInterface
      fun openLink(url: String) {
        val uri = Uri.parse(url)
        if (uri.scheme !in setOf("http", "https", "mailto")) return
        runOnUiThread {
          try {
            startActivity(Intent(Intent.ACTION_VIEW, uri))
          } catch (e: ActivityNotFoundException) {
            // No app on the phone opens this kind of link; the tap does nothing.
          }
        }
      }

      // Android 17 blocks connections to the local network, where the user's
      // own model server usually is, until the user allows them. Settings asks
      // when an address is saved, so the prompt comes with its reason.
      @JavascriptInterface
      fun askForLocalNetwork() {
        if (Build.VERSION.SDK_INT >= 37 && checkSelfPermission(LOCAL_NETWORK) != PackageManager.PERMISSION_GRANTED) {
          runOnUiThread { requestPermissions(arrayOf(LOCAL_NETWORK), 0) }
        }
      }
    }, "BowerbirdAndroid")
  }

  private companion object {
    const val LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"
  }
}
