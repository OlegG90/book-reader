import { defineConfig } from 'vite'

export default defineConfig({
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    // The Rust side (its target/ holds gigabytes of build output), the docs and
    // the sample books aren't the page: watching them slows the start down.
    watch: { ignored: ['**/src-tauri/**', '**/docs/**', '**/samples/**'] },
  },
  // The app's one page. Without this, the dev server's dependency scan also
  // crawls every other .html in the repo (docs/design's reference included)
  // and holds the first page load for tens of seconds.
  optimizeDeps: { entries: ['index.html'] },
  build: {
    target: 'es2022',
    outDir: 'dist',
    emptyOutDir: true,
  },
})
