import { invoke } from '@tauri-apps/api/core'
/** What the backend reports about OPDS catalogs (see catalog.rs / opds.rs). */

export interface Source {
  id: string
  name: string
  url: string
  user: string
  sync: boolean
  hasPassword: boolean
}

export type FileFormat = 'epub' | 'fb2' | 'fb2Zip'

export interface BookFile {
  href: string
  format: FileFormat
}

export interface CatalogBook {
  kind: 'book'
  id: string
  title: string
  authors: string[]
  summary: string
  cover: string | null
  files: BookFile[]
}

export interface CatalogFolder {
  kind: 'folder'
  title: string
  href: string
  summary: string
}

export type CatalogEntry = CatalogBook | CatalogFolder

export interface Feed {
  title: string
  entries: CatalogEntry[]
  next: string | null
  search: string | null
}

const FORMAT_ORDER: FileFormat[] = ['epub', 'fb2Zip', 'fb2']
export const FORMAT_LABELS: Record<FileFormat, string> = { epub: 'EPUB', fb2: 'FB2', fb2Zip: 'FB2' }

/** The file downloaded by default: EPUB if the catalog has it, otherwise FB2. */
export function preferredFile(files: BookFile[]): BookFile | undefined {
  for (const format of FORMAT_ORDER) {
    const file = files.find(f => f.format === format)
    if (file) return file
  }
  return undefined
}

const fold = (text: string) =>
  text
    .toLocaleLowerCase()
    .normalize('NFKD')
    .replace(/\p{M}/gu, '')
    .replace(/[^\p{L}\p{N}]+/gu, ' ')
    .trim()

/** Last word of a name: authors are written "First Last" in feeds and in the library alike. */
const surname = (author: string) => fold(author).split(' ').pop() ?? ''

/**
 * Whether the library seems to hold this book: the same title and the first
 * author's surname. Only a hint for the list; downloading checks the content.
 */
export function inLibrary(book: { title: string; authors: string[] }, library: { title: string; authors: string[] }[]): boolean {
  const title = fold(book.title)
  const author = surname(book.authors[0] ?? '')
  return library.some(b => fold(b.title) === title && surname(b.authors[0] ?? '') === author)
}

/** The catalog sources, or none when they can't be read. */
export async function fetchSources(): Promise<Source[]> {
  return invoke<Source[]>('catalog_sources').catch((e): Source[] => {
    console.error(e)
    return []
  })
}
