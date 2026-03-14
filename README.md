# Cx Site

Official website and documentation site for Cx, built with Astro.

This repo contains:

- the marketing home page
- the docs pages under `/docs`
- the blog under `/blog`
- the shared design system, theme tokens, and layout components

## Stack

- Astro
- MDX content collections
- Tailwind CSS v4
- Vercel adapter

## Commands

Run everything from the project root:

| Command | What it does |
| --- | --- |
| `pnpm install` | Install dependencies |
| `pnpm dev` | Start the local dev server on `localhost:4321` |
| `pnpm build` | Build the production site into `dist/` |
| `pnpm preview` | Preview the production build locally |
| `pnpm astro sync` | Regenerate Astro content types |

## Project Structure

```text
/
├── public/                 # static assets
├── src/
│   ├── components/         # shared UI pieces
│   ├── content/
│   │   ├── blog/           # blog MDX files
│   │   ├── docs/           # docs MDX files
│   │   └── config.ts       # content schemas
│   ├── layouts/            # site and docs layouts
│   ├── lib/                # site constants
│   ├── pages/              # Astro routes
│   └── styles/             # global theme and typography
├── astro.config.mjs
└── package.json
```

## Content

Docs and blog posts are driven by Astro content collections:

- docs live in `src/content/docs`
- blog posts live in `src/content/blog`
- schemas are defined in `src/content/config.ts`

After changing collection schemas, run:

```sh
pnpm astro sync
```

## Theme

The site uses CSS variables from `src/styles/global.css` for:

- dark and light theme
- contrast modes
- typography tokens
- shared colors and surfaces

The accessibility menu in the header controls theme state client-side.

## Deployment

The site is deployed through Vercel.

Current workflow:

- work is pushed to the `site` branch
- Vercel deploys from that branch

If a push lands on GitHub but the site does not update, trigger a fresh deploy by pushing a new commit to `site`.
