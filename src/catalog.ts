import { invoke } from '@tauri-apps/api/core'
import type { ModuleInfo } from './settings-panel'
import { MODULE } from './modules'
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

const EXTENSION_FORMAT: Record<string, FileFormat> = { epub: 'epub', fb2: 'fb2', 'fb2.zip': 'fb2Zip', fbz: 'fb2Zip' }
let formatOrder: FileFormat[] = ['epub', 'fb2Zip', 'fb2']
let formatLabels: Record<FileFormat, string> = { epub: 'EPUB', fb2: 'FB2', fb2Zip: 'FB2' }
let formatModule: Record<FileFormat, string> = { epub: MODULE.epub, fb2: MODULE.fb2, fb2Zip: MODULE.fb2 }
let formatsOff = new Set<string>()

/** Learns the formats from the backend's module list: order, labels and switches. */
export function setFormats(modules: ModuleInfo[]) {
  const order: FileFormat[] = []
  const labels = {} as Record<FileFormat, string>
  const owners = {} as Record<FileFormat, string>
  const off = new Set<string>()
  for (const m of modules) {
    if (m.kind !== 'format') continue
    if (!m.enabled) off.add(m.id)
    for (const ext of m.extensions) {
      const format = EXTENSION_FORMAT[ext]
      if (!format || order.includes(format)) continue
      order.push(format)
      labels[format] = m.label
      owners[format] = m.id
    }
  }
  if (!order.length) return
  formatOrder = order
  formatLabels = labels
  formatModule = owners
  formatsOff = off
}

/** The label for a download button. */
export function formatLabel(format: FileFormat): string {
  return formatLabels[format]
}

/** Whether the format's module is turned off. */
export function formatOff(format: FileFormat): boolean {
  return formatsOff.has(formatModule[format])
}

/** The file downloaded by default: the enabled formats in backend order. */
export function preferredFile(files: BookFile[]): BookFile | undefined {
  for (const format of formatOrder) {
    if (formatOff(format)) continue
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
