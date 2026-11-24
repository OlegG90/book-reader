// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createSourcesPanel } from './sources-panel'
import type { Source } from './catalog'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

const source = (over: Partial<Source> = {}): Source => ({
  id: '1',
  name: 'CWA',
  url: 'https://books.example/opds',
  user: '',
  sync: false,
  hasPassword: false,
  ...over,
})

function page(syncOff = false, cloudReady = false) {
  const host = document.createElement('div')
  const panel = createSourcesPanel(host, { syncOff: () => syncOff, cloudReady: () => cloudReady })
  return { host, panel }
}

describe('sources panel sync state', () => {
  beforeEach(() => {
    invokeMock.mockReset()
  })

  it('says when sync is turned off, without the badge', async () => {
    invokeMock.mockResolvedValue([source({ sync: true })])
    const { host, panel } = page(true)
    await panel.refresh()
    expect(host.textContent).toContain('Progress sync is turned off.')
    expect(host.querySelector('.badge')).toBeNull()
  })

  it('shows the badge for a sync source and stays quiet', async () => {
    invokeMock.mockResolvedValue([source({ sync: true })])
    const { host, panel } = page()
    await panel.refresh()
    expect(host.textContent).toContain('sync')
    expect(host.textContent).not.toContain('Progress sync is turned off.')
    expect(host.textContent).not.toContain('Nothing to sync through')
  })

  it('stays quiet when a cloud carries the sync', async () => {
    invokeMock.mockResolvedValue([source()])
    const { host, panel } = page(false, true)
    await panel.refresh()
    expect(host.textContent).not.toContain('Nothing to sync through')
  })

  it('says when nothing can sync', async () => {
    invokeMock.mockResolvedValue([source()])
    const { host, panel } = page()
    await panel.refresh()
    expect(host.textContent).toContain('Nothing to sync through')
    expect(host.querySelector('.badge')).toBeNull()
  })

  it('clears its own line when sync comes back', async () => {
    invokeMock.mockResolvedValue([source()])
    const host = document.createElement('div')
    let off = true
    const panel = createSourcesPanel(host, { syncOff: () => off, cloudReady: () => false })
    await panel.refresh()
    expect(host.textContent).toContain('Progress sync is turned off.')
    off = false
    invokeMock.mockResolvedValue([source({ sync: true })])
    await panel.refresh()
    expect(host.textContent).not.toContain('Progress sync is turned off.')
  })
})
