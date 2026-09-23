# Deploying the built web app

The web bundle, the wasm vendor tarballs, and the guest `program.elf` are
built by the `artifacts` workflow (Actions tab → `artifacts` → *Run
workflow*; pass the public wRPC endpoint as `wrpc-url`). This is the manual
deploy path from those artifacts.

## 1. Fetch artifacts

    gh run list --workflow artifacts --limit 1
    gh run download <run-id> -D deploy-artifacts

## 2. Web static serving (operator box)

Any static server with a `/api` reverse proxy works; Caddy is the smallest:

    tt.example.com {
        root * /srv/tt-web
        encode gzip zstd
        handle /api/* {
            reverse_proxy 127.0.0.1:9880
        }
        handle {
            try_files {path} /index.html
            file_server
        }
    }

`/api` must reach the DA HTTP server the `ttd` node runs (`TT_DA_BIND`,
default `127.0.0.1:9880`) — the built app talks to it same-origin. The wRPC
endpoint is baked into the bundle at build time (`wrpc-url` input); serve it
over TLS too if browsers need wss:

    ws.example.com {
        reverse_proxy 127.0.0.1:17210
    }

## 3. Ship

    rsync -a --delete deploy-artifacts/web-dist/ box:/srv/tt-web/
    rsync -a deploy-artifacts/program-elf/program.elf box:<node-and-prover-dir>/

**Node and prover must run the same `program.elf` file** — ship this one
artifact to both (`TT_PROGRAM_ELF` or the default
`guest/compiled/program.elf` path on the box). Never mix a workflow ELF with
a locally built one: different builds carry different image IDs and receipts
will not verify.

## 4. Reload + verify

Reload Caddy (`systemctl reload caddy`), then open the public URL in a clean
browser profile: the board must load, keys must fund, a game must reach a
playable state, and the console must stay error-free.
