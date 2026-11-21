package com.olegg90.bookreader

import android.app.Activity
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.Settings
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin

/**
 * The phone's shared storage for the library (see storage.rs). With All files
 * access turned on, the library is a plain folder (Documents/Books by default)
 * that the Rust side reads and writes like any other, so the same library code
 * runs on Windows and Android.
 */
@TauriPlugin
class StoragePlugin(private val activity: Activity) : Plugin(activity) {

  /** Whether All files access is on, and where the shared Documents folder is. */
  @Command
  fun status(invoke: Invoke) {
    val granted = Build.VERSION.SDK_INT < Build.VERSION_CODES.R || Environment.isExternalStorageManager()
    val documents = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOCUMENTS).absolutePath
    invoke.resolve(JSObject().put("granted", granted).put("documents", documents))
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
