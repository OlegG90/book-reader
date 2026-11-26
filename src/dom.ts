/** The page's element with `id`, typed as the caller expects (the page always has it). */
export const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T
