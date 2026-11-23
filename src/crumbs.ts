import { el } from 'reader-core'

/** Breadcrumbs for `titles`: each but the last a step back to it (`stepTo(index)`), the last the page shown. */
export function crumbs(titles: string[], stepTo: (index: number) => void) {
  return titles.flatMap((title, i) => {
    const crumb =
      i === titles.length - 1
        ? el('span', { className: 'here', textContent: title, ariaCurrent: 'page' })
        : el('button', { type: 'button', textContent: title, onclick: () => stepTo(i) })
    return i ? [el('span', { className: 'sep', textContent: '›', ariaHidden: 'true' }), crumb] : [crumb]
  })
}
