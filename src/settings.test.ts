// @vitest-environment jsdom
import { describe, expect, it } from 'vitest'
import { readSettings } from './settings'

describe('readSettings', () => {
  it('fills in every default', () => {
    const settings = readSettings({})
    expect(settings.flow).toBe('paginated')
    expect(settings.librarySort).toBe('author')
    expect(settings.libraryStatus).toBe('all')
    expect(settings.libraryAddMode).toBe('move')
    expect(settings.savePositions).toBe(true)
    expect(settings.offerLibrary).toBe(true)
    expect(settings.volumeKeys).toBe(false)
    expect(settings.historyLength).toBe(3)
    expect(settings.uiFont).toBe('classic')
  })

  it('keeps what was stored', () => {
    const settings = readSettings({ flow: 'scrolled', volumeKeys: true, historyLength: 5, librarySort: 'title' })
    expect(settings.flow).toBe('scrolled')
    expect(settings.volumeKeys).toBe(true)
    expect(settings.historyLength).toBe(5)
    expect(settings.librarySort).toBe('title')
  })

  it('falls back on unknown values', () => {
    const settings = readSettings({ flow: 'sideways', librarySort: 'colour', historyLength: 99, uiFont: 'comic sans' })
    expect(settings.flow).toBe('paginated')
    expect(settings.librarySort).toBe('author')
    expect(settings.historyLength).toBe(3)
    expect(settings.uiFont).toBe('classic')
  })
})
