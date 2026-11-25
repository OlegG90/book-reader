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
    connectCloud: vi.fn(),
    disconnectCloud: vi.fn(),
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
  { id: 'capability:dropbox', kind: 'capability', label: 'X-Dropbox', hint: 'X-Cloud.', extensions: [], needsNetwork: true, optional: true, enabled: false, inUse: false, inUseReason: null },
]

function moduleBox(host: HTMLElement, label: string) {
  const row = [...host.querySelectorAll('label.row')].find(l => l.textContent?.includes(label))
  const box = row?.querySelector('input')
  if (!box) throw new Error(`no module "${label}"`)
  return box as HTMLInputElement
}

function sectionStrip(host: HTMLElement) {
  const strip = host.querySelector<HTMLElement>('.sections')
  if (!strip) throw new Error('no section strip')
  return strip
}

function stubStrip(strip: HTMLElement, metrics: { scrollWidth: number; clientWidth: number; scrollLeft?: number }) {
  Object.defineProperties(strip, {
    scrollWidth: { value: metrics.scrollWidth, configurable: true },
    clientWidth: { value: metrics.clientWidth, configurable: true },
  })
  strip.scrollLeft = metrics.scrollLeft ?? 0
  strip.dispatchEvent(new Event('scroll'))
}

function stripArrows(host: HTMLElement) {
  const [prev, next] = [...host.querySelectorAll<HTMLButtonElement>('.sections-arrow')]
  if (!prev || !next) throw new Error('no strip arrows')
  return { prev, next }
}

describe('section strip arrows', () => {
  it('stays hidden when every section fits', () => {
    const { host } = page()
    stubStrip(sectionStrip(host), { scrollWidth: 300, clientWidth: 300 })
    const { prev, next } = stripArrows(host)
    expect(prev.hidden).toBe(true)
    expect(next.hidden).toBe(true)
  })

  it('shows an arrow on each edge that hides more sections', () => {
    const { host } = page()
    const strip = sectionStrip(host)
    stubStrip(strip, { scrollWidth: 600, clientWidth: 300, scrollLeft: 0 })
    let arrows = stripArrows(host)
    expect(arrows.prev.hidden).toBe(true)
    expect(arrows.next.hidden).toBe(false)
    stubStrip(strip, { scrollWidth: 600, clientWidth: 300, scrollLeft: 150 })
    arrows = stripArrows(host)
    expect(arrows.prev.hidden).toBe(false)
    expect(arrows.next.hidden).toBe(false)
    stubStrip(strip, { scrollWidth: 600, clientWidth: 300, scrollLeft: 300 })
    arrows = stripArrows(host)
    expect(arrows.prev.hidden).toBe(false)
    expect(arrows.next.hidden).toBe(true)
  })

  it('scrolls the strip when an arrow is pressed', () => {
    const { host } = page()
    const strip = sectionStrip(host)
    stubStrip(strip, { scrollWidth: 600, clientWidth: 300, scrollLeft: 0 })
    const scrollBy = vi.fn()
    strip.scrollBy = scrollBy
    const { prev, next } = stripArrows(host)
    next.click()
    expect(scrollBy).toHaveBeenCalledWith(expect.objectContaining({ left: 240 }))
    stubStrip(strip, { scrollWidth: 600, clientWidth: 300, scrollLeft: 300 })
    prev.click()
    expect(scrollBy).toHaveBeenCalledWith(expect.objectContaining({ left: -240 }))
  })

  it('rechecks when a tab hides', () => {
    const { host, panel } = page()
    const strip = sectionStrip(host)
    stubStrip(strip, { scrollWidth: 600, clientWidth: 300, scrollLeft: 0 })
    expect(stripArrows(host).next.hidden).toBe(false)
    stubStrip(strip, { scrollWidth: 300, clientWidth: 300, scrollLeft: 0 })
    panel.setSectionVisible('catalogs', false)
    expect(stripArrows(host).next.hidden).toBe(true)
  })
})

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
    expect(moduleBox(host, 'X-Dropbox').checked).toBe(false)
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
    const { actions, button, panel } = page()
    button('Clear history').click()
    expect(actions.clearHistory).toHaveBeenCalledOnce()
    button('Register').click()
    expect(actions.toggleFileTypes).toHaveBeenCalledOnce()
    button('Connect').click()
    expect(actions.connectCloud).toHaveBeenCalledWith('dropbox')
    panel.setCloudConnected('dropbox', true, null)
    button('Disconnect').click()
    expect(actions.disconnectCloud).toHaveBeenCalledOnce()
    button('Change…').click()
    expect(actions.changeLibrary).toHaveBeenCalledOnce()
    button('Organize…').click()
    expect(actions.organizeLibrary).toHaveBeenCalledOnce()
    button('← Back').click()
    expect(actions.close).toHaveBeenCalledOnce()
  })

  it('groups the System rows into Library, History and Sync', () => {
    const { host, panel } = page()
    panel.showSection('system')
    const section = host.querySelector('#settings-system')
    if (!section) throw new Error('no system section')
    const groups = [...section.querySelectorAll('.group')]
    expect(groups.map(g => g.querySelector('h3')?.textContent)).toEqual(['Library', 'History', 'Sync'])
    const rowsOf = (title: string) => {
      const found = groups.find(g => g.querySelector('h3')?.textContent === title)
      if (!found) throw new Error(`no group "${title}"`)
      return [...found.querySelectorAll('.row-label')].map(l => l.textContent)
    }
    expect(rowsOf('Library')).toEqual(['Library folder', 'Offer to add books opened from elsewhere'])
    expect(rowsOf('History')).toEqual(['Continue reading', 'History'])
    expect(rowsOf('Sync')).toEqual(['Cloud connection', 'Add cloud'])
  })

  it('connects the picked cloud', () => {
    const { host, actions, button } = page()
    const select = host.querySelector<HTMLSelectElement>('select[aria-label="Cloud to connect"]')
    if (!select) throw new Error('no cloud select')
    select.value = 'google'
    select.dispatchEvent(new Event('change'))
    expect(button('Connect').disabled).toBe(false)
    button('Connect').click()
    expect(actions.connectCloud).toHaveBeenCalledWith('google')
  })

  it('locks a cloud whose module is off', () => {
    const { host, panel, button } = page()
    panel.show({ ...initial, disabledModules: ['capability:googledrive'] })
    const select = host.querySelector<HTMLSelectElement>('select[aria-label="Cloud to connect"]')
    if (!select) throw new Error('no cloud select')
    const google = [...select.options].find(o => o.value === 'google')
    expect(google?.disabled).toBe(true)
    select.value = 'google'
    select.dispatchEvent(new Event('change'))
    expect(button('Connect').disabled).toBe(true)
    expect(host.textContent).toContain('Turn on Google Drive in Modules to connect.')
  })

  it('shows the library folder', () => {
    const { host, panel } = page()
    expect(host.textContent).toContain('Not chosen yet')
    panel.setLibraryFolder(String.raw`D:\Books`)
    expect(host.textContent).toContain(String.raw`D:\Books`)
  })

  it('shows the cloud connection state', () => {
    const { host, panel, button } = page()
    expect(host.textContent).toContain('Not connected')
    panel.setCloudConnected('dropbox', true, null)
    expect(host.textContent).toContain('Dropbox')
    panel.setCloudConnected('dropbox', true, 'Dropbox upload failed (401)')
    expect(host.textContent).toContain('Dropbox upload failed (401)')
    expect(button('Disconnect')).toBeDefined()
    panel.setCloudVisible(false)
    expect(button('Disconnect').closest<HTMLElement>('.row')?.hidden).toBe(true)
    panel.setCloudVisible(true)
    expect(button('Disconnect').closest<HTMLElement>('.row')?.hidden).toBe(false)
  })

  it('locks the cloud picker on unavailable clouds and while connected', () => {
    const { host, panel, actions, button } = page()
    const select = host.querySelector<HTMLSelectElement>('select[aria-label="Cloud to connect"]')
    if (!select) throw new Error('no cloud select')
    // Only OneDrive waits for its transport; the rest connect.
    expect([...select.options].map(o => o.textContent)).toEqual(['Dropbox', 'OneDrive (soon)', 'Google Drive'])
    select.value = 'onedrive'
    select.dispatchEvent(new Event('change'))
    expect(button('Connect').disabled).toBe(true)
    expect(host.textContent).toContain('Coming soon')
    select.value = 'dropbox'
    select.dispatchEvent(new Event('change'))
    expect(button('Connect').disabled).toBe(false)
    // While a cloud is connected the picker locks until disconnect.
    panel.setCloudConnected('dropbox', true, null)
    expect(button('Connect').disabled).toBe(true)
    expect(select.disabled).toBe(true)
    expect(host.textContent).toContain('Disconnect to switch clouds.')
    button('Connect').click()
    expect(actions.connectCloud).not.toHaveBeenCalled()
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

  it('hides the capability rows', () => {
    const { button, panel } = page()
    panel.setOrganizeVisible(false)
    expect(button('Organize…').hidden).toBe(true)
    panel.setOrganizeVisible(true)
    expect(button('Organize…').hidden).toBe(false)
    panel.setFileTypesVisible(false)
    expect(button('Register').closest<HTMLElement>('.row')?.hidden).toBe(true)
    panel.setFileTypesVisible(true)
    expect(button('Register').closest<HTMLElement>('.row')?.hidden).toBe(false)
    panel.setUpdatesVisible(false)
    expect(button('Check for updates').closest<HTMLElement>('.row')?.hidden).toBe(true)
    panel.setUpdatesVisible(true)
    expect(button('Check for updates').closest<HTMLElement>('.row')?.hidden).toBe(false)
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
