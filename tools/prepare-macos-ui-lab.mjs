// Prepare only: launching and native UI observation are separate acceptance gates.
import { spawnSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { chmodSync, cpSync, existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const repo = fileURLToPath(new URL('../', import.meta.url));
const quote = value => `'${value.replaceAll("'", "'\\''")}'`;
const sha256 = file => createHash('sha256').update(readFileSync(file)).digest('hex');
const plutil = args => {
  const result = spawnSync('/usr/bin/plutil', args, { encoding: 'utf8' });
  if (result.error) throw result.error;
  if (result.status !== 0) throw Error(`Lab plist preparation failed: ${result.stderr}`);
  return result.stdout;
};

export function prepareMacosUiLab(bundle, outputDirectory) {
  if (process.platform !== 'darwin') throw Error('This preparation tool requires macOS.');
  bundle = resolve(bundle);
  outputDirectory = resolve(outputDirectory);
  const executable = join(bundle, 'Contents/MacOS/mobarust');
  if (!lstatSync(executable).isFile() || !(lstatSync(executable).mode & 0o111)) {
    throw Error('The source mobarust executable must be a regular executable file.');
  }
  for (const name of ['portable.flag', 'portable-data']) {
    if (existsSync(join(dirname(executable), name))) {
      throw Error('Use a clean built bundle without portable state.');
    }
  }
  mkdirSync(outputDirectory, { recursive: true });
  const root = mkdtempSync(join(outputDirectory, 'mobarust-ui-lab-'));
  try {
    const suffix = randomUUID().replaceAll('-', '');
    const name = `MobaRust UI Lab ${suffix.slice(0, 8)}`;
    const app = join(root, `${name}.app`);
    cpSync(bundle, app, { recursive: true, filter: path => {
      if (lstatSync(path).isSymbolicLink()) throw Error('Lab source must not contain symlinks.');
      return true;
    } });
    const binary = join(app, 'Contents/MacOS/mobarust');
    const home = join(root, 'home');
    mkdirSync(home, { mode: 0o700 });
    for (const directory of ['config', 'data', 'cache']) mkdirSync(join(home, directory), { mode: 0o700 });
    const environment = {
      PATH: '/usr/bin:/bin:/usr/sbin:/sbin', LANG: 'en_US.UTF-8', SHELL: '/bin/zsh',
      HOME: home, ZDOTDIR: home, XDG_CONFIG_HOME: join(home, 'config'),
      XDG_DATA_HOME: join(home, 'data'), XDG_CACHE_HOME: join(home, 'cache'),
      BASH_ENV: '/dev/null', ENV: '/dev/null', SSH_AUTH_SOCK: '', SSH_AGENT_PID: '', SSH_ASKPASS: '',
    };
    const info = join(app, 'Contents/Info.plist');
    const plist = JSON.parse(plutil(['-convert', 'json', '-o', '-', info]));
    delete plist.LSEnvironment; // LaunchServices can replace HOME/agent settings here.
    Object.assign(plist, { CFBundleIdentifier: `com.othmane.mobarust.uilab.lab${suffix}`, CFBundleExecutable: 'mobarust', CFBundleName: name, CFBundleDisplayName: name });
    writeFileSync(info, JSON.stringify(plist));
    plutil(['-convert', 'xml1', info]);
    // Keep the runtime's main executable matched to its own bundle metadata.
    const launcherApp = join(root, `${name} Launcher.app`);
    mkdirSync(join(launcherApp, 'Contents/MacOS'), { recursive: true, mode: 0o700 });
    const launcher = join(launcherApp, 'Contents/MacOS/MobaRustLabLauncher');
    writeFileSync(launcher, `#!/bin/sh\nexec /usr/bin/env -i ${Object.entries(environment).map(([key, value]) => quote(`${key}=${value}`)).join(' ')} ${quote(binary)} "$@"\n`, { flag: 'wx', mode: 0o700 });
    chmodSync(launcher, 0o700);
    const launcherBundleId = `${plist.CFBundleIdentifier}.launcher`;
    const launcherInfo = join(launcherApp, 'Contents/Info.plist');
    writeFileSync(launcherInfo, JSON.stringify({ CFBundleIdentifier: launcherBundleId, CFBundleExecutable: 'MobaRustLabLauncher', CFBundleName: `${name} Launcher`, CFBundleDisplayName: `${name} Launcher`, CFBundlePackageType: 'APPL', CFBundleInfoDictionaryVersion: '6.0', CFBundleVersion: plist.CFBundleVersion ?? '1' }));
    plutil(['-convert', 'xml1', launcherInfo]);
    writeFileSync(join(dirname(binary), 'portable.flag'), '', { flag: 'wx', mode: 0o600 });
    const data = join(dirname(binary), 'portable-data');
    mkdirSync(data, { mode: 0o700 });
    writeFileSync(join(data, 'sessions.json'), JSON.stringify({ schema_version: 1, sessions: [] }), { flag: 'wx', mode: 0o600 });
    const digest = sha256(binary);
    if (digest !== sha256(executable)) throw Error('Lab executable differs from its source.');
    const receipt = { root, app, binary, launcherApp, launcher, launcherBundleId, home, sha256: digest, bundleId: plist.CFBundleIdentifier };
    writeFileSync(join(root, 'owned.json'), JSON.stringify(receipt, null, 2), { flag: 'wx', mode: 0o600 });
    return receipt;
  } catch (error) {
    rmSync(root, { recursive: true, force: true }); // Only this invocation's generated directory.
    throw error;
  }
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  if (process.argv.length > 3) throw Error('Usage: node tools/prepare-macos-ui-lab.mjs [CLEAN_BUILT_APP]');
  const receipt = prepareMacosUiLab(resolve(process.argv[2] ?? join(repo, 'target/debug/bundle/macos/MobaRust.app')), join(repo, 'target'));
  console.log(JSON.stringify(receipt, null, 2));
  console.error('Prepared an unsigned, disposable lab copy; no app or fixture was started.');
  console.error('Open launcherApp to start the isolated runtime, then select the running app by bundleId. Verify its process environment and native window before starting loopback fixtures.');
}
