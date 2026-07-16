import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'DeeLip',
  description: 'A lightweight, SIP softphone for Linux, Windows, and macOS',
  // Required for a GitHub Pages *project* page — this repo publishes at
  // https://smyrnis.github.io/DeeLip/, not the domain root, so every
  // generated asset/link needs this prefix. Case must match the repo name.
  base: '/DeeLip/',
  // Light theme only — no dark/light toggle, no prefers-color-scheme detection.
  appearance: false,
  head: [['link', { rel: 'icon', href: '/DeeLip/icon.png' }]],

  themeConfig: {
    // Renders bold + underlined top-left, doubling as the "Home" link —
    // the first item in the top bar.
    siteTitle: 'DeeLip Home',

    nav: [
      { text: 'Downloads', link: '/downloads/' },
      { text: 'Changelog', link: '/changelog/' },
      { text: 'FAQ', link: '/faq/' },
      { text: 'Troubleshooting', link: '/troubleshooting/' },
      { text: 'Contact', link: '/contact/' },
    ],

    // This site is a showcase/marketing page for the app, not a documentation
    // book — every route is a clean single page with just the top bar, no
    // sidebar. In-depth engineering notes live in `docs/crates/` in the repo
    // instead (linked directly to GitHub where relevant), not published here.

    socialLinks: [{ icon: 'github', link: 'https://github.com/Smyrnis/DeeLip' }],
  },
})
