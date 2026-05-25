# Publishing Anveesa to npm

## Prerequisites

1. **Install Node.js** (>=16)
2. **Install Rust** (untuk build binary)
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
   ```
3. **Install npm** (global npm registry)
   ```bash
   npm config set registry https://registry.npmjs.org
   ```

## Build the Binary

```bash
npm run build
```

## Publish to npm

The npm installer downloads prebuilt binaries from the GitHub release that
matches `package.json` exactly. For `0.2.4`, the release tag must be `v0.2.4`
and must contain assets named like:

```text
anveesa-0.2.4-aarch64-apple-darwin.tar.gz
anveesa-0.2.4-x86_64-apple-darwin.tar.gz
anveesa-0.2.4-x86_64-unknown-linux-gnu.tar.gz
anveesa-0.2.4-aarch64-unknown-linux-gnu.tar.gz
anveesa-0.2.4-x86_64-pc-windows-msvc.tar.gz
```

```bash
# Login ke npm registry (jika belum)
npm login

# 1. Commit the version you want to publish
git tag v$(node -p "require('./package.json').version")
git push origin main --tags

# 2. Wait for the GitHub "Release Binaries" workflow to finish

# 3. Publish
npm publish
```

## Notes

- Binary platform-specific tidak di-include langsung di npm package.
- Saat install, `scripts/install.js` akan download binary dari GitHub release.
- Jika release asset belum ada, installer akan fallback build dari source.
- User hanya perlu Rust kalau prebuilt binary untuk versi/platform itu belum ada.
- Binary akan otomatis terdeteksi oleh `anveesa.js` di `bin/`

## Alternative: Use Cargo Publish

Jika ingin publish Rust binary langsung ke crates.io:

```bash
cargo publish
```

Ini akan publish ke crates.io, bukan npm.

## Post-Publish

Setelah publish, user bisa install dengan:

```bash
npm install anveesa -g  # untuk global CLI
npm install anveesa      # untuk programmatic use
```
