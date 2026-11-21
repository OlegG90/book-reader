/** A place in a book the reader marked (kept per book, on this device only). */
export interface Bookmark {
  cfi: string
  fraction: number
  /** The chapter it is in, when the book has one there. */
  chapter: string
}

/** Adds a bookmark at `mark`, or removes the one already there; the list stays in book order. */
export function toggleBookmark(list: Bookmark[], mark: Bookmark): Bookmark[] {
  if (list.some(b => b.cfi === mark.cfi)) return list.filter(b => b.cfi !== mark.cfi)
  return [...list, mark].sort((a, b) => a.fraction - b.fraction)
}

export const hasBookmark = (list: Bookmark[], cfi: string | undefined) => !!cfi && list.some(b => b.cfi === cfi)

/** "Chapter 3 · 42%", or just "42%". */
export function bookmarkLabel({ chapter, fraction }: Bookmark): string {
  const percent = `${Math.round(fraction * 100)}%`
  return chapter ? `${chapter} · ${percent}` : percent
}
