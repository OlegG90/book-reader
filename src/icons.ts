import { el } from 'reader-core'

/** One of index.html's sprite icons (Lucide), as an element to put in a button. */
export function icon(name: string) {
  return el('span', { className: 'icon-wrap', innerHTML: `<svg class="icon" aria-hidden="true"><use href="#i-${name}"/></svg>` })
}
