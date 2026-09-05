# btt documentation

An independently deployable documentation site built with Astro Starlight.
Documentation source lives in `src/content/docs`; the generated AI-readable
files in `public` are committed so they can be inspected and are refreshed by
the build.

Run commands from `web`:

```sh
pnpm install --frozen-lockfile
pnpm dev:docs
pnpm --filter @btt/docs check
pnpm build:docs
```

## AI-readable output

`scripts/generate-llms.mjs` derives these files from the documentation source:

- `public/llms.txt` — concise page index
- `public/llms-full.txt` — complete documentation in one Markdown document
- `public/*.md` — focused Markdown mirror for every human-facing page

Run `pnpm --filter @btt/docs generate:llms` after editing content. The docs
check fails when committed generated output is stale.

## Deployment

Build with `pnpm build:docs` and deploy `docs/dist` to the configured static
host, currently `https://docs.btt.maddiaa.com`. `serve.json` contains the
production security and caching headers for hosts that support that format.
