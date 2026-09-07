// Edit the real native screen recordings with the web-optimized FFmpeg profile.
// Prerequisites: captures from record-demo.mjs, overlays from demo-titles.swift.
import { spawnSync } from 'node:child_process';
import { mkdirSync } from 'node:fs';

const run = (command, args) => {
  const result = spawnSync(command, args, { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  if (result.status !== 0) throw Error(`${command}: ${result.stderr}`);
  return result.stdout;
};
const root = 'target/demo-video';
mkdirSync('site/media', { recursive: true });
const encoding = ['-an', '-c:v', 'libx264', '-crf', '20', '-preset', 'slow', '-profile:v', 'high', '-level', '4.0', '-pix_fmt', 'yuv420p', '-colorspace', 'bt709', '-color_trc', 'bt709', '-color_primaries', 'bt709', '-color_range', 'tv', '-g', '60', '-movflags', '+faststart'];
for (const [name, duration] of [['intro', 3], ['outro', 4]]) {
  run('ffmpeg', ['-y', '-v', 'error', '-loop', '1', '-framerate', '30', '-i', `${root}/titles/${name}.png`, '-t', String(duration), '-vf', 'setsar=1,fade=t=in:d=0.2:color=0xf3f5ef', ...encoding, `${root}/${name}.mp4`]);
}
for (const [name, title, duration] of [['terminal','terminal',13], ['split','split',10], ['connect','connect',11], ['snippets','tools',12], ['dark','dark',9]]) {
  const input = `${root}/${name}.mov`;
  const metadata = JSON.parse(run('ffprobe', ['-v', 'error', '-show_streams', '-show_format', '-of', 'json', input]));
  const video = metadata.streams.find(s => s.codec_type === 'video');
  if (video?.width !== 2560 || video.height !== 1426) throw Error(`Unexpected window geometry: ${name}`);
  if (Number(metadata.format.duration) < duration - .2) throw Error(`Incomplete recording: ${name}`);
  const background = name === 'dark' ? '0x101615' : '0xf3f5ef';
  // Remove only the native title bar. Keep the entire application content.
  const graph = `[0:v]crop=2560:1370:0:56,fps=30,scale=1776:950,setsar=1,pad=1920:1080:72:112:color=${background}[app];[app][1:v]overlay=0:0,fade=t=in:d=0.16:color=${background},fade=t=out:st=${duration - .16}:d=0.16:color=${background}[v]`;
  run('ffmpeg', ['-y', '-v', 'error', '-i', input, '-i', `${root}/titles/${title}.png`, '-filter_complex', graph, '-map', '[v]', '-t', String(duration), ...encoding, `${root}/${name}.mp4`]);
  console.log(`Rendered ${name}`);
}
run('ffmpeg', ['-y', '-v', 'error', '-f', 'concat', '-safe', '0', '-i', 'tools/demo-concat.txt', '-c', 'copy', '-movflags', '+faststart', 'site/media/mobarust-desktop-demo.mp4']);
run('ffmpeg', ['-y', '-v', 'error', '-ss', '12', '-i', 'site/media/mobarust-desktop-demo.mp4', '-frames:v', '1', '-q:v', '2', 'site/media/mobarust-demo-poster.jpg']);
console.log(run('ffprobe', ['-v', 'error', '-show_entries', 'stream=codec_name,width,height,pix_fmt:format=duration,size', '-of', 'json', 'site/media/mobarust-desktop-demo.mp4']));
