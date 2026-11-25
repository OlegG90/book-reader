import { describe, expect, it } from 'vitest'
import { filterBooks, groupSeries, seriesLabel, sortBooks, statusOf, type LibraryBook } from './library'

const book = (overrides: Partial<LibraryBook>): LibraryBook => ({
  key: overrides.title ?? 'key',
  fingerprint: overrides.key ?? 'fp',
  title: '',
  authors: [],
  authorSort: '',
  series: null,
  seriesIndex: null,
  format: 'EPUB',
  fraction: null,
  added: 0,
  hasCover: false,
  ...overrides,
})

const titles = (books: LibraryBook[]) => books.map(b => b.title)

describe('statusOf', () => {
  it('splits books into not started, reading and finished', () => {
    expect(statusOf(book({ fraction: null }))).toBe('new')
    expect(statusOf(book({ fraction: 0 }))).toBe('new')
    expect(statusOf(book({ fraction: 0.4 }))).toBe('reading')
    expect(statusOf(book({ fraction: 0.99 }))).toBe('finished')
  })
})

describe('filterBooks', () => {
  const books = [
    book({ title: 'Захар Беркут', authors: ['Іван Франко'], fraction: 0.5 }),
    book({ title: 'Guards! Guards!', authors: ['Terry Pratchett'], series: 'Discworld' }),
    book({ title: 'Café Society', authors: ['Someone'], fraction: 1 }),
  ]

  it('matches every word in title, author or series, ignoring case and accents', () => {
    expect(titles(filterBooks(books, 'франко', 'all'))).toEqual(['Захар Беркут'])
    expect(titles(filterBooks(books, 'discworld guards', 'all'))).toEqual(['Guards! Guards!'])
    expect(titles(filterBooks(books, 'cafe', 'all'))).toEqual(['Café Society'])
    expect(filterBooks(books, '  ', 'all')).toHaveLength(3)
  })

  it('filters by status', () => {
    expect(titles(filterBooks(books, '', 'reading'))).toEqual(['Захар Беркут'])
    expect(titles(filterBooks(books, '', 'new'))).toEqual(['Guards! Guards!'])
    expect(titles(filterBooks(books, '', 'finished'))).toEqual(['Café Society'])
  })
})

describe('sortBooks', () => {
  const books = [
    book({ title: 'Night Watch', authorSort: 'Pratchett Terry', series: 'Discworld', seriesIndex: '29', added: 3 }),
    book({ title: 'Untitled', added: 5 }),
    book({ title: 'Guards! Guards!', authorSort: 'Pratchett Terry', series: 'Discworld', seriesIndex: '8', added: 1 }),
    book({ title: 'Emma', authorSort: 'Austen Jane', added: 2 }),
    book({ title: 'Good Omens', authorSort: 'Pratchett Terry', added: 4 }),
  ]

  it('by author puts series first, in reading order, and authorless books last', () => {
    expect(titles(sortBooks(books, 'author'))).toEqual(['Emma', 'Guards! Guards!', 'Night Watch', 'Good Omens', 'Untitled'])
  })

  it('by title and by date added', () => {
    expect(titles(sortBooks(books, 'title'))).toEqual(['Emma', 'Good Omens', 'Guards! Guards!', 'Night Watch', 'Untitled'])
    expect(titles(sortBooks(books, 'added'))).toEqual(['Untitled', 'Good Omens', 'Night Watch', 'Emma', 'Guards! Guards!'])
  })
})

it('labels series with their number', () => {
  expect(seriesLabel(book({ series: 'Discworld', seriesIndex: '3' }))).toBe('Discworld #3')
  expect(seriesLabel(book({ series: 'Discworld' }))).toBe('Discworld')
  expect(seriesLabel(book({}))).toBe('')
})

describe('groupSeries', () => {
  it('gathers each series in reading order and counts the books finished', () => {
    const books = [
      book({ title: 'Small Gods', series: 'Discworld', seriesIndex: '13', fraction: 1 }),
      book({ title: 'The Colour of Magic', series: 'Discworld', seriesIndex: '1' }),
      book({ title: 'Mort', series: 'Discworld', seriesIndex: '4', fraction: 0.3 }),
      book({ title: 'Dune', series: 'Dune', seriesIndex: '1', fraction: 1 }),
      book({ title: 'Solaris' }),
    ]
    const groups = groupSeries(books)
    expect(groups.map(g => g.name)).toEqual(['Discworld', 'Dune'])
    expect(titles(groups[0].books)).toEqual(['The Colour of Magic', 'Mort', 'Small Gods'])
    expect(groups[0].read).toBe(1)
    expect(groups[1].read).toBe(1)
  })
})
