import { el } from 'reader-core'
import { crumbs } from './crumbs'
import { $ } from './dom'
import { groupSeries, type LibraryBook } from './library'

/** The Series page: the library's series as cards, or one series' books in reading order. */
export function createSeriesView(deps: {
  library: () => LibraryBook[]
  bookCard: (book: LibraryBook) => HTMLElement
  cover: (title: string, coverKey: string | null, fraction: number) => HTMLElement
  scrollTop: () => void
}) {
  /** The series open; null shows the list of them. */
  let open: string | null = null

  function render() {
    const groups = groupSeries(deps.library())
    const group = groups.find(g => g.name === open)
    if (!group) open = null
    $('series-crumbs').replaceChildren(...crumbs(group ? ['Series', group.name] : ['Series'], () => go(null)))
    $('series-empty').hidden = groups.length > 0
    $('series-list').hidden = !!group
    $('series-books').hidden = !group
    $('series-books').replaceChildren(...(group?.books ?? []).map(deps.bookCard))
    if (group) return
    $('series-list').replaceChildren(
      ...groups.map(g => {
        const n = g.books.length
        const first = g.books.find(b => b.hasCover) ?? g.books[0]
        return el(
          'li',
          {},
          el(
            'button',
            { type: 'button', onclick: () => go(g.name) },
            deps.cover(g.name, first.hasCover ? first.key : null, g.read / n),
            el('span', { className: 'title', textContent: g.name }),
            el('span', { className: 'meta', textContent: `${n} book${n === 1 ? '' : 's'} · ${g.read} read` }),
          ),
        )
      }),
    )
  }

  function go(name: string | null) {
    open = name
    render()
    deps.scrollTop()
  }

  return {
    /** Draws the page (again, for a changed library). */
    show: render,
    /** Esc / Back: from a series to the list; false when the list shows. */
    back() {
      if (!open) return false
      go(null)
      return true
    },
  }
}
