// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { pollOauthAnswer } from './platform'

vi.mock('@tauri-apps/api/core', () => ({ addPluginListener: vi.fn(), invoke: vi.fn() }))
vi.mock('@tauri-apps/api/app', () => ({ onBackButtonPress: vi.fn() }))

describe('pollOauthAnswer', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  const tick = (ms = 1500) => vi.advanceTimersByTimeAsync(ms)

  it('finishes on the first answer and stops', async () => {
    const take = vi.fn<() => Promise<string | null>>().mockResolvedValueOnce(null).mockResolvedValueOnce('url')
    const finish = vi.fn<() => Promise<void>>().mockResolvedValue(undefined)
    pollOauthAnswer(take, finish)
    await tick()
    expect(take).toHaveBeenCalledTimes(1)
    expect(finish).not.toHaveBeenCalled()
    await tick()
    expect(take).toHaveBeenCalledTimes(2)
    expect(finish).toHaveBeenCalledTimes(1)
    expect(finish).toHaveBeenCalledWith('url')
    await tick()
    await tick()
    expect(take).toHaveBeenCalledTimes(2)
  })

  it('gives up after the misses run out', async () => {
    const take = vi.fn<() => Promise<string | null>>().mockResolvedValue(null)
    const finish = vi.fn<() => Promise<void>>().mockResolvedValue(undefined)
    pollOauthAnswer(take, finish, 3)
    await tick()
    await tick()
    await tick()
    expect(take).toHaveBeenCalledTimes(3)
    expect(finish).not.toHaveBeenCalled()
    await tick()
    await tick()
    expect(take).toHaveBeenCalledTimes(3)
  })

  it('stops early on the returned stopper', async () => {
    const take = vi.fn<() => Promise<string | null>>().mockResolvedValue(null)
    const finish = vi.fn<() => Promise<void>>().mockResolvedValue(undefined)
    const stop = pollOauthAnswer(take, finish)
    await tick()
    stop()
    await tick()
    await tick()
    expect(take).toHaveBeenCalledTimes(1)
    expect(finish).not.toHaveBeenCalled()
  })

  it('counts a failed take as a miss', async () => {
    const take = vi.fn<() => Promise<string | null>>().mockRejectedValue(new Error('no backend'))
    const finish = vi.fn<() => Promise<void>>().mockResolvedValue(undefined)
    pollOauthAnswer(take, finish, 2)
    await tick()
    await tick()
    expect(take).toHaveBeenCalledTimes(2)
    expect(finish).not.toHaveBeenCalled()
    await tick()
    expect(take).toHaveBeenCalledTimes(2)
  })
})
