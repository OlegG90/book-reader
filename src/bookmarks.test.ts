import { describe, expect, it } from 'vitest'
import { bookmarkLabel, hasBookmark, toggleBookmark, type Page } from './bookmarks'

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
})
