// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest'
import { createRemovedView } from './removed-view'

/** A Removed page in a fresh host, with its actions recorded. */
function page() {
  const host = document.createElement('div')
  const actions = { removeFromClouds: vi.fn(), restore: vi.fn() }
  const view = createRemovedView(host, actions)
  const buttons = (label: string) => [...host.querySelectorAll<HTMLButtonElement>('button')].filter(b => b.textContent === label)
  return { host, actions, view, buttons }
}

describe('createRemovedView', () => {
  it('says how books get here while it has none', () => {
    const { host, buttons } = page()
    expect(host.querySelectorAll('li')).toHaveLength(0)
    expect(host.textContent).toContain('Books deleted from this library show here')
    expect(buttons('Remove all from clouds')[0].hidden).toBe(true)
  })

  it('brings a book back, or removes books from the clouds', () => {
    const { host, actions, view, buttons } = page()
    view.show([
      { fingerprint: 'a', name: 'A/One.epub', size: 1 },
      { fingerprint: 'b', name: 'B/Two.epub', size: 2 },
    ])
    expect(host.textContent).toContain('B/Two.epub')
    buttons('Download again')[0].click()
    expect(actions.restore).toHaveBeenCalledWith(['a'])
    buttons('Remove from clouds')[1].click()
    expect(actions.removeFromClouds).toHaveBeenCalledWith(['b'])
    const all = buttons('Remove all from clouds')[0]
    expect(all.hidden).toBe(false)
    all.click()
    expect(actions.removeFromClouds).toHaveBeenLastCalledWith(['a', 'b'])
  })
})
