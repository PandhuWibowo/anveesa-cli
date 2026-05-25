# Anveesa - AI Agent CLI

A Node.js wrapper package for the Rust `anveesa` CLI tool.

## Prerequisites

1. Install Rust and build the CLI binary:
   ```bash
   cd anveesa-cli
   cargo build --release
   ```

2. The binary will be at `target/release/anveesa`

## Installation

```bash
npm install anveesa@0.1.0
```

## Usage

```bash
# Use the CLI directly
anveesa "your prompt here"

# Or programmatically
const { execSync } = require('child_process');
const response = execSync('anveesa "what is rust?"').toString();
console.log(response);
```

## About

This is a minimal npm wrapper that provides a Node.js interface to the Rust anveesa binary. The actual AI agent functionality is implemented in Rust, and this package simply exposes the CLI binary to npm.

## Note

This package wraps the Rust binary. To use anveesa, you must have Rust installed and the binary built.