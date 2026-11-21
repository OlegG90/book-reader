import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { openUrl } from '@tauri-apps/plugin-opener'
import {
  bookCss,
  lineWidthPx,
  isAppearanceKey,
  nextTheme,
  oneOf,
  readAppearance,
  stepFontSize,
  type ResolvedTheme,
} from './appearance'
import { el, suppressContextMenu } from './dom'
import { commandFor, type Command } from './keys'
import type { Flow } from './layout'
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
import { BookView, type BookStyle, type Relocation } from './reader'
import { createSettingsPanel, type PanelValues } from './settings-panel'
import { createTOCView } from './vendor/foliate-js/ui/tree.js'

interface Position {
  cfi: string
  fraction: number
}

interface BookInfo {
  fileName: string
  fingerprint: string
  position: Position | null
  noSave: boolean
}

interface PendingSave {
  fingerprint: string
  position: Position
}

interface Settings extends PanelValues {
  flow: Flow
  librarySort: Sort
  libraryStatus: Status
}

const FLOWS: readonly Flow[] = ['paginated', 'scrolled']

function readSettings(stored: Record<string, unknown>): Settings {
  return {
    ...readAppearance(stored),
    flow: oneOf(stored.flow, FLOWS, 'paginated'),
    librarySort: oneOf(stored.librarySort, SORTS, 'author'),
    libraryStatus: oneOf(stored.libraryStatus, STATUSES, 'all'),
    savePositions: stored.savePositions !== false,
  }
}

const SAVE_DELAY_MS = 500
const TOP_EDGE_PX = 48

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T
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
    settingsPanel.show(settings)
    showLibraryControls()
  })
let currentTocHref: string | undefined
let book: BookView | null = null
let fingerprint = ''
/** `--no-save`: this window keeps no positions (the backend skips recent books too). */
let noSave = false
let tocView: { element: HTMLElement; setCurrentHref(href: string): void } | null = null

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
    const source = await loadBook(info)
    await settingsReady
    book = await BookView.open($('stage'), $('footnote'), source, {
      flow: settings.flow,
      lastLocation: info.position?.cfi,
      style: currentBookStyle(),
    }, {
      relocate: onRelocate,
      key: onKey,
      pointer: onPointer,
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
  }
  const title = book.title || info.fileName
  void appWindow.setTitle(title).catch(console.error)
  void invoke('remember_book', { fingerprint, title, author: book.author }).catch(console.error)
  $('toc-title').textContent = title
  $('toc-author').textContent = book.author
  tocView = createTOCView(book.toc, (href: string) => {
    book?.view.goTo(href).catch(console.error)
    closePanels()
  })
  $('toc-tree').replaceChildren(tocView.element)
  currentTocHref = undefined
  highlightToc(book.view.lastLocation?.tocItem?.href)
  $('progress').hidden = false
  updateTopbarButtons()
}

function closeBook() {
  book?.destroy()
  book = null
  tocView = null
  closePanels()
  updateTopbarButtons()
  $('toc-tree').replaceChildren()
  $('start').hidden = true
  document.body.classList.remove('library-mode')
  $('error').hidden = true
  $('progress').hidden = true
  topbar.classList.remove('shown')
}

/** Contents and the flow need an open book; the rest of the topbar works without one. */
function updateTopbarButtons() {
  const withoutBook = !book
  $('toc-button').toggleAttribute('disabled', withoutBook)
  $('flow-button').toggleAttribute('disabled', withoutBook)
  updateFlowButton()
}

interface RecentBook {
  key: string
  title: string
  author: string
  fileName: string
  fraction: number | null
  exists: boolean
}

interface LibraryInfo {
  folder: string | null
  suggested: string
}

/** The window without a book: the library, or the first-run folder choice. */
async function showLibraryScreen() {
  void appWindow.setTitle('Book Reader').catch(console.error)
  document.body.classList.add('library-mode')
  $('start').hidden = false
  const info = await invoke<LibraryInfo>('library_info').catch((e): LibraryInfo => {
    console.error(e)
    return { folder: null, suggested: '' }
  })
  libraryFolder = info.folder
  settingsPanel.setLibraryFolder(info.folder)
  $('setup').hidden = !!info.folder
  $('library').hidden = !info.folder
  if (!info.folder) {
    $('setup-suggested').textContent = info.suggested
    $('setup-default').focus()
    return
  }
  await Promise.all([refreshRecent(), refreshLibrary()])
}

async function refreshRecent() {
  const books = await invoke<RecentBook[]>('recent_books').catch((e): RecentBook[] => {
    console.error(e)
    return []
  })
  $('recent').replaceChildren(...books.map(recentItem))
  $('recent-title').hidden = !books.length
}

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
  $<HTMLSelectElement>('library-status').value = settings.libraryStatus
  $<HTMLSelectElement>('library-sort').value = settings.librarySort
}

function renderLibrary() {
  const query = $<HTMLInputElement>('library-search').value
  const shown = sortBooks(filterBooks(libraryBooks, query, settings.libraryStatus), settings.librarySort)
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
    status === 'finished' ? 'Finished' : status === 'reading' ? `${Math.round((book.fraction ?? 0) * 100)}%` : ''
  const cover = el('div', { className: 'cover' }, el('span', { textContent: book.title }))
  if (book.hasCover) loadCover(cover, book.key)
  const button = el(
    'button',
    {
      title: [book.title, book.authors.join(', '), seriesLabel(book)].filter(Boolean).join('\n'),
      onclick: () => void invoke('open_library_book', { key: book.key }).catch(e => showError(book.title, e)),
    },
    cover,
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

function recentItem(book: RecentBook) {
  const span = (className: string, textContent: string) => el('span', { className, textContent })
  const button = el(
    'button',
    book.exists
      ? { onclick: () => void invoke('open_recent', { key: book.key }).catch(console.error) }
      : {
          className: 'missing',
          title: 'File not found. Click to remove it from the list.',
          onclick: () => void invoke('forget_recent', { key: book.key }).then(refreshRecent, console.error),
        },
    span('title', book.title || book.fileName),
    span('percent', book.fraction == null ? '' : `${Math.round(book.fraction * 100)}%`),
    span('author', book.author),
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
  $('progress-label').textContent = tocItem?.label ? `${percent}% · ${tocItem.label}` : `${percent}%`
  highlightToc(tocItem?.href)
  scheduleSave({ fingerprint, position: { cfi, fraction } })
}

function highlightToc(href: string | undefined) {
  if (!href || href === currentTocHref) return
  currentTocHref = href
  tocView?.setCurrentHref(href)
}

// ---- Commands ---------------------------------------------------------------

const tocOpen = () => !$('toc').hidden
const settingsOpen = () => !$('settings').hidden
const aboutOpen = () => !$('about').hidden
const shortcutsOpen = () => !$('shortcuts').hidden
/** Panels that open without the scrim (the contents have one, so need no outside check). */
const scrimlessOpen = () => settingsOpen() || aboutOpen() || shortcutsOpen()

/** Commands that still work while a side panel has the keyboard. */
const PANEL_COMMANDS: Command[] = ['toc', 'settings', 'escape', 'fullscreen', 'cycleTheme', 'fontBigger', 'fontSmaller']

/** Whether a panel should get the key instead of the reader. */
function panelHasKeyboard(e: KeyboardEvent, command: Command) {
  const target = e.target instanceof Element ? e.target : null
  // Letters pick options in a focused list; only Escape leaves it.
  if (target?.closest('select, input')) return command !== 'escape'
  // Keys pressed in the book (its iframes) still turn pages beside the settings.
  return !!target?.closest('#toc, #settings, #about, #shortcuts') && !PANEL_COMMANDS.includes(command)
}

function onKey(e: KeyboardEvent) {
  const command = commandFor(e)
  if (!command || panelHasKeyboard(e, command)) return
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
  }
}

async function handleEscape() {
  if (!$('footnote').hidden) book?.hideFootnote()
  else if (tocOpen() || scrimlessOpen()) closePanels()
  else if (await appWindow.isFullscreen()) await appWindow.setFullscreen(false)
}

function toggleFlow() {
  if (!book) return
  const flow = (book.flow = book.flow === 'paginated' ? 'scrolled' : 'paginated')
  updateFlowButton()
  changeSettings({ flow })
}

function updateFlowButton() {
  $('flow-button').textContent = book?.flow === 'scrolled' ? 'Paginated' : 'Scrolled'
}

function openToc() {
  closePanels()
  $('scrim').hidden = false
  $('toc').hidden = false
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
function openSettings() {
  closePanels()
  $('settings').hidden = false
  void refreshFileTypesStatus()
  $('settings').querySelector<HTMLElement>('button, select, input')?.focus()
}

function closePanels() {
  $('scrim').hidden = true
  $('toc').hidden = true
  $('settings').hidden = true
  $('about').hidden = true
  $('shortcuts').hidden = true
}

// ---- Settings and appearance ------------------------------------------------

const systemDark = matchMedia('(prefers-color-scheme: dark)')
let fileTypesRegistered = false
const settingsPanel = createSettingsPanel(
  $('settings'),
  settings,
  changes => void changeSettings(changes),
  {
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
  },
)

const resolvedTheme = (): ResolvedTheme =>
  settings.theme === 'system' ? (systemDark.matches ? 'dark' : 'light') : settings.theme

/** Sets the app theme (style.css defines the colours per data-theme). */
function applyTheme() {
  document.documentElement.dataset.theme = resolvedTheme()
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
  settingsPanel.show(settings)
  if (book && book.flow !== settings.flow) {
    book.flow = settings.flow
    updateFlowButton()
  }
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
  settingsPanel.setFileTypesRegistered(registered)
}

// ---- Edge controls ----------------------------------------------------------

let hideTopbarTimer: number | undefined

function onPointer(y: number) {
  if (y < TOP_EDGE_PX) {
    clearTimeout(hideTopbarTimer)
    topbar.classList.add('shown')
  } else if (topbar.classList.contains('shown') && !topbar.matches(':hover')) {
    clearTimeout(hideTopbarTimer)
    hideTopbarTimer = window.setTimeout(() => topbar.classList.remove('shown'), 600)
  }
}

// ---- Wiring -----------------------------------------------------------------

document.addEventListener('keydown', onKey)
document.addEventListener('mousemove', e => onPointer(e.clientY))
$('scrim').addEventListener('click', closePanels)

// The book shows through, so the panels open without the scrim;
// a press anywhere outside still closes them.
// Bubble phase, so the book sees the press first and can swallow its click.
document.addEventListener('pointerdown', e => {
  if (!scrimlessOpen()) return
  const target = e.target instanceof Element ? e.target : null
  if (target?.closest('#settings, #settings-button, #about, #about-button, #shortcuts, #shortcuts-button')) return
  closePanels()
})
suppressContextMenu()
$('toc-button').addEventListener('click', () => runCommand('toc'))
$('library-button').addEventListener('click', () => runCommand('library'))
$('setup-default').addEventListener('click', () => void chooseLibrary(false))
$('setup-pick').addEventListener('click', () => void chooseLibrary(true))
$('library-search').addEventListener('input', renderLibrary)
$('library-status').append(...STATUSES.map(s => el('option', { value: s, textContent: STATUS_LABELS[s] })))
$('library-sort').append(...SORTS.map(s => el('option', { value: s, textContent: SORT_LABELS[s] })))
$<HTMLSelectElement>('library-status').addEventListener('change', e => {
  changeSettings({ libraryStatus: (e.target as HTMLSelectElement).value as Status })
  renderLibrary()
})
$<HTMLSelectElement>('library-sort').addEventListener('change', e => {
  changeSettings({ librarySort: (e.target as HTMLSelectElement).value as Sort })
  renderLibrary()
})
// Books may have been added, or read in another window, meanwhile.
appWindow.onFocusChanged(({ payload: focused }) => {
  if (!focused || !libraryVisible()) return
  void refreshRecent()
  void refreshLibrary()
})
appWindow.listen('library-changed', () => {
  libraryBooks = []
  if (!book) void showLibraryScreen()
  else void invoke<LibraryInfo>('library_info').then(info => settingsPanel.setLibraryFolder(info.folder), console.error)
})
$('settings-button').addEventListener('click', () => runCommand('settings'))
$('shortcuts-button').addEventListener('click', () => (shortcutsOpen() ? closePanels() : openInfoPanel('shortcuts')))
$('about-button').addEventListener('click', () => (aboutOpen() ? closePanels() : openInfoPanel('about')))
$('open-button').addEventListener('click', () => runCommand('open'))
$('flow-button').addEventListener('click', () => runCommand('toggleFlow'))
$('fullscreen-button').addEventListener('click', () => runCommand('fullscreen'))
$('progress').addEventListener('click', e => {
  book?.view.goToFraction(e.clientX / innerWidth).catch(console.error)
})

appWindow.listen('book-changed', () => openCurrentBook())
// Another window changed a setting: apply it here too (it is already saved).
appWindow.listen<Record<string, unknown>>('settings-changed', ({ payload }) => {
  const valid = readSettings({ ...settings, ...payload })
  applySettings(Object.fromEntries(Object.keys(payload).map(k => [k, valid[k as keyof Settings]])))
})
// flushSave never throws, so a failed save cannot keep the window open.
appWindow.onCloseRequested(async () => {
  await Promise.all([flushSave(), flushSettings()])
})
systemDark.addEventListener('change', () => settings.theme === 'system' && restyle())

void refreshFileTypesStatus()
void invoke<string>('app_version')
  .then(version => {
    $('about-version').textContent = `v${version}`
  })
  .catch(console.error)
openCurrentBook()
