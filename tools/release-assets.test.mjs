import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { existsSync, mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('./release-assets.mjs', import.meta.url));
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
    put('apps/desktop/src-tauri/tauri.conf.json', '{"productName":"MobaRust","version":"0.1.0"}');
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
  put('target/release/bundle/deb/MobaRust_0.1.0_amd64.deb', 'debian-package');
  assert.notEqual(invoke('collect', 'linux-x64').status, 0);
  put('target/release/bundle/appimage/MobaRust_0.1.0_amd64.AppImage', 'appimage-package');
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

test('Windows collector preserves installer bytes and hashes the delivered file', () => fixture(({ cwd, put, invoke }) => {
  put('apps/desktop/src-tauri/helpers/mobarust-vnc-helper.exe', 'helper');
  put('target/release/bundle/nsis/MobaRust_0.1.0_x64-setup.exe', 'windows-package');
  assert.equal(invoke('collect', 'windows-x64').status, 0);
  const name = 'MobaRust-0.1.0-windows-x64.exe';
  assert.equal(readFileSync(join(cwd, 'target/release-assets', name), 'utf8'), 'windows-package');
  assert.equal(readFileSync(join(cwd, 'target/release-assets/SHA256SUMS-windows-x64.txt'), 'utf8'),
    `${createHash('sha256').update('windows-package').digest('hex')}  ${name}\n`);
}));

test('collector rejects source product, version and architecture mismatches before writing', () => {
  const packages = {
    'windows-x64': ['MobaRust_0.1.0_x64-setup.exe'],
    'linux-x64': ['MobaRust_0.1.0_amd64.deb', 'MobaRust_0.1.0_amd64.AppImage'],
    'macos-arm64': ['MobaRust_0.1.0_aarch64.dmg'],
    'macos-x64': ['MobaRust_0.1.0_x64.dmg'],
  };
  for (const [platform, names] of Object.entries(packages)) {
    for (const name of names) {
      const wrongArch = name.replace(/amd64|aarch64|x64/, platform === 'macos-arm64' ? 'x64'
        : name.endsWith('.dmg') || name.endsWith('.AppImage') ? 'aarch64' : 'arm64');
      for (const wrongName of [name.replace('0.1.0', '0.0.9'), name.replace('MobaRust', 'OtherApp'), wrongArch]) {
        fixture(({ cwd, put, invoke }) => {
          put(`apps/desktop/src-tauri/helpers/mobarust-vnc-helper${platform === 'windows-x64' ? '.exe' : ''}`, 'helper');
          for (const original of names) put(`target/release/bundle/${original === name ? wrongName : original}`, 'package-bytes');
          const result = invoke('collect', platform);
          assert.notEqual(result.status, 0, `${platform} must refuse ${wrongName}`);
          assert.match(result.stderr, /Installer name does not match/);
          assert.equal(existsSync(join(cwd, 'target/release-assets')), false);
          assert.equal(readFileSync(join(cwd, 'target/release/bundle', wrongName), 'utf8'), 'package-bytes');
        });
      }
    }
  }
});

test('macOS collector rejects an ARM DMG labeled as Intel', () => fixture(({ cwd, put, invoke }) => {
  put('apps/desktop/src-tauri/helpers/mobarust-vnc-helper', 'helper');
  put('target/release/bundle/dmg/MobaRust_0.1.0_aarch64.dmg', 'arm-package');
  assert.notEqual(invoke('collect', 'macos-x64').status, 0);
  assert.equal(invoke('collect', 'macos-arm64').status, 0);
  put('target/x86_64-apple-darwin/release/bundle/dmg/MobaRust_0.1.0_x64.dmg', 'intel-package');
  assert.equal(invoke('collect', 'macos-x64', 'target/x86_64-apple-darwin/release/bundle').status, 0);
  const manifest = readFileSync(join(cwd, 'target/release-assets/SHA256SUMS-macos-x64.txt'), 'utf8');
  assert.equal(manifest, `${createHash('sha256').update('intel-package').digest('hex')}  MobaRust-0.1.0-macos-x64.dmg\n`);
}));

test('Linux collector rejects duplicate formats before writing output', () => {
  for (const extension of ['.deb', '.AppImage']) fixture(({ cwd, put, invoke }) => {
    put('apps/desktop/src-tauri/helpers/mobarust-vnc-helper', 'helper');
    put(`target/release/bundle/first${extension}`, 'first-package');
    put(`target/release/bundle/second${extension}`, 'second-package');
    const result = invoke('collect', 'linux-x64');
    assert.notEqual(result.status, 0, `Two ${extension} files must not replace the required pair`);
    assert.match(result.stderr, /Expected exactly one installer per format/);
    assert.equal(existsSync(join(cwd, 'target/release-assets')), false);
    assert.equal(readFileSync(join(cwd, `target/release/bundle/first${extension}`), 'utf8'), 'first-package');
    assert.equal(readFileSync(join(cwd, `target/release/bundle/second${extension}`), 'utf8'), 'second-package');
  });
});

test('collector refuses stale installers without deleting them', () => fixture(({ cwd, put, invoke }) => {
  put('apps/desktop/src-tauri/helpers/mobarust-vnc-helper', 'helper');
  put('target/release/bundle/dmg/MobaRust_0.1.0_aarch64.dmg', 'current-package');
  put('target/release-assets/MobaRust-0.0.9-macos-arm64.dmg', 'old-package');
  assert.notEqual(invoke('collect', 'macos-arm64').status, 0);
  assert.equal(readFileSync(join(cwd, 'target/release-assets/MobaRust-0.0.9-macos-arm64.dmg'), 'utf8'), 'old-package');
  assert.equal(invoke('collect', 'macos-arm64', 'target/release/bundle', 'target/release-assets/v0.1.0').status, 0);
  assert.equal(readFileSync(join(cwd, 'target/release-assets/v0.1.0/MobaRust-0.1.0-macos-arm64.dmg'), 'utf8'), 'current-package');
}));
