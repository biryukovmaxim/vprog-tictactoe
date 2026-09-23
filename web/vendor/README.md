# Vendored wasm packages

Two npm tarballs consumed as `file:` dependencies from `web/package.json`.
They are build outputs, not committed files: run `just web-vendor` (from the
repo root) before the first `npm install` in a fresh clone, or fetch them
from a CI `wasm-vendor` artifact.

- `kaspa-wasm-<version>.tgz` — the Rusty Kaspa SDK wasm package, built for
  `wasm32` from [rusty-kaspa](https://github.com/kaspanet/rusty-kaspa) at the
  rev this repo's `Cargo.lock` pins (`--target web`, features
  `wasm32-core,wasm32-rpc`). npm only ships 0.13.x; the wallet needs 2.0.x.
  ISC license, see the LICENSE inside.
- `vprog-tictactoe-encoder-wasm-<version>.tgz` — `wasm-pack build
  encoder-wasm --release --target web` output of this repo's `encoder-wasm`
  crate.

`just web-vendor` regenerates both in place; the version suffix in the
filename comes from the crate version. If a version bump renames a tarball,
update the `file:vendor/...` path in `web/package.json` to match.
