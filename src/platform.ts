import { addPluginListener, invoke } from '@tauri-apps/api/core'
import { onBackButtonPress } from '@tauri-apps/api/app'

/** True in the phone app (an Android WebView); false on the desktop. */
export const isAndroid = /Android/.test(navigator.userAgent)

// Hides what only the other platform does (`.desktop-only`, `.android-only` in style.css).
document.documentElement.classList.toggle('android', isAndroid)

/** What the delete-book confirmation says about the file. */
export function deleteFileNote(): string {
  return isAndroid ? 'The file is deleted from the phone.' : 'The file goes to the Recycle Bin.'
}

/** While a book is open: on a phone the screen stays on, the status bar hides, and the volume keys turn pages if the setting is on. Nothing to do on the desktop. */
export function setReading(on: boolean, volumeKeys: boolean): void {
  if (isAndroid) void invoke('set_reading', { on, volumeKeys }).catch(console.error)
}

/** Registers Android's Back button. Listening takes Back over from Android, which would otherwise leave the app on every press. Nothing to do on the desktop, which has no Back button. */
export function initBackButton(onBack: () => void): void {
  if (isAndroid) void onBackButtonPress(onBack).catch(console.error)
}

/** A book handed to the app from outside ("Open with", "Share" on a phone): opens it at startup and whenever another one arrives. On the desktop books arrive on the command line instead. */
export function initIncoming(open: () => void): void {
  if (!isAndroid) return
  open()
  void addPluginListener('system', 'incoming', open).catch(console.error)
}

/** Volume keys turn pages while a book is open (a phone setting). */
export function initVolumeKeys(onKey: (direction: 'next' | 'prev') => void): void {
  if (!isAndroid) return
  void addPluginListener<{ direction: 'next' | 'prev' }>('system', 'volumeKey', ({ direction }) => onKey(direction)).catch(console.error)
}

/** Calls `arrived` when the phone says a Google sign-in answer came back on
the app's scheme; the answer itself waits in a stash (`google_take_answer`).
Nothing arrives on the desktop, where the login waits on a loopback address. */
export function initOauthAnswer(arrived: () => void): void {
  if (!isAndroid) return
  void addPluginListener('system', 'oauth', () => arrived()).catch(console.error)
}

/** Polls the phone's stashed OAuth answer until the browser comes back with it.
`take` reads the stash exactly once, `finish` completes the login. Stops after
the answer or after `misses` empty rounds; returns a function that stops it
sooner. The push event cannot be relied on (logcat-verified on the phone: the
`system` plugin listeners never register), so the connect flow polls instead. */
export function pollOauthAnswer(
  take: () => Promise<string | null>,
  finish: (url: string) => Promise<void>,
  misses = 160,
  everyMs = 1500,
): () => void {
  let left = misses
  const timer = setInterval(() => {
    void take()
      .then(answer => {
        if (answer == null) {
          if (--left <= 0) clearInterval(timer)
          return
        }
        clearInterval(timer)
        void finish(answer)
      })
      .catch(() => {
        if (--left <= 0) clearInterval(timer)
      })
  }, everyMs)
  return () => clearInterval(timer)
}
