/**
 * The app-owned settings and their defaults: this module is the only place
 * that assigns them, and settings.test.ts locks the key set.
 *
 * Appearance keys (theme, font, ...) default in reader-core's readAppearance
 * (spread in first); the backend stores settings opaquely (see store.rs),
 * so a new key here needs no Rust change — old installs read it fresh,
 * new code tolerates its absence.
 */
import { oneOf, readAppearance, type Flow } from 'reader-core'
import { HISTORY_LENGTHS, UI_FONTS, type PanelValues } from './settings-panel'
import { SORTS, STATUSES, type Sort, type Status } from './library'

export interface Settings extends PanelValues {
  flow: Flow
  librarySort: Sort
  libraryStatus: Status
  /** The last choice in the "add to library" bar, offered first next time. */
  libraryAddMode: 'move' | 'copy'
}

export const FLOWS: readonly Flow[] = ['paginated', 'scrolled']

/** Debounce before state (positions, settings) is written. */
export const SAVE_DELAY_MS = 500

export function readSettings(stored: Record<string, unknown>): Settings {
  return {
    ...readAppearance(stored),
    flow: oneOf(stored.flow, FLOWS, 'paginated'),
    librarySort: oneOf(stored.librarySort, SORTS, 'author'),
    libraryStatus: oneOf(stored.libraryStatus, STATUSES, 'all'),
    libraryAddMode: oneOf(stored.libraryAddMode, ['move', 'copy'] as const, 'move'),
    savePositions: stored.savePositions !== false,
    offerLibrary: stored.offerLibrary !== false,
    volumeKeys: stored.volumeKeys === true,
    swipePanels: stored.swipePanels !== false,
    swipePages: stored.swipePages !== false,
  booksWifiOnly: stored.booksWifiOnly !== false,
    historyLength: HISTORY_LENGTHS.find(n => n === stored.historyLength) ?? 3,
    uiFont: oneOf(stored.uiFont, UI_FONTS, 'classic'),
    disabledModules: Array.isArray(stored.disabledModules)
      ? (stored.disabledModules as unknown[]).filter((m): m is string => typeof m === 'string')
      : [],
  }
}
