import { getCollection } from "astro:content";
import { SITE } from "../lib/site";

const routes = ["/", "/blog", "/docs"];

function escapeXml(value: string) {
  return value
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;")
    .replace(/'/g, "&apos;");
}

function absolutePath(path: string) {
  return new URL(path, SITE.url).toString();
}

export async function GET() {
  const docs = await getCollection("docs");
  const posts = (await getCollection("blog")).filter((post) => !post.data.draft);

  const urls = [
    ...routes,
    ...docs.map((doc) => `/docs/${doc.slug}`),
    ...posts.map((post) => `/blog/${post.slug}`),
  ].sort();

  const body = `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">
${urls.map((url) => `  <url><loc>${escapeXml(absolutePath(url))}</loc></url>`).join("\n")}
</urlset>`;

  return new Response(body, {
    headers: {
      "Content-Type": "application/xml; charset=utf-8",
    },
  });
}
