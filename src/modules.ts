/** The modules' ids as the backend names them (`modules::id` in src-tauri/src/modules.rs). */
export const MODULE = {
  epub: 'format:epub',
  fb2: 'format:fb2',
  catalogs: 'capability:catalogs',
  progressSync: 'capability:progress-sync',
  organize: 'capability:organize',
  fileTypes: 'capability:file-types',
  updateCheck: 'capability:update-check',
  dropbox: 'capability:dropbox',
  googleDrive: 'capability:googledrive',
} as const
