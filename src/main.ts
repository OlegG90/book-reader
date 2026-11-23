import { addPluginListener, invoke } from '@tauri-apps/api/core'
import { onBackButtonPress } from '@tauri-apps/api/app'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { openUrl } from '@tauri-apps/plugin-opener'
import {
  BookView,
  bookCss,
  el,
  isAppearanceKey,
  lineWidthPx,
  nextTheme,
  oneOf,
  readAppearance,
  stepFontSize,
  suppressContextMenu,
  type BookStyle,
  type Flow,
  type Relocation,
  type ResolvedTheme,
} from 'reader-core'
import 'reader-core/book-view.css'
// The interface fonts, bundled (offline-first): Latin and Cyrillic only.
import '@fontsource/cormorant-garamond/latin-600.css'
import '@fontsource/cormorant-garamond/cyrillic-600.css'
import '@fontsource/lora/latin-400.css'
import '@fontsource/lora/cyrillic-400.css'
import '@fontsource/lora/latin-600.css'
import '@fontsource/lora/cyrillic-600.css'
import { createTOCView } from 'reader-core/foliate-js/ui/tree.js'
import { commandFor, type Command } from './keys'
import {
  SORTS,
  SORT_LABELS,
  STATUSES,
  STATUS_LABELS,
  filterBooks,
  seriesLabel,
  sortBooks,
  statusOf,
  type LibraryBook,
  type Sort,
  type Status,
} from './library'
import { createCatalogView } from './catalog-view'
import { sidebarResizer } from './sidebar-resize'
import { createSourcesPanel } from './sources-panel'
import { bookmarkLabel, hasBookmark, toggleBookmark, type Bookmark } from './bookmarks'
import { HISTORY_LENGTHS, UI_FONTS, createAppearancePanel, createSettingsPage, type PanelValues, type SettingsSection } from './settings-panel'

interface Position {
  cfi: string
  fraction: number
}

interface BookInfo {
  fileName: string
  fingerprint: string
  position: Position | null
  noSave: boolean
  /** What the bar under the book offers: adding it to the library, or choosing a library first. */
  offer: Offer | null
}

type Offer = 'add' | 'setup'

type AddMode = 'move' | 'copy'

interface PendingSave {
  fingerprint: string
  position: Position
}

interface Settings extends PanelValues {
  flow: Flow
  librarySort: Sort
  libraryStatus: Status
  /** The last choice in the "add to library" bar, offered first next time. */
  libraryAddMode: AddMode
}

const FLOWS: readonly Flow[] = ['paginated', 'scrolled']

function readSettings(stored: Record<string, unknown>): Settings {
  return {
    ...readAppearance(stored),
    flow: oneOf(stored.flow, FLOWS, 'paginated'),
    librarySort: oneOf(stored.librarySort, SORTS, 'author'),
    libraryStatus: oneOf(stored.libraryStatus, STATUSES, 'all'),
    libraryAddMode: oneOf(stored.libraryAddMode, ['move', 'copy'] as const, 'move'),
    savePositions: stored.savePositions !== false,
    offerLibrary: stored.offerLibrary !== false,
    volumeKeys: stored.volumeKeys === true,
    historyLength: HISTORY_LENGTHS.find(n => n === stored.historyLength) ?? 3,
    uiFont: oneOf(stored.uiFont, UI_FONTS, 'classic'),
  }
}

const SAVE_DELAY_MS = 500
const TOP_EDGE_PX = 48
/** The bottom bar shows when the mouse comes this close to the bottom edge. */
const BOTTOM_EDGE_PX = 56

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T
// Hides what only the other platform does (`.desktop-only`, `.android-only` in style.css).
const isAndroid = /Android/.test(navigator.userAgent)
document.documentElement.classList.toggle('android', isAndroid)
const appWindow = getCurrentWindow()
const topbar = $('topbar')

let settings: Settings = readSettings({})
// Loads alongside the first book; the backend already set the saved theme
// before the first paint (see theme_script in lib.rs).
const settingsReady = invoke<Record<string, unknown>>('get_settings')
  .catch(e => {
    console.error(e)
    return {}
  })
  .then(stored => {
    settings = readSettings(stored)
    applyTheme()
    showSettings()
    showLibraryControls()
  })
void loadLibraryInfo()
let currentTocHref: string | undefined
let book: BookView | null = null
let fingerprint = ''
/** `--no-save`: this window keeps no positions (the backend skips recent books too). */
let noSave = false
let tocView: { element: HTMLElement; setCurrentHref(href: string): void } | null = null
/** The book is opening: its relocates restore the saved position, they don't move it. */
let restoring = false

// ---- Reading position -------------------------------------------------------

let pendingSave: PendingSave | null = null
let saveTimer: number | undefined

function scheduleSave(save: PendingSave) {
  if (!settings.savePositions || noSave) return
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

// ---- Opening books ----------------------------------------------------------

/** Reads the window's book as a file for foliate-js. */
async function loadBook({ fileName }: BookInfo): Promise<File> {
  const bytes = await invoke<ArrayBuffer>('read_book')
  // foliate-js detects some formats by extension, so normalise its case.
  return new File([bytes], fileName.toLowerCase())
}

/** Opens the window's book at its saved position. */
async function openCurrentBook() {
  await flushSave()
  closeBook()
  let info: BookInfo | null = null
  try {
    info = await invoke<BookInfo | null>('current_book')
    if (!info) return await showLibraryScreen()
    fingerprint = info.fingerprint
    noSave = info.noSave
    restoring = true
    const source = await loadBook(info)
    await settingsReady
    book = await BookView.open($('stage'), source, {
      flow: settings.flow,
      lastLocation: info.position?.cfi,
      style: currentBookStyle(),
    }, {
      relocate: onRelocate,
      key: onKey,
      pointer: onPointer,
      menu: toggleTopbar,
      pointerDown: () => {
        if (!scrimlessOpen()) return false
        closePanels()
        return true
      },
      externalLink: href => void openUrl(href).catch(console.error),
    })
  } catch (e) {
    showError(info?.fileName, e)
    // The library, Ctrl+O and the recent books stay one step away.
    return showLibraryScreen()
  } finally {
    restoring = false
  }
  const title = book.title || info.fileName
  void appWindow.setTitle(title).catch(console.error)
  void invoke('remember_book', { fingerprint, title, author: book.author }).catch(console.error)
  $('toc-title').textContent = title
  $('toc-author').textContent = book.author
  $('topbar-book').textContent = title
  $('topbar-author').textContent = book.author
  $('progress-ticks').replaceChildren(
    ...book.chapterStarts().map(start => el('span', { style: `left: ${start * 100}%` })),
  )
  tocView = createTOCView(book.toc, (href: string) => {
    book?.view.goTo(href).catch(console.error)
    closePanels()
  })
  $('toc-tree').replaceChildren(tocView.element)
  currentTocHref = undefined
  highlightToc(book.view.lastLocation?.tocItem?.href)
  $('progress').hidden = false
  $('bottombar').hidden = false
  updateTopbarButtons()
  if (info.offer && settings.offerLibrary) showOffer(info.offer)
  bookmarks = await invoke<Bookmark[]>('get_bookmarks', { fingerprint }).catch((e): Bookmark[] => {
    console.error(e)
    return []
  })
  renderBookmarks()
  setReading(true)
  void pullSync()
}

/** Android, while a book is open: screen on, status bar hidden, volume keys (if set) turn pages. */
function setReading(on: boolean) {
  if (isAndroid) void invoke('set_reading', { on, volumeKeys: settings.volumeKeys }).catch(console.error)
}

// ---- Progress sync ----------------------------------------------------------

/** How often a changed position goes to the sync server while reading. */
const SYNC_EVERY_MS = 60_000
/** Closing the window waits this long at most for the last push. */
const SYNC_ON_CLOSE_MS = 3_000

interface SyncOffer {
  cfi: string
  fraction: number
  device: string
  time: number
}

let syncOffer: SyncOffer | null = null
/** The open book's id on the sync server; null when it doesn't sync. */
let syncDocument: string | null = null
/** The position moved since the last push. */
let syncDirty = false

/** Asks the sync server whether another device read on, and offers to jump there. */
async function pullSync() {
  const document = await invoke<string | null>('sync_document').catch(e => {
    console.warn('sync:', e)
    return null
  })
  if (!document || !book) return
  syncDocument = document
  const state = await invoke<{ offer: SyncOffer | null; push: boolean }>('sync_pull', { document, fingerprint }).catch(e => {
    console.warn('sync:', e)
    return null
  })
  if (!state || !book || syncDocument !== document) return
  if (state.push) syncDirty = true
  const offer = state.offer
  if (!offer) return
  syncOffer = offer
  $('sync-text').textContent = `${offer.device || 'Another device'}, ${timeAgo(offer.time)}: ${Math.round(offer.fraction * 100)}%`
  $('sync-bar').hidden = false
}

/** "2 h ago", "yesterday"... */
function timeAgo(time: number) {
  const minutes = Math.round((Date.now() - time) / 60_000)
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours} h ago`
  const days = Math.round(hours / 24)
  return days === 1 ? 'yesterday' : `${days} days ago`
}

async function jumpToSynced() {
  const offer = syncOffer
  hideSyncBar()
  if (!offer || !book) return
  // The same file on both devices, so the CFI should resolve; the share read is the fallback.
  await book.view.goTo(offer.cfi).catch(() => book?.view.goToFraction(offer.fraction))
}

function hideSyncBar() {
  syncOffer = null
  $('sync-bar').hidden = true
}

/** Sends the position if it moved; failures stay quiet and are retried next time. */
async function pushSync() {
  const location = book?.view.lastLocation
  const document = syncDocument
  if (!syncDirty || !location || !document) return
  syncDirty = false
  await invoke('sync_push', { document, cfi: location.cfi, fraction: location.fraction }).catch(e => {
    // Retried next time, unless another book has opened meanwhile.
    if (syncDocument === document) syncDirty = true
    console.warn('sync:', e)
  })
}

// ---- Bookmarks --------------------------------------------------------------

let bookmarks: Bookmark[] = []

/** B: bookmarks the current page, or removes the bookmark that is there. */
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
            closePanels()
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

// ---- Adding a book to the library ------------------------------------------

/**
 * Shows the bar: Move / Copy (the last used first) for a book outside the
 * library, or the way to the first-run folder choice when there is no library yet.
 */
function showOffer(offer: Offer) {
  $('offer-text').textContent =
    offer === 'add' ? 'This book is not in your library.' : 'Book Reader has no library folder yet.'
  $('offer-buttons').hidden = false
  $('offer-setup').hidden = offer !== 'setup'
  $('offer-buttons').querySelectorAll<HTMLElement>('.add').forEach(button => (button.hidden = offer !== 'add'))
  $('offer-never').hidden = true
  $('offer-more').hidden = offer !== 'add'
  $('offer-move').classList.toggle('primary', settings.libraryAddMode === 'move')
  $('offer-copy').classList.toggle('primary', settings.libraryAddMode === 'copy')
  $('offer').hidden = false
}

let offerTimer: number | undefined

function hideOffer() {
  clearTimeout(offerTimer)
  $('offer').hidden = true
}

async function addToLibrary(mode: AddMode) {
  changeSettings({ libraryAddMode: mode })
  $('offer-buttons').hidden = true
  try {
    const { path, existed, originalKept } = await invoke<{ path: string; existed: boolean; originalKept: boolean }>('add_to_library', { mode })
    $('offer-text').textContent = existed
      ? `Already in the library: ${path}`
      : originalKept
        ? `Copied to the library (the original couldn't be removed): ${path}`
        : `${mode === 'move' ? 'Moved' : 'Copied'} to the library: ${path}`
  } catch (e) {
    $('offer-text').textContent = `Couldn't add the book: ${e}`
  }
  offerTimer = window.setTimeout(hideOffer, 4000)
}

function closeBook() {
  if (book) setReading(false)
  void pushSync()
  syncDocument = null
  syncDirty = false
  hideSyncBar()
  hideOffer()
  book?.destroy()
  book = null
  tocView = null
  bookmarks = []
  renderBookmarks()
  closePanels()
  updateTopbarButtons()
  $('toc-tree').replaceChildren()
  $('start').hidden = true
  document.body.classList.remove('library-mode')
  embedSettings(false)
  $('error').hidden = true
  $('progress').hidden = true
  $('bottombar').hidden = true
  setChrome(false)
}

/** Contents and the flow need an open book; the rest of the topbar works without one. */
function updateTopbarButtons() {
  const withoutBook = !book
  $('toc-button').toggleAttribute('disabled', withoutBook)
  $('flow-button').toggleAttribute('disabled', withoutBook)
  // --no-save keeps nothing, bookmarks included.
  $('bookmark-button').toggleAttribute('disabled', withoutBook || noSave)
  updateFlowButton()
}

interface RecentBook {
  key: string
  title: string
  author: string
  fileName: string
  fraction: number | null
  exists: boolean
  /** The library's key for its cover, when the library has the book. */
  coverKey: string | null
}

interface LibraryInfo {
  folder: string | null
  suggested: string
}

/** The window without a book: the library, or the first-run folder choice. */
async function showLibraryScreen() {
  void appWindow.setTitle('Book Reader').catch(console.error)
  document.body.classList.add('library-mode')
  embedSettings(true)
  $('start').hidden = false
  const info = await loadLibraryInfo()
  libraryReady = !!info.folder
  if (!info.folder) {
    if (settingsEmbedded()) showView(view ?? 'library')
    $('setup-suggested').textContent = info.suggested
    $('setup-default').focus()
    return
  }
  await refreshLibraryScreen()
  // The desktop keeps what its pane showed; at first, the history if there is one.
  if (settingsEmbedded()) showView(view ?? (hasRecent ? 'recent' : 'library'))
}

/** Reads the library folder (and the first-run suggestion), and shows it in the settings. */
async function loadLibraryInfo(): Promise<LibraryInfo> {
  const info = await invoke<LibraryInfo>('library_info').catch((e): LibraryInfo => {
    console.error(e)
    return { folder: null, suggested: '' }
  })
  libraryFolder = info.folder
  settingsPage.setLibraryFolder(info.folder)
  return info
}

const refreshLibraryScreen = () => Promise.all([refreshRecent(), refreshLibrary()])

async function refreshRecent() {
  const books = await invoke<RecentBook[]>('recent_books').catch((e): RecentBook[] => {
    console.error(e)
    return []
  })
  $('recent').replaceChildren(...books.slice(0, settings.historyLength).map(recentItem))
  $('recent-title').hidden = !books.length
  $('recent-empty').hidden = books.length > 0
  hasRecent = books.length > 0
}

/** The reading history isn't empty (the desktop then opens on it). */
let hasRecent = false

/** Uses the suggested library folder, or asks for another one. */
async function chooseLibrary(pick: boolean) {
  const folder = await invoke<string | null>('set_library', { pick }).catch(e => {
    showError(undefined, e)
    return null
  })
  if (folder && !book) await showLibraryScreen()
}

// ---- Library ----------------------------------------------------------------

let libraryFolder: string | null = null
let libraryBooks: LibraryBook[] = []
let scanning: Promise<void> | null = null
const libraryVisible = () => !book && !$('library').hidden

/** Rescans the folder (cheap: unchanged files come from the cache) and redraws. */
function refreshLibrary(): Promise<void> {
  scanning ??= (async () => {
    if (!libraryBooks.length) showLibraryMessage('Looking for books…')
    try {
      libraryBooks = await invoke<LibraryBook[]>('scan_library')
      renderLibrary()
    } catch (e) {
      showLibraryMessage(`Can't read the library folder: ${e}`)
    } finally {
      scanning = null
    }
  })()
  return scanning
}

function showLibraryMessage(text: string) {
  $('library-message').textContent = text
  $('library-message').hidden = !text
}

function showLibraryControls() {
  $<HTMLSelectElement>('library-sort').value = settings.librarySort
  renderLibrary()
}

function renderLibrary() {
  const query = $<HTMLInputElement>('library-search').value
  const shown = sortBooks(filterBooks(libraryBooks, query, settings.libraryStatus), settings.librarySort)
  // The status switch: each choice with how many of the matching books it holds.
  $('library-status').replaceChildren(
    ...STATUSES.map(status =>
      el(
        'button',
        {
          type: 'button',
          role: 'radio',
          ariaChecked: String(status === settings.libraryStatus),
          onclick: () => {
            changeSettings({ libraryStatus: status })
            renderLibrary()
          },
        },
        STATUS_LABELS[status],
        el('span', { className: 'count', textContent: String(filterBooks(libraryBooks, query, status).length) }),
      ),
    ),
  )
  if (!libraryBooks.length) {
    showLibraryMessage(`The library is empty. Open a book (Ctrl+O) or put EPUB and FB2 files into ${libraryFolder}.`)
  } else {
    showLibraryMessage(shown.length ? '' : 'No books match.')
  }
  $('library-books').replaceChildren(...shown.map(bookCard))
}

function bookCard(book: LibraryBook) {
  const status = statusOf(book)
  const progress =
    status === 'finished' ? 'Finished' : status === 'reading' ? `${Math.max(1, Math.round((book.fraction ?? 0) * 100))}%` : ''
  const button = el(
    'button',
    {
      title: [book.title, book.authors.join(', '), seriesLabel(book)].filter(Boolean).join('\n'),
      onclick: () => void invoke('open_library_book', { key: book.key }).catch(e => showError(book.title, e)),
    },
    coverWithProgress(book.title, book.hasCover ? book.key : null, book.fraction),
    el('span', { className: 'title', textContent: book.title }),
    el('span', { className: 'author', textContent: book.authors.join(', ') }),
    el('span', { className: 'series', textContent: seriesLabel(book) }),
    el('span', { className: 'meta', textContent: [book.format, progress].filter(Boolean).join(' · ') }),
  )
  return el('li', {}, button)
}

/** Cover images by book key, kept for the window's life so redraws don't reload them. */
const coverUrls = new Map<string, Promise<string>>()

/** Loads covers as their cards come near the visible part of the library. */
const coverObserver = new IntersectionObserver(
  entries => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue
      const host = entry.target as HTMLElement
      coverObserver.unobserve(host)
      const key = host.dataset.key ?? ''
      let url = coverUrls.get(key)
      if (!url) {
        url = invoke<ArrayBuffer>('library_cover', { key }).then(bytes => URL.createObjectURL(new Blob([bytes])))
        coverUrls.set(key, url)
      }
      url.then(src => host.replaceChildren(el('img', { src, alt: '' }))).catch(() => coverUrls.delete(key))
    }
  },
  { rootMargin: '400px' },
)

function loadCover(host: HTMLElement, key: string) {
  host.dataset.key = key
  coverObserver.observe(host)
}

/** A book's cover (or its title on a plain tile), with how far it is read as a line under it. */
function coverWithProgress(title: string, coverKey: string | null, fraction: number | null) {
  const cover = el('div', { className: 'cover' }, el('span', { textContent: title }))
  if (coverKey) loadCover(cover, coverKey)
  const line = el('span', { className: 'read-line' })
  if (fraction) line.append(el('span', { style: `width: ${Math.max(2, Math.round(fraction * 100))}%` }))
  return el('div', { className: 'cover-box' }, cover, line)
}

/** A "Continue reading" book: a card like the library's. */
function recentItem(book: RecentBook) {
  const title = book.title || book.fileName
  const percent = book.fraction == null ? '' : `${Math.max(1, Math.round(book.fraction * 100))}%`
  const button = el(
    'button',
    book.exists
      ? { title: [title, book.author].filter(Boolean).join('\n'), onclick: () => void invoke('open_recent', { key: book.key }).catch(console.error) }
      : {
          className: 'missing',
          title: 'File not found. Click to remove it from the list.',
          onclick: () => void invoke('forget_recent', { key: book.key }).then(refreshRecent, console.error),
        },
    coverWithProgress(title, book.coverKey, book.fraction),
    el('span', { className: 'title', textContent: title }),
    el('span', { className: 'author', textContent: book.author }),
    el('span', { className: 'meta', textContent: book.exists ? percent : 'File not found' }),
  )
  return el('li', {}, button)
}

function showError(fileName: string | undefined, error: unknown) {
  const box = $('error')
  const title = document.createElement('p')
  title.textContent = fileName ? `Can't open “${fileName}”` : "Can't open the book"
  const detail = document.createElement('p')
  detail.className = 'muted'
  const message = error instanceof Error ? error.message : String(error)
  detail.textContent = message || 'The file is damaged or in an unsupported format'
  box.replaceChildren(title, detail)
  box.hidden = false
}

function onRelocate(location: Relocation) {
  const { fraction, cfi, tocItem } = location
  const percent = Math.round(fraction * 100)
  $('progress-fill').style.width = `${fraction * 100}%`
  $('progress-done').style.width = `${fraction * 100}%`
  $('progress-knob').style.left = `${fraction * 100}%`
  $('progress-chapter').textContent = tocItem?.label?.trim() ?? ''
  // Pages count within the chapter (the book-wide position is the percentage).
  const at = book?.chapterPage
  $('progress-where').textContent = at ? `p. ${at.page} of ${at.of} · ${percent}%` : `${percent}%`
  highlightToc(tocItem?.href)
  updateBookmarkButton()
  if (restoring) return
  scheduleSave({ fingerprint, position: { cfi, fraction } })
  if (!noSave) syncDirty = true
}

function highlightToc(href: string | undefined) {
  if (!href || href === currentTocHref) return
  currentTocHref = href
  tocView?.setCurrentHref(href)
}

// ---- Commands ---------------------------------------------------------------

const tocOpen = () => !$('toc').hidden
/** The settings page over the window (a book window, or the phone); the library window on a desktop has them in its content. */
const settingsOpen = () => !$('settings').hidden && !settingsEmbedded()
const appearanceOpen = () => !$('appearance').hidden
const aboutOpen = () => !$('about').hidden
const shortcutsOpen = () => !$('shortcuts').hidden
/** Panels that open without the scrim (the contents have one, so need no outside check). */
const scrimlessOpen = () => appearanceOpen() || aboutOpen() || shortcutsOpen()

/** Commands that still work while a side panel has the keyboard. */
const PANEL_COMMANDS: Command[] = ['toc', 'settings', 'escape', 'fullscreen', 'cycleTheme', 'fontBigger', 'fontSmaller']

/** Whether a panel should get the key instead of the reader. */
function panelHasKeyboard(e: KeyboardEvent, command: Command) {
  const target = e.target instanceof Element ? e.target : null
  // Letters pick options in a focused list; only Escape leaves it.
  if (target?.closest('select, input')) return command !== 'escape'
  // Keys pressed in the book (its iframes) still turn pages beside the settings.
  return !!target?.closest('#toc, #settings, #appearance, #about, #shortcuts') && !PANEL_COMMANDS.includes(command)
}

function onKey(e: KeyboardEvent) {
  const command = commandFor(e)
  if (!command || panelHasKeyboard(e, command)) return
  // The organize preview is modal: only Escape gets past it.
  if (organizeOpen() && command !== 'escape') return
  e.preventDefault()
  runCommand(command).catch(console.error)
}

async function runCommand(command: Command) {
  switch (command) {
    case 'escape':
      return handleEscape()
    case 'fullscreen':
      return appWindow.setFullscreen(!(await appWindow.isFullscreen()))
    case 'settings':
      // S / Ctrl+, and the Aa button: the appearance while reading, the settings in the library.
      if (book) return appearanceOpen() ? closePanels() : openAppearance()
      if (settingsEmbedded()) return showView(view?.startsWith('settings') ? 'library' : 'settings:reading')
      return settingsOpen() ? closePanels() : openSettings()
    case 'cycleTheme':
      return changeSettings({ theme: nextTheme(settings.theme) })
    case 'fontBigger':
      return changeSettings({ fontSize: stepFontSize(settings.fontSize, 1) })
    case 'fontSmaller':
      return changeSettings({ fontSize: stepFontSize(settings.fontSize, -1) })
    case 'open':
      return invoke('pick_book')
    case 'library':
      return invoke('show_library')
    case 'closeWindow':
      return appWindow.close()
  }
  if (!book) return
  switch (command) {
    case 'left':
      return book.view.goLeft()
    case 'right':
      return book.view.goRight()
    case 'next':
      return book.view.next()
    case 'prev':
      return book.view.prev()
    case 'lineDown':
      return book.lineDown()
    case 'lineUp':
      return book.lineUp()
    case 'chapterStart':
      return book.goToChapterEdge('start')
    case 'chapterEnd':
      return book.goToChapterEdge('end')
    case 'toc':
      return tocOpen() ? closePanels() : openToc()
    case 'toggleFlow':
      return toggleFlow()
    case 'bookmark':
      return toggleCurrentBookmark()
  }
}

/** Closes the footnote, or else any open panel; false when there was none. */
function closeTopLayer() {
  if (book?.footnoteOpen) book.hideFootnote()
  else if (tocOpen() || organizeOpen() || settingsOpen() || scrimlessOpen()) closePanels()
  else return false
  return true
}

async function handleEscape() {
  if (closeTopLayer()) return
  // In the library window, Esc goes back from the settings or the catalogs to the books.
  if (settingsEmbedded() && view !== 'library' && view !== 'recent') return showView('library')
  if (await appWindow.isFullscreen()) await appWindow.setFullscreen(false)
}

/**
 * Android's Back, one step at a time: a note or a panel closes first, then the
 * book returns to the library, and the library sends the app to the background.
 */
function handleBack() {
  if (closeTopLayer()) return
  if (!book) {
    // In the catalogs, Back goes up a folder; from the top, and from the settings, to the books.
    if (view === 'catalogs' && catalogView.back()) return
    if (view !== 'recent' && view !== 'library') return showView(hasRecent ? 'recent' : 'library')
  }
  void invoke(book ? 'show_library' : 'leave_app').catch(console.error)
}

function toggleFlow() {
  if (!book) return
  const flow = (book.flow = book.flow === 'paginated' ? 'scrolled' : 'paginated')
  updateFlowButton()
  changeSettings({ flow })
}

function updateFlowButton() {
  const scrolled = book?.flow === 'scrolled'
  const icon = () => el('span', { innerHTML: `<svg class="icon" aria-hidden="true"><use href="#i-${scrolled ? 'scroll' : 'book-open'}"/></svg>` })
  const mode = scrolled ? 'Scrolled' : 'Paginated'
  $('flow-button').replaceChildren(icon())
  $('flow-button').title = `${mode} — switch to ${scrolled ? 'paginated' : 'scrolled'} (M)`
  $('flow-button').setAttribute('aria-label', mode)
  appearancePanel.show(settings, book?.flow)
  $('phone-flow').replaceChildren(icon(), el('span', { textContent: mode }))
}

/** The drawer's two tabs; the last one chosen stays for the session. */
let tocTab: 'contents' | 'bookmarks' = 'contents'

function showTocTab(tab: 'contents' | 'bookmarks') {
  tocTab = tab
  $('toc-tree').hidden = tab !== 'contents'
  $('bookmarks').hidden = tab !== 'bookmarks'
  $('toc-tab-contents').setAttribute('aria-selected', String(tab === 'contents'))
  $('toc-tab-bookmarks').setAttribute('aria-selected', String(tab === 'bookmarks'))
}

function openToc() {
  closePanels()
  $('scrim').hidden = false
  $('toc').hidden = false
  showTocTab(tocTab)
  const current = $('toc-tree').querySelector<HTMLElement>('[aria-current]')
  ;(current ?? $('toc-tree').querySelector<HTMLElement>('[role="treeitem"]'))?.focus()
}

/** Info panels open without the scrim, like the settings. */
function openInfoPanel(id: 'about' | 'shortcuts') {
  closePanels()
  $(id).hidden = false
  $(id).querySelector<HTMLElement>('button')?.focus()
}

/** Settings open without the scrim, so changes show on the book right away. */
function openSettings(section?: SettingsSection) {
  if (settingsEmbedded()) return showView(`settings:${section ?? 'reading'}`)
  closePanels()
  $('settings').hidden = false
  void refreshFileTypesStatus()
  if (section) settingsPage.showSection(section)
  $('settings').querySelector<HTMLElement>('button')?.focus()
}

/** Aa: opens without the scrim, so changes show on the book right away. */
function openAppearance() {
  closePanels()
  $('appearance').hidden = false
  $('settings-button').setAttribute('aria-expanded', 'true')
  $('appearance').querySelector<HTMLElement>('button[aria-pressed="true"], button')?.focus()
}

// ---- Organize library ---------------------------------------------------------

interface Rename {
  from: string
  to: string
}

const organizeOpen = () => !$('organize').hidden

/** Shows what "Organize library" would rename, for confirmation. */
async function openOrganize() {
  closePanels()
  $('scrim').hidden = false
  $('organize').hidden = false
  $('organize-summary').textContent = 'Checking the library…'
  $('organize-list').replaceChildren()
  $('organize-apply').hidden = true
  try {
    // The plan works on the last scan, so make it current first.
    await invoke('scan_library')
    const renames = await invoke<Rename[]>('organize_preview')
    showOrganizePlan(renames)
  } catch (e) {
    $('organize-summary').textContent = `Can't read the library: ${e}`
  }
}

function showOrganizePlan(renames: Rename[]) {
  $('organize-summary').textContent = renames.length
    ? `${renames.length === 1 ? 'This book' : `These ${renames.length} books`} will be renamed to “Author/Series NN - Title”. Nothing is deleted or overwritten.`
    : 'Every book already follows the naming pattern.'
  $('organize-list').replaceChildren(
    ...renames.map(r => el('li', {}, el('span', { className: 'from muted', textContent: r.from }), el('span', { className: 'to', textContent: `→ ${r.to}` }))),
  )
  $('organize-apply').textContent = `Rename ${renames.length === 1 ? 'the book' : `${renames.length} books`}`
  $('organize-apply').hidden = !renames.length
  $('organize-cancel').textContent = renames.length ? 'Cancel' : 'Close'
  ;(renames.length ? $('organize-apply') : $('organize-cancel')).focus()
}

async function applyOrganize() {
  $('organize-apply').hidden = true
  $('organize-summary').textContent = 'Renaming…'
  try {
    const count = await invoke<number>('organize_apply')
    $('organize-summary').textContent = `Renamed ${count} ${count === 1 ? 'book' : 'books'}.`
    $('organize-list').replaceChildren()
    $('organize-cancel').textContent = 'Close'
  } catch (e) {
    $('organize-summary').textContent = `Renaming stopped: ${e}`
  }
}

function closePanels() {
  closeMoreMenu()
  $('scrim').hidden = true
  $('organize').hidden = true
  $('toc').hidden = true
  if (!settingsEmbedded()) $('settings').hidden = true
  $('appearance').hidden = true
  $('settings-button').setAttribute('aria-expanded', 'false')
  $('about').hidden = true
  $('shortcuts').hidden = true
}

// ---- Settings and appearance ------------------------------------------------

const systemDark = matchMedia('(prefers-color-scheme: dark)')
let fileTypesRegistered = false
const settingsPage = createSettingsPage($('settings'), settings, changes => void changeSettings(changes), {
  clearHistory: () => {
    void invoke('clear_recent')
      .then(() => {
        if (!book) void refreshRecent()
      })
      .catch(console.error)
  },
  toggleFileTypes: () => {
    const command = fileTypesRegistered ? 'unregister_file_types' : 'register_file_types'
    void invoke(command).then(refreshFileTypesStatus, console.error)
  },
  changeLibrary: () => void chooseLibrary(true),
  organizeLibrary: () => void openOrganize(),
  close: closePanels,
})
const appearancePanel = createAppearancePanel($('appearance'), settings, changes => void changeSettings(changes), {
  flow: flow => changeSettings({ flow }),
  allSettings: () => openSettings('reading'),
})
// The catalogs (OPDS servers), the key list and the version have their own sections in the settings.
const sourcesPanel = createSourcesPanel(settingsPage.catalogs)
settingsPage.shortcuts.append($('shortcuts').querySelector('dl')!.cloneNode(true))
const settingsVersion = el('span')
settingsPage.about.append(
  el('p', {}, 'Book Reader ', settingsVersion),
  el('p', { className: 'muted', textContent: 'A compact reader for EPUB and FB2 books.' }),
)

/** Refreshes both: the settings page and Aa (with the open book's mode). */
function showSettings() {
  settingsPage.show(settings)
  appearancePanel.show(settings, book?.flow)
}
const catalogView = createCatalogView({ library: () => libraryBooks, openSettings: () => openSettings('catalogs') })

/** The library window's two tabs: the books on disk, and the catalogs to get more from. */
// ---- The library window's navigation (desktop) ----------------------------------

/** What the library window shows: the reading history, the books, the catalogs, or a settings section. */
type View = 'recent' | 'library' | 'catalogs' | `settings:${SettingsSection}`
/** Unset until the library first shows: then the history if there is one, else the books. */
let view: View | null = null
/** A library folder is chosen (else the first-run choice shows). */
let libraryReady = false

/** The library (the desktop's window, the phone's screen): the settings sit in its content, not over it. */
const settingsEmbedded = () => $('settings').classList.contains('embedded')

/** Puts the settings in the library's content (`true`) or back over the window. */
function embedSettings(embed: boolean) {
  if (embed === settingsEmbedded()) return
  const settingsEl = $('settings')
  settingsEl.classList.toggle('embedded', embed)
  settingsEl.hidden = true
  if (embed) $('start-main').append(settingsEl)
  else document.body.append(settingsEl)
}

/** Shows `to` in the library window's content and marks it in the pane. */
function showView(to: View) {
  view = to
  const settingsView = to.startsWith('settings:')
  $('setup').hidden = libraryReady || settingsView
  $('library').hidden = !libraryReady || settingsView
  if (settingsEmbedded()) $('settings').hidden = !settingsView
  if (settingsView) {
    settingsPage.showSection(to.slice('settings:'.length) as SettingsSection)
    void refreshFileTypesStatus()
    foldSettingsNav(false)
  } else if (libraryReady) {
    showTab(to === 'catalogs' ? 'catalogs' : 'books')
    // The history and the books are separate pages here (the phone shows both on one).
    $('books-view').dataset.show = to === 'recent' ? 'recent' : 'all'
  }
  for (const item of document.querySelectorAll<HTMLElement>('[data-nav]')) {
    // The phone's Settings stands for all its sections.
    const group = item.dataset.navGroup
    item.setAttribute('aria-current', String(item.dataset.nav === to || (!!group && to.startsWith(`${group}:`))))
  }
}

/** Folds the Settings group in the pane away (`true`) or open. */
function foldSettingsNav(fold: boolean) {
  $('settings-nav').hidden = fold
  document.querySelector('#sidebar .section-head')?.setAttribute('aria-expanded', String(!fold))
}

function showTab(tab: 'books' | 'catalogs') {
  $('books-view').hidden = tab !== 'books'
  $('catalog-view').hidden = tab !== 'catalogs'
  $('tab-books').setAttribute('aria-selected', String(tab === 'books'))
  $('tab-catalogs').setAttribute('aria-selected', String(tab === 'catalogs'))
  if (tab === 'catalogs') void catalogView.show()
}

const resolvedTheme = (): ResolvedTheme =>
  settings.theme === 'system' ? (systemDark.matches ? 'dark' : 'light') : settings.theme

/** Sets the app theme (style.css defines the colours per data-theme). */
function applyTheme() {
  document.documentElement.dataset.theme = resolvedTheme()
  // The interface font: the bundled pair by default (style.css), or one font everywhere.
  const root = document.documentElement.style
  if (settings.uiFont === 'classic') {
    root.removeProperty('--font-body')
    root.removeProperty('--font-heading')
  } else {
    const stack = `'${settings.uiFont}', system-ui, sans-serif`
    root.setProperty('--font-body', stack)
    root.setProperty('--font-heading', stack)
  }
}

/** The book is drawn with the app theme's colours, so the two always match. */
function currentBookStyle(): BookStyle {
  const root = getComputedStyle(document.documentElement)
  const colors = {
    theme: resolvedTheme(),
    text: root.getPropertyValue('--fg').trim(),
    link: root.getPropertyValue('--link').trim(),
  }
  return { css: bookCss(settings, colors), lineWidth: lineWidthPx(settings) }
}

function restyle() {
  applyTheme()
  book?.setStyle(currentBookStyle())
}

let pendingSettings: Partial<Settings> = {}
let settingsTimer: number | undefined

/** Applies settings in this window: the book's look and mode, and the panel. */
function applySettings(changes: Partial<Settings>) {
  settings = { ...settings, ...changes }
  if (Object.keys(changes).some(isAppearanceKey)) restyle()
  showSettings()
  if (book && book.flow !== settings.flow) {
    book.flow = settings.flow
    updateFlowButton()
  }
  if ('volumeKeys' in changes) setReading(!!book)
  if ('historyLength' in changes && !book) void refreshRecent()
  if ('uiFont' in changes) applyTheme()
}

/** Applies changes made in this window, and saves them after a pause. */
function changeSettings(changes: Partial<Settings>) {
  applySettings(changes)
  pendingSettings = { ...pendingSettings, ...changes }
  clearTimeout(settingsTimer)
  settingsTimer = window.setTimeout(flushSettings, SAVE_DELAY_MS)
}

async function flushSettings() {
  clearTimeout(settingsTimer)
  const changes = pendingSettings
  pendingSettings = {}
  if (Object.keys(changes).length) await invoke('update_settings', { changes }).catch(console.error)
}

/** Shows whether the app is registered for its book file types. */
async function refreshFileTypesStatus() {
  const registered = await invoke<boolean>('file_types_registered').catch(e => {
    console.error(e)
    return false
  })
  fileTypesRegistered = registered
  settingsPage.setFileTypesRegistered(registered)
}

// ---- Edge controls ----------------------------------------------------------

let hideChromeTimer: number | undefined
const bottombar = $('bottombar')

/** The reading chrome: the top bar and the bottom bar come and go together. */
function setChrome(shown: boolean) {
  clearTimeout(hideChromeTimer)
  topbar.classList.toggle('shown', shown)
  bottombar.classList.toggle('shown', shown)
  if (!shown) closeMoreMenu()
}

/** A tap in the middle of the page (touch screens) shows or hides the chrome. */
function toggleTopbar() {
  setChrome(!topbar.classList.contains('shown'))
}

/** The mouse near the top or bottom edge shows the chrome; elsewhere it fades after a moment. */
function onPointer(y: number) {
  if (y < TOP_EDGE_PX || (book && y > innerHeight - BOTTOM_EDGE_PX)) return setChrome(true)
  const inUse = topbar.matches(':hover') || bottombar.matches(':hover') || !$('more-menu').hidden || trackDrag
  if (!topbar.classList.contains('shown') || inUse) return
  clearTimeout(hideChromeTimer)
  hideChromeTimer = window.setTimeout(() => setChrome(false), 600)
}

// ---- The "more" menu ----------------------------------------------------------

function toggleMoreMenu() {
  const open = $('more-menu').hidden
  closePanels()
  $('more-menu').hidden = !open
  $('more-button').setAttribute('aria-expanded', String(open))
  if (open) $('more-menu').querySelector<HTMLElement>('button:not([hidden])')?.focus()
}

function closeMoreMenu() {
  $('more-menu').hidden = true
  $('more-button').setAttribute('aria-expanded', 'false')
}

// ---- The progress track ---------------------------------------------------------

/** Dragging the knob: the book goes there when the finger or mouse lets go. */
let trackDrag = false

function trackFraction(e: PointerEvent) {
  const rect = $('progress-track').getBoundingClientRect()
  return Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width))
}

function showTrackAt(fraction: number) {
  $('progress-done').style.width = `${fraction * 100}%`
  $('progress-knob').style.left = `${fraction * 100}%`
}

// ---- Wiring -----------------------------------------------------------------

document.addEventListener('keydown', onKey)
// Mouse only: on a touch screen the menu comes and goes with a tap in the page's middle.
document.addEventListener('pointermove', e => e.pointerType === 'mouse' && onPointer(e.clientY))
$('scrim').addEventListener('click', closePanels)

// The book shows through, so the panels open without the scrim;
// a press anywhere outside still closes them.
// Bubble phase, so the book sees the press first and can swallow its click.
document.addEventListener('pointerdown', e => {
  if (!scrimlessOpen()) return
  const target = e.target instanceof Element ? e.target : null
  if (target?.closest('#settings, #appearance, #settings-button, #about, #about-button, #shortcuts, #shortcuts-button, #more-menu, #phone-actions')) return
  closePanels()
})
// The "more" menu closes on any press outside it (and its buttons).
document.addEventListener('pointerdown', e => {
  const target = e.target instanceof Element ? e.target : null
  if ($('more-menu').hidden || target?.closest('#more-menu, #more-button, [data-command="more"]')) return
  closeMoreMenu()
})
suppressContextMenu()
$('toc-button').addEventListener('click', () => runCommand('toc'))
$('library-button').addEventListener('click', () => runCommand('library'))
$('setup-default').addEventListener('click', () => void chooseLibrary(false))
$('setup-pick').addEventListener('click', () => void chooseLibrary(true))
$('library-search').addEventListener('input', renderLibrary)
$('library-sort').append(...SORTS.map(s => el('option', { value: s, textContent: SORT_LABELS[s] })))
$<HTMLSelectElement>('library-sort').addEventListener('change', e => {
  changeSettings({ librarySort: (e.target as HTMLSelectElement).value as Sort })
  renderLibrary()
})
/** Alt-tabbing back rescans at most this often. */
const FOCUS_RESCAN_MS = 15_000
let lastFocusRescan = 0
// Books may have been added, or read in another window, meanwhile.
appWindow.onFocusChanged(({ payload: focused }) => {
  if (!focused || !libraryVisible() || Date.now() - lastFocusRescan < FOCUS_RESCAN_MS) return
  lastFocusRescan = Date.now()
  void refreshLibraryScreen()
})
// Books were added or renamed (by this or another window).
appWindow.listen('library-updated', () => {
  if (libraryVisible()) void refreshLibraryScreen()
})
$('offer-move').addEventListener('click', () => void addToLibrary('move'))
$('offer-copy').addEventListener('click', () => void addToLibrary('copy'))
$('offer-later').addEventListener('click', hideOffer)
$('offer-more').addEventListener('click', () => {
  $('offer-more').hidden = true
  $('offer-never').hidden = false
  $('offer-never').focus()
})
$('toc-tab-contents').addEventListener('click', () => showTocTab('contents'))
$('toc-tab-bookmarks').addEventListener('click', () => showTocTab('bookmarks'))
$('offer-setup').addEventListener('click', () => {
  hideOffer()
  void runCommand('library')
})
$('offer-never').addEventListener('click', () => {
  hideOffer()
  void invoke('decline_library', { fingerprint }).catch(console.error)
})
$('organize-apply').addEventListener('click', () => void applyOrganize())
$('sync-jump').addEventListener('click', () => void jumpToSynced())
$('sync-stay').addEventListener('click', hideSyncBar)
setInterval(() => void pushSync(), SYNC_EVERY_MS)
// Going to the background (minimising, switching apps on the phone) sends the position too.
document.addEventListener('visibilitychange', () => document.hidden && void pushSync())
$('organize-cancel').addEventListener('click', closePanels)
$('tab-books').addEventListener('click', () => showTab('books'))
$('tab-catalogs').addEventListener('click', () => showTab('catalogs'))
// The library window's pane (desktop): its items, Settings folding out, its width.
for (const item of document.querySelectorAll<HTMLElement>('[data-nav]')) {
  item.addEventListener('click', () => showView(item.dataset.nav as View))
}
document.querySelector('#sidebar .section-head')?.addEventListener('click', () => foldSettingsNav(!$('settings-nav').hidden))
sidebarResizer($('start'), $('sidebar-resizer'))
appWindow.listen('catalogs-changed', () => {
  void sourcesPanel.refresh()
  if (!$('catalog-view').hidden) void catalogView.show()
})
appWindow.listen('library-changed', () => {
  libraryBooks = []
  if (!book) void showLibraryScreen()
  else void loadLibraryInfo()
})
$('settings-button').addEventListener('click', () => runCommand('settings'))
$('shortcuts-button').addEventListener('click', () => (shortcutsOpen() ? closePanels() : openInfoPanel('shortcuts')))
$('about-button').addEventListener('click', () => (aboutOpen() ? closePanels() : openInfoPanel('about')))
$('open-button').addEventListener('click', () => runCommand('open'))
$('flow-button').addEventListener('click', () => runCommand('toggleFlow'))
$('bookmark-button').addEventListener('click', () => runCommand('bookmark'))
$('fullscreen-button').addEventListener('click', () => runCommand('fullscreen'))
$('more-button').addEventListener('click', toggleMoreMenu)
// A chosen item closes the menu (its own handler above does the work).
$('more-menu').addEventListener('click', e => {
  if ((e.target as Element).closest('button')) closeMoreMenu()
})
// The phone's bottom bar: the same commands as the top bar's buttons.
for (const button of document.querySelectorAll<HTMLButtonElement>('#phone-actions [data-command]')) {
  const command = button.dataset.command
  button.addEventListener('click', () => (command === 'more' ? toggleMoreMenu() : void runCommand(command as Command)))
}
$('progress-track').addEventListener('pointerdown', e => {
  if (!book || e.button !== 0) return
  trackDrag = true
  $('progress-track').setPointerCapture(e.pointerId)
  showTrackAt(trackFraction(e))
})
$('progress-track').addEventListener('pointermove', e => trackDrag && showTrackAt(trackFraction(e)))
$('progress-track').addEventListener('pointerup', e => {
  if (!trackDrag) return
  trackDrag = false
  book?.view.goToFraction(trackFraction(e)).catch(console.error)
})
$('progress-track').addEventListener('pointercancel', () => (trackDrag = false))

appWindow.listen('book-changed', () => openCurrentBook())
// Listening takes Back over from Android, which would otherwise leave the app on every press.
if (isAndroid) {
  void onBackButtonPress(handleBack).catch(console.error)
  // A book handed over by "Open with" or "Share", at startup or later.
  const openIncoming = () => void invoke('open_incoming').catch(e => showError(undefined, e))
  openIncoming()
  void addPluginListener('system', 'incoming', openIncoming).catch(console.error)
  void addPluginListener<{ direction: 'next' | 'prev' }>('system', 'volumeKey', ({ direction }) => {
    if (book) void runCommand(direction)
  }).catch(console.error)
}
// Another window changed a setting: apply it here too (it is already saved).
appWindow.listen<Record<string, unknown>>('settings-changed', ({ payload }) => {
  const valid = readSettings({ ...settings, ...payload })
  applySettings(Object.fromEntries(Object.keys(payload).map(k => [k, valid[k as keyof Settings]])))
})
// flushSave never throws, so a failed save cannot keep the window open.
appWindow.onCloseRequested(async () => {
  const lastPush = Promise.race([pushSync(), new Promise(done => setTimeout(done, SYNC_ON_CLOSE_MS))])
  await Promise.all([flushSave(), flushSettings(), lastPush])
})
systemDark.addEventListener('change', () => settings.theme === 'system' && restyle())

void refreshFileTypesStatus()
void invoke<string>('app_version')
  .then(version => {
    $('about-version').textContent = `v${version}`
    settingsVersion.textContent = `v${version}`
  })
  .catch(console.error)
openCurrentBook()
