// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'

/** What the backend answers: the window's book (null on the library screen). */
let current: unknown = null
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async (command: string) => (command === 'current_book' ? current : command === 'read_book' ? new ArrayBuffer(0) : null)),
}))
vi.mock('@tauri-apps/plugin-opener', () => ({ openUrl: vi.fn() }))
vi.mock('reader-core/foliate-js/ui/tree.js', () => ({ createTOCView: vi.fn() }))
vi.mock('./platform', () => ({ setReading: vi.fn() }))
/** The book being laid out: settles when the test says so; its events go to `events`. */
let settle: (view: unknown) => void = () => {}
let events: { relocate(location: unknown): void } = { relocate: () => {} }
vi.mock('reader-core', async original => ({
  ...(await original<typeof import('reader-core')>()),
  BookView: {
    open: vi.fn((_stage: unknown, _source: unknown, _options: unknown, handlers: typeof events) => {
      events = handlers
      return new Promise(done => (settle = done))
    }),
  },
}))

const { createReadingView } = await import('./reading-view')

const IDS = 'bookmark-button bookmark-count bookmark-list bookmarks-empty bottombar error progress progress-chapter progress-done progress-fill progress-knob progress-ticks progress-where stage start toc-author toc-title toc-tree topbar-author topbar-book sync-bar sync-text'

/** A reading view over the page's elements, recording when it shows the library. */
function setup() {
  document.body.innerHTML = IDS.split(' ')
    .map(id => `<div id="${id}"></div>`)
    .join('')
  const library = vi.fn(async () => {})
  const reading = createReadingView({
    getSettings: () => ({ flow: 'paginated', swipePages: true, savePositions: true, offerLibrary: false, disabledModules: [] }) as never,
    settingsReady: Promise.resolve(),
    currentBookStyle: () => ({}) as never,
    showLibraryScreen: library,
    showError: vi.fn(),
    onKey: vi.fn(),
    onPointer: vi.fn(),
    toggleTopbar: vi.fn(),
    scrimlessOpen: () => false,
    closePanels: vi.fn(),
    updateTopbarButtons: vi.fn(),
    showOffer: vi.fn(),
    hideOffer: vi.fn(),
    setTitle: vi.fn(),
    embedSettings: vi.fn(),
    setChrome: vi.fn(),
  })
  return { reading, library }
}

/** Lets pending promise callbacks run. */
const tick = () => new Promise(done => setTimeout(done, 0))

describe('open', () => {
  beforeEach(() => {
    current = { fileName: 'big.fb2', fingerprint: 'fp', position: null, noSave: false, offer: null }
  })

  it('counts a book still loading as opening', async () => {
    const { reading } = setup()
    void reading.open()
    await tick()
    expect(reading.book).toBeNull()
    expect(reading.opening).toBe(true)
  })

  it('drops a book that finishes loading after Back took the window to the library', async () => {
    const { reading, library } = setup()
    const first = reading.open()
    await tick()
    const late = settle
    const lateEvents = events
    // Back: the window's book is gone, and the library shows.
    current = null
    await reading.open()
    expect(library).toHaveBeenCalled()
    // The late book settling on its page is no move of the reader's.
    lateEvents.relocate({ fraction: 0.5, cfi: 'epubcfi(/6/2)' })
    await reading.flush()
    const { invoke } = await import('@tauri-apps/api/core')
    expect(vi.mocked(invoke).mock.calls.some(([command]) => command === 'save_position')).toBe(false)
    const destroy = vi.fn()
    late({ destroy })
    await first
    expect(destroy).toHaveBeenCalled()
    expect(reading.book).toBeNull()
    expect(reading.opening).toBe(false)
  })
})
