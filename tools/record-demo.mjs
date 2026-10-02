// Capture only an explicitly selected disposable native window.
// Drive the actual application separately; never capture the whole display.
import { spawnSync } from 'node:child_process';
import { mkdirSync } from 'node:fs';

const [windowId, scene, seconds = '12'] = process.argv.slice(2);
if (!/^\d+$/.test(windowId ?? '') || !/^[a-z]+$/.test(scene ?? '') || !/^\d+$/.test(seconds) || Number(seconds) < 3 || Number(seconds) > 30) {
  throw Error('Usage: node tools/record-demo.mjs DEMO_WINDOW_ID SCENE SECONDS (3–30)');
}
const root = 'target/readme-demo-v017/captures';
mkdirSync(root, { recursive: true });
const result = spawnSync('screencapture', ['-v', '-o', `-l${windowId}`, '-V', seconds, '-x', `${root}/${scene}.mov`], { stdio: 'inherit' });
if (result.status !== 0) throw Error(`Recording failed: ${result.status}`);
console.log(`Captured ${scene}: ${seconds}s`);
