export type Command =
  | 'left'
  | 'right'
  | 'next'
  | 'prev'
  | 'lineDown'
  | 'lineUp'
  | 'chapterStart'
  | 'chapterEnd'
  | 'toc'
  | 'library'
  | 'bookmark'
  | 'toggleFlow'
  | 'settings'
  | 'cycleTheme'
  | 'fontBigger'
  | 'fontSmaller'
  | 'fullscreen'
  | 'open'
  | 'closeWindow'
  | 'escape'

type Key = Pick<KeyboardEvent, 'key' | 'shiftKey' | 'ctrlKey' | 'altKey' | 'metaKey'> & Partial<Pick<KeyboardEvent, 'code'>>

/** Keys named by where they sit (a US keyboard), so shortcuts work in any layout. */
const BY_POSITION: Record<string, string> = { Comma: ',', Equal: '=', Minus: '-' }

/**
 * The key as the shortcuts name it: letters and the few punctuation keys by
 * their place on the keyboard (T is T in a Ukrainian layout too), letters in
 * lower case; other keys (arrows, Space, F11, numpad +) by what they are.
 */
function shortcutKey(e: Key) {
  const code = e.code ?? ''
  if (code.startsWith('Key')) return code.slice(3).toLowerCase()
  if (code in BY_POSITION) return BY_POSITION[code]
  return e.key.length === 1 ? e.key.toLowerCase() : e.key
}

/** Maps a key press to a reader command; `null` leaves it to the browser. */
export function commandFor(e: Key): Command | null {
  if (e.altKey || e.metaKey) return null
  const key = shortcutKey(e)
  if (e.ctrlKey) {
    switch (key) {
      case '=':
      case '+':
        return 'fontBigger'
      case '-':
        return 'fontSmaller'
      case ',':
        return 'settings'
    }
    switch (key) {
      case 'o':
        return 'open'
      case 'w':
        return 'closeWindow'
    }
    return null
  }
  // Letters work with or without Shift / Caps Lock, in any keyboard layout.
  switch (key) {
    case 'ArrowLeft':
      return 'left'
    case 'ArrowRight':
      return 'right'
    case 'ArrowDown':
      return 'lineDown'
    case 'ArrowUp':
      return 'lineUp'
    case 'PageDown':
      return 'next'
    case 'PageUp':
      return 'prev'
    case ' ':
      return e.shiftKey ? 'prev' : 'next'
    case 'Home':
      return 'chapterStart'
    case 'End':
      return 'chapterEnd'
    case 't':
      return 'toc'
    case 'l':
      return 'library'
    case 'b':
      return 'bookmark'
    case 'm':
      return 'toggleFlow'
    case 's':
      return 'settings'
    case 'd':
      return 'cycleTheme'
    case 'F11':
      return 'fullscreen'
    case 'Escape':
      return 'escape'
  }
  return null
}
