// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest'
import { DEFAULT_APPEARANCE } from 'reader-core'
import { createAppearancePanel, createSettingsPage, type PanelValues } from './settings-panel'

const initial: PanelValues = { ...DEFAULT_APPEARANCE, savePositions: true, offerLibrary: true, volumeKeys: false, historyLength: 3 }

const buttonIn = (host: HTMLElement) => (text: string) => {
  const found = [...host.querySelectorAll('button')].find(b => b.textContent === text)
  if (!found) throw new Error(`no button "${text}"`)
  return found
}

function appearance(extras = {}) {
  const host = document.createElement('aside')
  const onChange = vi.fn()
  const panel = createAppearancePanel(host, initial, onChange, extras)
  return { host, onChange, panel, button: buttonIn(host) }
}

function page() {
  const host = document.createElement('section')
  const onChange = vi.fn()
  const actions = { clearHistory: vi.fn(), toggleFileTypes: vi.fn(), changeLibrary: vi.fn(), organizeLibrary: vi.fn(), close: vi.fn() }
  const panel = createSettingsPage(host, initial, onChange, actions)
  return { host, onChange, actions, panel, button: buttonIn(host) }
}

describe('appearance panel (Aa)', () => {
  it('reports value changes', () => {
    const { host, onChange, button } = appearance()
    button('Dark').click()
    expect(onChange).toHaveBeenCalledWith({ theme: 'dark' })

    const font = host.querySelector('select')
    if (!font) throw new Error('no font select')
    font.value = 'Calibri'
    font.dispatchEvent(new Event('change'))
    expect(onChange).toHaveBeenCalledWith({ font: 'Calibri' })

    button('A+').click()
    expect(onChange).toHaveBeenCalledWith({ fontSize: initial.fontSize + 1 })
  })

  it('marks the current theme on show', () => {
    const { panel, button } = appearance()
    panel.show({ ...initial, theme: 'sepia' })
    expect(button('Sepia').getAttribute('aria-pressed')).toBe('true')
    expect(button('Dark').getAttribute('aria-pressed')).toBe('false')
  })

  it("offers the book's mode and all settings when asked to", () => {
    const flow = vi.fn()
    const allSettings = vi.fn()
    const { panel, button } = appearance({ flow, allSettings })
    panel.show(initial, 'paginated')
    expect(button('Paginated').getAttribute('aria-pressed')).toBe('true')
    button('Scrolled').click()
    expect(flow).toHaveBeenCalledWith('scrolled')
    button('All settings…').click()
    expect(allSettings).toHaveBeenCalledOnce()
  })

  it('has no mode buttons without a book', () => {
    const { panel, button } = appearance({ flow: vi.fn() })
    panel.show(initial)
    expect(button('Paginated').disabled).toBe(true)
  })

  it('leaves out the mode and the link by default', () => {
    const { host } = appearance()
    expect(host.textContent).not.toContain('Paginated')
    expect(host.textContent).not.toContain('All settings')
  })
})

describe('settings page', () => {
  it('has the appearance and the reading switches', () => {
    const { host, onChange, button } = page()
    button('Light').click()
    expect(onChange).toHaveBeenCalledWith({ theme: 'light' })
    const remember = [...host.querySelectorAll('label')].find(l => l.textContent?.includes('Remember reading position'))
    const box = remember?.querySelector('input')
    if (!box) throw new Error('no "Remember reading position"')
    box.checked = false
    box.dispatchEvent(new Event('change'))
    expect(onChange).toHaveBeenCalledWith({ savePositions: false })
  })

  it('forwards the action buttons', () => {
    const { actions, button } = page()
    button('Clear history').click()
    expect(actions.clearHistory).toHaveBeenCalledOnce()
    button('Register').click()
    expect(actions.toggleFileTypes).toHaveBeenCalledOnce()
    button('Change…').click()
    expect(actions.changeLibrary).toHaveBeenCalledOnce()
    button('Organize…').click()
    expect(actions.organizeLibrary).toHaveBeenCalledOnce()
    button('← Back').click()
    expect(actions.close).toHaveBeenCalledOnce()
  })

  it('shows the library folder', () => {
    const { host, panel } = page()
    expect(host.textContent).toContain('Not chosen yet')
    panel.setLibraryFolder(String.raw`D:\Books`)
    expect(host.textContent).toContain(String.raw`D:\Books`)
  })

  it('shows the file-type registration state', () => {
    const { host, panel, button } = page()
    panel.setFileTypesRegistered(true)
    expect(host.textContent).toContain('Registered for .epub, .fb2 and .fbz')
    expect(button('Unregister')).toBeDefined()
    panel.setFileTypesRegistered(false)
    expect(host.textContent).toContain('Not registered')
    expect(button('Register')).toBeDefined()
  })

  it('shows one section at a time, as tabs', () => {
    const { host, panel, button } = page()
    const shown = () => [...host.querySelectorAll<HTMLElement>('.section')].filter(s => !s.hidden).map(s => s.id)
    expect(shown()).toEqual(['settings-reading'])
    panel.showSection('catalogs')
    expect(shown()).toEqual(['settings-catalogs'])
    expect(button('Catalogs').getAttribute('aria-selected')).toBe('true')
    button('System').click()
    expect(shown()).toEqual(['settings-system'])
    expect(button('Catalogs').getAttribute('aria-selected')).toBe('false')
  })

  it('sets how many books Continue reading shows', () => {
    const { host, onChange, panel } = page()
    const select = host.querySelector<HTMLSelectElement>('select[aria-label="Books in Continue reading"]')
    if (!select) throw new Error('no history length')
    expect(select.value).toBe('3')
    select.value = '5'
    select.dispatchEvent(new Event('change'))
    expect(onChange).toHaveBeenCalledWith({ historyLength: 5 })
    panel.show({ ...initial, historyLength: 1 })
    expect(select.value).toBe('1')
  })
})
