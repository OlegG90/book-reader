package com.olegg90.bookreader

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.provider.Settings
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.view.KeyEvent
import android.view.WindowManager
import android.webkit.WebView
import androidx.activity.result.ActivityResult
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.security.KeyStore
import org.json.JSONArray
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

@InvokeArg
class PasswordArgs {
  lateinit var id: String
  var password: String? = null
}

@InvokeArg
class ReadingArgs {
  var on: Boolean = false
  var volumeKeys: Boolean = false
}

@InvokeArg
class UriArgs {
  lateinit var uri: String
}

@InvokeArg
class IdsArgs {
  var ids: List<String>? = null
}

/**
 * What the app needs from Android (see platform_android.rs).
 *
 * - The library in shared storage: with All files access turned on, the
 *   library is a plain folder (Documents/Books by default) that the Rust side
 *   reads and writes like any other, so the same library code runs on Windows
 *   and Android. Android 11 (API 30) is the minimum (see app/build.gradle.kts):
 *   All files access starts there. Another folder comes from Android's folder
 *   picker, turned back into a path.
 * - Books from outside the library ("Open with", "Share", the book picker)
 *   arrive as content:// documents; they are copied into the app's own
 *   storage (the newest few are kept, so "Not now" and the recent books keep
 *   working) and opened from there, and the original is remembered so Move
 *   can delete it.
 * - Catalog passwords, encrypted with a key kept in the Android Keystore.
 * - Reading: the screen stays on, the status bar hides, volume keys turn pages.
 */
@TauriPlugin
class SystemPlugin(private val activity: Activity) : Plugin(activity) {

  companion object {
    /** The plugin, for MainActivity's key handling. */
    var instance: SystemPlugin? = null

    private const val KEY_ALIAS = "catalog-passwords"
    private const val PREFS = "catalog-passwords"
    private const val OUTSIDE = "outside"
    /** Copies of books from outside kept at once; older ones are removed. */
    private const val KEEP_OUTSIDE = 20
  }

  /** Volume keys turn pages (a setting, and only while a book is open). */
  @Volatile var volumeKeys = false

  /** A book handed to the app ("Open with", "Share") and not yet opened. */
  @Volatile private var pending: Uri? = null

  override fun load(webView: WebView) {
    instance = this
    takeBook(activity.intent)
  }

  override fun onNewIntent(intent: Intent) {
    if (takeBook(intent)) trigger("incoming", JSObject())
  }

  /** Keeps a book intent for `takeIncoming`; false for any other intent. */
  private fun takeBook(intent: Intent?): Boolean {
    if (intent == null) return false
    val uri = bookUri(intent) ?: return false
    // Reopened from the recent apps: the book was handed over before.
    if (intent.flags and Intent.FLAG_ACTIVITY_LAUNCHED_FROM_HISTORY != 0) return false
    pending = uri
    // Handled once: a recreated activity must not open it again.
    activity.intent = Intent(Intent.ACTION_MAIN)
    return true
  }

  private fun bookUri(intent: Intent): Uri? = when (intent.action) {
    Intent.ACTION_VIEW -> intent.data
    @Suppress("DEPRECATION")
    Intent.ACTION_SEND -> intent.getParcelableExtra(Intent.EXTRA_STREAM)
    else -> null
  }

  /** Called by MainActivity: true when the key turns pages (one per press). */
  fun volumeKey(event: KeyEvent): Boolean {
    if (!volumeKeys) return false
    val direction = when (event.keyCode) {
      KeyEvent.KEYCODE_VOLUME_DOWN -> "next"
      KeyEvent.KEYCODE_VOLUME_UP -> "prev"
      else -> return false
    }
    if (event.action == KeyEvent.ACTION_DOWN && event.repeatCount == 0) trigger("volumeKey", JSObject().put("direction", direction))
    return true
  }

  // ---- Library ---------------------------------------------------------------

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

  /** Android's folder picker; resolves `{ path }`, or `{}` when cancelled. */
  @Command
  fun pickFolder(invoke: Invoke) {
    startActivityForResult(invoke, Intent(Intent.ACTION_OPEN_DOCUMENT_TREE), "folderPicked")
  }

  @ActivityCallback
  fun folderPicked(invoke: Invoke, result: ActivityResult) {
    val uri = result.data?.data
    if (result.resultCode != Activity.RESULT_OK || uri == null) return invoke.resolve(JSObject())
    // The phone's own storage: "primary:Documents/Books" from its root, or
    // "home:Books" from the Documents shortcut.
    val id = if (uri.authority == "com.android.externalstorage.documents") DocumentsContract.getTreeDocumentId(uri) else ""
    val (volume, path) = id.split(":", limit = 2).let { it[0] to it.getOrElse(1) { "" } }
    val root = when (volume) {
      "primary" -> Environment.getExternalStorageDirectory()
      "home" -> Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOCUMENTS)
      else -> return invoke.reject("Choose a folder in the phone's own storage (not a memory card or another app's storage)")
    }
    val folder = File(root, path)
    invoke.resolve(JSObject().put("path", folder.absolutePath))
  }

  // ---- Books from outside the library ------------------------------------------

  /** Android's document picker for a book; resolves like `takeIncoming`. */
  @Command
  fun pickBook(invoke: Invoke) {
    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("*/*")
    startActivityForResult(invoke, intent, "bookPicked")
  }

  @ActivityCallback
  fun bookPicked(invoke: Invoke, result: ActivityResult) {
    val uri = result.data?.data
    if (result.resultCode != Activity.RESULT_OK || uri == null) return invoke.resolve(JSObject())
    Thread { resolveCopy(invoke, uri) }.start()
  }

  /** The book handed to the app, copied in: `{ path, uri }`, or `{}`. */
  @Command
  fun takeIncoming(invoke: Invoke) {
    val uri = pending ?: return invoke.resolve(JSObject())
    pending = null
    resolveCopy(invoke, uri)
  }

  private fun resolveCopy(invoke: Invoke, uri: Uri) {
    try {
      invoke.resolve(JSObject().put("path", copyIn(uri).absolutePath).put("uri", uri.toString()))
    } catch (e: Exception) {
      invoke.reject("Can't read the book: ${e.message}")
    }
  }

  /** Copies a document into `files/outside/<time>/`, under its own name. */
  private fun copyIn(uri: Uri): File {
    val all = File(activity.filesDir, OUTSIDE)
    all.listFiles()?.sortedByDescending { it.lastModified() }?.drop(KEEP_OUTSIDE - 1)?.forEach { it.deleteRecursively() }
    val dir = File(all, System.currentTimeMillis().toString()).apply { mkdirs() }
    val target = File(dir, displayName(uri).replace(Regex("[/\\\\]"), "_"))
    val input = activity.contentResolver.openInputStream(uri) ?: throw IllegalStateException("no data")
    input.use { from -> target.outputStream().use { from.copyTo(it) } }
    return target
  }

  private fun displayName(uri: Uri): String {
    activity.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use {
      if (it.moveToFirst() && !it.isNull(0)) return it.getString(0)
    }
    return uri.lastPathSegment?.substringAfterLast('/') ?: "book"
  }

  /** Deletes the original of a book moved into the library, where Android allows it. */
  @Command
  fun deleteDocument(invoke: Invoke) {
    val uri = Uri.parse(invoke.parseArgs(UriArgs::class.java).uri)
    val resolver = activity.contentResolver
    val deleted = if (uri.scheme == "file") File(uri.path ?: "").delete()
    else runCatching { DocumentsContract.deleteDocument(resolver, uri) }
      .recoverCatching { resolver.delete(uri, null, null) > 0 }
      .getOrDefault(false)
    invoke.resolve(JSObject().put("deleted", deleted))
  }

  // ---- Reading -------------------------------------------------------------------

  /**
   * A book is open (or not): the screen stays on, the status bar hides, and
   * the volume keys turn pages when `volumeKeys` (the setting) is on.
   */
  @Command
  fun reading(invoke: Invoke) {
    val args = invoke.parseArgs(ReadingArgs::class.java)
    val on = args.on
    volumeKeys = on && args.volumeKeys
    activity.runOnUiThread {
      val window = activity.window
      if (on) window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
      else window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
      val bars = WindowCompat.getInsetsController(window, window.decorView)
      bars.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
      if (on) bars.hide(WindowInsetsCompat.Type.statusBars()) else bars.show(WindowInsetsCompat.Type.statusBars())
      invoke.resolve(JSObject())
    }
  }

  // ---- Catalog passwords ---------------------------------------------------------

  /** The Keystore key that encrypts the passwords, made on first use. */
  private val key: SecretKey by lazy {
    val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
    (store.getEntry(KEY_ALIAS, null) as? KeyStore.SecretKeyEntry)?.secretKey ?: newKey()
  }

  private fun newKey(): SecretKey {
    val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
    generator.init(
      KeyGenParameterSpec.Builder(KEY_ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
        .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
        .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
        .build()
    )
    return generator.generateKey()
  }

  private fun prefs() = activity.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

  /** Keeps a catalog's password, encrypted with the Keystore key: "iv:ciphertext". */
  @Command
  fun savePassword(invoke: Invoke) {
    val args = invoke.parseArgs(PasswordArgs::class.java)
    try {
      val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.ENCRYPT_MODE, key) }
      val sealed = cipher.doFinal((args.password ?: "").toByteArray())
      val text = Base64.encodeToString(cipher.iv, Base64.NO_WRAP) + ":" + Base64.encodeToString(sealed, Base64.NO_WRAP)
      prefs().edit().putString(args.id, text).apply()
      invoke.resolve(JSObject())
    } catch (e: Exception) {
      invoke.reject("Can't keep the password: ${e.message}")
    }
  }

  /** A catalog's password: `{ password }`, or `{}` when none is kept (or it can't be read). */
  @Command
  fun password(invoke: Invoke) {
    val id = invoke.parseArgs(PasswordArgs::class.java).id
    val text = prefs().getString(id, null) ?: return invoke.resolve(JSObject())
    try {
      val (iv, sealed) = text.split(":", limit = 2).map { Base64.decode(it, Base64.NO_WRAP) }
      val cipher = Cipher.getInstance("AES/GCM/NoPadding").apply { init(Cipher.DECRYPT_MODE, key, GCMParameterSpec(128, iv)) }
      invoke.resolve(JSObject().put("password", String(cipher.doFinal(sealed))))
    } catch (e: Exception) {
      invoke.resolve(JSObject())
    }
  }

  /** Whether a catalog has a password kept: `{ kept }` (nothing is decrypted). */
  @Command
  fun hasPassword(invoke: Invoke) {
    invoke.resolve(JSObject().put("kept", prefs().contains(invoke.parseArgs(PasswordArgs::class.java).id)))
  }

  /** Whether each catalog has a password kept: `{ kept: [...] }`, in the asked order. */
  @Command
  fun hasPasswords(invoke: Invoke) {
    val ids = invoke.parseArgs(IdsArgs::class.java).ids ?: emptyList()
    invoke.resolve(JSObject().put("kept", JSONArray(ids.map { prefs().contains(it) })))
  }

  @Command
  fun forgetPassword(invoke: Invoke) {
    prefs().edit().remove(invoke.parseArgs(PasswordArgs::class.java).id).apply()
    invoke.resolve(JSObject())
  }
}
