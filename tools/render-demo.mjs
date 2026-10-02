// Edit actual native recordings; no application screens are synthesized.
import { spawnSync } from 'node:child_process';
import { mkdirSync, writeFileSync } from 'node:fs';
const run = (command, args) => {
  const result = spawnSync(command, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  if (result.status !== 0) throw Error(`${command}: ${result.stderr}`);
  return result.stdout;
};
const root = 'target/readme-demo-v017/captures';
const scenes = [['terminal','terminal',10], ['split','split',10], ['connect','connect',10], ['files','files',14], ['transfer','files',5], ['tunnel','tunnel',10], ['request','tunnel',10], ['snippets','tools',10], ['dark','dark',8]];
mkdirSync('site/media', { recursive: true });
const encoding = ['-an', '-c:v', 'libx264', '-crf', '23', '-preset', 'slow', '-profile:v', 'high', '-level', '4.0', '-pix_fmt', 'yuv420p', '-colorspace', 'bt709', '-color_trc', 'bt709', '-color_primaries', 'bt709', '-color_range', 'tv', '-g', '60', '-movflags', '+faststart'];
for (const [name, duration] of [['intro', 3], ['outro', 4]]) {
  run('ffmpeg', ['-y', '-v', 'error', '-loop', '1', '-framerate', '30', '-i', `${root}/titles/${name}.png`, '-t', String(duration), '-vf', 'setsar=1,fade=t=in:d=0.2:color=0x101b18', ...encoding, `${root}/${name}.mp4`]);
}
const selected = new Set(process.argv.slice(2));
for (const [name, title, duration] of scenes) {
  if (selected.size && !selected.has(name)) continue;
  const input = `${root}/${name}.mov`;
  const metadata = JSON.parse(run('ffprobe', ['-v', 'error', '-show_streams', '-show_format', '-of', 'json', input]));
  const video = metadata.streams.find(s => s.codec_type === 'video');
  if (video?.width !== 2560 || video.height !== 1436) throw Error(`Unexpected window geometry: ${name}`);
  if (Number(metadata.format.duration) < duration - .2) throw Error(`Incomplete recording: ${name}`);
  // Obscure the local account label and absolute fixture download path.
  const masks = { transfer: [305, 1166, 1000, 30], connect: [495, 728, 118, 34], dark: [495, 796, 118, 34], request: [459, 728, 118, 34] };
  const mask = masks[name];
  const privacy = mask ? `drawbox=x=${mask[0]}:y=${mask[1]}:w=${mask[2]}:h=${mask[3]}:color=0x0d1411:t=fill,` : '';
  const graph = `[0:v]${privacy}crop=2560:1370:0:56,fps=30,scale=1776:950,setsar=1,pad=1920:1080:72:112:color=0x101b18[app];[app][1:v]overlay=0:0,fade=t=in:d=0.16:color=0x101b18,fade=t=out:st=${duration - .16}:d=0.16:color=0x101b18[v]`;
  run('ffmpeg', ['-y', '-v', 'error', '-i', input, '-i', `${root}/titles/${title}.png`, '-filter_complex', graph, '-map', '[v]', '-t', String(duration), ...encoding, `${root}/${name}.mp4`]);
  console.log(`Rendered ${name}`);
}
writeFileSync(`${root}/concat.txt`, ['intro', ...scenes.map(([name]) => name), 'outro'].map(name => `file '${name}.mp4'`).join('\n'));
run('ffmpeg', ['-y', '-v', 'error', '-f', 'concat', '-safe', '0', '-i', `${root}/concat.txt`, '-c', 'copy', '-movflags', '+faststart', 'site/media/mobarust-desktop-demo.mp4']);
run('ffmpeg', ['-y', '-v', 'error', '-ss', '18', '-i', 'site/media/mobarust-desktop-demo.mp4', '-frames:v', '1', '-q:v', '2', 'site/media/mobarust-demo-poster.jpg']);
console.log(run('ffprobe', ['-v', 'error', '-show_entries', 'stream=codec_name,width,height,pix_fmt:format=duration,size', '-of', 'json', 'site/media/mobarust-desktop-demo.mp4']));
