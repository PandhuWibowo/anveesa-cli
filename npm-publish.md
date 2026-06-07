# npm publishing guide — anveesa

## How it works

The npm package (`package.json` at repo root) is a thin wrapper around the Rust binary. It does **not** bundle the binary itself. On `npm install`, the postinstall script (`scripts/install.js`) runs and either:

1. Downloads the correct pre-built binary from the matching GitHub Release, or
2. Falls back to `cargo build --release` if no pre-built binary is available.

## Package structure

```
anveesa-cli/
├── package.json          ← npm package manifest (root-level)
├── bin/
│   └── anveesa.js        ← bin entry point: finds and execs the Rust binary
├── scripts/
│   └── install.js        ← postinstall: downloads binary or builds from source
└── README.md             ← included in the npm tarball
```

> **Note:** The `npm/` directory is a leftover scaffold. The active npm package is the root `package.json`.

## Supported platforms

| Platform | Binary target |
|---|---|
| macOS arm64 | `aarch64-apple-darwin` |
| macOS x86_64 | `x86_64-apple-darwin` |
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux arm64 | `aarch64-unknown-linux-gnu` |
| Windows x86_64 | `x86_64-pc-windows-msvc` |

## GitHub Release asset naming

The install script downloads:

```
anveesa-{version}-{target}.tar.gz
```

For example, for version `0.7.0` on macOS arm64:

```
anveesa-0.7.0-aarch64-apple-darwin.tar.gz
```

Each archive contains a single binary named `anveesa` (or `anveesa.exe` on Windows).

## Publishing a new version

### Automated (recommended)

1. Bump `version` in **both** `Cargo.toml` and `package.json` to the same value
2. Run final checks:
   ```bash
   cargo fmt && cargo clippy -- -D warnings && cargo test
   ```
3. Commit, tag, and push:
   ```bash
   git add Cargo.toml Cargo.lock package.json
   git commit -m "feat: vX.Y.Z — ..."
   git tag vX.Y.Z
   git push origin main --tags
   ```
4. GitHub Actions (`release.yml`) automatically:
   - Builds binaries for all 5 platforms
   - Uploads them to GitHub Release `vX.Y.Z`
   - Runs `npm publish` via the `NPM_TOKEN` secret
   - Runs `cargo publish` via the `CARGO_REGISTRY_TOKEN` secret

### Manual (if CI secrets not configured)

After the GitHub Release binaries are uploaded (wait ~5 min after pushing the tag):

```bash
npm publish --access public
```

You must be logged in as `pandhuw`:

```bash
npm whoami   # should print: pandhuw
npm login    # if not logged in
```

## GitHub Secrets required for automated publishing

| Secret name | Where to get it | Used for |
|---|---|---|
| `NPM_TOKEN` | `npm token create --type=automation` | `npm publish` in CI |
| `CARGO_REGISTRY_TOKEN` | crates.io → Account Settings → API Tokens | `cargo publish` in CI |

Add both at: **GitHub → repo → Settings → Secrets and variables → Actions**

## Verifying a publish

```bash
npm view anveesa dist-tags        # should show latest: X.Y.Z
npm view anveesa X.Y.Z            # confirm version details
```

After `npm install -g anveesa`, the binary is installed at:

- **macOS/Linux:** `$(npm root -g)/../bin/anveesa` → delegates to `bin/anveesa.js`
- **Windows:** `%APPDATA%\npm\anveesa.cmd`

## Troubleshooting

**"No prebuilt binary for this version"**
The install script falls back to `cargo build --release`. If Rust is not installed, it exits with an error and a link to the GitHub Release page for manual download.

**"HTTP 404" during install**
The GitHub Release for that version doesn't exist yet. Wait for the release workflow to complete, then re-run `npm install -g anveesa`.

**"Permission denied" on macOS**
Run with `sudo npm install -g anveesa` or use a Node version manager (nvm/fnm) to avoid needing sudo.

**Updating the install script logic**
Edit `scripts/install.js`. The binary download URL is constructed as:
```
https://github.com/PandhuWibowo/anveesa-cli/releases/download/v{version}/anveesa-{version}-{target}.tar.gz
```
