// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest'
import { closePopup, showMenu } from './book-menu'

describe('showMenu', () => {
  it('checks the chosen one of a set, with a count at the end', () => {
    const chosen = vi.fn()
    showMenu({ x: 0, y: 0 }, [
      { label: 'All books', detail: '51', checked: true, action: vi.fn() },
      { label: 'Reading', detail: '12', checked: false, action: chosen },
    ])
    const items = [...document.querySelectorAll<HTMLButtonElement>('.popup-menu button')]
    expect(items.map(b => [b.getAttribute('role'), b.getAttribute('aria-checked'), b.textContent])).toEqual([
      ['menuitemradio', 'true', 'All books51'],
      ['menuitemradio', 'false', 'Reading12'],
    ])
    items[1].click()
    expect(chosen).toHaveBeenCalled()
    expect(document.querySelector('.popup-menu')).toBeNull()
  })

  it('keeps plain items plain', () => {
    showMenu({ x: 0, y: 0 }, [{ label: 'Open', action: vi.fn() }])
    const item = document.querySelector<HTMLButtonElement>('.popup-menu button')!
    expect(item.getAttribute('role')).toBe('menuitem')
    expect(item.hasAttribute('aria-checked')).toBe(false)
    closePopup()
  })
})
