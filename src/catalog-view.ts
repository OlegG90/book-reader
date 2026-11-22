import { invoke } from '@tauri-apps/api/core'
import { FORMAT_LABELS, fetchSources, inLibrary, preferredFile, type CatalogBook, type CatalogEntry, type Feed, type Source } from './catalog'
import { el } from './dom'

/** A page the reader went to, kept so Back shows it again without a refetch. */
interface Page {
  title: string
  entries: CatalogEntry[]
  next: string | null
}

interface CatalogViewDeps {
  /** The library's books, to mark what is already there. */
  library(): { title: string; authors: string[] }[]
  /** Opens the settings, where catalogs are added. */
  openSettings(): void
}

/** The Catalogs tab: browse a catalog's folders, search it, download books into the library. */
export function createCatalogView(deps: CatalogViewDeps) {
  const $ = (id: string) => document.getElementById(id) as HTMLElement
  const sourceSelect = $('catalog-source') as HTMLSelectElement
  const search = $('catalog-search') as HTMLInputElement
  const back = $('catalog-back') as HTMLButtonElement
  const more = $('catalog-more') as HTMLButtonElement
  const list = $('catalog-list')

  let sources: Source[] = []
  let source: Source | undefined
  let pages: Page[] = []
  /** Ignores answers to requests that a later one has replaced. */
  let request = 0

  const message = (text: string, action?: HTMLElement) => {
    $('catalog-message').replaceChildren(text, ...(action ? [' ', action] : []))
    $('catalog-message').hidden = !text
  }

  async function loadSources() {
    sources = await fetchSources()
    sourceSelect.replaceChildren(...sources.map(s => el('option', { value: s.id, textContent: s.name })))
    sourceSelect.hidden = sources.length < 2
    if (!sources.length) {
      source = undefined
      pages = []
      render()
      message(
        'No catalogs yet.',
        el('button', { className: 'link', textContent: 'Add one in the settings', onclick: deps.openSettings }),
      )
      return
    }
    const kept = sources.find(s => s.id === source?.id)
    source = kept ?? sources[0]
    if (!kept) pages = []
    sourceSelect.value = source.id
    if (!pages.length) await open(null)
  }

  /** Opens a feed: the catalog's root (null), a folder, or a search. */
  async function open(href: string | null, query?: string) {
    if (!source) return
    const id = source.id
    const mine = ++request
    message('Loading…')
    try {
      const feed = query
        ? await invoke<Feed>('catalog_search', { id, query })
        : await invoke<Feed>('catalog_feed', { id, url: href })
      if (mine !== request) return
      const title = query ? `Search: ${query}` : feed.title || source.name
      pages.push({ title, entries: feed.entries, next: feed.next })
      message(feed.entries.length ? '' : 'Nothing here.')
      render()
    } catch (e) {
      if (mine !== request) return
      message(String(e))
    }
  }

  async function loadMore() {
    const page = pages.at(-1)
    if (!source || !page?.next) return
    const mine = ++request
    more.disabled = true
    try {
      const feed = await invoke<Feed>('catalog_feed', { id: source.id, url: page.next })
      if (mine !== request) return
      page.entries.push(...feed.entries)
      page.next = feed.next
      render()
    } catch (e) {
      if (mine === request) message(String(e))
    } finally {
      more.disabled = false
    }
  }

  function render() {
    const page = pages.at(-1)
    $('catalog-title').textContent = page?.title ?? ''
    back.hidden = pages.length < 2
    more.hidden = !page?.next
    list.replaceChildren(...(page?.entries ?? []).map(entryRow))
  }

  function entryRow(entry: CatalogEntry) {
    if (entry.kind === 'folder') {
      return el(
        'li',
        { className: 'folder' },
        el(
          'button',
          { onclick: () => void open(entry.href) },
          el('span', { className: 'title', textContent: entry.title }),
          el('span', { className: 'summary', textContent: entry.summary }),
        ),
      )
    }
    return bookRow(entry)
  }

  function bookRow(book: CatalogBook) {
    const file = preferredFile(book.files)
    const cover = el('div', { className: 'cover' })
    if (book.cover) loadCover(cover, book.cover)
    const action = el('button', { className: 'download' })
    // Only a hint (same title and author): downloading checks the content.
    const known = inLibrary(book, deps.library())
    if (file) {
      const label = `Download ${FORMAT_LABELS[file.format]}`
      action.textContent = label
      action.onclick = async () => {
        if (!source) return
        action.disabled = true
        action.textContent = 'Downloading…'
        try {
          const added = await invoke<{ path: string; existed: boolean }>('catalog_download', {
            id: source.id,
            url: file.href,
            format: file.format,
          })
          action.textContent = added.existed ? 'In library' : 'Downloaded'
          action.title = added.path
        } catch (e) {
          action.disabled = false
          action.textContent = label
          action.title = String(e)
          message(String(e))
        }
      }
    }
    return el(
      'li',
      { className: 'book' },
      cover,
      el(
        'div',
        { className: 'about' },
        el('span', { className: 'title', textContent: book.title }),
        el('span', { className: 'author', textContent: book.authors.join(', ') }),
        el('span', {
          className: 'meta',
          textContent: [...new Set(book.files.map(f => FORMAT_LABELS[f.format]))].join(' · ') + (known ? ' · in library' : ''),
        }),
      ),
      action,
    )
  }

  /** Covers come through the backend (with the catalog's login), as they scroll into view. */
  const covers = new IntersectionObserver(
    entries => {
      for (const entry of entries) {
        if (!entry.isIntersecting) continue
        const host = entry.target as HTMLElement
        covers.unobserve(host)
        invoke<ArrayBuffer>('catalog_image', { id: host.dataset.source, url: host.dataset.url })
          .then(bytes => {
            const src = URL.createObjectURL(new Blob([bytes]))
            host.replaceChildren(el('img', { src, alt: '', onload: () => URL.revokeObjectURL(src) }))
          })
          .catch(() => {})
      }
    },
    { rootMargin: '400px' },
  )

  function loadCover(host: HTMLElement, url: string) {
    if (!source) return
    host.dataset.source = source.id
    host.dataset.url = url
    covers.observe(host)
  }

  sourceSelect.addEventListener('change', () => {
    source = sources.find(s => s.id === sourceSelect.value)
    pages = []
    void open(null)
  })
  function goBack() {
    request++
    pages.pop()
    message('')
    render()
  }

  back.addEventListener('click', goBack)
  more.addEventListener('click', () => void loadMore())
  search.addEventListener('keydown', e => {
    if (e.key !== 'Enter' || !search.value.trim()) return
    void open(null, search.value.trim())
  })

  return {
    /** Shows the tab: reloads the sources (they may have changed in the settings). */
    show: loadSources,
    /** Back inside the catalog: true when there was a page to go back to. */
    back(): boolean {
      if (pages.length < 2) return false
      goBack()
      return true
    },
  }
}
