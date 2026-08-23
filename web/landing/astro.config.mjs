import { defineConfig } from "astro/config";

export default defineConfig({
  site: "https://btt.maddiaa.com",
  output: "static",
  trailingSlash: "never",
  build: {
    inlineStylesheets: "never",
  },
  vite: {
    build: {
      assetsInlineLimit: 0,
    },
  },
});
