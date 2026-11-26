import { invoke } from '@tauri-apps/api/core'
import type { BookView } from 'reader-core'
import { $ } from './dom'

/** How often a changed position goes to the sync server while reading. */
export const SYNC_EVERY_MS = 60_000
/** Closing the window waits this long at most for the last push. */
export const SYNC_ON_CLOSE_MS = 3_000

export interface SyncOffer {
  cfi: string
  fraction: number
  device: string
  /** When the other device was there (ms since the Unix epoch). */
  time: number
}

export interface SyncDeps {
  book: () => BookView | null
  fingerprint: () => string
  /** The progress-sync module is turned off: never touch the network. */
  syncOff: () => boolean
}

export interface SyncManager {
  /** Asks the sync server whether another device read on, and offers to jump there. */
  pull(): Promise<void>
  /** Hands the position to the sync engine if it moved; the engine sends it
  and retries each transport that fails on its own. */
  push(): Promise<void>
  /** Jumps to the offered position, falling back to its share read. */
  jump(): Promise<void>
  /** Hides the offer without jumping. */
  hide(): void
  /** The position moved: the next push sends it. */
  markDirty(): void
  /** Forgets the book: no offer, nothing to push. */
  reset(): void
}

/** "2 h ago", "yesterday"... */
export function timeAgo(time: number) {
  const minutes = Math.round((Date.now() - time) / 60_000)
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes} min ago`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours} h ago`
  const days = Math.round(hours / 24)
  return days === 1 ? 'yesterday' : `${days} days ago`
}

/** Keeps the open book's sync offer and whether its position moved since it was last handed to the engine. */
export function createSyncManager(deps: SyncDeps): SyncManager {
  let offer: SyncOffer | null = null
  /** The open book's id on the sync server; null when it doesn't sync. */
  let document: string | null = null
  /** The position moved since it was last handed to the engine. */
  let dirty = false

  async function pull() {
    if (deps.syncOff()) return
    const book = deps.book()
    const fingerprint = deps.fingerprint()
    const synced = await invoke<string | null>('sync_document').catch(e => {
      console.warn('sync:', e)
      return null
    })
    if (!synced || !book) return
    document = synced
    const state = await invoke<{ offer: SyncOffer | null; push: boolean }>('sync_pull', { document, fingerprint }).catch(e => {
      console.warn('sync:', e)
      return null
    })
    if (!state || !deps.book() || document !== synced) return
    if (state.push) dirty = true
    if (!state.offer) return
    offer = state.offer
    $('sync-text').textContent = `${offer.device || 'Another device'}, ${timeAgo(offer.time)}: ${Math.round(offer.fraction * 100)}%`
    $('sync-bar').hidden = false
  }

  async function push() {
    if (deps.syncOff()) return
    const location = deps.book()?.view.lastLocation
    const synced = document
    const fingerprint = deps.fingerprint()
    if (!dirty || !location || !synced || !fingerprint) return
    dirty = false
    try {
      await invoke('sync_queue_position', { document: synced, fingerprint, cfi: location.cfi, fraction: location.fraction })
    } catch (e) {
      // Handed over next time, unless another book has opened meanwhile.
      if (document === synced) dirty = true
      console.warn('sync:', e)
    }
  }

  async function jump() {
    const fell = offer
    hide()
    const book = deps.book()
    if (!fell || !book) return
    // The same file on both devices, so the CFI should resolve; the share read is the fallback.
    await book.view.goTo(fell.cfi).catch(() => book?.view.goToFraction(fell.fraction))
  }

  function hide() {
    offer = null
    $('sync-bar').hidden = true
  }

  return {
    pull,
    push,
    jump,
    hide,
    markDirty: () => {
      dirty = true
    },
    reset: () => {
      document = null
      dirty = false
      hide()
    },
  }
}
