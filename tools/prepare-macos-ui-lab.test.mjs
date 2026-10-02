import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { chmodSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, relative } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';
import { prepareMacosUiLab } from './prepare-macos-ui-lab.mjs';

test('lab launcher isolates actual child environment, quotes paths and preserves the source', { skip: process.platform !== 'darwin' }, () => {
  const root = mkdtempSync(join(tmpdir(), 'mobarust-ui-preparation-test-'));
  let cliRoot;
  try {
    // Spaces, apostrophes and shell metacharacters must stay literal.
    const source = join(root, "fixture ' $(must-not-run).app");
    const macos = join(source, 'Contents/MacOS');
    mkdirSync(macos, { recursive: true });
    const binary = join(macos, 'mobarust');
    const probe = '#!/bin/sh\nprintf "%s\\n" "$HOME" "$ZDOTDIR" "$XDG_CONFIG_HOME" "$XDG_DATA_HOME" "$XDG_CACHE_HOME" "$SSH_AUTH_SOCK" "$SSH_AGENT_PID" "$SSH_ASKPASS" "$BASH_ENV" "$ENV" "${LAB_DROP_ME-unset}" "$@"\n';
    writeFileSync(binary, probe); chmodSync(binary, 0o700);
    const info = join(source, 'Contents/Info.plist');
    const original = JSON.stringify({ CFBundleExecutable: 'mobarust', CFBundleIdentifier: 'fixture', LSEnvironment: { HOME: 'ambient' }, CFBundleVersion: '1' });
    writeFileSync(info, original);
    const output = join(root, "output ' $literal");
    const lab = prepareMacosUiLab(relative(process.cwd(), source), relative(process.cwd(), output));
    const result = spawnSync(lab.launcher, ['argument with spaces', '$literal'], {
      encoding: 'utf8', env: { HOME: '/ambient', ZDOTDIR: '/ambient', SSH_AUTH_SOCK: '/ambient-agent', LAB_DROP_ME: 'must-not-survive' },
    });
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(result.stdout.split('\n'), [lab.home, lab.home, join(lab.home, 'config'), join(lab.home, 'data'), join(lab.home, 'cache'), '', '', '', '/dev/null', '/dev/null', 'unset', 'argument with spaces', '$literal', '']);
    assert.equal(lstatSync(lab.root).mode & 0o777, 0o700);
    assert.equal(lstatSync(lab.launcher).mode & 0o777, 0o700);
    assert.equal(lstatSync(join(lab.root, 'owned.json')).mode & 0o777, 0o600);
    assert.equal(readFileSync(lab.binary, 'utf8'), probe);
    assert.equal(readFileSync(binary, 'utf8'), probe);
    assert.equal(readFileSync(info, 'utf8'), original);
    assert.equal(existsSync(join(macos, 'portable.flag')), false);
    const plist = spawnSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', join(lab.app, 'Contents/Info.plist')], { encoding: 'utf8' });
    assert.equal(plist.status, 0);
    const prepared = JSON.parse(plist.stdout);
    assert.equal(prepared.CFBundleExecutable, 'mobarust');
    assert.equal(prepared.CFBundleIdentifier, lab.bundleId);
    assert.equal(prepared.LSEnvironment, undefined);
    const launcherPlist = spawnSync('/usr/bin/plutil', ['-convert', 'json', '-o', '-', join(lab.launcherApp, 'Contents/Info.plist')], { encoding: 'utf8' });
    assert.equal(launcherPlist.status, 0);
    assert.equal(JSON.parse(launcherPlist.stdout).CFBundleExecutable, 'MobaRustLabLauncher');
    assert.equal(JSON.parse(launcherPlist.stdout).CFBundleIdentifier, lab.launcherBundleId);
    assert.deepEqual(JSON.parse(readFileSync(join(lab.app, 'Contents/MacOS/portable-data/sessions.json'))), { schema_version: 1, sessions: [] });
    const cli = spawnSync(process.execPath, [fileURLToPath(new URL('./prepare-macos-ui-lab.mjs', import.meta.url)), source], { encoding: 'utf8' });
    assert.equal(cli.status, 0, cli.stderr);
    const cliLab = JSON.parse(cli.stdout);
    cliRoot = cliLab.root;
    assert.equal(cliLab.sha256, lab.sha256, 'CLI stdout is a usable preparation receipt');
    assert.notEqual(prepareMacosUiLab(source, output).bundleId, lab.bundleId);
    const before = readdirSync(output);
    symlinkSync(binary, join(macos, 'escape'));
    assert.throws(() => prepareMacosUiLab(source, output), /symlinks/);
    assert.deepEqual(readdirSync(output), before, 'failed copy removes only its newly owned directory');
    rmSync(join(macos, 'escape'));
    mkdirSync(join(macos, 'portable-data'));
    assert.throws(() => prepareMacosUiLab(source, output), /without portable state/);
    assert.deepEqual(readdirSync(output), before);
  } finally {
    if (cliRoot) rmSync(cliRoot, { recursive: true, force: true });
    rmSync(root, { recursive: true, force: true });
  }
});

test('lab launcher preserves the native main bundle identity', { skip: process.platform !== 'darwin' }, () => {
  const root = mkdtempSync(join(tmpdir(), 'mobarust-main-bundle-test-'));
  try {
    const source = join(root, 'metadata fixture.app');
    const macos = join(source, 'Contents/MacOS');
    mkdirSync(macos, { recursive: true });
    const binary = join(macos, 'mobarust');
    // Foundation metadata only: this probe creates no application or window.
    const probe = '#import <Foundation/Foundation.h>\n#include <stdio.h>\nint main(void) { @autoreleasepool { NSBundle *b = NSBundle.mainBundle; printf("%s\\n%s\\n", b.bundleIdentifier.UTF8String ?: "", [[b objectForInfoDictionaryKey:@"CFBundleExecutable"] UTF8String] ?: ""); } return 0; }\n';
    const embedded = join(root, 'embedded.plist');
    writeFileSync(embedded, '<?xml version="1.0"?><plist version="1.0"><dict><key>CFBundleIdentifier</key><string>fixture.embedded</string><key>CFBundleExecutable</key><string>mobarust</string><key>CFBundleVersion</key><string>0</string></dict></plist>');
    const compile = spawnSync('/usr/bin/clang', ['-x', 'objective-c', '-framework', 'Foundation', '-Xlinker', '-sectcreate', '-Xlinker', '__TEXT', '-Xlinker', '__info_plist', '-Xlinker', embedded, '-o', binary, '-'], { input: probe, encoding: 'utf8' });
    assert.equal(compile.status, 0, compile.stderr);
    writeFileSync(join(source, 'Contents/Info.plist'), JSON.stringify({ CFBundleExecutable: 'mobarust', CFBundleIdentifier: 'fixture.metadata', CFBundlePackageType: 'APPL', CFBundleVersion: '1' }));
    const lab = prepareMacosUiLab(source, join(root, 'output'));
    const result = spawnSync(lab.launcher, [], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, `${lab.bundleId}\nmobarust\n`);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
