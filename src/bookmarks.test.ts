import { describe, expect, it } from 'vitest'
import { bookmarkLabel, hasBookmark, toggleBookmark, upgradeBookmarks, type Page } from './bookmarks'

/** A page of section 2 from paragraph `from` to paragraph `to`. */
const page = (from: number, to: number, fraction: number): Page => ({
  cfi: `epubcfi(/6/4!/4,/${from}/1:0,/${to}/1:0)`,
  fraction,
  chapter: '',
})

describe('bookmarks', () => {
  it('adds in book order and removes on a second toggle', () => {
    let list = toggleBookmark([], page(20, 30, 0.5))
    list = toggleBookmark(list, page(2, 10, 0.1))
    expect(list.map(b => b.fraction)).toEqual([0.1, 0.5])
    expect(hasBookmark(list, page(20, 30, 0.5).cfi)).toBe(true)
    list = toggleBookmark(list, page(20, 30, 0.5))
    expect(list.map(b => b.fraction)).toEqual([0.1])
    expect(hasBookmark(list, undefined)).toBe(false)
  })

  it('still finds a bookmark after the pages are laid out differently', () => {
    const list = toggleBookmark([], page(20, 30, 0.5))
    // Bigger font: the bookmarked spot now sits on a page starting earlier.
    expect(hasBookmark(list, page(16, 22, 0.5).cfi)).toBe(true)
    expect(hasBookmark(list, page(22, 28, 0.55).cfi)).toBe(false)
    expect(toggleBookmark(list, page(16, 22, 0.5))).toEqual([])
  })

  it('labels with the chapter when there is one', () => {
    expect(bookmarkLabel({ cfi: 'x', fraction: 0.424, chapter: 'Chapter 3' })).toBe('Chapter 3 · 42%')
    expect(bookmarkLabel({ cfi: 'x', fraction: 0.07, chapter: '' })).toBe('7%')
  })

  describe('upgradeBookmarks', () => {
    // section 2 of the old layout became part 5 (section index 4) of the cut book
    const upgrade = (cfi: string) => cfi.replace('/6/4!/4/40', '/6/10[fb2-parts-80000]!/4/2')
    const old = toggleBookmark([], page(40, 50, 0.5))

    it('moves bookmarks made before the cut, so the new pages find them', () => {
      const moved = upgradeBookmarks(old, upgrade)
      expect(moved?.[0].cfi).toBe('epubcfi(/6/10[fb2-parts-80000]!/4/2/1:0)')
      expect(moved?.[0].fraction).toBe(0.5)
      const newPage = 'epubcfi(/6/10[fb2-parts-80000]!/4,/2/1:0,/12/1:0)'
      expect(hasBookmark(old, newPage)).toBe(false)
      expect(hasBookmark(moved!, newPage)).toBe(true)
    })

    it('reports nothing to save when no bookmark moves or the book has no upgrade', () => {
      expect(upgradeBookmarks(old, cfi => cfi)).toBeNull()
      expect(upgradeBookmarks(old, undefined)).toBeNull()
      expect(upgradeBookmarks([], upgrade)).toBeNull()
    })
  })
})
