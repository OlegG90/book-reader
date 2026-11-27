/** A book as the backend's library scan reports it (see scan_library in lib.rs). */
export interface LibraryBook {
  key: string
  /** Content identity, for sync and details from any window. */
  fingerprint: string
  title: string
  authors: string[]
  authorSort: string
  series: string | null
  seriesIndex: string | null
  format: string
  /** Share read, if a position is saved. */
  fraction: number | null
  /** When the book came into the library (ms). */
  added: number
  hasCover: boolean
  /** Its format is turned off in Settings › Modules: listed, but it won't open. */
  disabled: boolean
}

export const STATUSES = ['all', 'new', 'reading', 'finished'] as const
export type Status = (typeof STATUSES)[number]
export const STATUS_LABELS: Record<Status, string> = {
  all: 'All books',
  new: 'Not started',
  reading: 'Reading',
  finished: 'Finished',
}

export const SORTS = ['author', 'title', 'added'] as const
export type Sort = (typeof SORTS)[number]
export const SORT_LABELS: Record<Sort, string> = { author: 'Author', title: 'Title', added: 'Recently added' }

/** How a page of books shows: cover cards, or one compact line each (each page has its own). */
export const LAYOUTS = ['covers', 'list'] as const
export type Layout = (typeof LAYOUTS)[number]

/** Past this share a book counts as finished (the last pages are often notes or ads). */
export const FINISHED_AT = 0.98

export function statusOf(book: LibraryBook): Exclude<Status, 'all'> {
  if (book.fraction == null || book.fraction <= 0) return 'new'
  return book.fraction >= FINISHED_AT ? 'finished' : 'reading'
}

const fold = (text: string) => text.toLocaleLowerCase().normalize('NFKD').replace(/\p{M}/gu, '')

/** Each book's searchable text, folded once (books are replaced on every scan). */
const searchText = new WeakMap<LibraryBook, string>()
const searchTextOf = (book: LibraryBook) => {
  let text = searchText.get(book)
  if (text == null) {
    text = fold([book.title, ...book.authors, book.series ?? ''].join(' '))
    searchText.set(book, text)
  }
  return text
}

/** Books matching every word of `query` (in title, authors or series) and the status. */
export function filterBooks(books: LibraryBook[], query: string, status: Status): LibraryBook[] {
  const words = fold(query).split(/\s+/).filter(Boolean)
  return books.filter(book => {
    if (status !== 'all' && statusOf(book) !== status) return false
    const text = searchTextOf(book)
    return words.every(word => text.includes(word))
  })
}

const collator = new Intl.Collator(undefined, { numeric: true, sensitivity: 'base' })
const compareText = (a: string, b: string) => {
  // Books without an author or title sort last.
  if (!a !== !b) return a ? -1 : 1
  return collator.compare(a, b)
}
const seriesNumber = (book: LibraryBook) => {
  const n = parseFloat(book.seriesIndex ?? '')
  return Number.isFinite(n) ? n : Infinity
}

const COMPARE: Record<Sort, (a: LibraryBook, b: LibraryBook) => number> = {
  title: (a, b) => compareText(a.title, b.title),
  // By author, then their series in reading order, then their other books by title.
  author: (a, b) =>
    compareText(a.authorSort, b.authorSort) ||
    compareText(a.series ?? '', b.series ?? '') ||
    seriesNumber(a) - seriesNumber(b) ||
    compareText(a.title, b.title),
  added: (a, b) => b.added - a.added || compareText(a.title, b.title),
}

export function sortBooks(books: LibraryBook[], sort: Sort): LibraryBook[] {
  return [...books].sort(COMPARE[sort])
}

/** "Discworld #3", "Discworld", or "". */
export function seriesLabel(book: LibraryBook): string {
  if (!book.series) return ''
  return book.seriesIndex ? `${book.series} #${book.seriesIndex}` : book.series
}

/** A series in the library: its books in reading order, and how many of them are finished. */
export interface SeriesGroup {
  name: string
  books: LibraryBook[]
  read: number
}

/** The library's series by name, each one's books in reading order (books without a series aren't in any). */
export function groupSeries(books: LibraryBook[]): SeriesGroup[] {
  const byName = new Map<string, LibraryBook[]>()
  for (const book of books) {
    if (!book.series) continue
    const list = byName.get(book.series)
    if (list) list.push(book)
    else byName.set(book.series, [book])
  }
  return [...byName]
    .map(([name, list]) => ({
      name,
      books: list.sort((a, b) => seriesNumber(a) - seriesNumber(b) || compareText(a.title, b.title)),
      read: list.filter(b => statusOf(b) === 'finished').length,
    }))
    .sort((a, b) => compareText(a.name, b.name))
}
