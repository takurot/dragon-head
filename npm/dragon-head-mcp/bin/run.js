#!/usr/bin/env node
'use strict';

const path = require('path');
const fs = require('fs');
const { spawn } = require('child_process');

const PLATFORM_PACKAGES = {
  'darwin,arm64': 'dragon-head-mcp-darwin-arm64',
  'darwin,x64': 'dragon-head-mcp-darwin-x64',
  'linux,x64': 'dragon-head-mcp-linux-x64',
  'linux,arm64': 'dragon-head-mcp-linux-arm64',
  'win32,x64': 'dragon-head-mcp-win32-x64',
};

function resolveBinaryPath() {
  const key = `${process.platform},${process.arch}`;
  const pkgName = PLATFORM_PACKAGES[key];

  if (!pkgName) {
    const isLinux = process.platform === 'linux';
    const muslHint = isLinux
      ? ' Note: only glibc Linux builds are published (no musl/Alpine support yet).'
      : '';
    console.error(
      `dragon-head-mcp: unsupported platform "${key}".${muslHint}\n` +
        'Use scripts/install.sh or build from source: https://github.com/takurot/dragon-head#install'
    );
    process.exit(1);
  }

  // Resolve via the platform package's own package.json rather than guessing
  // a node_modules path directly, so this also works under non-flat layouts
  // (pnpm, yarn PnP) where require.resolve still honors the resolver's own
  // module map even without a literal node_modules tree.
  let pkgJsonPath;
  try {
    pkgJsonPath = require.resolve(`${pkgName}/package.json`);
  } catch {
    console.error(
      `dragon-head-mcp: optional dependency "${pkgName}" is not installed.\n` +
        'Re-run npm install, or check that npm config (ignore-scripts / os-cpu ' +
        'mismatch) did not skip optionalDependencies.'
    );
    process.exit(1);
  }

  const pkgJson = require(pkgJsonPath);
  const relativeBinaryPath = pkgJson.dragonHeadBinary;
  if (!relativeBinaryPath) {
    console.error(`dragon-head-mcp: "${pkgName}" is missing its dragonHeadBinary field.`);
    process.exit(1);
  }

  const binaryPath = path.join(path.dirname(pkgJsonPath), relativeBinaryPath);
  if (!fs.existsSync(binaryPath)) {
    console.error(`dragon-head-mcp: binary not found at ${binaryPath} (corrupt install?).`);
    process.exit(1);
  }

  return binaryPath;
}

function main() {
  const binaryPath = resolveBinaryPath();
  const child = spawn(binaryPath, process.argv.slice(2), { stdio: 'inherit' });

  const forwardSignal = (signal) => {
    child.kill(signal);
  };
  process.on('SIGINT', forwardSignal);
  process.on('SIGTERM', forwardSignal);

  child.on('error', (err) => {
    console.error(`dragon-head-mcp: failed to start binary: ${err.message}`);
    process.exit(1);
  });

  child.on('exit', (code, signal) => {
    if (signal) {
      // Re-raise the same signal on this process so a wrapping supervisor
      // sees the real termination cause instead of a synthetic exit code.
      process.kill(process.pid, signal);
    } else {
      process.exit(code ?? 1);
    }
  });
}

main();
