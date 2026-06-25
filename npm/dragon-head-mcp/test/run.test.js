'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');
const fs = require('node:fs');
const os = require('node:os');
const { execFileSync, spawnSync } = require('node:child_process');

const { PLATFORM_PACKAGES, resolveBinaryPath } = require('../bin/run.js');

const WRAPPER_PKG = require('../package.json');
const PLATFORM_ROOT = path.join(__dirname, '..', '..', 'platform');

function platformPackageJson(dirName) {
  return require(path.join(PLATFORM_ROOT, dirName, 'package.json'));
}

// Run `fn`, capturing the message passed to the first `process.exit` call
// instead of actually exiting the test process. Mirrors the project's Rust
// convention of injecting test seams rather than mutating real global state
// (see CLAUDE.md #11) — here the seam is a temporarily stubbed process.exit.
function captureExit(fn) {
  const originalExit = process.exit;
  const originalError = console.error;
  let exitCode;
  let errorOutput = '';
  process.exit = (code) => {
    exitCode = code;
    throw new Error('__test_process_exit__');
  };
  console.error = (msg) => {
    errorOutput += `${msg}\n`;
  };
  try {
    fn();
  } catch (err) {
    if (err.message !== '__test_process_exit__') throw err;
  } finally {
    process.exit = originalExit;
    console.error = originalError;
  }
  return { exitCode, errorOutput };
}

test('PLATFORM_PACKAGES, wrapper optionalDependencies, and platform package names all agree', () => {
  const platformDirs = fs
    .readdirSync(PLATFORM_ROOT, { withFileTypes: true })
    .filter((entry) => entry.isDirectory())
    .map((entry) => entry.name)
    .sort();

  const namesFromPlatformPackages = platformDirs
    .map((dir) => platformPackageJson(dir).name)
    .sort();

  const namesFromRunJs = Object.values(PLATFORM_PACKAGES).sort();
  const namesFromOptionalDeps = Object.keys(WRAPPER_PKG.optionalDependencies).sort();

  assert.deepEqual(
    namesFromRunJs,
    namesFromOptionalDeps,
    'run.js PLATFORM_PACKAGES values must exactly match wrapper package.json optionalDependencies keys'
  );
  assert.deepEqual(
    namesFromRunJs,
    namesFromPlatformPackages,
    'run.js PLATFORM_PACKAGES values must exactly match every npm/platform/*/package.json "name" field'
  );
});

test('every optionalDependency version is exact-pinned (no ^ or ~ range)', () => {
  for (const version of Object.values(WRAPPER_PKG.optionalDependencies)) {
    assert.doesNotMatch(version, /^[\^~]/, `optionalDependency version "${version}" must be exact-pinned`);
  }
});

test('win32 platform package is the only one with an .exe dragonHeadBinary', () => {
  for (const dir of fs.readdirSync(PLATFORM_ROOT)) {
    const pkg = platformPackageJson(dir);
    if (dir.startsWith('win32-')) {
      assert.match(pkg.dragonHeadBinary, /\.exe$/, `${dir} should declare an .exe binary`);
    } else {
      assert.doesNotMatch(pkg.dragonHeadBinary, /\.exe$/, `${dir} should not declare an .exe binary`);
    }
  }
});

test('resolveBinaryPath rejects an unsupported platform with exit code 1', () => {
  const { exitCode, errorOutput } = captureExit(() => resolveBinaryPath('freebsd', 'x64'));
  assert.equal(exitCode, 1);
  assert.match(errorOutput, /unsupported platform "freebsd,x64"/);
});

test('unsupported-platform message only hints at musl/Alpine for a supported Linux arch', () => {
  const { errorOutput: x64Output } = captureExit(() => resolveBinaryPath('linux', 'x64'));
  // linux,x64 IS supported, so force an unsupported combo on a supported arch
  // by using an arch that isn't in PLATFORM_PACKAGES at all but IS musl-relevant:
  const { errorOutput: armOutput } = captureExit(() => resolveBinaryPath('linux', 'arm'));
  const { errorOutput: ia32Output } = captureExit(() => resolveBinaryPath('linux', 'ia32'));

  assert.doesNotMatch(x64Output, /unsupported platform/); // linux,x64 is supported — no error at all
  assert.doesNotMatch(armOutput, /musl\/Alpine/, '32-bit arm is an architecture gap, not a libc gap');
  assert.doesNotMatch(ia32Output, /musl\/Alpine/, 'ia32 is an architecture gap, not a libc gap');
});

test('resolveBinaryPath errors when the optional dependency is not installed', () => {
  const { exitCode, errorOutput } = captureExit(() => resolveBinaryPath('darwin', 'arm64'));
  // In this test environment the platform optional dependency is never
  // actually installed (it's only staged by CI at publish time), so this
  // exercises the "not installed" branch end-to-end.
  assert.equal(exitCode, 1);
  assert.match(errorOutput, /optional dependency "dragon-head-mcp-darwin-arm64" is not installed/);
});

test('end-to-end: packed wrapper + platform tarballs resolve and exec a fake binary with correct exit code', (t) => {
  const platform = os.platform();
  const arch = os.arch();
  const platformDir = `${platform}-${arch}`;
  if (!fs.existsSync(path.join(PLATFORM_ROOT, platformDir))) {
    t.skip(`no platform package for ${platformDir} on this CI runner`);
    return;
  }

  const tmpRoot = fs.mkdtempSync(path.join(os.tmpdir(), 'dragon-head-mcp-e2e-'));
  const fakeBinaryDir = path.join(tmpRoot, 'fakebin');
  fs.mkdirSync(fakeBinaryDir, { recursive: true });

  // Stage a fake binary into a throwaway copy of the platform package so we
  // never touch the real npm/platform/*/bin (which must stay binary-free in git).
  const stagedPlatformDir = path.join(tmpRoot, 'platform-src');
  fs.cpSync(path.join(PLATFORM_ROOT, platformDir), stagedPlatformDir, { recursive: true });
  const pkg = require(path.join(stagedPlatformDir, 'package.json'));
  const binaryName = path.basename(pkg.dragonHeadBinary);
  const stagedBinaryPath = path.join(stagedPlatformDir, 'bin', binaryName);
  fs.mkdirSync(path.dirname(stagedBinaryPath), { recursive: true });
  fs.writeFileSync(stagedBinaryPath, '#!/usr/bin/env bash\necho FAKE_BINARY_RAN "$@"\nexit 7\n');
  fs.chmodSync(stagedBinaryPath, 0o755);

  const packOutDir = path.join(tmpRoot, 'tarballs');
  fs.mkdirSync(packOutDir);
  const wrapperDir = path.join(__dirname, '..');

  const packOne = (dir) => {
    const out = execFileSync('npm', ['pack', '--silent', '--pack-destination', packOutDir], {
      cwd: dir,
    })
      .toString()
      .trim();
    return path.join(packOutDir, out);
  };

  const wrapperTarball = packOne(wrapperDir);
  const platformTarball = packOne(stagedPlatformDir);

  const projectDir = path.join(tmpRoot, 'proj');
  fs.mkdirSync(projectDir);
  fs.writeFileSync(path.join(projectDir, 'package.json'), JSON.stringify({ name: 'e2e', version: '0.0.0' }));
  execFileSync('npm', ['install', '--no-audit', '--no-fund', wrapperTarball, platformTarball], {
    cwd: projectDir,
  });

  const result = spawnSync('node', [path.join(projectDir, 'node_modules', 'dragon-head-mcp', 'bin', 'run.js'), '--doctor'], {
    encoding: 'utf8',
  });

  assert.equal(result.status, 7, `expected the fake binary's exit code to propagate, got: ${result.stderr}`);
  assert.match(result.stdout, /FAKE_BINARY_RAN --doctor/);

  fs.rmSync(tmpRoot, { recursive: true, force: true });
});
