import { z } from "zod";

const MAX_MEMORY_NAME_BYTES = 128;
const MAX_MEMORY_CONTENT_BYTES = 64 * 1024;

const utf8Length = (value: string) => new TextEncoder().encode(value).byteLength;

const AutomationMemoryNameSchema = z
  .string()
  .regex(/^[A-Za-z0-9][A-Za-z0-9._-]*\.md$/, "must be an ASCII Markdown basename")
  .refine((name) => utf8Length(name) <= MAX_MEMORY_NAME_BYTES, "must be at most 128 bytes");

export const AutomationMemorySummarySchema = z.object({
  name: AutomationMemoryNameSchema,
  revision: z.number().int().positive(),
  updated_at: z.string().min(1),
});

export const AutomationMemoryFileSchema = AutomationMemorySummarySchema.extend({
  content: z
    .string()
    .refine((content) => utf8Length(content) <= MAX_MEMORY_CONTENT_BYTES, "must be at most 64 KiB"),
});

export const ListAutomationMemoriesResponseSchema = z.object({
  items: z.array(AutomationMemorySummarySchema),
});
