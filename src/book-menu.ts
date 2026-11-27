import { el } from 'reader-core'

/** One choice in a pop-up menu. */
export interface MenuItem {
  label: string
  action(): void
  /** Destroys something (Delete…): drawn in the danger colour. */
  danger?: boolean
  /** One of a set of choices: whether it is the chosen one (a check mark shows). */
  checked?: boolean
  /** A short note at the end of the line, such as a count. */
  detail?: string
}

/** Closes the open menu, if any. */
let closeOpen: (() => void) | null = null

/**
 * A small menu at `at` (window coordinates, kept on screen): a press outside
 * it or Esc closes it, a choice closes it and acts.
 */
export function showMenu(at: { x: number; y: number }, items: MenuItem[]) {
  closeOpen?.()
  const menu = el(
    'div',
    { className: 'popup-menu', role: 'menu' },
    ...items.map(item =>
      el(
        'button',
        {
          type: 'button',
          role: item.checked == null ? 'menuitem' : 'menuitemradio',
          ariaChecked: item.checked == null ? null : String(item.checked),
          className: item.danger ? 'danger' : '',
          onclick: () => {
            close()
            item.action()
          },
        },
        item.label,
        ...(item.detail ? [el('span', { className: 'detail', textContent: item.detail })] : []),
      ),
    ),
  )
  document.body.append(menu)
  const { width, height } = menu.getBoundingClientRect()
  menu.style.left = `${Math.max(8, Math.min(at.x, innerWidth - width - 8))}px`
  menu.style.top = `${Math.max(8, Math.min(at.y, innerHeight - height - 8))}px`

  const outside = (e: PointerEvent) => {
    if (!menu.contains(e.target as Node)) close()
  }
  const keys = (e: KeyboardEvent) => {
    if (e.key !== 'Escape') return
    e.stopPropagation()
    close()
  }
  const close = () => {
    menu.remove()
    document.removeEventListener('pointerdown', outside, true)
    document.removeEventListener('keydown', keys, true)
    closeOpen = null
  }
  document.addEventListener('pointerdown', outside, true)
  document.addEventListener('keydown', keys, true)
  closeOpen = close
  menu.querySelector('button')?.focus()
}

/** Closes the open menu or question (Back on the phone; Esc they handle themselves); false when there was none. */
export function closePopup() {
  if (closeOpen) closeOpen()
  else if (asking) asking.dispatchEvent(new Event('cancel'))
  else return false
  return true
}

/** The open question, if any. */
let asking: HTMLDialogElement | null = null

/**
 * Asks before something that can't be taken back from inside the app:
 * resolves true for `confirm`, false for Cancel or Esc.
 */
export function confirmDialog({ title, text, confirm }: { title: string; text: string; confirm: string }): Promise<boolean> {
  return new Promise(resolve => {
    const dialog = el('dialog', { className: 'confirm' })
    const done = (yes: boolean) => {
      asking = null
      dialog.close()
      dialog.remove()
      resolve(yes)
    }
    dialog.append(
      el('h2', { textContent: title }),
      el('p', { className: 'muted', textContent: text }),
      el(
        'p',
        { className: 'buttons' },
        el('button', { type: 'button', className: 'danger', textContent: confirm, onclick: () => done(true) }),
        el('button', { type: 'button', textContent: 'Cancel', onclick: () => done(false) }),
      ),
    )
    dialog.addEventListener('cancel', e => {
      e.preventDefault()
      done(false)
    })
    document.body.append(dialog)
    dialog.showModal()
    asking = dialog
    // Cancel first: the safe choice under the keyboard.
    dialog.querySelectorAll('button')[1]?.focus()
  })
}

/** Long press (a finger held still) on `target`: `action` at the finger; the click that follows is swallowed. */
export function onLongPress(target: HTMLElement, action: (at: { x: number; y: number }) => void) {
  const HOLD_MS = 500
  const SLACK_PX = 10
  let timer: number | undefined
  let start = { x: 0, y: 0 }
  let fired = false
  let touching = false
  const cancel = () => clearTimeout(timer)
  const fire = () => {
    cancel()
    if (fired) return
    fired = true
    action(start)
  }
  target.addEventListener('pointerdown', e => {
    fired = false
    touching = e.pointerType === 'touch'
    if (!touching) return
    start = { x: e.clientX, y: e.clientY }
    timer = window.setTimeout(fire, HOLD_MS)
  })
  target.addEventListener('pointermove', e => {
    if (Math.hypot(e.clientX - start.x, e.clientY - start.y) > SLACK_PX) cancel()
  })
  target.addEventListener('pointerup', cancel)
  target.addEventListener('pointercancel', () => {
    cancel()
    // No click follows a cancelled press: nothing to swallow.
    fired = false
  })
  // The WebView's own long-press "context menu" (before or after the timer):
  // the same one press, so the menu shows once, and the target's own handler doesn't run.
  target.addEventListener(
    'contextmenu',
    e => {
      if (!touching) return
      e.preventDefault()
      e.stopImmediatePropagation()
      fire()
    },
    true,
  )
  target.addEventListener(
    'click',
    e => {
      if (!fired) return
      fired = false
      e.preventDefault()
      e.stopImmediatePropagation()
    },
    true,
  )
}
