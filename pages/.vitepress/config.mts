import { defineConfig } from 'vitepress'

export default defineConfig({
  title: 'DeeLip',
  description: 'A lightweight, MicroSIP-inspired SIP softphone for Linux',
  // Required for a GitHub Pages *project* page — this repo publishes at
  // https://smyrnis.github.io/DeeLip/, not the domain root, so every
  // generated asset/link needs this prefix. Case must match the repo name.
  base: '/DeeLip/',
  // Light theme only — no dark/light toggle, no prefers-color-scheme detection.
  appearance: false,
  head: [['link', { rel: 'icon', href: '/DeeLip/icon.png' }]],

  themeConfig: {
    // Renders bold + underlined top-left, doubling as the "Home" link —
    // mirrors the "MicroSIP Home" first item in a MicroSIP-style top bar.
    siteTitle: 'DeeLip Home',

    nav: [
      { text: 'Downloads', link: '/downloads/' },
      { text: 'Documentation', link: '/docs/README' },
      { text: 'Changelog', link: '/docs/changelogs/CHANGELOG' },
      { text: 'FAQ', link: '/faq/' },
      { text: 'Troubleshooting', link: '/troubleshooting/' },
      { text: 'Contact', link: '/contact/' },
    ],

    // Only shown under /docs/ — Downloads/FAQ/Troubleshooting/Contact stay
    // clean single pages with just the top bar, matching the MicroSIP feel.
    sidebar: {
      '/docs/': [
        {
          text: 'Starting',
          items: [
            { text: 'Install', link: '/docs/install/install' },
            { text: 'Uninstall', link: '/docs/install/uninstall' },
            { text: 'Health check', link: '/docs/install/health-check' },
          ],
        },
        {
          text: 'Using DeeLip',
          items: [
            { text: 'Calling & security', link: '/docs/guide/calling-security' },
            { text: 'Audio & video quality', link: '/docs/guide/audio-video' },
            { text: 'Your data & privacy', link: '/docs/guide/data-privacy' },
            { text: 'Working behind your router (NAT)', link: '/docs/guide/nat' },
            { text: 'The interface', link: '/docs/guide/interface' },
            { text: 'Staying up to date', link: '/docs/guide/updates' },
            { text: 'Language support', link: '/docs/guide/language' },
          ],
        },
        {
          text: 'Changelog',
          items: [{ text: 'Changelog', link: '/docs/changelogs/CHANGELOG' }],
        },
      ],
    },

    socialLinks: [{ icon: 'github', link: 'https://github.com/Smyrnis/DeeLip' }],
  },
})
