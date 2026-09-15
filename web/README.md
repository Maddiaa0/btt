# btt web workspace

This workspace contains two independently deployable static sites:

- `landing` — the product landing page at `btt.maddiaa.com`
- `docs` — the Starlight documentation site, configured for
  `docs.btt.maddiaa.com`

## Commands

Run commands from this directory:

```sh
corepack enable
pnpm install --frozen-lockfile
pnpm dev
pnpm dev:docs
pnpm check
pnpm build
pnpm build:docs
PORT=3000 pnpm start
```

Dependencies and the pnpm version are pinned. Newly published package versions
are quarantined for 14 days by `.npmrc`, and `esbuild` is the only dependency
permitted to run an installation script.

## Landing deployment

The deployable application lives in `landing`. Build from `/web` with
`pnpm --filter @btt/landing build` and serve with
`pnpm --filter @btt/landing start`.

The page is entirely static. It has no application secrets, analytics, or
third-party browser requests.

## Documentation deployment

The docs are a separate package and build artifact:

```sh
pnpm --filter @btt/docs build
```

Deploy `docs/dist` to the documentation host. The site is static and includes
Pagefind search, per-page Markdown mirrors, `llms.txt`, and `llms-full.txt`.
The AI-readable files are derived from the canonical Markdown pages by
`docs/scripts/generate-llms.mjs` and checked for drift during `pnpm check`.

## Security boundary

Everything under `web` is public at source or build time. Astro variables
prefixed with `PUBLIC_` are exposed to browsers and must never contain tokens,
credentials, private endpoints, or other sensitive configuration.

Production response headers are defined separately in each package's
`public/serve.json`. Keep their Content Security Policies synchronized with
intentional browser capabilities.
