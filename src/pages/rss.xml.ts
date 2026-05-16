import rss from "@astrojs/rss";
import { getCollection } from "astro:content";
import { SITE } from "../lib/site";

export async function GET(context: { site?: URL }) {
  const posts = (await getCollection("blog"))
    .filter((post) => !post.data.draft)
    .sort((a, b) => b.data.date.getTime() - a.data.date.getTime());

  return rss({
    title: "Cx Blog",
    description: "Cx release notes, build notes, and guides.",
    site: context.site ?? SITE.url,
    customData: "<language>en-us</language>",
    items: posts.map((post) => ({
      title: post.data.title,
      description: post.data.description ?? post.data.title,
      pubDate: post.data.date,
      link: `/blog/${post.slug}`,
      categories: [post.data.kind ?? "note"].filter(Boolean),
    })),
  });
}
