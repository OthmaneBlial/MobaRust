// Record only an explicitly selected native demo window, never the display.
// The target must be an isolated, disposable MobaRust profile.
import { spawn } from 'node:child_process';
import { resolve } from 'node:path';

const [pid, windowId, scene] = process.argv.slice(2);
if (!/^\d+$/.test(pid ?? '') || !/^\d+$/.test(windowId ?? '')) throw Error('Provide demo PID and window ID');
const wait = ms => new Promise(r => setTimeout(r, ms));
const run = (file, args) => new Promise((done, reject) => {
  const child = spawn(file, args, { stdio: 'inherit' });
  child.on('error', reject);
  child.on('exit', code => code === 0 ? done() : reject(Error(`${file}: ${code}`)));
});
const control = (action, label, text) => run(resolve('target/demo-video/control'), [pid, action, label, ...(text === undefined ? [] : [text])]);
const scenes = {
  terminal: { duration: 13, action: async () => {
    await control('type', 'Terminal input', 'uname -sm\n');
    await wait(1400);
    await control('type', 'Terminal input', 'curl -s http://127.0.0.1:4319/health\n');
  }},
  split: { duration: 10, action: async () => {
    await control('press', 'Split terminal right');
    await wait(1200);
    await control('type', 'Terminal input', 'date "+%H:%M:%S"\n');
  }},
  connect: { duration: 11, action: async () => {
    await control('press', 'Quick connect ⌘ K');
    await wait(900);
    await control('type', 'Paste a connection URI optional', 'ssh://demo@example.com:22');
    await wait(900);
    await control('press', 'Apply');
  }},
  snippets: { duration: 12, action: async () => {
    await control('press', 'Snippets');
    await wait(800);
    await control('type', 'Title', 'Check local API');
    await control('type', 'Command', 'curl -s http://127.0.0.1:4319/health');
  }},
  dark: { duration: 9, action: async () => {
    await control('press', 'Switch to dark mode');
    await wait(1500);
    await control('type', 'Terminal input', 'uname -sm\n');
  }},
};
const selected = scenes[scene];
if (!selected) throw Error('Unknown scene');
const capture = run('screencapture', ['-v', '-o', `-l${windowId}`, '-V', String(selected.duration), '-x', `target/demo-video/${scene}.mov`]);
await wait(1800);
await selected.action();
await capture;
console.log(`Captured ${scene}: ${selected.duration}s`);
