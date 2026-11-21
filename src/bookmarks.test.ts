import { describe, expect, it } from 'vitest'
import { bookmarkLabel, hasBookmark, toggleBookmark, type Bookmark } from './bookmarks'

const mark = (cfi: string, fraction: number, chapter = ''): Bookmark => ({ cfi, fraction, chapter })

describe('bookmarks', () => {
  it('adds in book order and removes on a second toggle', () => {
    let list = toggleBookmark([], mark('b', 0.5))
    list = toggleBookmark(list, mark('a', 0.1))
    expect(list.map(b => b.cfi)).toEqual(['a', 'b'])
    expect(hasBookmark(list, 'b')).toBe(true)
    list = toggleBookmark(list, mark('b', 0.5))
    expect(list.map(b => b.cfi)).toEqual(['a'])
    expect(hasBookmark(list, 'b')).toBe(false)
    expect(hasBookmark(list, undefined)).toBe(false)
  })

  it('labels with the chapter when there is one', () => {
    expect(bookmarkLabel(mark('x', 0.424, 'Chapter 3'))).toBe('Chapter 3 · 42%')
    expect(bookmarkLabel(mark('x', 0.07))).toBe('7%')
  })
})
