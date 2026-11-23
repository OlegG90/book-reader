// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createSyncManager, timeAgo } from './sync-manager'

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
  })

  const manager = () =>
    createSyncManager({
      book: () => null,
      fingerprint: () => 'fp',
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

  it('pull and jump resolve quietly with no book', async () => {
    const sync = manager()
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    await expect(sync.pull()).resolves.toBeUndefined()
    await expect(sync.jump()).resolves.toBeUndefined()
    expect(document.getElementById('sync-bar')!.hidden).toBe(true)
    warn.mockRestore()
  })
})
