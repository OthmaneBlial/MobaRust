import { createHash } from 'node:crypto';
import { copyFileSync, existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { basename, join } from 'node:path';

const config = JSON.parse(readFileSync('apps/desktop/src-tauri/tauri.conf.json', 'utf8'));
if (process.argv[2] === 'check-version') {
  const tag = process.env.GITHUB_REF_NAME;
  const frontend = JSON.parse(readFileSync('apps/desktop/package.json', 'utf8'));
  const cargo = readFileSync('Cargo.toml', 'utf8').match(/\[workspace.package\]\s+version = "([^"]+)"/)[1];
  if (tag !== `v${config.version}` || frontend.version !== config.version || cargo !== config.version) {
    throw new Error('Tag, Tauri, Cargo, and frontend versions must match');
  }
} else if (process.argv[2] === 'collect') {
  const platform = process.argv[3];
  if (!['windows-x64', 'linux-x64', 'macos-arm64', 'macos-x64'].includes(platform)) throw new Error('Unknown platform');
  const helperDir = 'apps/desktop/src-tauri/helpers';
  const helper = `mobarust-vnc-helper${platform.startsWith('windows') ? '.exe' : ''}`;
  if (!existsSync(join(helperDir, helper)) || readdirSync(helperDir).some(name => name.includes('rdp'))) throw new Error('Invalid helper staging');
  const extensions = platform.startsWith('windows') ? ['.exe'] : platform.startsWith('linux') ? ['.deb', '.AppImage'] : ['.dmg'];
  function walk(dir) {
    return readdirSync(dir, { withFileTypes: true }).flatMap(entry => entry.isDirectory() ? walk(join(dir, entry.name)) : [join(dir, entry.name)]);
  }
  const files = walk('target/release/bundle').filter(path => extensions.some(ext => path.endsWith(ext)));
  if (files.length !== extensions.length) throw new Error(`Expected ${extensions.length} installers, found ${files.length}`);
  const out = 'target/release-assets';
  mkdirSync(out, { recursive: true });
  const checksums = files.map(path => {
    const name = `MobaRust-${config.version}-${platform}${extensions.find(ext => path.endsWith(ext))}`;
    copyFileSync(path, join(out, name));
    return `${createHash('sha256').update(readFileSync(path)).digest('hex')}  ${basename(name)}`;
  });
  writeFileSync(join(out, `SHA256SUMS-${platform}.txt`), `${checksums.join('\n')}\n`);
} else {
  throw new Error('Use check-version or collect <platform>');
}
