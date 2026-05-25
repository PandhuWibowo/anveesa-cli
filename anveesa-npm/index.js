#!/usr/bin/env node

/**
 * Anveesa CLI wrapper for npm
 * 
 * This is a minimal wrapper that calls the Rust anveesa binary
 * You need to build the Rust binary first: cargo build --release
 */

const { execSync, exec } = require('child_process');
const path = require('path');
const fs = require('fs');

// Determine binary path
const binDir = path.dirname(require.main.filename);
const binaryPath = path.join(binDir, 'target', 'release', 'anveesa');

function checkBinary() {
  if (!fs.existsSync(binaryPath)) {
    console.error('⚠️  Anveesa binary not found at:', binaryPath);
    console.error('');
    console.error('Build the Rust binary first:');
    console.error('  cd anveesa-cli && cargo build --release');
    console.error('');
    throw new Error('Anveesa binary not found. Please build the Rust project first.');
  }
}

function runCommand(args) {
  if (args.length === 0) {
    // Interactive mode
    checkBinary();
    return execSync(binaryPath, { stdio: 'inherit' });
  }
  
  const allArgs = [...args, '--', ...args.slice(1)];
  checkBinary();
  return execSync(binaryPath, { args: allArgs, stdio: 'inherit' });
}

// Handle command line arguments
if (require.main === module) {
  const args = process.argv.slice(2);
  runCommand(args);
}

// Export for programmatic use
module.exports = {
  run: runCommand,
  checkBinary: checkBinary
};