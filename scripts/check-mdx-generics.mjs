#!/usr/bin/env node
// Guard: fail the build if any blog MDX body has a bare `<Foo>`-style generic
// outside inline code or fenced code blocks. MDX v3 parses those as unclosed
// JSX tags, which is what breaks Vercel deploys (see AUDIT.md and the
// 2026-06-06 incident). This runs as `prebuild`, before Astro reads MDX.

import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, relative } from "node:path";

const BLOG_DIR = "src/content/blog";
// Pattern MDX misreads as JSX: identifier immediately followed by `<…>`.
// Deliberately dumb: no attempt to understand valid JSX, just the shape that
// bites us (Handle<T>, Result<T>, Handle<str>, Result<T, E>, Vec<Type>,
// Box<SemanticType>, etc.). Lowercase-first type params (str, i64, u8) and
// multi-param forms (T, E) are both real Cx generics, so the inner character
// class allows any identifier-ish run, not just a single capitalized word.
const BARE_GENERIC = /[A-Za-z_][A-Za-z0-9_]*<[A-Za-z_][A-Za-z0-9_,\s]*>/;

function listMdx(dir) {
  return readdirSync(dir)
    .filter((f) => f.endsWith(".mdx"))
    .map((f) => join(dir, f))
    .filter((p) => statSync(p).isFile());
}

function scan(path) {
  const text = readFileSync(path, "utf8");
  const lines = text.split(/\r?\n/);

  let inFrontmatter = false;
  let seenFrontmatter = false;
  let inFence = false;
  const hits = [];

  for (let i = 0; i < lines.length; i++) {
    const raw = lines[i];

    // YAML frontmatter: first --- opens, next --- closes. Skip both.
    if (raw.trim() === "---") {
      if (!seenFrontmatter) {
        inFrontmatter = true;
        seenFrontmatter = true;
        continue;
      }
      if (inFrontmatter) {
        inFrontmatter = false;
        continue;
      }
    }
    if (inFrontmatter) continue;

    // Fenced code blocks: toggle on any line starting with ```
    if (/^```/.test(raw)) {
      inFence = !inFence;
      continue;
    }
    if (inFence) continue;

    // Strip inline code spans (`...`) — those are safe.
    const stripped = raw.replace(/`[^`]*`/g, "");
    const m = stripped.match(BARE_GENERIC);
    if (m) {
      // Report the column from the ORIGINAL line so the user can find it.
      const col = raw.indexOf(m[0]);
      hits.push({
        line: i + 1,
        col: col >= 0 ? col + 1 : 1,
        token: m[0],
        snippet: raw.trim().slice(0, 160),
      });
    }
  }

  return hits;
}

const files = listMdx(BLOG_DIR);
let total = 0;

for (const path of files) {
  const hits = scan(path);
  if (!hits.length) continue;
  total += hits.length;
  const rel = relative(process.cwd(), path).replaceAll("\\", "/");
  for (const h of hits) {
    console.error(
      `${rel}:${h.line}:${h.col}  bare generic '${h.token}' — wrap in backticks: \`${h.token}\``,
    );
    console.error(`  ${h.snippet}`);
  }
}

if (total > 0) {
  console.error("");
  console.error(
    `check-mdx-generics: ${total} bare generic${total === 1 ? "" : "s"} in blog MDX. ` +
      `MDX v3 parses '<T>' as an unclosed JSX tag and the build will fail on Vercel. ` +
      `Wrap each token in backticks.`,
  );
  process.exit(1);
}

console.log(`check-mdx-generics: ${files.length} file${files.length === 1 ? "" : "s"} clean.`);
