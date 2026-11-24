import { describe, expect, it } from 'vitest'
import { formatLabel, formatOff, inLibrary, preferredFile, setFormats } from './catalog'
import type { ModuleInfo } from './settings-panel'

const module = (over: Partial<ModuleInfo> & { id: string }): ModuleInfo => ({
  kind: 'format',
  label: 'X',
  hint: 'X.',
  extensions: [],
  needsNetwork: false,
  optional: true,
  enabled: true,
  inUse: false,
  inUseReason: null,
  ...over,
})

describe('catalog', () => {
  it('prefers EPUB, then FB2', () => {
    expect(preferredFile([{ href: 'a', format: 'fb2' }, { href: 'b', format: 'epub' }])?.href).toBe('b')
    expect(preferredFile([{ href: 'a', format: 'fb2' }])?.href).toBe('a')
    expect(preferredFile([])).toBeUndefined()
  })

  it('takes the order and labels from the backend list', () => {
    setFormats([
      module({ id: 'format:fb2', label: 'X-FB2', extensions: ['fb2.zip', 'fbz', 'fb2'] }),
      module({ id: 'format:epub', label: 'X-EPUB', extensions: ['epub'] }),
    ])
    expect(formatLabel('epub')).toBe('X-EPUB')
    expect(preferredFile([{ href: 'a', format: 'fb2' }, { href: 'b', format: 'epub' }])?.href).toBe('a')
    setFormats([
      module({ id: 'format:epub', label: 'EPUB', extensions: ['epub'] }),
      module({ id: 'format:fb2', label: 'FB2', extensions: ['fb2.zip', 'fbz', 'fb2'] }),
    ])
  })

  it('ignores an empty or foreign list', () => {
    setFormats([
      module({ id: 'format:epub', label: 'EPUB', extensions: ['epub'] }),
      module({ id: 'format:fb2', label: 'FB2', extensions: ['fb2.zip', 'fbz', 'fb2'] }),
      module({ id: 'capability:catalogs', kind: 'capability', label: 'X', extensions: [] }),
      module({ id: 'format:mobi', label: 'X-Mobi', extensions: ['mobi'] }),
    ])
    expect(formatLabel('epub')).toBe('EPUB')
    expect(preferredFile([{ href: 'a', format: 'fb2' }, { href: 'b', format: 'epub' }])?.href).toBe('b')
    setFormats([])
    expect(formatLabel('epub')).toBe('EPUB')
    expect(preferredFile([{ href: 'a', format: 'fb2' }, { href: 'b', format: 'epub' }])?.href).toBe('b')
  })

  it('skips a disabled format', () => {
    setFormats([
      module({ id: 'format:epub', label: 'EPUB', extensions: ['epub'], enabled: false }),
      module({ id: 'format:fb2', label: 'FB2', extensions: ['fb2.zip', 'fbz', 'fb2'] }),
    ])
    expect(formatOff('epub')).toBe(true)
    expect(preferredFile([{ href: 'a', format: 'epub' }, { href: 'b', format: 'fb2' }])?.href).toBe('b')
    expect(preferredFile([{ href: 'a', format: 'epub' }])).toBeUndefined()
    setFormats([
      module({ id: 'format:epub', label: 'EPUB', extensions: ['epub'] }),
      module({ id: 'format:fb2', label: 'FB2', extensions: ['fb2.zip', 'fbz', 'fb2'] }),
    ])
  })

  it('spots books the library seems to hold', () => {
    const library = [{ title: 'Захар Беркут', authors: ['Іван Якович Франко'] }]
    expect(inLibrary({ title: 'захар  беркут', authors: ['Іван Франко'] }, library)).toBe(true)
    expect(inLibrary({ title: 'Захар Беркут', authors: ['Someone Else'] }, library)).toBe(false)
    expect(inLibrary({ title: 'Інша книга', authors: ['Іван Франко'] }, library)).toBe(false)
  })
})
