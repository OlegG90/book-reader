import { FONTS, LINE_HEIGHT, LINE_LENGTH, THEMES, el, stepFontSize, type Appearance, type Flow, type Font, type Range, type Theme } from 'reader-core'

/** Everything the settings edit. */
export interface PanelValues extends Appearance {
  savePositions: boolean
  /** Offer to copy or move books opened from outside the library into it. */
  offerLibrary: boolean
  /** Android: the volume keys turn pages. */
  volumeKeys: boolean
  /** How many books "Continue reading" shows (1–5). */
  historyLength: number
  /** The interface's font: "classic" (Lora, Cormorant Garamond for headings) or one of the system fonts. */
  uiFont: UiFont
}

/** The interface fonts: the bundled pair, or one font everywhere. */
export const UI_FONTS = ['classic', 'system', 'Georgia', 'Cambria', 'Segoe UI', 'Calibri'] as const
/** Windows' fonts: a phone doesn't have them, so it isn't offered them. */
const WINDOWS_FONTS: readonly UiFont[] = ['Georgia', 'Cambria', 'Segoe UI', 'Calibri']
export type UiFont = (typeof UI_FONTS)[number]

/** The choices for `historyLength`. */
export const HISTORY_LENGTHS = [1, 2, 3, 4, 5] as const

/** Buttons in the settings page that act instead of changing a value. */
export interface PanelActions {
  /** Forgets the recent books list (reading positions are kept). */
  clearHistory(): void
  /** Registers or unregisters the app for its book file types. */
  toggleFileTypes(): void
  /** Picks another library folder. */
  changeLibrary(): void
  /** Renames the library's books to follow the naming pattern (after a preview). */
  organizeLibrary(): void
  /** Closes the settings page. */
  close(): void
  /** Opens the app's source on GitHub in the browser. */
  openRepository(): void
  /** Asks GitHub for the latest release: its version, and whether it is newer than this copy. */
  checkForUpdate(): Promise<{ latest: string; newer: boolean }>
  /** Opens the latest release's page, with its files, in the browser. */
  openLatestRelease(): void
}

const THEME_LABELS: Record<Theme, string> = { system: 'System', light: 'Light', dark: 'Dark', sepia: 'Sepia' }
const fontLabel = (font: Font) => (font === 'publisher' ? 'Publisher default' : font)

function field(label: string, ...controls: HTMLElement[]) {
  return el('div', { className: 'field' }, el('span', { className: 'label' }, label), ...controls)
}

function range({ min, max, step }: Range) {
  return el('input', { type: 'range', min: String(min), max: String(max), step: String(step) })
}

/** What the appearance controls offer besides the appearance itself. */
interface AppearanceExtras {
  /** Paginated / scrolled, for the open book. */
  flow?(flow: Flow): void
  /** Opens the settings page ("All settings…"). */
  allSettings?(): void
}

/**
 * The appearance controls: theme swatches, font and size, line spacing and
 * length, and (with `extras`) the book's flow and a way to all settings. Used
 * in the Aa pop-up while reading and on the settings page. Every change goes
 * to `onChange` with just the changed values; `show` refreshes the controls.
 */
export function createAppearancePanel(
  host: HTMLElement,
  initial: Appearance,
  onChange: (changes: Partial<Appearance>) => void,
  extras: AppearanceExtras = {},
) {
  let current = initial

  // Swatches: style.css draws each theme's page with "Aa" above the name.
  const themeButtons = THEMES.map(theme => {
    const button = el('button', { type: 'button', className: 'swatch', textContent: THEME_LABELS[theme], onclick: () => onChange({ theme }) })
    button.dataset.theme = theme
    return button
  })
  const font = el('select', { ariaLabel: 'Font' }, ...FONTS.map(f => el('option', { value: f, textContent: fontLabel(f) })))
  font.onchange = () => onChange({ font: font.value as Font })

  const size = el('output')
  const sizeButton = (text: string, direction: 1 | -1, label: string) =>
    el('button', { type: 'button', textContent: text, ariaLabel: label, onclick: () => onChange({ fontSize: stepFontSize(current.fontSize, direction) }) })

  const lineHeight = range(LINE_HEIGHT)
  const lineHeightValue = el('output')
  lineHeight.oninput = () => onChange({ lineHeight: Number(lineHeight.value) })

  const lineLength = range(LINE_LENGTH)
  const lineLengthValue = el('output')
  lineLength.oninput = () => onChange({ lineLength: Number(lineLength.value) })

  host.append(
    field('Theme', el('div', { className: 'swatches' }, ...themeButtons)),
    el(
      'div',
      { className: 'font-row' },
      font,
      el('div', { className: 'stepper' }, sizeButton('A−', -1, 'Smaller'), size, sizeButton('A+', 1, 'Bigger')),
    ),
    field('Line spacing', lineHeight, lineHeightValue),
    field('Line length', lineLength, lineLengthValue),
  )

  const flows = ['paginated', 'scrolled'] as const
  const flowButtons = flows.map(flow =>
    el('button', { type: 'button', textContent: flow === 'paginated' ? 'Paginated' : 'Scrolled', onclick: () => extras.flow?.(flow) }),
  )
  if (extras.flow) host.append(el('div', { className: 'segmented' }, ...flowButtons))
  if (extras.allSettings) {
    host.append(
      el(
        'p',
        { className: 'all-settings' },
        el('button', { type: 'button', className: 'link', textContent: 'All settings…', onclick: extras.allSettings }),
        el('span', { className: 'muted', textContent: 'S · Ctrl+,' }),
      ),
    )
  }

  /** Refreshes the controls; `flow` is the open book's (none in the library). */
  function show(values: Appearance, flow?: Flow) {
    current = values
    themeButtons.forEach((button, i) => button.setAttribute('aria-pressed', String(THEMES[i] === values.theme)))
    font.value = values.font
    size.textContent = String(values.fontSize)
    lineHeight.value = String(values.lineHeight)
    lineHeightValue.textContent = values.lineHeight.toFixed(1)
    lineLength.value = String(values.lineLength)
    lineLengthValue.textContent = `≈${values.lineLength} chars`
    flowButtons.forEach((b, i) => b.setAttribute('aria-pressed', String(flows[i] === flow)))
    flowButtons.forEach(b => (b.disabled = !flow))
  }

  show(initial)
  return { show }
}

/** A settings row's parts: what it is (and a hint: text, or an element the page updates) on the left, its controls on the right. */
function rowParts(label: string | Node, hint: string | HTMLElement, controls: HTMLElement[]) {
  const hintEl = typeof hint === 'string' ? el('span', { className: 'hint', textContent: hint }) : hint
  return [el('div', { className: 'row-text' }, el('span', { className: 'row-label' }, label), hintEl), el('div', { className: 'row-controls' }, ...controls)]
}

/** A settings row: what it is and a hint on the left, its controls on the right. */
export function row(label: string | Node, hint: string | HTMLElement, ...controls: HTMLElement[]) {
  return el('div', { className: 'row' }, ...rowParts(label, hint, controls))
}

/** A checkbox row: the whole row toggles it. */
function checkRow(label: string, hint: string, box: HTMLInputElement) {
  return el('label', { className: 'row' }, ...rowParts(label, hint, [box]))
}

const SECTIONS = [
  { id: 'reading', title: 'Reading' },
  { id: 'catalogs', title: 'Catalogs' },
  { id: 'system', title: 'System' },
  { id: 'shortcuts', title: 'Shortcuts', desktopOnly: true },
  { id: 'about', title: 'About' },
] as const

export type SettingsSection = (typeof SECTIONS)[number]['id']

/**
 * The settings page: a back button, the sections' names on the left as tabs,
 * and one section at a time (Reading with the appearance, Catalogs, System
 * with the library folder, Shortcuts). Every change goes to `onChange` with just the changed values;
 * button presses go to `actions`. `catalogs`, `shortcuts` and `about` are
 * the hosts the app fills in (the sources panel, the key list, the version).
 */
export function createSettingsPage(host: HTMLElement, initial: PanelValues, onChange: (changes: Partial<PanelValues>) => void, actions: PanelActions) {
  const appearance = el('div', { className: 'appearance' })
  const appearancePanel = createAppearancePanel(appearance, initial, onChange)

  const checkbox = (key: 'savePositions' | 'offerLibrary' | 'volumeKeys') => {
    const box = el('input', { type: 'checkbox' })
    box.onchange = () => onChange({ [key]: box.checked })
    return box
  }
  const savePositions = checkbox('savePositions')
  const offerLibrary = checkbox('offerLibrary')
  const volumeKeys = checkbox('volumeKeys')

  const libraryFolder = el('span', { className: 'hint path' })
  const fileTypesStatus = el('span', { className: 'hint' })
  const fileTypesButton = el('button', { type: 'button', onclick: actions.toggleFileTypes })
  const catalogs = el('div', { className: 'catalogs' })
  const shortcuts = el('div', { className: 'shortcuts' })
  const about = el('div', { className: 'about' })
  const updateStatus = el('span', { className: 'hint', textContent: 'Asks GitHub for the latest release.' })
  const openRelease = el('button', { type: 'button', textContent: 'Open the release', hidden: true, onclick: actions.openLatestRelease })
  const checkUpdate = el('button', { type: 'button', textContent: 'Check for updates', onclick: () => void checkForUpdate() })

  const libraryRow = row(
    'Library folder',
    libraryFolder,
    el('button', { type: 'button', textContent: 'Change…', onclick: actions.changeLibrary }),
    el('button', { type: 'button', textContent: 'Organize…', onclick: actions.organizeLibrary }),
  )
  const fileTypesRow = row('File types', fileTypesStatus, fileTypesButton)
  const historyLength = el(
    'select',
    { ariaLabel: 'Books in Continue reading' },
    ...HISTORY_LENGTHS.map(n => el('option', { value: String(n), textContent: n === 1 ? '1 book' : `${n} books` })),
  )
  historyLength.onchange = () => onChange({ historyLength: Number(historyLength.value) })
  const uiFont = el(
    'select',
    { ariaLabel: 'Interface font' },
    ...UI_FONTS.map(f =>
      el('option', {
        value: f,
        textContent: f === 'classic' ? 'Classical (Lora)' : f === 'system' ? 'System' : f,
        className: WINDOWS_FONTS.includes(f) ? 'desktop-only' : '',
      }),
    ),
  )
  uiFont.onchange = () => onChange({ uiFont: uiFont.value as UiFont })
  // The volume keys are a phone's.
  const volumeKeysRow = checkRow('Volume keys turn pages', 'While a book is open.', volumeKeys)
  volumeKeysRow.classList.add('android-only')

  const content: Record<SettingsSection, HTMLElement[]> = {
    reading: [
      appearance,
      checkRow('Remember reading position', 'Positions sync through the catalog marked "sync".', savePositions),
      volumeKeysRow,
    ],
    catalogs: [catalogs],
    system: [
      libraryRow,
      checkRow('Offer to add books opened from elsewhere', 'Shows Move / Copy to library when a file is outside the library folder.', offerLibrary),
      fileTypesRow,
      row('Interface font', 'The library and the panels; the book has its own (Reading).', uiFont),
      row('Continue reading', 'How many of the last books read it shows.', historyLength),
      row('History', 'Forgets the recent books list; reading positions are kept.', el('button', { type: 'button', textContent: 'Clear history', onclick: actions.clearHistory })),
    ],
    shortcuts: [shortcuts],
    about: [
      about,
      row('Source code', 'Book Reader on GitHub.', el('button', { type: 'button', textContent: 'Open on GitHub', onclick: actions.openRepository })),
      row('Updates', updateStatus, openRelease, checkUpdate),
    ],
  }

  const sections = SECTIONS.map(({ id, title }) =>
    el('section', { id: `settings-${id}`, className: 'section' }, el('h2', { textContent: title }), ...content[id]),
  )
  const nav = SECTIONS.map(({ id, title, ...rest }) =>
    el('button', {
      type: 'button',
      role: 'tab',
      textContent: title,
      className: 'desktopOnly' in rest ? 'desktop-only' : '',
      onclick: () => showSection(id),
    }),
  )
  sections.forEach(section => section.setAttribute('role', 'tabpanel'))

  host.append(
    el(
      'header',
      { className: 'page-header' },
      el('button', { type: 'button', className: 'back', textContent: '← Back', onclick: actions.close }),
      el('h1', { textContent: 'Settings' }),
    ),
    el('div', { className: 'page-body' }, el('nav', { className: 'sections', ariaLabel: 'Sections', role: 'tablist' }, ...nav), el('div', { className: 'page-content' }, ...sections)),
  )

  /** Shows one section (a tab) and marks it in the list. */
  function showSection(id: SettingsSection) {
    SECTIONS.forEach((section, i) => {
      const current = section.id === id
      nav[i].setAttribute('aria-selected', String(current))
      sections[i].hidden = !current
    })
  }

  function show(values: PanelValues) {
    appearancePanel.show(values)
    savePositions.checked = values.savePositions
    offerLibrary.checked = values.offerLibrary
    volumeKeys.checked = values.volumeKeys
    historyLength.value = String(values.historyLength)
    uiFont.value = values.uiFont
  }

  /** Asks for the latest release and says what it found; a newer one gets a button to its page. */
  async function checkForUpdate() {
    checkUpdate.disabled = true
    updateStatus.textContent = 'Asking GitHub…'
    try {
      const found = await actions.checkForUpdate()
      updateStatus.textContent = found.newer ? `Version ${found.latest} is out.` : 'This is the latest version.'
      openRelease.hidden = !found.newer
    } catch (e) {
      updateStatus.textContent = String(e)
    } finally {
      checkUpdate.disabled = false
      // Disabling the button dropped the keyboard focus: put it on what comes next.
      ;(openRelease.hidden ? checkUpdate : openRelease).focus()
    }
  }

  function setFileTypesRegistered(registered: boolean) {
    fileTypesStatus.textContent = registered ? 'Registered for .epub, .fb2 and .fbz' : 'Not registered'
    fileTypesButton.textContent = registered ? 'Unregister' : 'Register'
  }

  function setLibraryFolder(folder: string | null) {
    libraryFolder.textContent = folder ?? 'Not chosen yet'
    libraryFolder.title = folder ?? ''
  }

  show(initial)
  setFileTypesRegistered(false)
  setLibraryFolder(null)
  showSection('reading')
  return { show, setFileTypesRegistered, setLibraryFolder, showSection, catalogs, shortcuts, about }
}
