# npm distribution (dragon-head-mcp)

Implements the optionalDependencies-per-platform pattern (same approach as
esbuild/swc/turbo): a thin wrapper package with a `bin` shim, plus one tiny
package per platform containing only the prebuilt binary. npm selects the
right platform package automatically via the `os`/`cpu` fields — there is no
postinstall script and no install-time network fetch.

```
npm/
  dragon-head-mcp/        # published as "dragon-head-mcp" — the package users install
    package.json
    bin/run.js            # resolves the platform package and execs the real binary
  platform/
    darwin-arm64/package.json
    darwin-x64/package.json
    linux-x64/package.json
    linux-arm64/package.json
    win32-x64/package.json
```

Each `platform/*/package.json` is a template only — **no binary is committed
to git**. CI copies the matching release artifact into
`platform/<name>/bin/dragon-head-mcp[.exe]` at publish time, immediately
before `npm publish` for that package, then discards the staged binary. The
`dragonHeadBinary` field in each platform package.json tells `bin/run.js`
where to find it relative to that package's own `package.json`.

See [issue #167](https://github.com/takurot/dragon-head/issues/167) for the
full rollout plan, including the npm Trusted Publishing (OIDC) setup this
depends on and the CI integration work still to be done.
