#!/usr/bin/env node
'use strict';

const path = require('path');
const fs = require('fs');
const { spawn } = require('child_process');

// Keep this map in sync with the wrapper package.json's optionalDependencies
// and the 5 npm/platform/*/package.json names (see test/run.test.js, which
// asserts all three agree). scripts/install.sh and .github/workflows/release.yml
// maintain their own parallel platform lists in different languages/formats —
// check those too when adding or removing a platform here.
const PLATFORM_PACKAGES = {
  'darwin,arm64': 'dragon-head-mcp-darwin-arm64',
  'darwin,x64': 'dragon-head-mcp-darwin-x64',
  'linux,x64': 'dragon-head-mcp-linux-x64',
  'linux,arm64': 'dragon-head-mcp-linux-arm64',
  'win32,x64': 'dragon-head-mcp-win32-x64',
};

// Architectures we publish glibc Linux builds for. Used only to decide
// whether the musl/Alpine hint below is relevant — an unsupported Linux
// architecture outside this set (e.g. ia32, arm) is an arch problem, not a
// libc problem, and the hint would be misleading there.
const SUPPORTED_LINUX_ARCHS = new Set(['x64', 'arm64']);

// True when `childPath` resolves to a location strictly inside `parentDir`
// (not equal to it, and not reachable only via a `..` escape). Used to guard
// against a platform package's package.json declaring a dragonHeadBinary
// path that traverses outside its own package directory.
function isPathInsideDirectory(childPath, parentDir) {
  const rel = path.relative(parentDir, childPath);
  const isEscape = rel === '..' || rel.startsWith(`..${path.sep}`);
  return rel !== '' && !isEscape && !path.isAbsolute(rel);
}

// Reads a platform package's package.json (given its absolute path) and
// resolves+validates its declared binary. Split out from resolveBinaryPath
// so tests can exercise the traversal guard directly against a fabricated
// package.json instead of needing a real optional dependency installed.
function resolvePackageBinary(pkgJsonPath, pkgName) {
  let pkgJson;
  try {
    // Use JSON.parse(fs.readFileSync(...)) rather than require() so a
    // corrupted or malicious package.json never pollutes Node's require
    // cache and re-reads pick up on-disk changes.
    pkgJson = JSON.parse(fs.readFileSync(pkgJsonPath, 'utf8'));
  } catch (err) {
    console.error(
      `dragon-head-mcp: "${pkgName}"'s package.json is corrupted (${err.message}).\n` +
        'Re-run npm install to repair it.'
    );
    process.exit(1);
    return undefined;
  }

  const relativeBinaryPath = pkgJson.dragonHeadBinary;
  if (!relativeBinaryPath) {
    console.error(`dragon-head-mcp: "${pkgName}" is missing its dragonHeadBinary field.`);
    process.exit(1);
    return undefined;
  }

  const pkgDir = path.dirname(pkgJsonPath);
  const binaryPath = path.resolve(pkgDir, relativeBinaryPath);

  if (!isPathInsideDirectory(binaryPath, pkgDir)) {
    console.error(
      `dragon-head-mcp: "${pkgName}" declares a dragonHeadBinary path outside its package directory (${binaryPath}).`
    );
    process.exit(1);
    return undefined;
  }

  if (!fs.existsSync(binaryPath)) {
    console.error(`dragon-head-mcp: binary not found at ${binaryPath} (corrupt install?).`);
    process.exit(1);
    return undefined;
  }

  return binaryPath;
}

function resolveBinaryPath(platform = process.platform, arch = process.arch) {
  const key = `${platform},${arch}`;
  const pkgName = PLATFORM_PACKAGES[key];

  if (!pkgName) {
    const muslHint =
      platform === 'linux' && SUPPORTED_LINUX_ARCHS.has(arch)
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

  return resolvePackageBinary(pkgJsonPath, pkgName);
}

// Wraps spawn() in try/catch: Node's spawn() can throw synchronously (e.g.
// malformed arguments, and ENOENT in some environments) rather than only
// emitting the async 'error' event. Without this, a synchronous throw here
// would crash the process with an unhandled exception instead of a clear,
// actionable error message.
function spawnBinary(binaryPath, args) {
  try {
    return spawn(binaryPath, args, { stdio: 'inherit' });
  } catch (err) {
    console.error(`dragon-head-mcp: failed to start binary: ${err.message}`);
    process.exit(1);
    return undefined;
  }
}

// Registers SIGINT/SIGTERM forwarding to `child` and returns handles to
// forward a signal and to mark the child as exited. Once markExited() has
// been called, further signals are not forwarded and the listeners are
// removed — without this, a signal arriving after the child has already
// exited would call child.kill() on a dead process and throw synchronously.
function registerSignalForwarding(child) {
  let childExited = false;

  const forwardSignal = (signal) => {
    if (childExited) return;
    try {
      child.kill(signal);
    } catch {
      // The child may have exited between our check above and kill() below;
      // there's nothing useful to do with that race other than ignore it.
    }
  };

  process.on('SIGINT', forwardSignal);
  process.on('SIGTERM', forwardSignal);

  const markExited = () => {
    childExited = true;
    process.removeListener('SIGINT', forwardSignal);
    process.removeListener('SIGTERM', forwardSignal);
  };

  return { forwardSignal, markExited };
}

function main() {
  const binaryPath = resolveBinaryPath();
  const child = spawnBinary(binaryPath, process.argv.slice(2));

  const { markExited } = registerSignalForwarding(child);

  child.on('error', (err) => {
    markExited();
    console.error(`dragon-head-mcp: failed to start binary: ${err.message}`);
    process.exit(1);
  });

  child.on('exit', (code, signal) => {
    markExited();
    if (signal) {
      if (process.platform === 'win32') {
        // Windows has no POSIX signal delivery: process.kill(pid, signal)
        // cannot re-raise most named signals there, so fall back to a
        // conventional non-zero exit code instead of throwing.
        process.exit(1);
      } else {
        // Re-raise the same signal on this process so a wrapping supervisor
        // sees the real termination cause instead of a synthetic exit code.
        process.kill(process.pid, signal);
      }
    } else {
      process.exit(code ?? 1);
    }
  });
}

module.exports = {
  PLATFORM_PACKAGES,
  resolveBinaryPath,
  resolvePackageBinary,
  isPathInsideDirectory,
  spawnBinary,
  registerSignalForwarding,
};

if (require.main === module) {
  main();
}
