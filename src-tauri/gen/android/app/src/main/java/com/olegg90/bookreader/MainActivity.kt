package com.olegg90.bookreader

import android.os.Bundle
import android.view.KeyEvent
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  /** Volume keys turn pages while a book is open, when the setting is on. */
  override fun dispatchKeyEvent(event: KeyEvent): Boolean {
    if (SystemPlugin.instance?.volumeKey(event) == true) return true
    return super.dispatchKeyEvent(event)
  }
}
