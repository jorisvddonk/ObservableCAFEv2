# ADR-130: Documentation site built with mdBook

**Status**: Implemented

**Date**: 2026-09-20

**Driver**: The documentation was 65 loose markdown files under `docs/`, readable
on GitHub but with no navigation, search, or cross-page structure. We wanted a
prettier, browsable site that can be hosted statically (GitHub Pages) without
rewriting the docs or moving the source of truth.

**Context**: The docs already follow [Diataxis](https://diataxis.fr/) and use
Mermaid diagrams. Any generator had to render the existing markdown as-is
(no conversion to a bespoke format), handle relative cross-links and Mermaid,
and fit a Rust-first monorepo. Hosting had to be static — GitHub Pages, not a
server.

**Decision**:

1. **mdBook** (`book.toml`, `src = "docs"`), building to `site/` (gitignored).
   Rust-native, single binary, built-in search and sidebar navigation. Mermaid
   via the `mdbook-mermaid` preprocessor plus vendored `mermaid.min.js` +
   `mermaid-init.js` (no CDN dependency at view time).
2. **Navigation** lives in `docs/SUMMARY.md`, mirroring Diataxis plus a grouped
   ADR section. Home is `docs/index.md`; the root `README.md` is deliberately
   **not** rendered into the site.
3. **Internal material moves out of `docs/`.** mdBook copies every non-markdown
   file from `src` into the output verbatim, and it has no file-exclude option;
   extension-less notes in `docs/planning/` and `docs/design/` were therefore
   published as raw files. Internal docs now live in `internal-docs/`
   (`AGENT.md`, `design/`, `planning/`), leaving `docs/` as publishable content
   only. Links to them from the site point at GitHub.
4. **Out-of-tree links become GitHub URLs.** Links to `README.md`, `AGENTS.md`,
   `agents-js/*`, `cafe-*/src/*`, and `tests/*` are rewritten to
   `github.com/jorisvddonk/ObservableCAFEv2/blob/main/...` so the static site
   doesn't 404.
5. **Deploy** via `.github/workflows/docs.yml` on push to `main` using the
   GitHub Pages Actions flow (`configure-pages` → `upload-pages-artifact` →
   `deploy-pages`). Requires enabling *Settings → Pages → Source: GitHub
   Actions* once. Local: `just docs-build` / `just docs-serve`.

**Consequences**:

- Positive: existing markdown is the site source of truth; no doc rewrite.
- Positive: search, sidebar nav, themes, and Mermaid come for free.
- Positive: publishing is stateless (artifact deploy, no `gh-pages` branch).
- Negative: `docs/SUMMARY.md` must be updated whenever a page is added;
  mdBook silently omits pages that are not listed.
- Negative: the vendored `mermaid.min.js` is ~2.6 MB committed to the repo.
- Negative: links from docs to source files go through GitHub, and only work
  once the branch/URL is stable.
- Neutral: the docs deploy workflow is separate from `ci.yml`, so doc builds
  don't slow the test pipeline.

**Alternatives considered**:
- **MkDocs + Material**: prettiest out of the box, but Python and a second
  toolchain; mdBook keeps it Rust-native.
- **Zola**: Rust and fast, but needs theme/template setup for a docs look.
- **Docusaurus / Astro Starlight**: heavy Node/React toolchains for what is
  plain markdown.
- **Keep raw markdown on GitHub**: no navigation or search; the status quo.

Related: [README project list](https://github.com/jorisvddonk/ObservableCAFEv2#projects),
[ADR-001](./adr-001-unix-socket-message-bus.md) (the repo's ADR convention).
