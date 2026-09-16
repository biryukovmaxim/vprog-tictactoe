# Vendored wasm packages

Two npm tarballs consumed as `file:` dependencies from `web/package.json`, so the expanded
packages never land in git as text:

- `kaspa-wasm-2.0.1.tgz` — the Rusty Kaspa SDK wasm package, built locally for `wasm32`
  from [rusty-kaspa](https://github.com/kaspanet/rusty-kaspa) master (`wasm/` crate).
  npm only ships 0.13.x; the wallet needs 2.0.x. ISC license, see the LICENSE inside.
- `vprog-tictactoe-encoder-wasm-0.1.5.tgz` — `wasm-pack` output of this repo's
  `encoder-wasm` crate.

Regenerating:

```bash
# kaspa-wasm: build the wasm crate (rusty-kaspa checkout), then
npm pack <pkg-dir> --pack-destination web/vendor
# encoder: wasm-pack build encoder-wasm, then
npm pack <pkg-dir> --pack-destination web/vendor
# bump the file: versions in web/package.json and reinstall
```
