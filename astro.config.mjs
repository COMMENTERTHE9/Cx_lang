// @ts-check
import { readFileSync } from 'node:fs';
import { defineConfig } from 'astro/config';

import tailwindcss from '@tailwindcss/vite';

import mdx from '@astrojs/mdx';

import vercel from '@astrojs/vercel';

const cxGrammar = JSON.parse(
  readFileSync(new URL('./src/lib/cx.tmLanguage.json', import.meta.url), 'utf8')
);

// https://astro.build/config
export default defineConfig({
  markdown: {
    shikiConfig: {
      langs: [cxGrammar]
    }
  },

  vite: {
    plugins: [tailwindcss()]
  },

  integrations: [mdx()],
  adapter: vercel()
});
