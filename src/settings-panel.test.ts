// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest'
import { DEFAULT_APPEARANCE } from 'reader-core'
import { createAppearancePanel, createSettingsPage, type ModuleInfo, type PanelValues } from './settings-panel'

const initial: PanelValues = { ...DEFAULT_APPEARANCE, disabledModules: [], savePositions: true, offerLibrary: true, volumeKeys: false, historyLength: 3, uiFont: 'classic' }

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

function page(found: Promise<{ latest: string; newer: boolean }> = Promise.resolve({ latest: '0.7.0', newer: false })) {
  const host = document.createElement('section')
  const onChange = vi.fn()
  const actions = {
    toggleModule: vi.fn(),
    clearHistory: vi.fn(),
    toggleFileTypes: vi.fn(),
    changeLibrary: vi.fn(),
    organizeLibrary: vi.fn(),
    close: vi.fn(),
    openRepository: vi.fn(),
    checkForUpdate: vi.fn(() => found),
    openLatestRelease: vi.fn(),
  }
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

const fakeModules: ModuleInfo[] = [
  { id: 'format:epub', kind: 'format', label: 'X-Epub', hint: 'X-Epub books.', extensions: ['epub'], needsNetwork: false, optional: true, enabled: true, inUse: false, inUseReason: null },
  { id: 'format:fb2', kind: 'format', label: 'X-Fb2', hint: 'X-Fb2 books.', extensions: ['fb2'], needsNetwork: false, optional: true, enabled: false, inUse: false, inUseReason: null },
  { id: 'capability:catalogs', kind: 'capability', label: 'X-Catalogs', hint: 'X-Browse.', extensions: [], needsNetwork: true, optional: true, enabled: true, inUse: true, inUseReason: 'wipe the shelves first' },
  { id: 'capability:file-types', kind: 'capability', label: 'X-Types', hint: 'X-Open.', extensions: [], needsNetwork: false, optional: false, enabled: true, inUse: false, inUseReason: null },
]

function moduleBox(host: HTMLElement, label: string) {
  const row = [...host.querySelectorAll('label.row')].find(l => l.textContent?.includes(label))
  const box = row?.querySelector('input')
  if (!box) throw new Error(`no module "${label}"`)
  return box as HTMLInputElement
}

describe('modules section', () => {
  it('renders the backend modules grouped by kind', () => {
    const { host, panel } = page()
    panel.showSection('modules')
    panel.setModules(fakeModules)
    // Group titles are the page's chrome; module names come from the backend.
    expect(host.textContent).toContain('Formats')
    expect(host.textContent).toContain('Capabilities')
    expect(host.textContent).toContain('X-Epub')
    expect(host.textContent).toContain('X-Fb2 books.')
    expect(moduleBox(host, 'X-Epub').checked).toBe(true)
    expect(moduleBox(host, 'X-Fb2').checked).toBe(false)
  })

  it('toggles a module off and back on', () => {
    const { host, actions, panel } = page()
    panel.setModules(fakeModules)
    const box = moduleBox(host, 'X-Epub')
    box.checked = false
    box.dispatchEvent(new Event('change'))
    expect(actions.toggleModule).toHaveBeenCalledWith('format:epub', true)
    box.checked = true
    box.dispatchEvent(new Event('change'))
    expect(actions.toggleModule).toHaveBeenCalledWith('format:epub', false)
  })

  it('locks a module that is in use and says why', () => {
    const { host, panel } = page()
    panel.setModules(fakeModules)
    expect(moduleBox(host, 'X-Catalogs').disabled).toBe(true)
    expect(host.textContent).toContain('wipe the shelves first')
  })

  it('locks a built-in module', () => {
    const { host, panel } = page()
    panel.setModules(fakeModules)
    expect(moduleBox(host, 'X-Types').disabled).toBe(true)
    expect(host.textContent).toContain('Built in')
  })

  it('follows disabledModules on show', () => {
    const { host, panel } = page()
    panel.setModules(fakeModules.map(m => ({ ...m, enabled: true })))
    panel.show({ ...initial, disabledModules: ['format:epub'] })
    expect(moduleBox(host, 'X-Epub').checked).toBe(false)
    expect(moduleBox(host, 'X-Fb2').checked).toBe(true)
  })

  it('is a tab of its own', () => {
    const { host, panel, button } = page()
    panel.showSection('modules')
    const shown = [...host.querySelectorAll<HTMLElement>('.section')].filter(s => !s.hidden).map(s => s.id)
    expect(shown).toEqual(['settings-modules'])
    expect(button('Modules').getAttribute('aria-selected')).toBe('true')
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

  it('hides a section tab', () => {
    const { button, panel } = page()
    panel.setSectionVisible('catalogs', false)
    expect(button('Catalogs').hidden).toBe(true)
    panel.setSectionVisible('catalogs', true)
    expect(button('Catalogs').hidden).toBe(false)
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

  it('opens the repository from About', () => {
    const { actions, button } = page()
    button('Open on GitHub').click()
    expect(actions.openRepository).toHaveBeenCalled()
  })

  it('says when this is the latest version', async () => {
    const { host, button } = page()
    button('Check for updates').click()
    await vi.waitFor(() => expect(host.textContent).toContain('This is the latest version.'))
    expect(button('Open the release').hidden).toBe(true)
  })

  it('offers the release page for a newer version', async () => {
    const { host, actions, button } = page(Promise.resolve({ latest: '0.8.0', newer: true }))
    button('Check for updates').click()
    await vi.waitFor(() => expect(host.textContent).toContain('Version 0.8.0 is out.'))
    button('Open the release').click()
    expect(actions.openLatestRelease).toHaveBeenCalled()
  })

  it('shows why the check failed', async () => {
    const { host, button } = page(Promise.reject('Cannot reach GitHub: check the connection'))
    button('Check for updates').click()
    await vi.waitFor(() => expect(host.textContent).toContain('Cannot reach GitHub'))
    expect(button('Check for updates').disabled).toBe(false)
  })
})
