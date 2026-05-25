#!/usr/bin/env node

/**
 * Installation script for anveesa npm package
 * This script checks if the Rust binary exists and provides instructions if not
 */

const fs = require('fs');
const path = require('path');
const https = require('https');
const { execFileSync } = require('child_process');

const PACKAGE = require(path.join(__dirname, '..', 'package.json'));
const REPO = 'pandhuwibowo/anveesa-cli';
const BIN_DIR = path.join(__dirname, '..', 'bin');
const REDIRECT_STATUSES = new Set([301, 302, 303, 307, 308]);

function getPlatformInfo() {
  const platform = process.platform;
  const arch = process.arch;

  if (platform === 'darwin' && arch === 'arm64')
    return { platform: 'macos', target: 'aarch64-apple-darwin', ext: '' };
  if (platform === 'darwin' && arch === 'x64')
    return { platform: 'macos', target: 'x86_64-apple-darwin', ext: '' };
  if (platform === 'linux' && arch === 'x64')
    return { platform: 'linux', target: 'x86_64-unknown-linux-gnu', ext: '' };
  if (platform === 'linux' && arch === 'arm64')
    return { platform: 'linux', target: 'aarch64-unknown-linux-gnu', ext: '' };
  if (platform === 'win32' && arch === 'x64')
    return { platform: 'windows', target: 'x86_64-pc-windows-msvc', ext: '.exe' };

  return null;
}

function getExecutableName() {
  return process.platform === 'win32' ? 'anveesa.exe' : 'anveesa';
}

function getBinaryPath() {
  return path.join(BIN_DIR, getExecutableName());
}

function getBinaryUrl() {
  const info = getPlatformInfo();
  if (!info) return null;
  const version = PACKAGE.version;
  return `https://github.com/${REPO}/releases/download/v${version}/anveesa-${version}-${info.target}.tar.gz`;
}

function download(url, dest, redirects = 0) {
  return new Promise((resolve, reject) => {
    const request = https.get(url, {
      headers: { 'User-Agent': `anveesa-install/${PACKAGE.version}` },
    }, (res) => {
      if (REDIRECT_STATUSES.has(res.statusCode)) {
        res.resume();
        if (!res.headers.location) {
          reject(new Error(`HTTP ${res.statusCode} without Location header`));
          return;
        }
        if (redirects >= 5) {
          reject(new Error('too many redirects'));
          return;
        }

        const nextUrl = new URL(res.headers.location, url).toString();
        resolve(download(nextUrl, dest, redirects + 1));
        return;
      }

      if (res.statusCode !== 200) {
        res.resume();
        reject(new Error(res.statusCode === 404 ? '404' : `HTTP ${res.statusCode}`));
        return;
      }

      const file = fs.createWriteStream(dest);
      file.on('finish', () => { file.close(resolve); });
      file.on('error', (err) => {
        fs.unlink(dest, () => {});
        reject(err);
      });
      res.pipe(file);
    }).on('error', (err) => {
      fs.unlink(dest, () => {});
      reject(err);
    });
    request.setTimeout(30000, () => {
      request.destroy(new Error('download timed out'));
    });
  });
}

async function tryDownloadBinary() {
  const url = getBinaryUrl();
  if (!url) {
    console.log('⚠ Unsupported platform:', process.platform, process.arch);
    return false;
  }

  console.log('⬇ Downloading prebuilt binary for', process.platform, process.arch);

  const tarPath = path.join(__dirname, 'anveesa-bin.tar.gz');
  try {
    if (!fs.existsSync(BIN_DIR)) fs.mkdirSync(BIN_DIR, { recursive: true });

    await download(url, tarPath);

    // Extract
    execFileSync('tar', ['xzf', tarPath, '-C', BIN_DIR], { stdio: 'inherit' });
    fs.unlinkSync(tarPath);

    // Make executable
    const binary = getBinaryPath();
    if (fs.existsSync(binary)) {
      if (process.platform !== 'win32') fs.chmodSync(binary, 0o755);
    } else {
      console.log('⚠ Downloaded archive did not contain', getExecutableName());
      return false;
    }

    console.log('✓ Binary downloaded successfully');
    return true;
  } catch (e) {
    if (e.message === '404') {
      console.log('ℹ No prebuilt binary for this version yet');
    }
    fs.unlink(tarPath, () => {});
    return false;
  }
}

function tryBuildFromSource() {
  try {
    console.log('⚙ Building from source (requires Rust)...');
    execFileSync('cargo', ['build', '--release'], { cwd: path.join(__dirname, '..'), stdio: 'inherit' });

    const src = path.join(__dirname, '..', 'target', 'release', getExecutableName());
    const dest = getBinaryPath();

    if (fs.existsSync(src)) {
      if (!fs.existsSync(BIN_DIR)) fs.mkdirSync(BIN_DIR, { recursive: true });
      fs.copyFileSync(src, dest);
      if (process.platform !== 'win32') fs.chmodSync(dest, 0o755);
      console.log('✓ Built from source successfully');
      return true;
    }
  } catch (e) {
    if (e.code === 'ENOENT') {
      console.log('⚠ Build from source failed: cargo was not found');
    } else {
      console.log('⚠ Build from source failed');
    }
  }
  return false;
}

function hasUsableExistingBinary() {
  const existing = getBinaryPath();
  if (!fs.existsSync(existing)) return false;

  try {
    if (process.platform !== 'win32') fs.chmodSync(existing, 0o755);
    execFileSync(existing, ['--help'], { stdio: 'ignore', timeout: 5000 });
    return true;
  } catch (e) {
    console.log('ℹ Existing anveesa binary is not usable on this platform; replacing it');
    return false;
  }
}

async function install() {
  // Check if binary already exists
  if (hasUsableExistingBinary()) {
    console.log('✓ anveesa binary already installed');
    return;
  }

  // Try download first
  if (await tryDownloadBinary()) return;

  // Fallback to build from source
  if (tryBuildFromSource()) return;

  console.error('');
  console.error('✗ Could not install anveesa binary.');
  console.error('');
  const url = getBinaryUrl();
  if (url) {
    console.error(`No prebuilt binary was available for anveesa v${PACKAGE.version}:`);
    console.error(`  ${url}`);
    console.error('');
  }
  console.error('Install Rust: https://rustup.rs/');
  console.error('Then run: npm install -g anveesa');
  console.error('');
  console.error('Or grab a prebuilt binary from:');
  console.error('  https://github.com/pandhuwibowo/anveesa-cli/releases');
  process.exit(1);
}

install();
