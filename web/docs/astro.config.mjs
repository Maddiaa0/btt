import { defineConfig } from "astro/config";
import starlight from "@astrojs/starlight";

const repositoryUrl = "https://github.com/Maddiaa0/btt";

export default defineConfig({
  site: "https://docs.btt.maddiaa.com",
  output: "static",
  trailingSlash: "never",
  integrations: [
    starlight({
      title: "btt / docs",
      description:
        "Branch tree testing documentation: specify test structure, scaffold it, and keep implementation in sync.",
      favicon: "/favicon.svg",
      customCss: ["./src/styles/custom.css"],
      components: {
        ThemeProvider: "./src/components/ThemeProvider.astro",
        ThemeSelect: "./src/components/ThemeSelect.astro",
      },
      expressiveCode: {
        themes: ["starlight-light"],
        useStarlightDarkModeSwitch: false,
      },
      editLink: {
        baseUrl: `${repositoryUrl}/edit/main/web/docs/`,
      },
      lastUpdated: true,
      pagination: true,
      social: [
        {
          icon: "github",
          label: "btt on GitHub",
          href: repositoryUrl,
        },
      ],
      head: [
        {
          tag: "link",
          attrs: {
            rel: "alternate",
            type: "text/plain",
            href: "/llms.txt",
            title: "btt documentation for language models",
          },
        },
        { tag: "meta", attrs: { name: "theme-color", content: "#faf9f5" } },
      ],
      sidebar: [
        { label: "Overview", slug: "index" },
        { label: "Getting started", slug: "getting-started" },
        { label: "Installation", slug: "installation" },
        {
          label: "Creating an extension",
          slug: "creating-an-extension",
          attrs: { class: "nav-divider" },
        },
        {
          label: "Installing the skill + usage",
          slug: "installing-the-skill",
        },
      ],
    }),
  ],
  build: {
    inlineStylesheets: "never",
  },
  vite: {
    build: {
      assetsInlineLimit: 0,
    },
  },
});
