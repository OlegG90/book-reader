import { invoke } from '@tauri-apps/api/core'
import { fetchSources, type Source } from './catalog'
import { el } from 'reader-core'

/**
 * The Catalogs part of the settings: the OPDS servers, with a form to add or
 * change one. Passwords go straight to the backend, which keeps them in the
 * OS credential store; they never come back.
 */
export function createSourcesPanel(host: HTMLElement) {
  const list = el('ul', { className: 'sources' })
  const status = el('p', { className: 'muted source-status' })
  const add = el('button', { type: 'button', textContent: 'Add catalog…', onclick: () => edit(undefined) })
  const form = el('form', { className: 'source-form', hidden: true })
  host.append(el('span', { className: 'label', textContent: 'Catalogs' }), list, add, form, status)

  function input(label: string, props: object) {
    const field = el('input', props)
    return { field, row: el('label', { className: 'stacked' }, label, field) }
  }

  async function refresh() {
    const sources = await fetchSources()
    list.replaceChildren(
      ...sources.map(source =>
        el(
          'li',
          {},
          el(
            'span',
            { className: 'name' },
            source.name,
            source.sync ? el('span', { className: 'badge', textContent: 'sync', title: 'Syncs reading progress' }) : '',
          ),
          el('span', { className: 'url muted', textContent: source.url }),
          el(
            'span',
            { className: 'actions' },
            el('button', { type: 'button', textContent: 'Edit', onclick: () => edit(source) }),
            el('button', { type: 'button', textContent: 'Remove', onclick: () => void remove(source) }),
          ),
        ),
      ),
    )
  }

  function edit(source: Source | undefined) {
    const name = input('Name', { value: source?.name ?? '', placeholder: 'Calibre-Web' })
    const url = input('Server address (its OPDS catalog is found by itself)', {
      value: source?.url ?? '',
      placeholder: 'http://192.168.1.10:8083',
      required: true,
      type: 'url',
    })
    const user = input('User name', { value: source?.user ?? '', autocomplete: 'username' })
    const password = input('Password', {
      type: 'password',
      autocomplete: 'current-password',
      placeholder: source?.hasPassword ? 'Unchanged' : '',
    })
    const sync = el('input', { type: 'checkbox', checked: source?.sync ?? false })
    const formMessage = el('p', { className: 'muted' })
    form.replaceChildren(
      name.row,
      url.row,
      user.row,
      password.row,
      el('label', { className: 'check' }, sync, 'Sync reading progress (KOSync, Calibre-Web-Automated)'),
      formMessage,
      el(
        'p',
        { className: 'buttons' },
        el('button', { type: 'submit', textContent: 'Save' }),
        el('button', { type: 'button', textContent: 'Cancel', onclick: close }),
      ),
    )
    form.onsubmit = async e => {
      e.preventDefault()
      formMessage.textContent = 'Looking for the catalog…'
      try {
        const saved = await invoke<{ id: string; warning: string | null }>('save_catalog_source', {
          input: {
            id: source?.id ?? null,
            name: name.field.value,
            url: url.field.value,
            user: user.field.value,
            password: password.field.value || null,
            sync: sync.checked,
          },
        })
        // The list follows through catalogs-changed.
        close()
        status.textContent = saved.warning ?? ''
      } catch (err) {
        formMessage.textContent = String(err)
      }
    }
    form.hidden = false
    add.hidden = true
    status.textContent = ''
    name.field.focus()
  }

  function close() {
    form.hidden = true
    form.replaceChildren()
    add.hidden = false
  }

  async function remove(source: Source) {
    await invoke('delete_catalog_source', { id: source.id }).catch(e => (status.textContent = String(e)))
  }

  void refresh()
  return { refresh }
}
