import { el } from 'reader-core'

/** One choice in a pop-up menu. */
export interface MenuItem {
  label: string
  action(): void
  /** Destroys something (Delete…): drawn in the danger colour. */
  danger?: boolean
}

let open: { menu: HTMLElement; close(): void } | null = null

/** Closes the pop-up menu, if one is open. */
export function closeMenu() {
  open?.close()
}

/**
 * A small menu at `at` (window coordinates, kept on screen): a press outside
 * it or Esc closes it, a choice closes it and acts.
 */
export function showMenu(at: { x: number; y: number }, items: MenuItem[]) {
  closeMenu()
  const menu = el(
    'div',
    { className: 'popup-menu', role: 'menu' },
    ...items.map(item =>
      el('button', {
        type: 'button',
        role: 'menuitem',
        className: item.danger ? 'danger' : '',
        textContent: item.label,
        onclick: () => {
          closeMenu()
          item.action()
        },
      }),
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
    open = null
  }
  document.addEventListener('pointerdown', outside, true)
  document.addEventListener('keydown', keys, true)
  open = { menu, close }
  menu.querySelector('button')?.focus()
}

/** Closes the open menu or question (Back on the phone; Esc they handle themselves); false when there was none. */
export function closePopup() {
  if (open) closeMenu()
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
  const cancel = () => clearTimeout(timer)
  target.addEventListener('pointerdown', e => {
    fired = false
    if (e.pointerType !== 'touch') return
    start = { x: e.clientX, y: e.clientY }
    timer = window.setTimeout(() => {
      fired = true
      action(start)
    }, HOLD_MS)
  })
  target.addEventListener('pointermove', e => {
    if (Math.hypot(e.clientX - start.x, e.clientY - start.y) > SLACK_PX) cancel()
  })
  target.addEventListener('pointerup', cancel)
  target.addEventListener('pointercancel', cancel)
  // A long press also brings the browser's own menu: not here.
  target.addEventListener('contextmenu', e => fired && e.preventDefault())
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
