# btt web workspace

This workspace contains btt's static landing page.

## Commands

Run commands from this directory:

```sh
corepack enable
pnpm install --frozen-lockfile
pnpm dev
pnpm check
pnpm build
PORT=3000 pnpm start
```

Dependencies and the pnpm version are pinned. Newly published package versions
are quarantined for 14 days by `.npmrc`, and `esbuild` is the only dependency
permitted to run an installation script.

## Deployment

The deployable application lives in `landing`. Build from `/web` with
`pnpm --filter @btt/landing build` and serve with
`pnpm --filter @btt/landing start`.

The page is entirely static. It has no application secrets, analytics, or
third-party browser requests.

## Security boundary

Everything under `web` is public at source or build time. Astro variables
prefixed with `PUBLIC_` are exposed to browsers and must never contain tokens,
credentials, private endpoints, or other sensitive configuration.

Production response headers are defined in `landing/public/serve.json`. Keep
the Content Security Policy synchronized with intentional browser capabilities.
