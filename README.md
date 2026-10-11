# Mangrove

[Français](README.fr.md) | **English**

A native desktop Git client for exploring a repository’s history, branches, and diffs. It leaves files and the index untouched; fetching updates remote references. The native interface is rendered with GPUI on top of the Rust `mangrove-core` engine, without a WebView or local server.

## Preview

Screenshots of Mangrove on macOS, using a demo repository with sample data.

**Main window** — branches, history graph, and commit details.

![Mangrove’s main window showing branches, the commit graph, and changed files](docs/screenshots/main-window.jpg)

**Side-by-side diff** — syntax highlighting, additions, deletions, and highlighted word changes.

![Side-by-side TypeScript diff in Mangrove, with highlighted line and word changes](docs/screenshots/diff.jpg)

## Features

- **Repositories**: local and bare repositories, and worktrees; recent and favorite repositories per profile; tabs that restore their selection, search, and scroll position.
- **History**: branch and merge graph, references column, and search by message, author, SHA, or reference.
- **Branches**: local and remote branches, tags, collapsible folders, filtering, and **Solo** mode; ahead/behind counts relative to the upstream branch; worktrees and stashes ([details](docs/repository-context.md)).
- **Commits**: details, changed files, unified or side-by-side diffs with syntax highlighting and highlighted word changes, and before/after image previews.
- **Files**: contents at a given revision, history that follows renames, and blame ([details](docs/file-browser.md)).
- **Local changes**: read-only views of staged, unstaged, and untracked files.
- **Fetch**: manual or automatic, with error summaries and detailed Git output.
- **Updates**: checks at startup and every six hours, background installation of signed releases, and one-click restart, configurable in settings ([details](docs/releases.md#mises-à-jour-intégrées)).
- **Integrations**: profiles, GitLab, GitHub, and Bitbucket accounts (repositories, PRs/MRs, CI), and HTTPS or SSH cloning; tokens stored in the OS credential store ([details](docs/integrations.md)).
- **Interface**: seven themes, including [Catppuccin](https://catppuccin.com/palette/) Macchiato, a **Cmd/Ctrl + P** command palette ([commands](docs/command-palette.md)), keyboard navigation, resizable panels, and zoom.

## Install

Builds for macOS (Apple Silicon and Intel), Linux, and Windows are attached to [GitHub releases](https://github.com/Dpblandin/mangrove/releases); see the [installation instructions](docs/releases.md#installer-une-release). Fetching and cloning use Git installed on your machine.

Interactive QA is performed on macOS Apple Silicon. Linux and Windows builds are compiled and tested in CI, but have not yet been visually validated.

## Build

Prerequisites: Rust via rustup (the version is pinned in `rust-toolchain.toml`) and the [GPUI Kit prerequisites](https://gpui-kit.com/docs/installation/) for your OS, including the full Xcode installation on macOS.

```sh
cargo run --release --locked --manifest-path src-gpui/Cargo.toml
cargo run --release --locked --manifest-path src-gpui/Cargo.toml -- /path/to/repository

# macOS bundle: src-gpui/target/release/bundle/Mangrove.app
sh src-gpui/scripts/package-macos.sh
```

## Develop

CI runs these checks on macOS, Linux, and Windows:

```sh
cargo fmt --all --check
cargo clippy --locked -p mangrove-core -p mangrove-remote -p mangrove-update --all-targets -- -D warnings
cargo test --locked -p mangrove-core -p mangrove-remote -p mangrove-update

cargo fmt --manifest-path src-gpui/Cargo.toml --all --check
cargo clippy --locked --manifest-path src-gpui/Cargo.toml --all-targets -- -D warnings
cargo test --locked --manifest-path src-gpui/Cargo.toml
```

To create a synthetic repository in `.mangrove-dev/` (only Node.js and Git are required):

```sh
node scripts/create-fixture.mjs --commits 10000 --large-change
```

Publish a release with `scripts/release.sh X.Y.Z`; see the [release documentation](docs/releases.md).

## Project structure

- `crates/mangrove-core`: Git reading, graph, search index, and cache.
- `crates/mangrove-remote`: profiles, OS credential store, hosting provider APIs, and SSH.
- `crates/mangrove-update`: GitHub releases, minisign signatures, and update installation.
- `src-gpui`: GPUI application, in a separate Cargo workspace ([README](src-gpui/README.md)).
- `scripts`: fixtures, benchmarks, and releases.
- `docs`: [architecture](docs/architecture.md), [performance](docs/performance.md), and feature documentation.

## License

Mangrove is distributed under the [Functional Source License 1.1, with MIT as the future license](LICENSE.md) (FSL-1.1-MIT). Use, including at work, modification, and redistribution are permitted; incorporating it into a competing commercial product or service is not. Each version becomes MIT-licensed two years after publication. Third-party component licenses are included with every published build.
