/** How far (px) and how fast (ms) a finger must go sideways to count as a swipe. */
const SWIPE_MIN_PX = 60
const SWIPE_MAX_MS = 600
/** The screen's edges belong to the system's back gesture. */
const EDGE_PX = 24

/** Where a touch landed and when (ms). */
export interface TouchPoint {
  x: number
  y: number
  time: number
}

/** Which way a swipe goes: to the next panel (the finger moves left) or the previous one. */
export type SwipeDirection = 'next' | 'prev'

/**
 * Which way a touch from `start` to `end` swiped, if it did: clearly
 * sideways (half again as far as up or down), far and quick enough, and not
 * begun at the screen's edges, where the system's back gesture lives.
 */
export function swipeDirection(start: TouchPoint, end: TouchPoint, width: number): SwipeDirection | null {
  const dx = end.x - start.x
  const dy = end.y - start.y
  if (start.x < EDGE_PX || start.x > width - EDGE_PX) return null
  if (end.time - start.time > SWIPE_MAX_MS || Math.abs(dx) < SWIPE_MIN_PX || Math.abs(dx) < 1.5 * Math.abs(dy)) return null
  return dx < 0 ? 'next' : 'prev'
}

/**
 * Whether a touch that starts on `target` is its own: a field, a slider, or
 * anything that scrolls sideways, up to (not including) `within`, the
 * element that listens for swipes.
 */
export function ownsSideways(target: Element | null, within: Element = document.body): boolean {
  for (let node = target; node && node !== within; node = node.parentElement) {
    if (node.matches('input, select, textarea, [contenteditable="true"]')) return true
    if (node.scrollWidth > node.clientWidth && /auto|scroll/.test(getComputedStyle(node).overflowX)) return true
  }
  return false
}

/** Calls `swiped` for a finger's swipe that starts on `target`, while `enabled()` says so. */
export function onSwipe(target: HTMLElement, enabled: () => boolean, swiped: (direction: SwipeDirection) => void): void {
  let start: TouchPoint | null = null
  target.addEventListener('pointerdown', e => {
    const own = e.pointerType === 'touch' && enabled() && !ownsSideways(e.target as Element, target)
    start = own ? { x: e.clientX, y: e.clientY, time: e.timeStamp } : null
  })
  target.addEventListener('pointerup', e => {
    const from = start
    start = null
    if (!from || e.pointerType !== 'touch') return
    const direction = swipeDirection(from, { x: e.clientX, y: e.clientY, time: e.timeStamp }, innerWidth)
    if (direction) swiped(direction)
  })
  target.addEventListener('pointercancel', () => (start = null))
}

/** The item after or before `current` in `order`; `null` at either end (no wrap-around). */
export function neighbour<T>(order: readonly T[], current: T, direction: SwipeDirection): T | null {
  const i = order.indexOf(current)
  if (i < 0) return null
  return order[direction === 'next' ? i + 1 : i - 1] ?? null
}
