# Tech stack, commands and toolchain

> Part of the Rabbit Chat Studio developer docs. Read this when you need to build, check or test something.
> The index for all of them is [`AGENTS.md`](../../AGENTS.md) — it also holds
> the rules that apply to every change, whatever file you ended up in.

## Commands

```sh
# Dev (hot-reload). Runs `npm run dev` (Vite on :1420) then launches Tauri.
npm run tauri dev

# Frontend only (Vite on :1420, strictPort — must be free)
npm run dev

# Production build. NOTE: this runs `sync-version:git-tag` first, so the
# repo must have a git tag (e.g. v1.1.1) or the build aborts.
npm run tauri build

# Type-checks
npx tsc --noEmit                                  # frontend
cargo check --manifest-path src-tauri/Cargo.toml  # rust

# Version sync (package.json ↔ Cargo.toml ↔ tauri.conf.json)
node scripts/sync-version.cjs --from-git-tag
node scripts/sync-version.cjs --bump patch
npm run sync-version:git-tag
npm run sync-version:bump-patch

# Rust unit tests (inside the `tools` module: dispatcher, `tools/todos`, `tools/timer`)
cargo test --manifest-path src-tauri/Cargo.toml

# Standalone library crates under `src-tauri/src/utils/` are workspace members,
# so the app crate's `cargo test` skips them — target them by package name.
cargo test --manifest-path src-tauri/Cargo.toml -p knowledge-graph
cargo test --manifest-path src-tauri/Cargo.toml -p pdf2md-rs
cargo test --manifest-path src-tauri/Cargo.toml -p model-catalog
```

### Cargo mirror (CN / restricted networks)

`~/.cargo/config` may point at the USTC git registry, which can be unreachable. If `cargo` fails to fetch deps, override the source:

```sh
cargo check --manifest-path src-tauri/Cargo.toml \
  --config 'source.crates-io.replace-with="rsproxy-sparse"' \
  --config 'source.rsproxy-sparse.registry="sparse+https://rsproxy.cn/index/"'
```
