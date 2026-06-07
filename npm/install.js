#!/usr/bin/env node
// Downloads the correct anveesa binary for the current platform from GitHub releases.
'use strict';

const https = require('https');
const fs = require('fs');
const path = require('path');
const { execSync } = require('child_process');
const zlib = require('zlib');

const VERSION = require('./package.json').version;
const REPO = 'PandhuWibowo/anveesa-cli';
const BIN_DIR = path.join(__dirname, 'bin');

function platformTarget() {
  const { platform, arch } = process;
  const targets = {
    'linux-x64':   'x86_64-unknown-linux-gnu',
    'linux-arm64': 'aarch64-unknown-linux-gnu',
    'darwin-x64':  'x86_64-apple-darwin',
    'darwin-arm64':'aarch64-apple-darwin',
    'win32-x64':   'x86_64-pc-windows-msvc',
  };
  const key = `${platform}-${arch}`;
  const target = targets[key];
  if (!target) throw new Error(`Unsupported platform: ${key}. Build from source: https://github.com/${REPO}`);
  return target;
}

function download(url) {
  return new Promise((resolve, reject) => {
    https.get(url, { headers: { 'User-Agent': 'anveesa-npm-installer' } }, res => {
      if (res.statusCode === 301 || res.statusCode === 302) {
        return download(res.headers.location).then(resolve).catch(reject);
      }
      if (res.statusCode !== 200) {
        return reject(new Error(`HTTP ${res.statusCode} downloading ${url}`));
      }
      const chunks = [];
      res.on('data', c => chunks.push(c));
      res.on('end', () => resolve(Buffer.concat(chunks)));
      res.on('error', reject);
    }).on('error', reject);
  });
}

async function extractTar(tarGz, destDir, binaryName) {
  // Write to temp file and use system tar (available on all supported platforms)
  const tmp = path.join(destDir, '_download.tar.gz');
  fs.writeFileSync(tmp, tarGz);
  try {
    execSync(`tar -xzf "${tmp}" -C "${destDir}"`, { stdio: 'inherit' });
  } finally {
    fs.unlinkSync(tmp);
  }
  const src = path.join(destDir, binaryName);
  if (!fs.existsSync(src)) throw new Error(`Binary not found in archive: ${binaryName}`);
  return src;
}

async function main() {
  const target = platformTarget();
  const binaryName = process.platform === 'win32' ? 'anveesa.exe' : 'anveesa';
  const archiveName = `anveesa-${VERSION}-${target}.tar.gz`;
  const url = `https://github.com/${REPO}/releases/download/v${VERSION}/${archiveName}`;

  console.log(`anveesa: downloading ${archiveName}`);
  fs.mkdirSync(BIN_DIR, { recursive: true });

  const data = await download(url);
  const binPath = await extractTar(data, BIN_DIR, binaryName);

  if (process.platform !== 'win32') {
    fs.chmodSync(binPath, 0o755);
  }

  // Verify it runs
  try {
    execSync(`"${binPath}" --version`, { stdio: 'pipe' });
    console.log(`anveesa: installed successfully`);
  } catch (_) {
    // Non-fatal — binary may need runtime libs not present in CI
    console.log(`anveesa: installed (run "anveesa --version" to verify)`);
  }
}

main().catch(err => {
  console.error(`\nanveesa install failed: ${err.message}`);
  console.error(`Build from source: https://github.com/${REPO}#install`);
  // Don't hard-fail npm install — let users build from source if needed
  process.exit(0);
});
