package com.olegg90.bookreader

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Environment
import android.provider.Settings
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * What the app needs from Android (see platform_android.rs).
 *
 * The phone's shared storage for the library: with All files
 * access turned on, the library is a plain folder (Documents/Books by default)
 * that the Rust side reads and writes like any other, so the same library code
 * runs on Windows and Android. Android 11 (API 30) is the minimum (see
 * app/build.gradle.kts): All files access starts there.
 */
@TauriPlugin
class SystemPlugin(private val activity: Activity) : Plugin(activity) {

  /** Whether All files access is on, and where the shared Documents folder is. */
  @Command
  fun status(invoke: Invoke) {
    val granted = Environment.isExternalStorageManager()
    val documents = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOCUMENTS).absolutePath
    invoke.resolve(JSObject().put("granted", granted).put("documents", documents))
  }

  /** Sends the app to the background, as Back on an app's first screen does. */
  @Command
  fun moveTaskToBack(invoke: Invoke) {
    activity.moveTaskToBack(true)
    invoke.resolve(JSObject())
  }

  /** Opens Android's page that turns All files access on for this app. */
  @Command
  fun askAllFilesAccess(invoke: Invoke) {
    try {
      activity.startActivity(Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION, Uri.parse("package:${activity.packageName}")))
      invoke.resolve(JSObject())
    } catch (e: Exception) {
      invoke.reject("Cannot open the setting: ${e.message}")
    }
  }
}
