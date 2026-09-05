import { readFile, writeFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const contentRoot = join(root, "src/content/docs");
const publicRoot = join(root, "public");
const site = "https://docs.btt.maddiaa.com";
const checkOnly = process.argv.includes("--check");

const pages = [
  {
    source: "index.md",
    output: "overview.md",
    path: "/",
  },
  {
    source: "getting-started.md",
    output: "getting-started.md",
    path: "/getting-started/",
  },
  {
    source: "installation.md",
    output: "installation.md",
    path: "/installation/",
  },
  {
    source: "creating-an-extension.md",
    output: "creating-an-extension.md",
    path: "/creating-an-extension/",
  },
  {
    source: "installing-the-skill.md",
    output: "installing-the-skill.md",
    path: "/installing-the-skill/",
  },
];

function parsePage(raw, source) {
  const match = raw.match(/^---\n([\s\S]*?)\n---\n?/);
  if (!match) throw new Error(`${source}: missing frontmatter`);

  const field = (name) => {
    const value = match[1].match(new RegExp(`^${name}:\\s*(.+)$`, "m"))?.[1];
    if (!value) throw new Error(`${source}: missing ${name}`);
    return value.replace(/^(["'])(.*)\1$/, "$2");
  };

  return {
    title: field("title"),
    description: field("description"),
    body: normalizeMarkdown(raw.slice(match[0].length).trim()),
  };
}

function normalizeMarkdown(markdown) {
  return markdown
    .replace(
      /<!-- architecture:start -->[\s\S]*?<!-- architecture:end -->/,
      `**Architecture at a glance**

Inputs:

- \`.tree files\`: the tests that should exist
- \`btt.toml\`: active packs and check levels
- Language pack: file routing, name mapping, source extraction, and scaffold templates

The btt core parses the tree, finds the test file, and builds the expected test structure. \`btt scaffold\` renders a test skeleton; \`btt check\` compares the test code and reports differences.`,
    )
    .replace(/^```([\w-]+) title="[^"]+"$/gm, "```$1")
    .replace(/^:::(?:tip|note|caution|danger)(?:\[([^\]]+)\])?\n([\s\S]*?)\n:::$/gm, (_, title, body) => {
      const lines = body.split("\n").map((line) => `> ${line}`);
      return `> **${title ?? "Note"}**\n>\n${lines.join("\n")}`;
    });
}

async function emit(relativePath, content) {
  const path = join(publicRoot, relativePath);
  const normalized = `${content.trim()}\n`;
  if (checkOnly) {
    let existing;
    try {
      existing = await readFile(path, "utf8");
    } catch {
      throw new Error(`${relativePath} is missing; run pnpm generate:llms`);
    }
    if (existing !== normalized) {
      throw new Error(`${relativePath} is stale; run pnpm generate:llms`);
    }
    return;
  }
  await writeFile(path, normalized);
}

const rendered = [];
for (const page of pages) {
  const raw = await readFile(join(contentRoot, page.source), "utf8");
  const parsed = parsePage(raw, page.source);
  const markdown = `# ${parsed.title}\n\n${parsed.description}\n\n${parsed.body}`;
  rendered.push({ ...page, ...parsed, markdown });
  await emit(page.output, markdown);
}

const index = `# btt documentation

> Branch tree testing for any language: specify test structure in readable .tree files, scaffold the test skeleton, and keep code and specification in sync.

These are the canonical machine-readable entry points for btt. Fetch individual Markdown pages for focused context or the complete file for one-shot ingestion.

## Documentation

${rendered.map((page) => `- [${page.title}](${site}/${page.output}): ${page.description}`).join("\n")}

## Complete documentation

- [All btt documentation](${site}/llms-full.txt): Every page concatenated as Markdown.

## Project

- [GitHub repository](https://github.com/Maddiaa0/btt): Source, examples, built-in packs, and issue tracking.
- [Human documentation](${site}/): Rendered documentation with search and navigation.`;

const full = `# btt documentation

> Branch tree testing for any language: specify test structure in readable .tree files, scaffold the test skeleton, and keep code and specification in sync.

Canonical documentation: ${site}/llms.txt

${rendered.map((page) => `${page.markdown}\n\nSource page: ${site}${page.path}`).join("\n\n---\n\n")}`;

await emit("llms.txt", index);
await emit("llms-full.txt", full);
