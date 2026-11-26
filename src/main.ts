import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import {
  bookCss,
  el,
  isAppearanceKey,
  lineWidthPx,
  nextTheme,
  stepFontSize,
  suppressContextMenu,
  type BookStyle,
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
import { commandFor, type Command } from './keys'
import { SORTS, SORT_LABELS, type Sort } from './library'
import { createCatalogView } from './catalog-view'
import { setFormats } from './catalog'
import { createSeriesView } from './series-view'
import { closePopup } from './book-menu'
import { icon } from './icons'
import { sidebarResizer } from './sidebar-resize'
import { createSourcesPanel } from './sources-panel'
import { cloudModuleId, createAppearancePanel, createSettingsPage, type ModuleInfo, type SettingsSection } from './settings-panel'
import { initBackButton, initIncoming, initOauthAnswer, initVolumeKeys, isAndroid, pollOauthAnswer, setReading } from './platform'
import { createReadingView, type AddMode, type Offer } from './reading-view'
import { createLibraryView, type View } from './library-view'
import { SYNC_EVERY_MS, SYNC_ON_CLOSE_MS } from './sync-manager'
import { readSettings, SAVE_DELAY_MS, type Settings } from './settings'

/** The top bar shows when the mouse comes this close to the top edge. */
const TOP_EDGE_PX = 48
/** The bottom bar shows when the mouse comes this close to the bottom edge. */
const BOTTOM_EDGE_PX = 56

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T
const appWindow = getCurrentWindow()
const topbar = $('topbar')

let settings: Settings = readSettings({})
// Loads alongside the first book; the backend already set the saved theme
// before the first paint (see theme_script in windows.rs).
const settingsReady = invoke<Record<string, unknown>>('get_settings')
  .catch(e => {
    console.error(e)
    return {}
  })
  .then(stored => {
    settings = readSettings(stored)
    applyTheme()
    showSettings()
    applyCapabilities()
    library.showControls()
  })
const library = createLibraryView({
  getSettings: () => settings,
  changeSettings,
  showError,
  showView,
  closePanels,
  setTitle: title => void appWindow.setTitle(title).catch(console.error),
  setLibraryFolder: folder => settingsPage.setLibraryFolder(folder),
  hasBook: () => reading.book !== null,
  getView: () => view,
  backInCatalogs: () => catalogView.back(),
  backInSeries: () => seriesView.back(),
  seriesShown: () => seriesView.show(),
})

const reading = createReadingView({
  getSettings: () => settings,
  settingsReady,
  currentBookStyle,
  showLibraryScreen: () => library.showScreen(),
  showError,
  onKey,
  onPointer,
  toggleTopbar,
  scrimlessOpen: () => scrimlessOpen(),
  updateTopbarButtons,
  showOffer,
  hideOffer,
  closePanels,
  setTitle: title => void appWindow.setTitle(title).catch(console.error),
  embedSettings: embed => library.embedSettings(embed),
  setChrome: shown => setChrome(shown),
})

void library.loadLibraryInfo()

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

/** Contents and the flow need an open book; the rest of the topbar works without one. */
function updateTopbarButtons() {
  const withoutBook = !reading.book
  $('toc-button').toggleAttribute('disabled', withoutBook)
  $('flow-button').toggleAttribute('disabled', withoutBook)
  // --no-save keeps nothing, bookmarks included.
  $('bookmark-button').toggleAttribute('disabled', withoutBook || reading.noSave)
  updateFlowButton()
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

// ---- Commands ---------------------------------------------------------------

const tocOpen = () => !$('toc').hidden
/** The settings page over the window (a book window, or the phone); the library window on a desktop has them in its content. */
const settingsOpen = () => !$('settings').hidden && !library.isSettingsEmbedded()
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
  if (library.isOrganizeOpen() && command !== 'escape') return
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
      if (reading.book) return appearanceOpen() ? closePanels() : openAppearance()
      // In the library: to the settings, and back to the page that was showing.
      if (library.isSettingsEmbedded()) return showView(view?.startsWith('settings:') ? lastPage : 'settings:reading')
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
  const open = reading.book
  if (!open) return
  switch (command) {
    case 'left':
      return open.view.goLeft()
    case 'right':
      return open.view.goRight()
    case 'next':
      return open.view.next()
    case 'prev':
      return open.view.prev()
    case 'lineDown':
      return open.lineDown()
    case 'lineUp':
      return open.lineUp()
    case 'chapterStart':
      return open.goToChapterEdge('start')
    case 'chapterEnd':
      return open.goToChapterEdge('end')
    case 'toc':
      return tocOpen() ? closePanels() : openToc()
    case 'toggleFlow':
      return toggleFlow()
    case 'bookmark':
      return reading.toggleBookmark()
  }
}

/** Closes the footnote, or else any open panel; false when there was none. */
function closeTopLayer() {
  if (closePopup()) return true
  const open = reading.book
  if (open?.footnoteOpen) open.hideFootnote()
  else if (tocOpen() || library.isOrganizeOpen() || library.isDetailsOpen() || settingsOpen() || scrimlessOpen()) closePanels()
  else return false
  return true
}

async function handleEscape() {
  if (closeTopLayer() || library.stepBack()) return
  if (await appWindow.isFullscreen()) await appWindow.setFullscreen(false)
}
/**
 * Android's Back, one step at a time: a note or a panel closes first, then the
 * book returns to the library, and the library sends the app to the background.
 */
function handleBack() {
  if (closeTopLayer() || library.stepBack()) return
  void invoke(reading.book ? 'show_library' : 'leave_app').catch(console.error)
}

function toggleFlow() {
  const open = reading.book
  if (!open) return
  const flow = (open.flow = open.flow === 'paginated' ? 'scrolled' : 'paginated')
  updateFlowButton()
  changeSettings({ flow })
}

function updateFlowButton() {
  const scrolled = reading.book?.flow === 'scrolled'
  const modeIcon = () => icon(scrolled ? 'scroll' : 'book-open')
  const mode = scrolled ? 'Scrolled' : 'Paginated'
  $('flow-button').replaceChildren(modeIcon())
  $('flow-button').title = `${mode} — switch to ${scrolled ? 'paginated' : 'scrolled'} (M)`
  $('flow-button').setAttribute('aria-label', mode)
  appearancePanel.show(settings, reading.book?.flow)
  $('phone-flow').replaceChildren(modeIcon(), el('span', { textContent: mode }))
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
  if (library.isSettingsEmbedded()) return showView(`settings:${section ?? 'reading'}`)
  closePanels()
  $('settings').hidden = false
  void refreshFileTypesStatus()
  void refreshCloudStatus()
  refreshModules()
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

function closePanels() {
  closeMoreMenu()
  $('scrim').hidden = true
  $('organize').hidden = true
  $('toc').hidden = true
  $('book-details').hidden = true
  if (!library.isSettingsEmbedded()) $('settings').hidden = true
  $('appearance').hidden = true
  $('settings-button').setAttribute('aria-expanded', 'false')
  $('about').hidden = true
  $('shortcuts').hidden = true
}

// ---- Settings and appearance ------------------------------------------------

const systemDark = matchMedia('(prefers-color-scheme: dark)')
let fileTypesRegistered = false
/** Which cloud is connected, if any: one at a time, like the picker. */
let connectedCloud: 'dropbox' | 'google' | null = null
const settingsPage = createSettingsPage($('settings'), settings, changes => void changeSettings(changes), {
  clearHistory: () => {
    void invoke('clear_recent')
      .then(() => {
        if (!reading.book) void library.refreshRecent()
      })
      .catch(console.error)
  },
  toggleFileTypes: () => {
    const command = fileTypesRegistered ? 'unregister_file_types' : 'register_file_types'
    void invoke(command).then(refreshFileTypesStatus, console.error)
  },
  connectCloud: provider => {
    // The picker never sends an unavailable cloud; a disabled module refuses.
    // A phone's answer comes back on the app's scheme: the push event is
    // unreliable there, so a Google connect polls the stash instead; the
    // desktop waits on a loopback address instead.
    const command = provider === 'google' && isAndroid ? 'google_connect_android' : provider === 'google' ? 'google_connect' : 'dropbox_connect'
    if (provider === 'google' && isAndroid) {
      stopOauthPoll?.()
      stopOauthPoll = pollOauthAnswer(takeOauthAnswer, finishOauthAnswer)
    }
    void invoke(command).then(
      () => {
        void refreshCloudStatus()
        void sourcesPanel.refresh()
      },
      e => {
        stopOauthPoll?.()
        stopOauthPoll = null
        showError(undefined, e)
        void refreshCloudStatus()
      },
    )
  },
  disconnectCloud: () => {
    stopOauthPoll?.()
    stopOauthPoll = null
    const command = connectedCloud === 'google' ? 'google_disconnect' : 'dropbox_disconnect'
    void invoke(command).then(
      () => {
        void refreshCloudStatus()
        void sourcesPanel.refresh()
      },
      e => showError(undefined, e),
    )
  },
  changeLibrary: () => void library.chooseLibrary(true),
  toggleModule: (id, off) => void toggleModule(id, off),
  openRepository: () => void invoke('open_repository').catch(console.error),
  checkForUpdate: () => invoke('check_for_update'),
  openLatestRelease: () => void invoke('open_latest_release').catch(console.error),
  organizeLibrary: () => void library.organize(),
  close: closePanels,
})
const appearancePanel = createAppearancePanel($('appearance'), settings, changes => void changeSettings(changes), {
  flow: flow => changeSettings({ flow }),
  allSettings: () => openSettings('reading'),
})
// The catalogs (OPDS servers), the key list and the version have their own sections in the settings.
const sourcesPanel = createSourcesPanel(settingsPage.catalogs, {
  syncOff: () => settings.disabledModules.includes('capability:progress-sync'),
  cloudReady: () => connectedCloud !== null && !settings.disabledModules.includes(cloudModuleId(connectedCloud)),
})
settingsPage.shortcuts.append($('shortcuts').querySelector('dl')!.cloneNode(true))
const settingsVersion = el('span')
settingsPage.about.append(
  el('p', {}, 'Book Reader ', settingsVersion),
  el('p', { className: 'muted', textContent: 'A compact reader for EPUB and FB2 books.' }),
)

/** Refreshes both: the settings page and Aa (with the open book's mode). */
function showSettings() {
  settingsPage.show(settings)
  appearancePanel.show(settings, reading.book?.flow)
}
const seriesView = createSeriesView({
  library: () => library.getBooks(),
  bookCard: book => library.bookCard(book),
  cover: (title, key, fraction) => library.coverWithProgress(title, key, fraction),
  scrollTop: () => ($('start-main').scrollTop = 0),
})
const catalogView = createCatalogView({ library: () => library.getBooks(), openSettings: () => openSettings('catalogs') })

// ---- The library's navigation (the desktop's pane, the phone's bottom bar) ----------

/** Unset until the library first shows: then the history if there is one, else the books. */
let view: View | null = null
/** The pane's and the bottom bar's items; the pane's Settings heading. */
const navItems = [...document.querySelectorAll<HTMLElement>('[data-nav]')]
const settingsHead = document.querySelector<HTMLElement>('#sidebar .section-head')!
/** The last page that isn't a settings section, for S / Ctrl+, to go back to. */
let lastPage: View = 'library'
/** The disabled modules the capabilities UI was last drawn for. */
let lastCapabilities = ''

/** Shows `to` in the library window's content and marks it in the pane. */
function showView(to: View) {
  const wasSettings = !!view?.startsWith('settings:')
  view = to
  if (!to.startsWith('settings:')) lastPage = to
  const settingsView = to.startsWith('settings:')
  $('setup').hidden = library.isReady() || settingsView
  $('library').hidden = !library.isReady() || settingsView
  if (library.isSettingsEmbedded()) $('settings').hidden = !settingsView
  if (settingsView) {
    if (!wasSettings) {
      refreshModules()
      void refreshFileTypesStatus()
      void refreshCloudStatus()
    }
    settingsPage.showSection(to.slice('settings:'.length) as SettingsSection)
    foldSettingsNav(false)
  } else if (library.isReady()) {
    showTab(to === 'catalogs' || to === 'series' ? to : 'books')
    // The history and the books are separate pages here (the phone shows both on one).
    $('books-view').dataset.show = to === 'recent' ? 'recent' : 'all'
  }
  for (const item of navItems) {
    // The phone's Settings stands for all its sections.
    const group = item.dataset.navGroup
    item.setAttribute('aria-current', String(item.dataset.nav === to || (!!group && to.startsWith(`${group}:`))))
  }
}

/** Folds the Settings group in the pane away (`true`) or open. */
function foldSettingsNav(fold: boolean) {
  $('settings-nav').hidden = fold
  settingsHead.setAttribute('aria-expanded', String(!fold))
}

/** The library's parts: the books on disk, their series, and the catalogs to get more from. */
function showTab(tab: 'books' | 'series' | 'catalogs') {
  $('books-view').hidden = tab !== 'books'
  $('series-view').hidden = tab !== 'series'
  $('catalog-view').hidden = tab !== 'catalogs'
  if (tab === 'catalogs') void catalogView.show()
  if (tab === 'series') seriesView.show()
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
    const stack = settings.uiFont === 'system' ? 'system-ui, sans-serif' : `'${settings.uiFont}', system-ui, sans-serif`
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
  reading.book?.setStyle(currentBookStyle())
}

let pendingSettings: Partial<Settings> = {}
let settingsTimer: number | undefined

/** Applies settings in this window: the book's look and mode, and the panel. */
function applySettings(changes: Partial<Settings>) {
  settings = { ...settings, ...changes }
  if (Object.keys(changes).some(isAppearanceKey)) restyle()
  showSettings()
  const open = reading.book
  if (open && open.flow !== settings.flow) {
    open.flow = settings.flow
    updateFlowButton()
  }
  if ('volumeKeys' in changes) setReading(!!open, settings.volumeKeys)
  if ('historyLength' in changes && !open) void library.refreshRecent()
  if ('uiFont' in changes) applyTheme()
  applyCapabilities()
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

/** Reloads the Modules section from the backend (switches and their reasons). */
function refreshModules() {
  void invoke<ModuleInfo[]>('list_modules').then(
    modules => {
      settingsPage.setModules(modules)
      setFormats(modules)
      applyCapabilities()
    },
    console.error,
  )
}

/** Hides what disabled capability modules take away (catalogs UI, sync UI). */
function applyCapabilities() {
  const off = new Set(settings.disabledModules)
  const catalogs = !off.has('capability:catalogs')
  for (const item of navItems) {
    if (item.dataset.nav === 'catalogs' || item.dataset.nav === 'settings:catalogs') item.hidden = !catalogs
  }
  settingsPage.setSectionVisible('catalogs', catalogs)
  settingsPage.setOrganizeVisible(!off.has('capability:organize'))
  settingsPage.setFileTypesVisible(!isAndroid && !off.has('capability:file-types'))
  settingsPage.setCloudVisible(!off.has('capability:dropbox') || !off.has('capability:googledrive'))
  settingsPage.setUpdatesVisible(!off.has('capability:update-check'))
  if (!catalogs && (view === 'catalogs' || view === 'settings:catalogs')) {
    showView(view === 'catalogs' ? 'library' : 'settings:modules')
  }
  const key = [...off].sort().join(',')
  if (key !== lastCapabilities) {
    lastCapabilities = key
    void sourcesPanel.refresh()
  }
}

/** Turns a layer-1 module off or back on, then refreshes its row. */
async function toggleModule(id: string, off: boolean) {
  const disabledModules = off ? [...new Set([...settings.disabledModules, id])] : settings.disabledModules.filter(m => m !== id)
  try {
    await invoke('update_settings', { changes: { disabledModules } })
  } catch (e) {
    showError(undefined, e)
    refreshModules()
    return
  }
  changeSettings({ disabledModules })
  refreshModules()
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

/** Shows which cloud is connected, and its last sync failure, if any. */
async function refreshCloudStatus() {
  interface CloudStatus {
    connected: boolean
    lastError: string | null
  }
  const quiet = (e: unknown) => {
    console.error(e)
    return { connected: false, lastError: String(e) }
  }
  const [dropbox, google] = await Promise.all([
    invoke<CloudStatus>('dropbox_status').catch(quiet),
    invoke<CloudStatus>('google_status').catch(quiet),
  ])
  // One cloud at a time; the picker lists Dropbox first.
  const active = dropbox.connected
    ? { id: 'dropbox' as const, status: dropbox }
    : google.connected
      ? { id: 'google' as const, status: google }
      : null
  connectedCloud = active?.id ?? null
  settingsPage.setCloudConnected(connectedCloud, active !== null, active?.status.lastError ?? null)
}

// ---- Edge controls ----------------------------------------------------------

let hideChromeTimer: number | undefined
const bottombar = $('bottombar')

/** The reading chrome: the top bar and the bottom bar come and go together. */
function setChrome(shown: boolean) {
  clearTimeout(hideChromeTimer)
  hideChromeTimer = undefined
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
  // Near the top, or (reading) the bottom: show the bars, unless they already are.
  if (y < TOP_EDGE_PX || (reading.book && y > innerHeight - BOTTOM_EDGE_PX)) {
    if (!topbar.classList.contains('shown') || hideChromeTimer) setChrome(true)
    return
  }
  // Kept while the mouse is on them, a menu or a panel opened from them is open, or the knob is held.
  const inUse =
    topbar.matches(':hover') || bottombar.matches(':hover') || !$('more-menu').hidden || appearanceOpen() || tocOpen() || trackDrag
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
  if (!rect.width) return 0
  return Math.min(1, Math.max(0, (e.clientX - rect.left) / rect.width))
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
$('setup-default').addEventListener('click', () => void library.chooseLibrary(false))
$('setup-pick').addEventListener('click', () => void library.chooseLibrary(true))
$('library-search').addEventListener('input', () => library.render())
$('library-sort').append(...SORTS.map(s => el('option', { value: s, textContent: SORT_LABELS[s] })))
$<HTMLSelectElement>('library-sort').addEventListener('change', e => {
  changeSettings({ librarySort: (e.target as HTMLSelectElement).value as Sort })
  library.render()
})
const libraryVisible = () => !reading.book && !$('library').hidden
/** Alt-tabbing back rescans at most this often. */
const FOCUS_RESCAN_MS = 15_000
let lastFocusRescan = 0
// Books may have been added, or read in another window, meanwhile.
appWindow.onFocusChanged(({ payload: focused }) => {
  if (!focused || !libraryVisible() || Date.now() - lastFocusRescan < FOCUS_RESCAN_MS) return
  lastFocusRescan = Date.now()
  void library.refreshScreen()
})
// Books were added or renamed (by this or another window).
appWindow.listen('library-updated', () => {
  if (libraryVisible()) void library.refreshScreen()
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
  void invoke('decline_library', { fingerprint: reading.fingerprint }).catch(console.error)
})
$('organize-apply').addEventListener('click', () => void library.applyOrganize())
$('sync-jump').addEventListener('click', () => void reading.sync.jump())
$('sync-stay').addEventListener('click', () => reading.sync.hide())
setInterval(() => void reading.sync.push(), SYNC_EVERY_MS)
// Going to the background (minimising, switching apps on the phone) sends the position too.
document.addEventListener('visibilitychange', () => document.hidden && void reading.sync.push())
$('organize-cancel').addEventListener('click', closePanels)
// The library window's pane (desktop): its items, Settings folding out, its width.
for (const item of navItems) item.addEventListener('click', () => showView(item.dataset.nav as View))
settingsHead.addEventListener('click', () => foldSettingsNav(!$('settings-nav').hidden))
sidebarResizer($('start'), $('sidebar-resizer'))
appWindow.listen('catalogs-changed', () => {
  void sourcesPanel.refresh()
  if (!$('catalog-view').hidden) void catalogView.show()
})
appWindow.listen('library-changed', () => {
  library.clearBooks()
  if (!reading.book) void library.showScreen()
  else void library.loadLibraryInfo()
})
$('settings-button').addEventListener('click', () => runCommand('settings'))
$('shortcuts-button').addEventListener('click', () => (shortcutsOpen() ? closePanels() : openInfoPanel('shortcuts')))
$('about-button').addEventListener('click', () => (aboutOpen() ? closePanels() : openInfoPanel('about')))
$('open-button').addEventListener('click', () => runCommand('open'))
$('flow-button').addEventListener('click', () => runCommand('toggleFlow'))
$('bookmark-button').addEventListener('click', () => runCommand('bookmark'))
$('fullscreen-button').addEventListener('click', () => runCommand('fullscreen'))
$('more-button').addEventListener('click', toggleMoreMenu)
$('book-details-button').addEventListener('click', () => {
  if (reading.book) library.showBookDetails(reading.fingerprint, $('topbar-book').textContent || 'Book')
})
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
  if (!reading.book || e.button !== 0) return
  trackDrag = true
  $('progress-track').setPointerCapture(e.pointerId)
  reading.showTrackAt(trackFraction(e))
})
$('progress-track').addEventListener('pointermove', e => trackDrag && reading.showTrackAt(trackFraction(e)))
$('progress-track').addEventListener('pointerup', e => {
  if (!trackDrag) return
  trackDrag = false
  reading.book?.view.goToFraction(trackFraction(e)).catch(console.error)
})
// Cancelled (the system took the touch): the knob goes back to where the book is.
$('progress-track').addEventListener('pointercancel', () => {
  trackDrag = false
  reading.showTrackAt(reading.book?.view.lastLocation?.fraction ?? 0)
})

appWindow.listen('book-changed', () => reading.open())
initBackButton(handleBack)
// A book handed over by "Open with" or "Share", at startup or later.
initIncoming(() => void invoke('open_incoming').catch(e => showError(undefined, e)))
initVolumeKeys(direction => {
  if (reading.book) void runCommand(direction)
})
// A Google sign-in answer back from the phone's browser, or one stashed
// while the app started: finish the login (draining takes it exactly once).
async function takeOauthAnswer(): Promise<string | null> {
  const answer = await invoke<string | null>('google_take_answer').catch(e => {
    console.error(e)
    return null
  })
  return answer
}

async function finishOauthAnswer(answer: string): Promise<void> {
  try {
    await invoke('google_finish', { answer })
    void refreshCloudStatus()
    void sourcesPanel.refresh()
  } catch (e) {
    showError(undefined, e)
    void refreshCloudStatus()
  }
}

async function drainOauthAnswer(): Promise<void> {
  const answer = await takeOauthAnswer()
  if (!answer) return
  await finishOauthAnswer(answer)
}

/** Stops the poll started by the last phone Google connect, if any. */
let stopOauthPoll: (() => void) | null = null
initOauthAnswer(() => void drainOauthAnswer())
void drainOauthAnswer()
// The clouds' newer positions, once per start: Continue reading shows the
// fresh percent, and opening such a book goes straight to it (no Jump offer).
// Silent when offline: a failed start is the normal no-network case.
void invoke<number>('sync_pull_all')
  .then(updated => {
    if (updated > 0) void library.refreshScreen()
  })
  .catch(console.error)
// Another window changed a setting: apply it here too (it is already saved).
appWindow.listen<Record<string, unknown>>('settings-changed', ({ payload }) => {
  const valid = readSettings({ ...settings, ...payload })
  applySettings(Object.fromEntries(Object.keys(payload).map(k => [k, valid[k as keyof Settings]])))
})
// reading.flush never throws, so a failed save cannot keep the window open.
appWindow.onCloseRequested(async () => {
  const lastPush = Promise.race([reading.sync.push(), new Promise(done => setTimeout(done, SYNC_ON_CLOSE_MS))])
  await Promise.all([reading.flush(), flushSettings(), lastPush])
})
systemDark.addEventListener('change', () => settings.theme === 'system' && restyle())

void refreshFileTypesStatus()
void refreshCloudStatus()
void invoke<string>('app_version')
  .then(version => {
    $('about-version').textContent = `v${version}`
    settingsVersion.textContent = `v${version}`
  })
  .catch(console.error)
void reading.open()
