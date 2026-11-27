import { el } from 'reader-core'

/** A book deleted from this library that the clouds still hold. */
export interface RemovedBook {
  fingerprint: string
  /** Its library path, as the cloud recorded it. */
  name: string
  size: number
}

/** What the Removed page asks the app to do with its books. */
export interface RemovedActions {
  /** Removes books from every cloud, for every device. */
  removeFromClouds(fingerprints: string[]): void
  /** Brings books back into this library from the clouds. */
  restore(fingerprints: string[]): void
}

/**
 * The Removed page: the books deleted from this library that the clouds still
 * hold. Each comes back with Download again, or leaves every cloud with
 * Remove from clouds.
 */
export function createRemovedView(host: HTMLElement, actions: RemovedActions) {
  const hint = el('p', { className: 'muted' })
  const removeAll = el('button', { type: 'button', textContent: 'Remove all from clouds' }) as HTMLButtonElement
  const list = el('ul', { className: 'removed-books' })
  host.replaceChildren(el('h2', { textContent: 'Removed' }), hint, el('p', {}, removeAll), list)

  /** Lists `books`; with none, says how books get here. */
  function show(books: RemovedBook[]) {
    const n = books.length
    hint.textContent = n
      ? `${n === 1 ? 'A book' : `${n} books`} deleted from this library, still in the clouds. Download again brings a book back; removing it from the clouds takes it off every device.`
      : 'Books deleted from this library show here while the clouds still hold them: from here they come back, or leave every cloud.'
    removeAll.hidden = n === 0
    removeAll.onclick = () => actions.removeFromClouds(books.map(b => b.fingerprint))
    list.replaceChildren(
      ...books.map(book =>
        el(
          'li',
          {},
          el('span', { className: 'name', textContent: book.name }),
          el(
            'span',
            { className: 'actions' },
            el('button', { type: 'button', textContent: 'Download again', onclick: () => actions.restore([book.fingerprint]) }),
            el('button', { type: 'button', textContent: 'Remove from clouds', onclick: () => actions.removeFromClouds([book.fingerprint]) }),
          ),
        ),
      ),
    )
  }

  show([])
  return { show }
}
