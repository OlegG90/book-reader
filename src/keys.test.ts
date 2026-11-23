import { describe, expect, it } from 'vitest'
import { commandFor } from './keys'

const key = (k: string, mods: Partial<Record<'shiftKey' | 'ctrlKey' | 'altKey' | 'metaKey', boolean>> = {}) =>
  commandFor({ key: k, shiftKey: false, ctrlKey: false, altKey: false, metaKey: false, ...mods })

describe('commandFor', () => {
  it('maps page turning keys', () => {
    expect(key('ArrowRight')).toBe('right')
    expect(key('PageDown')).toBe('next')
    expect(key(' ')).toBe('next')
    expect(key(' ', { shiftKey: true })).toBe('prev')
  })

  it('maps appearance shortcuts', () => {
    expect(key('s')).toBe('settings')
    expect(key(',', { ctrlKey: true })).toBe('settings')
    expect(key('D')).toBe('cycleTheme')
    expect(key('=', { ctrlKey: true })).toBe('fontBigger')
    expect(key('+', { ctrlKey: true })).toBe('fontBigger')
    expect(key('-', { ctrlKey: true })).toBe('fontSmaller')
  })

  it('maps window shortcuts', () => {
    expect(key('o', { ctrlKey: true })).toBe('open')
    expect(key('L')).toBe('library')
    expect(key('b')).toBe('bookmark')
    expect(key('W', { ctrlKey: true, shiftKey: true })).toBe('closeWindow')
  })

  it('leaves other combinations to the browser', () => {
    expect(key('s', { ctrlKey: true })).toBeNull()
    expect(key('ArrowRight', { altKey: true })).toBeNull()
    expect(key('x')).toBeNull()
  })

  it("works in any keyboard layout (by the key's place)", () => {
    const at = (k: string, code: string, mods = {}) =>
      commandFor({ key: k, code, shiftKey: false, ctrlKey: false, altKey: false, metaKey: false, ...mods })
    expect(at('е', 'KeyT')).toBe('toc')
    expect(at('и', 'KeyB')).toBe('bookmark')
    expect(at('б', 'Comma', { ctrlKey: true })).toBe('settings')
    expect(at('щ', 'KeyO', { ctrlKey: true })).toBe('open')
    expect(at(' ', 'Space')).toBe('next')
  })
})
