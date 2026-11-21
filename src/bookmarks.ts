import * as CFI from './vendor/foliate-js/epubcfi.js'

/** A place in a book the reader marked (kept per book, on this device only). */
export interface Bookmark {
  /** A point CFI: the start of the page it was made on. */
  cfi: string
  fraction: number
  /** The chapter it is in, when the book has one there. */
  chapter: string
}

/** The page shown: foliate-js reports it as a range CFI. */
export interface Page {
  cfi: string
  fraction: number
  chapter: string
}

/**
 * Whether a bookmark lies on the page. Pages move when the font or window
 * size changes, so this compares positions rather than CFI strings.
 */
export function onPage(mark: Bookmark, pageCfi: string): boolean {
  return CFI.compare(mark.cfi, CFI.collapse(pageCfi)) >= 0 && CFI.compare(mark.cfi, CFI.collapse(pageCfi, true)) <= 0
}

/** Bookmarks the page, or removes the bookmarks on it; the list stays in book order. */
export function toggleBookmark(list: Bookmark[], page: Page): Bookmark[] {
  if (list.some(mark => onPage(mark, page.cfi))) return list.filter(mark => !onPage(mark, page.cfi))
  const mark = { cfi: CFI.collapse(page.cfi), fraction: page.fraction, chapter: page.chapter }
  return [...list, mark].sort((a, b) => a.fraction - b.fraction)
}

export const hasBookmark = (list: Bookmark[], pageCfi: string | undefined) =>
  !!pageCfi && list.some(mark => onPage(mark, pageCfi))

/** "Chapter 3 · 42%", or just "42%". */
export function bookmarkLabel({ chapter, fraction }: Bookmark): string {
  const percent = `${Math.round(fraction * 100)}%`
  return chapter ? `${chapter} · ${percent}` : percent
}
