import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('../', import.meta.url));
for (const [command, args, cwd] of [
  ['pnpm', ['build'], `${root}/apps/desktop`],
  ['cargo', ['run', '--locked', '--manifest-path', `${root}/xtask/Cargo.toml`, '--', 'stage-helpers'], root],
]) {
  const result = spawnSync(command, args, {
    cwd, stdio: 'inherit', shell: process.platform === 'win32',
  });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status ?? 1);
}
