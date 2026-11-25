// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { createLibraryView, type LibraryDeps, type View } from './library-view'
import type { Settings } from './settings'

function setup(view: View | null, catalogsBack = false, seriesBack = false, hasBook = false) {
  // As on the library screen, where the settings sit in the content.
  document.body.innerHTML = '<div id="settings" class="embedded"></div><div id="cloud-offer" hidden><span id="cloud-offer-text"></span><span id="cloud-offer-buttons"><button id="cloud-download">Download</button><button id="cloud-later">Not now</button></span></div>'
  const shown: View[] = []
  const deps: LibraryDeps = {
    getSettings: () => ({}) as Settings,
    changeSettings: () => {},
    showError: () => {},
    showView: to => void shown.push(to),
    closePanels: () => {},
    setTitle: () => {},
    setLibraryFolder: () => {},
    hasBook: () => hasBook,
    getView: () => view,
    backInCatalogs: () => catalogsBack,
    backInSeries: () => seriesBack,
    seriesShown: () => {},
  }
  return { library: createLibraryView(deps), shown }
}

describe('stepBack', () => {
  it('stays out of the way with a book open', () => {
    const { library, shown } = setup('catalogs', true, false, true)
    expect(library.stepBack()).toBe(false)
    expect(shown).toEqual([])
  })

  it('goes up a catalog folder', () => {
    const { library, shown } = setup('catalogs', true)
    expect(library.stepBack()).toBe(true)
    expect(shown).toEqual([])
  })

  it('goes up a series folder', () => {
    const { library, shown } = setup('series', false, true)
    expect(library.stepBack()).toBe(true)
    expect(shown).toEqual([])
  })

  it('is already home on the history and the books', () => {
    for (const view of ['recent', 'library'] as const) {
      const { library, shown } = setup(view)
      expect(library.stepBack()).toBe(false)
      expect(shown).toEqual([])
    }
  })

  it('returns to the history, else the books, from elsewhere', () => {
    const { library, shown } = setup('settings:reading')
    expect(library.stepBack()).toBe(true)
    expect(shown).toEqual(['library'])
  })
})
