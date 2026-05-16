// @ts-check
import { readFileSync } from 'node:fs';
import { defineConfig } from 'astro/config';

import tailwindcss from '@tailwindcss/vite';

import mdx from '@astrojs/mdx';

import vercel from '@astrojs/vercel';
import sitemap from '@astrojs/sitemap';

const cxGrammar = JSON.parse(
  readFileSync(new URL('./src/lib/cx.tmLanguage.json', import.meta.url), 'utf8')
);

// https://astro.build/config
export default defineConfig({
  site: 'https://cx-lang.com',

  markdown: {
    shikiConfig: {
      langs: [cxGrammar]
    }
  },

  vite: {
    plugins: [tailwindcss()]
  },

  integrations: [mdx(), sitemap()],
  adapter: vercel()
});
