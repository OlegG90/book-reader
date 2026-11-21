import { describe, expect, it } from 'vitest'
import { inLibrary, preferredFile } from './catalog'

describe('catalog', () => {
  it('prefers EPUB, then FB2', () => {
    expect(preferredFile([{ href: 'a', format: 'fb2' }, { href: 'b', format: 'epub' }])?.href).toBe('b')
    expect(preferredFile([{ href: 'a', format: 'fb2' }])?.href).toBe('a')
    expect(preferredFile([])).toBeUndefined()
  })

  it('spots books the library seems to hold', () => {
    const library = [{ title: 'Захар Беркут', authors: ['Іван Якович Франко'] }]
    expect(inLibrary({ title: 'захар  беркут', authors: ['Іван Франко'] }, library)).toBe(true)
    expect(inLibrary({ title: 'Захар Беркут', authors: ['Someone Else'] }, library)).toBe(false)
    expect(inLibrary({ title: 'Інша книга', authors: ['Іван Франко'] }, library)).toBe(false)
  })
})
