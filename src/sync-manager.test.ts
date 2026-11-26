// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { BookView } from 'reader-core'
import { createSyncManager, timeAgo } from './sync-manager'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

describe('timeAgo', () => {
  it('says just now, minutes, hours and days', () => {
    const now = Date.now()
    expect(timeAgo(now)).toBe('just now')
    expect(timeAgo(now - 30 * 60_000)).toBe('30 min ago')
    expect(timeAgo(now - 3 * 3_600_000)).toBe('3 h ago')
    expect(timeAgo(now - 24 * 3_600_000)).toBe('yesterday')
    expect(timeAgo(now - 5 * 24 * 3_600_000)).toBe('5 days ago')
  })
})

describe('createSyncManager', () => {
  beforeEach(() => {
    document.body.innerHTML = '<div id="sync-bar" hidden><span id="sync-text"></span></div>'
    // No backend under jsdom: every bridge call fails, like Tauri's rejected promise.
    invokeMock.mockReset()
    invokeMock.mockImplementation(() => Promise.reject(new Error('no backend')))
  })

  const manager = (syncOff = false) =>
    createSyncManager({
      book: () => null,
      fingerprint: () => 'fp',
      syncOff: () => syncOff,
    })

  it('hides the offer bar', () => {
    document.getElementById('sync-bar')!.hidden = false
    const sync = manager()
    sync.hide()
    expect(document.getElementById('sync-bar')!.hidden).toBe(true)
  })

  it('reset forgets the book without touching the backend', async () => {
    const sync = manager()
    sync.markDirty()
    sync.reset()
    // No book, nothing dirty: pushing resolves quietly, without invoking.
    await expect(sync.push()).resolves.toBeUndefined()
    expect(document.getElementById('sync-bar')!.hidden).toBe(true)
  })

  it('hands a moved position to the engine once', async () => {
    const book = { view: { lastLocation: { cfi: 'a', fraction: 0.5 } } } as unknown as BookView
    const sync = createSyncManager({ book: () => book, fingerprint: () => 'fp', syncOff: () => false })
    invokeMock.mockImplementation((command: string) => {
      if (command === 'sync_document') return Promise.resolve('doc1')
      if (command === 'sync_pull') return Promise.resolve({ offer: null, push: false })
      if (command === 'sync_queue_position') return Promise.resolve()
      throw new Error(`unexpected ${command}`)
    })
    await sync.pull()
    sync.markDirty()
    await sync.push()
    // Nothing moved since: the engine already has it.
    await sync.push()
    const queued = invokeMock.mock.calls.filter(call => call[0] === 'sync_queue_position')
    expect(queued).toEqual([['sync_queue_position', { document: 'doc1', fingerprint: 'fp', cfi: 'a', fraction: 0.5 }]])
  })

  it('hands the position over again when the engine could not take it', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const book = { view: { lastLocation: { cfi: 'a', fraction: 0.5 } } } as unknown as BookView
    const sync = createSyncManager({ book: () => book, fingerprint: () => 'fp', syncOff: () => false })
    let refuse = true
    invokeMock.mockImplementation((command: string) => {
      if (command === 'sync_document') return Promise.resolve('doc1')
      if (command === 'sync_pull') return Promise.resolve({ offer: null, push: false })
      if (command === 'sync_queue_position') return refuse ? Promise.reject(new Error('store busy')) : Promise.resolve()
      throw new Error(`unexpected ${command}`)
    })
    await sync.pull()
    sync.markDirty()
    await sync.push()
    refuse = false
    await sync.push()
    expect(invokeMock.mock.calls.filter(call => call[0] === 'sync_queue_position')).toHaveLength(2)
    warn.mockRestore()
  })

  it('stays quiet when progress sync is turned off', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const sync = manager(true)
    sync.markDirty()
    await expect(sync.pull()).resolves.toBeUndefined()
    await expect(sync.push()).resolves.toBeUndefined()
    expect(warn).not.toHaveBeenCalled()
    expect(document.getElementById('sync-bar')!.hidden).toBe(true)
    warn.mockRestore()
  })

  it('pull and jump resolve quietly with no book', async () => {
    const sync = manager()
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    await expect(sync.pull()).resolves.toBeUndefined()
    await expect(sync.jump()).resolves.toBeUndefined()
    expect(document.getElementById('sync-bar')!.hidden).toBe(true)
    warn.mockRestore()
  })
})
