#!/usr/bin/env node

const { spawn } = require('child_process');
const path = require('path');
const fs = require('fs');

function findBinary() {
  const platform = process.platform;
  const ext = platform === 'win32' ? '.exe' : '';

  // Prefer the latest local release build in a development checkout.
  const devPath = path.join(__dirname, '..', 'target', 'release', 'anveesa' + ext);
  if (fs.existsSync(devPath)) return devPath;

  // Installed npm packages keep the downloaded binary in the package bin/ directory.
  const bundled = path.join(__dirname, 'anveesa' + ext);
  if (fs.existsSync(bundled)) return bundled;

  // Check if there's a sibling directory with the binary
  const sibling = path.join(__dirname, 'target', 'release', 'anveesa' + ext);
  if (fs.existsSync(sibling)) return sibling;

  return null;
}

const binaryPath = findBinary();

if (!binaryPath) {
  console.error('❌ anveesa binary not found.');
  console.error('');
  console.error('Try reinstalling: npm install -g anveesa');
  console.error('Or build from source: cargo build --release');
  process.exit(1);
}

const args = process.argv.slice(2);
const child = spawn(binaryPath, args, {
  cwd: process.cwd(),
  stdio: 'inherit',
  env: { ...process.env },
});

child.on('error', (error) => {
  console.error('Error running anveesa:', error.message);
  process.exit(1);
});

child.on('exit', (code) => {
  process.exit(code ?? 1);
});
