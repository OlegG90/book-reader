import * as CFI from 'reader-core/foliate-js/epubcfi.js'

/** A place in a book the reader marked (kept per book, on this device only). */
export interface Bookmark {
  /** A point CFI: the start of the page it was made on. */
  cfi: string
  fraction: number
  /** The chapter it is in, when the book has one there. */
  chapter: string
}

/** The page shown: like a bookmark, but its CFI is the page's range. */
export type Page = Bookmark

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

/**
 * The bookmarks with their CFIs moved to the book's current sections, or null
 * when none moved. An FB2 book whose long chapters reader-core cuts into parts
 * (from 0.4) numbers its sections anew; `upgrade` is its `upgradeCFI`, and
 * books that have none keep their CFIs.
 */
export function upgradeBookmarks(list: Bookmark[], upgrade?: (cfi: string) => string): Bookmark[] | null {
  if (!upgrade) return null
  const moved = list.map(mark => ({ ...mark, cfi: upgrade(mark.cfi) }))
  return moved.some((mark, i) => mark.cfi !== list[i].cfi) ? moved : null
}

export const hasBookmark = (list: Bookmark[], pageCfi: string | undefined) =>
  !!pageCfi && list.some(mark => onPage(mark, pageCfi))

/** "Chapter 3 · 42%", or just "42%". */
export function bookmarkLabel({ chapter, fraction }: Bookmark): string {
  const percent = `${Math.round(fraction * 100)}%`
  return chapter ? `${chapter} · ${percent}` : percent
}
