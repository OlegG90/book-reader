import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { el } from 'reader-core'
import { confirmDialog, onLongPress, showMenu, type MenuItem } from './book-menu'
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
  type Status,
} from './library'
import { deleteFileNote, isAndroid } from './platform'
import type { Settings } from './settings'
import type { SettingsSection } from './settings-panel'
import { $ } from './dom'
import { icon } from './icons'

/** How far a book is read, as the library shows it: at least 1% once started. */
const percent = (fraction: number) => `${Math.max(1, Math.round(fraction * 100))}%`

/** What the library window shows: the reading history, the books, the series, the books removed here, the catalogs, or a settings section. */
export type View = 'recent' | 'books' | 'series' | 'removed' | 'catalogs' | `settings:${SettingsSection}`

interface RecentBook {
  key: string
  title: string
  author: string
  fileName: string
  /** Share read, if a position is saved. */
  fraction: number | null
  /** False when the file has been moved or deleted. */
  exists: boolean
  /** The library's key for the same book (by content), when it has a cover there. */
  coverKey: string | null
}

interface LibraryInfo {
  folder: string | null
  suggested: string
}

interface Rename {
  from: string
  to: string
}

/** A cloud book the library lacks, for the download offer. */
interface MissingBook {
  fingerprint: string
  name: string
  size: number
}

interface BookDetails {
  title: string
  authors: string[]
  series: string | null
  seriesIndex: string | null
  language: string | null
  annotation: string | null
  path: string
  size: number
  format: string
  fingerprint: string
  catalogSource: string | null
}

export interface LibraryDeps {
  getSettings(): Settings
  changeSettings(changes: Partial<Settings>): void
  showError(fileName: string | undefined, error: unknown): void
  showView(to: View): void
  closePanels(): void
  setTitle(title: string): void
  setLibraryFolder(folder: string | null): void
  hasBook(): boolean
  getView(): View | null
  backInCatalogs(): boolean
  backInSeries(): boolean
  seriesShown(): void
}

export interface LibraryView {
  /** The books of the last scan (for the series and catalog views). */
  getBooks(): LibraryBook[]
  /** Forgets the books: the next scan reads the folder anew. */
  clearBooks(): void
  /** Shows what "Organize library" would rename, for confirmation. */
  organize(): Promise<void>
  /** Renames the books per the shown plan. */
  applyOrganize(): Promise<void>
  /** The book details dialog is open. */
  isDetailsOpen(): boolean
  /** Shows the details dialog for a book, by fingerprint. */
  showBookDetails(fingerprint: string, title: string): void
  /** The organize preview is open. */
  isOrganizeOpen(): boolean
  showScreen(): Promise<void>
  /** Rescans the library and the history; a `quiet` look (the phone's every 30 s) redraws only what changed. */
  refreshScreen(quiet?: boolean): Promise<void>
  /** Redraws the cards' cloud marks from what the sync engine last learned. */
  refreshCloudMarks(): void
  refreshRecent(quiet?: boolean): Promise<void>
  render(): void
  chooseLibrary(pick: boolean): Promise<void>
  bookCard(book: LibraryBook): HTMLElement
  coverWithProgress(title: string, coverKey: string | null, fraction: number | null): HTMLElement
  loadLibraryInfo(): Promise<LibraryInfo>
  homeView(): View
  /**
   * Esc and Back in the library: up a catalog folder; from the catalogs' start
   * or the settings, home (the history, else the books). False at home.
   */
  stepBack(): boolean
  isReady(): boolean
  isSettingsEmbedded(): boolean
  /** Puts the settings in the library's content (`true`) or back over the window. */
  embedSettings(embed: boolean): void
}

/** Where a library book is in the clouds: every cloud holds it, its upload
waits, or a cloud is full. */
export type CloudPlace = 'cloud' | 'waiting' | 'nofit'

const CLOUD_MARKS: Record<CloudPlace, { icon: string; label: string }> = {
  cloud: { icon: 'cloud-check', label: 'In the cloud' },
  waiting: { icon: 'cloud-upload', label: 'Waiting to upload' },
  nofit: { icon: 'cloud-off', label: "Doesn't fit in the cloud" },
}

/** A small cloud mark for a book's card; nothing without a state. */
export function cloudMark(place: CloudPlace | undefined): HTMLElement | string {
  if (!place) return ''
  const mark = icon(CLOUD_MARKS[place].icon)
  mark.classList.add('cloud-mark', place)
  mark.title = CLOUD_MARKS[place].label
  mark.setAttribute('role', 'img')
  mark.setAttribute('aria-label', CLOUD_MARKS[place].label)
  return mark
}

/** The library window: Continue reading, the book grid with its search and filters, Organize, book details and the cloud download offer. */
export function createLibraryView(deps: LibraryDeps): LibraryView {
  /** A library folder is chosen (else the first-run choice shows). */
  let libraryReady = false
  let libraryFolder: string | null = null
  let libraryBooks: LibraryBook[] = []
  /** Each book's place in the clouds, by fingerprint (see `sync_book_states`). */
  let cloudStates: Record<string, CloudPlace> = {}
  let scanning: Promise<void> | null = null
  /** What the library and Continue reading show, so a quiet rescan that finds the same redraws nothing. */
  let shownScan = ''
  let shownRecent = ''
  /** The reading history isn't empty (the desktop then opens on it). */
  let hasRecent = false

  /** The library window without a book: the settings sit in its content, not over it. */
  const isSettingsEmbedded = () => $('settings').classList.contains('embedded')

  /** Puts the settings in the library's content (`true`) or back over the window. */
  function embedSettings(embed: boolean) {
    if (embed === isSettingsEmbedded()) return
    const settingsEl = $('settings')
    settingsEl.classList.toggle('embedded', embed)
    settingsEl.hidden = true
    if (embed) $('start-main').append(settingsEl)
    else document.body.append(settingsEl)
  }

  /** Where the library starts, and Esc / Back lead: the history if there is one, else the books. */
  const homeView = (): View => (hasRecent ? 'recent' : 'books')

  /**
   * Esc and Back in the library: up a catalog folder; from the catalogs' start
   * or the settings, home (the history, else the books). False at home.
   */
  function stepBack(): boolean {
    if (deps.hasBook() || !isSettingsEmbedded()) return false
    const view = deps.getView()
    if (view === 'catalogs' && deps.backInCatalogs()) return true
    if (view === 'series' && deps.backInSeries()) return true
    if (view === 'recent' || view === 'books') return false
    deps.showView(homeView())
    return true
  }

  /** The window without a book: the library, or the first-run folder choice. */
  async function showScreen() {
    deps.setTitle('Book Reader')
    document.body.classList.add('library-mode')
    embedSettings(true)
    $('start').hidden = false
    const info = await loadLibraryInfo()
    libraryReady = !!info.folder
    if (!info.folder) {
      deps.showView(deps.getView() ?? 'books')
      $('setup-suggested').textContent = info.suggested
      $('setup-default').focus()
      return
    }
    await refreshScreen()
    // The library keeps the page it showed; at first, the history if there is one.
    deps.showView(deps.getView() ?? homeView())
  }

  /** Reads the library folder (and the first-run suggestion), and shows it in the settings. */
  async function loadLibraryInfo(): Promise<LibraryInfo> {
    const info = await invoke<LibraryInfo>('library_info').catch((e): LibraryInfo => {
      console.error(e)
      return { folder: null, suggested: '' }
    })
    libraryFolder = info.folder
    deps.setLibraryFolder(info.folder)
    return info
  }

  const refreshScreen = async (quiet = false): Promise<void> => {
    await Promise.all([refreshRecent(quiet), refreshLibrary(quiet)])
  }

  /** Fills Continue reading with the newest books from the history, as many as the setting asks. */
  async function refreshRecent(quiet = false) {
    const books = await invoke<RecentBook[]>('recent_books').catch((e): RecentBook[] => {
      console.error(e)
      return []
    })
    // A quiet look that finds the list as shown redraws nothing (no flicker of covers).
    const found = JSON.stringify(books)
    if (quiet && found === shownRecent) return
    shownRecent = found
    $('recent').replaceChildren(...books.slice(0, deps.getSettings().historyLength).map(recentItem))
    $('recent-title').hidden = !books.length
    $('recent-empty').hidden = books.length > 0
    hasRecent = books.length > 0
  }

  /** Forgets the books: the next scan reads the folder anew. */
  function clearBooks() {
    libraryBooks = []
    shownScan = ''
  }

  /** Shows what "Organize library" would rename, for confirmation. */
  async function organize() {
    deps.closePanels()
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

  /** Shows what Organize would rename, before anything moves. */
  function showOrganizePlan(renames: Rename[]) {
    $('organize-summary').textContent = renames.length
      ? `${renames.length === 1 ? 'This book' : `These ${renames.length} books`} will be renamed to “Author/Series NN - Title”. Nothing is deleted or overwritten.`
      : 'Every book already follows the naming pattern.'
    $('organize-list').replaceChildren(
      ...renames.map(r => el('li', {}, el('span', { className: 'from muted', textContent: r.from }), el('span', { className: 'to', textContent: `→ ${r.to}` }))),
    )
    $('organize-apply').textContent = `Rename ${renames.length === 1 ? 'the book' : `${renames.length} books`}.`
    $('organize-apply').hidden = !renames.length
    $('organize-cancel').textContent = renames.length ? 'Cancel' : 'Close'
    ;(renames.length ? $('organize-apply') : $('organize-cancel')).focus()
  }

  /** Renames the books in the shown plan and says how many moved. */
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

  const organizeOpen = () => !$('organize').hidden

  /** Uses the suggested library folder, or asks for another one. */
  async function chooseLibrary(pick: boolean) {
    const folder = await invoke<string | null>('set_library', { pick }).catch(e => {
      deps.showError(undefined, e)
      return null
    })
    if (folder) void showScreen()
  }

  /** Rescans the folder (cheap: unchanged files come from the cache) and redraws. */
  function refreshLibrary(quiet = false): Promise<void> {
    scanning ??= (async () => {
      if (!libraryBooks.length) showLibraryMessage('Looking for books…')
      // Late: importing this module (as the tests do) must not need the Tauri runtime.
      const stopProgress = await getCurrentWindow().listen<number>('scan-progress', ({ payload }) => {
        if (!libraryBooks.length) showLibraryMessage(`Looking for books… (${payload})`)
      }).catch((): undefined => undefined)
      try {
        const books = await invoke<LibraryBook[]>('scan_library', { quiet })
        const states = await bookCloudStates()
        // A quiet look that finds the books and marks as shown leaves the grid as it is.
        const found = JSON.stringify([books, states])
        if (quiet && found === shownScan) return
        shownScan = found
        libraryBooks = books
        cloudStates = states
        renderLibrary()
        void offerCloudBooks()
      } catch (e) {
        showLibraryMessage(`Can't read the library folder: ${e}`)
      } finally {
        stopProgress?.()
        scanning = null
      }
    })()
    return scanning
  }

  /** The engine's last word on where each book is; nothing when it can't say. */
  function bookCloudStates(): Promise<Record<string, CloudPlace>> {
    return invoke<Record<string, CloudPlace>>('sync_book_states').catch((): Record<string, CloudPlace> => ({}))
  }

  /** Redraws the cloud marks after the engine learned more (an upload, a reconcile). */
  async function refreshCloudMarks() {
    cloudStates = await bookCloudStates()
    // The marks shown are no longer the last scan's.
    shownScan = ''
    if (libraryBooks.length) renderLibrary()
  }

  /** Shows a line above the grid (empty hides it). */
  function showLibraryMessage(text: string) {
    $('library-message').textContent = text
    $('library-message').hidden = !text
  }

  /** Where a menu opened from a button in the bar shows: just under it. */
  function under(button: HTMLElement) {
    const rect = button.getBoundingClientRect()
    return { x: rect.left, y: rect.bottom + 4 }
  }

  /**
   * The bar's buttons as the settings stand: the status with its count (its
   * menu counts every status among the books matching the search), the sort,
   * and the switch between covers and a list.
   */
  function showBar(counts: Record<Status, number>) {
    const settings = deps.getSettings()
    const choose = (changes: Partial<Settings>) => {
      deps.changeSettings(changes)
      renderLibrary()
    }
    /** Has `button` open a menu of one setting's choices under it, the current one checked. */
    const menuOf = <K extends 'libraryStatus' | 'librarySort'>(
      button: HTMLElement,
      key: K,
      values: readonly Settings[K][],
      labels: Record<Settings[K], string>,
      detail?: (value: Settings[K]) => string,
    ) => {
      button.onclick = () =>
        showMenu(
          under(button),
          values.map(v => ({ label: labels[v], detail: detail?.(v), checked: v === settings[key], action: () => choose({ [key]: v }) })),
        )
    }
    const status = $('library-status')
    status.replaceChildren(
      el('span', { textContent: STATUS_LABELS[settings.libraryStatus] }),
      el('span', { className: 'count', textContent: String(counts[settings.libraryStatus]) }),
      icon('chevron'),
    )
    menuOf(status, 'libraryStatus', STATUSES, STATUS_LABELS, s => String(counts[s]))
    const sort = $('library-sort')
    sort.replaceChildren(icon('sort'), el('span', { className: 'label', textContent: SORT_LABELS[settings.librarySort] }))
    menuOf(sort, 'librarySort', SORTS, SORT_LABELS)
    showLayouts()
  }

  /** Each page's covers / list switch, and the lists it shapes. */
  const LAYOUT_SWITCHES = [
    { key: 'libraryLayout', button: 'library-layout', lists: ['library-books'] },
    { key: 'recentLayout', button: 'recent-layout', lists: ['recent'] },
    { key: 'seriesLayout', button: 'series-layout', lists: ['series-list', 'series-books'] },
  ] as const

  /**
   * Shows each page as its own setting says, as cover cards or as a list, and
   * its switch offering the other. Switching only reshapes the page's lists.
   */
  function showLayouts() {
    const settings = deps.getSettings()
    for (const { key, button, lists } of LAYOUT_SWITCHES) {
      const list = settings[key] === 'list'
      const toggle = $(button)
      toggle.replaceChildren(icon(list ? 'grid' : 'list'))
      toggle.title = list ? 'Show covers' : 'Show as a list'
      toggle.setAttribute('aria-label', toggle.title)
      toggle.onclick = () => {
        deps.changeSettings({ [key]: list ? 'covers' : 'list' })
        showLayouts()
      }
      for (const id of lists) $(id).classList.toggle('list', list)
    }
  }

  /** Draws the grid for the current search, status and sort, with the bar's counts. */
  function renderLibrary() {
    const settings = deps.getSettings()
    const query = $<HTMLInputElement>('library-search').value
    // One pass over the library: the books matching the search, counted by status.
    const matching = filterBooks(libraryBooks, query, 'all')
    const counts: Record<Status, number> = { all: matching.length, new: 0, reading: 0, finished: 0 }
    for (const b of matching) counts[statusOf(b)]++
    const chosen = settings.libraryStatus
    const shown = sortBooks(chosen === 'all' ? matching : matching.filter(b => statusOf(b) === chosen), settings.librarySort)
    showBar(counts)
    if (!libraryBooks.length) {
      showLibraryMessage(
        isAndroid
          ? `The library is empty. Put EPUB and FB2 files into ${libraryFolder}, or open one from another app (Open with, Share).`
          : `The library is empty. Open a book (Ctrl+O) or put EPUB and FB2 files into ${libraryFolder}.`,
      )
    } else {
      showLibraryMessage(shown.length ? '' : 'No books match.')
    }
    $('library-books').replaceChildren(...shown.map(bookCard))
    if (deps.getView() === 'series') deps.seriesShown()
  }

  /** A card's series line: the number in the series always shows, a long name is cut short before it. */
  function seriesLine(book: LibraryBook) {
    const line = el('span', { className: 'series' })
    if (!book.series) return line
    line.append(el('span', { className: 'series-name', textContent: book.series }))
    if (book.seriesIndex) line.append(el('span', { className: 'series-index', textContent: `#${book.seriesIndex}` }))
    return line
  }

  /** One book in the grid: its cover and progress, a click to open (or choose where to start), and its menu. */
  function bookCard(book: LibraryBook) {
    const status = statusOf(book)
    const progress =
      status === 'finished' ? 'Finished' : status === 'reading' ? percent(book.fraction ?? 0) : 'New'
    const open = (fromStart = false) =>
      void invoke('open_library_book', { key: book.key, fromStart }).catch(e => deps.showError(book.title, e))
    // A book read before: continue where it was, or start over (asked); a new one just opens.
    const details = { label: 'Book details', action: () => void showBookDetails(book.fingerprint, book.title) }
    const choices: MenuItem[] =
      status === 'new'
        ? [{ label: 'Open', action: () => open() }, details]
        : [
            { label: `Continue · ${percent(book.fraction ?? 0)}`, action: () => open() },
            { label: 'Start from the beginning', action: () => open(true) },
            details,
          ]
    const withDelete = [...choices.filter(c => c !== details), details, { label: 'Delete from library…', danger: true, action: () => void deleteBook(book) }]
    const button = el(
      'button',
      {
        className: book.disabled ? 'off' : '',
        title: [book.title, book.authors.join(', '), seriesLabel(book)].filter(Boolean).join('\n'),
        onclick: (e: MouseEvent) => (status === 'new' ? open() : showMenu(menuPlace(e, button), choices)),
        oncontextmenu: (e: MouseEvent) => {
          e.preventDefault()
          showMenu({ x: e.clientX, y: e.clientY }, withDelete)
        },
      },
      coverWithProgress(book.title, book.hasCover ? book.key : null, book.fraction),
      el('span', { className: 'title', textContent: book.title }),
      el('span', { className: 'author', textContent: book.authors.join(', ') }),
      seriesLine(book),
      el(
        'span',
        { className: 'meta' },
        cloudMark(cloudStates[book.fingerprint]),
        [book.format, book.disabled ? 'turned off' : progress].filter(Boolean).join(' · '),
      ),
    )
    onLongPress(button, at => showMenu(at, withDelete))
    return el('li', {}, button)
  }

  /** Where a menu opened by a click shows: at the mouse, or under the button for a key press. */
  function menuPlace(e: MouseEvent, button: HTMLElement) {
    if (e.detail) return { x: e.clientX, y: e.clientY }
    const rect = button.getBoundingClientRect()
    return { x: rect.left, y: rect.bottom }
  }

  /** Cover images by book key, kept for the window's life so redraws don't reload them. */
  const coverUrls = new Map<string, Promise<string>>()

  /** Loads covers as their cards come near the visible part of the library. */
  let coverObserver: IntersectionObserver | null = null
  /** The observer that loads covers as they scroll into view, made on first use. */
  function observer() {
    // Created late: importing this module (as the tests do) must not need a browser.
    return (coverObserver ??= new IntersectionObserver(
      entries => {
        for (const entry of entries) {
          if (!entry.isIntersecting) continue
          const host = entry.target as HTMLElement
          coverObserver?.unobserve(host)
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
    ))
  }

  /** Loads a library cover into `host` once it comes into view. */
  function loadCover(host: HTMLElement, key: string) {
    host.dataset.key = key
    observer().observe(host)
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
    const read = book.fraction == null ? '' : percent(book.fraction)
    const button = el(
      'button',
      book.exists
        ? { title: [title, book.author].filter(Boolean).join('\n'), onclick: () => void invoke('open_recent', { key: book.key }).catch(console.error) }
        : {
            className: 'missing',
            title: 'File not found. Click to remove it from the list.',
            onclick: () => void invoke('forget_recent', { key: book.key }).then(() => refreshRecent(), console.error),
          },
      coverWithProgress(title, book.coverKey, book.fraction),
      el('span', { className: 'title', textContent: title }),
      el('span', { className: 'author', textContent: book.author }),
      el('span', { className: 'meta', textContent: book.exists ? read : 'File not found' }),
    )
    return el('li', {}, button)
  }

  /** The book details dialog is open. */
  const isDetailsOpen = () => !$('book-details').hidden

  /** Deletes a book from the library, after asking: the file goes to the Recycle Bin (a phone deletes it). */
  async function deleteBook(book: LibraryBook) {
    const ok = await confirmDialog({
      title: `Delete “${book.title}” from the library?`,
      text: `${deleteFileNote()} Its reading position and bookmarks are kept, should it come back.`,
      confirm: 'Delete',
    })
    if (!ok) return
    // The library redraws on "library-updated".
    await invoke('delete_library_book', { key: book.key }).catch(e => deps.showError(book.title, e))
  }

  /** The dialog key it shows, to ignore late cloud answers after a switch. */
  let detailsKey: string | null = null

  /** Shows everything known about a book: metadata, file, catalog origin and cloud presence. */
  async function showBookDetails(fingerprint: string, title: string) {
    detailsKey = fingerprint
    $('book-details-title').textContent = title
    $('book-details-rows').replaceChildren()
    $('book-details-annotation').textContent = ''
    $('book-details').hidden = false
    let details: BookDetails | null = null
    try {
      details = await invoke<BookDetails>('book_details', { fingerprint })
    } catch (e) {
      deps.showError(title, e)
      $('book-details').hidden = true
      return
    }
    if (detailsKey !== fingerprint) return
    const rows = $('book-details-rows')
    const row = (label: string, value: string) => {
      rows.append(el('dt', { textContent: label }), el('dd', { textContent: value }))
    }
    row('Author', details.authors.join(', ') || '—')
    if (details.series) row('Series', details.series + (details.seriesIndex ? ` #${details.seriesIndex}` : ''))
    if (details.language) row('Language', details.language)
    row('Format', details.format || '—')
    row('File', details.path)
    row('Size', formatSize(details.size))
    row('Fingerprint', details.fingerprint)
    row('From catalog', details.catalogSource ?? 'Not from a catalog')
    if (details.annotation) $('book-details-annotation').textContent = details.annotation
    void fillCloudState(fingerprint)
  }

  /** Asks the clouds about the book; unknown on timeout or failure. */
  async function fillCloudState(fingerprint: string) {
    const rows = $('book-details-rows')
    const dt = el('dt', { textContent: 'Cloud' })
    const dd = el('dd', { textContent: 'Checking…' })
    rows.append(dt, dd)
    const settled = await Promise.race([
      invoke<{ id: string; label: string; state: string }[]>('book_cloud', { fingerprint }).catch((): null => null),
      new Promise<null>(resolve => setTimeout(() => resolve(null), 8000)),
    ])
    if (detailsKey !== fingerprint || $('book-details').hidden) return
    dt.remove()
    dd.remove()
    if (!settled) {
      rows.append(el('dt', { textContent: 'Cloud' }), el('dd', { textContent: 'Unknown' }))
      return
    }
    for (const cloud of settled) rows.append(el('dt', { textContent: cloud.label }), el('dd', { textContent: cloudStateText(cloud.state) }))
  }

  /** What the details dialog says for one cloud's verdict. */
  function cloudStateText(state: string): string {
    switch (state) {
      case 'synced':
        return 'Synced'
      case 'absent':
        return 'Not in the cloud'
      default:
        return 'Unknown'
    }
  }

  /** Closes the details dialog; a cloud answer still on its way is ignored. */
  function hideBookDetails() {
    detailsKey = null
    $('book-details').hidden = true
  }

  $('book-details-close').addEventListener('click', hideBookDetails)
  document.addEventListener('mousedown', e => {
    if (!$('book-details').hidden && !(e.target as HTMLElement).closest('#book-details')) hideBookDetails()
  })

  /** A file size in KB or MB, as the details dialog and the download offer show it. */
  function formatSize(bytes: number): string {
    return bytes >= 1048576 ? `${(bytes / 1048576).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1024))} KB`
  }

  /** Fingerprints the download offer skipped this run. */
  const dismissedCloud = new Set<string>()
  /** Books the last offer showed (the Download button takes them). */
  let cloudMissing: MissingBook[] = []

  /** Hides the offer to download cloud books. */
  function hideCloudOffer(): void {
    $('cloud-offer').hidden = true
  }

  /** Offers the cloud books the library lacks, once per book per run. */
  async function offerCloudBooks(): Promise<void> {
    let missing: MissingBook[] = []
    try {
      missing = await invoke<MissingBook[]>('sync_missing_books')
    } catch (e) {
      console.warn('sync:', e)
      return
    }
    cloudMissing = missing.filter(m => !dismissedCloud.has(m.fingerprint))
    if (!cloudMissing.length) {
      hideCloudOffer()
      return
    }
    const n = cloudMissing.length
    const size = formatSize(cloudMissing.reduce((sum, m) => sum + m.size, 0))
    $('cloud-offer-text').textContent = `Download ${n === 1 ? 'a book' : `${n} books`} (${size}) from the clouds?`
    $('cloud-offer').hidden = false
  }

  /** Hands the offered books to the sync engine; they appear as they land
  (on Wi-Fi only, when the setting says so). */
  async function downloadCloudBooks(): Promise<void> {
    const books = cloudMissing
    cloudMissing = []
    hideCloudOffer()
    for (const book of books) dismissedCloud.add(book.fingerprint)
    try {
      await invoke('sync_queue_downloads', { fingerprints: books.map(book => book.fingerprint) })
      const n = books.length
      showLibraryMessage(`${n === 1 ? 'The book is' : `${n} books are`} on the way; they appear here as they arrive.`)
    } catch (e) {
      showLibraryMessage(`Couldn't download from the cloud: ${e}`)
    }
  }

  $('cloud-download').addEventListener('click', () => void downloadCloudBooks())
  $('cloud-later').addEventListener('click', () => {
    for (const book of cloudMissing) dismissedCloud.add(book.fingerprint)
    cloudMissing = []
    hideCloudOffer()
  })

  return {
    getBooks: () => libraryBooks,
    clearBooks,
    organize,
    applyOrganize,
    isOrganizeOpen: organizeOpen,
    isDetailsOpen,
    showBookDetails: (fingerprint: string, title: string) => void showBookDetails(fingerprint, title),
    showScreen,
    refreshScreen,
    refreshCloudMarks: () => void refreshCloudMarks(),
    refreshRecent,
    render: renderLibrary,
    chooseLibrary,
    bookCard,
    coverWithProgress,
    loadLibraryInfo,
    homeView,
    stepBack,
    isReady: () => libraryReady,
    isSettingsEmbedded,
    embedSettings,
  }
}
