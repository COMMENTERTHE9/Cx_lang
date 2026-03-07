import { defineCollection, z } from "astro:content";

const docs = defineCollection({
  type: "content",
  schema: z.object({
    title: z.string(),
    description: z.string().optional(),
    order: z.number().optional(),
    section: z.string().optional(),
  }),
});

const blog = defineCollection({
  type: "content",
  schema: z.object({
    title: z.string(),
    description: z.string().optional(),
    date: z.coerce.date(),
    draft: z.boolean().optional().default(false),

    // NEW
    kind: z.enum(["release", "note", "guide"]).optional().default("note"),
    version: z.string().optional(), // e.g. "v0.8.0"
  }),
});

export const collections = { docs, blog };
