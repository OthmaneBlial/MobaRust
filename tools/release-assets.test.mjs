import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';

const script = resolve('tools/release-assets.mjs');
function fixture(run) {
  const cwd = mkdtempSync(join(tmpdir(), 'mobarust-release-test-'));
  const put = (name, content) => {
    const destination = join(cwd, name);
    mkdirSync(resolve(destination, '..'), { recursive: true });
    writeFileSync(destination, content);
  };
  const invoke = (...args) => spawnSync(process.execPath, [script, ...args], {
    cwd, encoding: 'utf8', env: { ...process.env, GITHUB_REF_NAME: 'v0.1.0' },
  });
  try {
    put('apps/desktop/src-tauri/tauri.conf.json', '{"version":"0.1.0"}');
    put('apps/desktop/package.json', '{"version":"0.1.0"}');
    put('Cargo.toml', '[workspace.package]\nversion = "0.1.0"\n');
    run({ cwd, put, invoke });
  } finally {
    rmSync(cwd, { recursive: true, force: true });
  }
}

test('release rejects inconsistent versions', () => fixture(({ put, invoke }) => {
  assert.equal(invoke('check-version').status, 0);
  put('apps/desktop/package.json', '{"version":"0.2.0"}');
  assert.notEqual(invoke('check-version').status, 0);
}));

test('Linux release requires both installers and hashes the delivered bytes', () => fixture(({ cwd, put, invoke }) => {
  put('apps/desktop/src-tauri/helpers/mobarust-vnc-helper', 'helper');
  put('target/release/bundle/deb/test.deb', 'debian-package');
  assert.notEqual(invoke('collect', 'linux-x64').status, 0);
  put('target/release/bundle/appimage/test.AppImage', 'appimage-package');
  assert.equal(invoke('collect', 'linux-x64').status, 0);
  const manifest = readFileSync(join(cwd, 'target/release-assets/SHA256SUMS-linux-x64.txt'), 'utf8');
  for (const line of manifest.trim().split('\n')) {
    const [hash, name] = line.split('  ');
    const bytes = readFileSync(join(cwd, 'target/release-assets', name));
    assert.equal(hash, createHash('sha256').update(bytes).digest('hex'));
  }
  put('apps/desktop/src-tauri/helpers/mobarust-rdp-helper', 'must-not-ship');
  assert.notEqual(invoke('collect', 'linux-x64').status, 0);
}));
