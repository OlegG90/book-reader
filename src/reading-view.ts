import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import { BookView, el, type BookStyle, type Relocation } from 'reader-core'
import { createTOCView } from 'reader-core/foliate-js/ui/tree.js'
import { setReading } from './platform'
import { createSyncManager, type SyncManager } from './sync-manager'
import { SAVE_DELAY_MS, type Settings } from './settings'
import { bookmarkLabel, hasBookmark, toggleBookmark, type Bookmark } from './bookmarks'

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T

export interface Position {
  cfi: string
  fraction: number
}

export interface BookInfo {
  fileName: string
  fingerprint: string
  position: Position | null
  noSave: boolean
  /** What the bar under the book offers: adding it to the library, or choosing a library first. */
  offer: Offer | null
}

export type Offer = 'add' | 'setup'

export type AddMode = 'move' | 'copy'

interface PendingSave {
  fingerprint: string
  position: Position
}

export interface ReadingDeps {
  getSettings(): Settings
  settingsReady: Promise<unknown>
  currentBookStyle(): BookStyle
  showLibraryScreen(): Promise<void>
  showError(fileName: string | undefined, error: unknown): void
  onKey(e: KeyboardEvent): void
  onPointer(y: number): void
  toggleTopbar(): void
  scrimlessOpen(): boolean
  closePanels(): void
  updateTopbarButtons(): void
  showOffer(offer: Offer): void
  hideOffer(): void
  setTitle(title: string): void
  embedSettings(embed: boolean): void
  setChrome(shown: boolean): void
}

export interface ReadingView {
  readonly sync: SyncManager
  readonly book: BookView | null
  readonly fingerprint: string
  readonly noSave: boolean
  /** Opens the window's book at its saved position. */
  open(): Promise<void>
  /** Writes an unsaved position, if any. Never throws. */
  flush(): Promise<void>
  close(): void
  relocated(location: Relocation): void
  /** Bookmarks the current page, or removes the bookmark that is there. */
  toggleBookmark: () => void
  showTrackAt(fraction: number): void
}

export function createReadingView(deps: ReadingDeps): ReadingView {
  let book: BookView | null = null
  let fingerprint = ''
  /** `--no-save`: this window keeps no positions (the backend skips recent books too). */
  let noSave = false
  let tocView: { element: HTMLElement; setCurrentHref(href: string): void } | null = null
  /** The book is opening: its relocates restore the saved position, they don't move it. */
  let restoring = false
  let bookmarks: Bookmark[] = []

  let pendingSave: PendingSave | null = null
  let saveTimer: number | undefined

  function scheduleSave(save: PendingSave) {
    if (!deps.getSettings().savePositions || noSave) return
    pendingSave = save
    clearTimeout(saveTimer)
    saveTimer = window.setTimeout(flushSave, SAVE_DELAY_MS)
  }

  async function flushSave() {
    clearTimeout(saveTimer)
    const save = pendingSave
    pendingSave = null
    if (save) await invoke('save_position', { ...save }).catch(console.error)
  }

  const sync = createSyncManager({
    book: () => book,
    fingerprint: () => fingerprint,
  })

  /** Reads the window's book as a file for foliate-js. */
  async function loadBook({ fileName }: BookInfo): Promise<File> {
    const bytes = await invoke<ArrayBuffer>('read_book')
    // foliate-js detects some formats by extension, so normalise its case.
    return new File([bytes], fileName.toLowerCase())
  }

  /** Opens the window's book at its saved position. */
  async function open() {
    await flushSave()
    close()
    let info: BookInfo | null = null
    try {
      info = await invoke<BookInfo | null>('current_book')
      if (!info) return await deps.showLibraryScreen()
      fingerprint = info.fingerprint
      noSave = info.noSave
      restoring = true
      const source = await loadBook(info)
      await deps.settingsReady
      book = await BookView.open(
        $('stage'),
        source,
        {
          flow: deps.getSettings().flow,
          lastLocation: info.position?.cfi,
          style: deps.currentBookStyle(),
        },
        {
          relocate: relocated,
          key: deps.onKey,
          pointer: deps.onPointer,
          menu: deps.toggleTopbar,
          pointerDown: () => {
            if (!deps.scrimlessOpen()) return false
            deps.closePanels()
            return true
          },
          externalLink: href => void openUrl(href).catch(console.error),
        },
      )
    } catch (e) {
      deps.showError(info?.fileName, e)
      // The library, Ctrl+O and the recent books stay one step away.
      return deps.showLibraryScreen()
    } finally {
      restoring = false
    }
    const title = book.title || info.fileName
    deps.setTitle(title)
    void invoke('remember_book', { fingerprint, title, author: book.author }).catch(console.error)
    $('toc-title').textContent = title
    $('toc-author').textContent = book.author
    $('topbar-book').textContent = title
    $('topbar-author').textContent = book.author
    $('progress-ticks').replaceChildren(...book.chapterStarts().map(start => el('span', { style: `left: ${start * 100}%` })))
    tocView = createTOCView(book.toc, (href: string) => {
      book?.view.goTo(href).catch(console.error)
      deps.closePanels()
    })
    $('toc-tree').replaceChildren(tocView.element)
    currentTocHref = undefined
    highlightToc(book.view.lastLocation?.tocItem?.href)
    $('progress').hidden = false
    $('bottombar').hidden = false
    deps.updateTopbarButtons()
    if (info.offer && deps.getSettings().offerLibrary) deps.showOffer(info.offer)
    bookmarks = await invoke<Bookmark[]>('get_bookmarks', { fingerprint }).catch((e): Bookmark[] => {
      console.error(e)
      return []
    })
    renderBookmarks()
    setReading(true, deps.getSettings().volumeKeys)
    void sync.pull()
  }

  function close() {
    if (book) setReading(false, deps.getSettings().volumeKeys)
    void sync.push()
    sync.reset()
    deps.hideOffer()
    book?.destroy()
    book = null
    tocView = null
    bookmarks = []
    renderBookmarks()
    deps.closePanels()
    deps.updateTopbarButtons()
    $('toc-tree').replaceChildren()
    $('start').hidden = true
    document.body.classList.remove('library-mode')
    deps.embedSettings(false)
    $('error').hidden = true
    $('progress').hidden = true
    $('bottombar').hidden = true
    deps.setChrome(false)
  }

  function relocated(location: Relocation) {
    const { fraction, cfi, tocItem } = location
    const percent = Math.round(fraction * 100)
    $('progress-fill').style.width = `${fraction * 100}%`
    showTrackAt(fraction)
    $('progress-chapter').textContent = tocItem?.label?.trim() ?? ''
    // Pages count within the chapter (the book-wide position is the percentage).
    const at = book?.chapterPage
    $('progress-where').textContent = at ? `p. ${at.page} of ${at.of} · ${percent}%` : `${percent}%`
    highlightToc(tocItem?.href)
    updateBookmarkButton()
    if (restoring) return
    scheduleSave({ fingerprint, position: { cfi, fraction } })
    if (!noSave) sync.markDirty()
  }

  let currentTocHref: string | undefined

  function highlightToc(href: string | undefined) {
    if (!href || href === currentTocHref) return
    currentTocHref = href
    tocView?.setCurrentHref(href)
  }

  function showTrackAt(fraction: number) {
    $('progress-done').style.width = `${fraction * 100}%`
    $('progress-knob').style.left = `${fraction * 100}%`
  }

  function toggleCurrentBookmark() {
    const location = book?.view.lastLocation
    if (!location || noSave) return
    const mark = { cfi: location.cfi, fraction: location.fraction, chapter: location.tocItem?.label?.trim() ?? '' }
    setBookmarks(toggleBookmark(bookmarks, mark))
  }

  function setBookmarks(list: Bookmark[]) {
    bookmarks = list
    renderBookmarks()
    void invoke('set_bookmarks', { fingerprint, bookmarks: list }).catch(console.error)
  }

  function renderBookmarks() {
    $('bookmark-count').textContent = bookmarks.length ? `(${bookmarks.length})` : ''
    $('bookmarks-empty').hidden = bookmarks.length > 0
    $('bookmark-list').replaceChildren(
      ...bookmarks.map(mark =>
        el(
          'li',
          {},
          el('button', {
            className: 'go',
            textContent: bookmarkLabel(mark),
            onclick: () => {
              book?.view.goTo(mark.cfi).catch(console.error)
              deps.closePanels()
            },
          }),
          el('button', {
            className: 'remove',
            textContent: '×',
            title: 'Remove bookmark',
            onclick: () => setBookmarks(bookmarks.filter(b => b !== mark)),
          }),
        ),
      ),
    )
    updateBookmarkButton()
  }

  function updateBookmarkButton() {
    const marked = hasBookmark(bookmarks, book?.view.lastLocation?.cfi)
    $('bookmark-button').setAttribute('aria-pressed', String(marked))
    // The icon fills in when the page is bookmarked (style.css).
    $('bookmark-button').setAttribute('aria-label', marked ? 'Bookmarked' : 'Bookmark')
    $('bookmark-button').title = marked ? 'Remove the bookmark on this page (B)' : 'Bookmark this page (B)'
  }

  return {
    sync,
    flush: flushSave,
    get book() {
      return book
    },
    get fingerprint() {
      return fingerprint
    },
    get noSave() {
      return noSave
    },
    open,
    close,
    relocated,
    toggleBookmark: toggleCurrentBookmark,
    showTrackAt,
  }
}
