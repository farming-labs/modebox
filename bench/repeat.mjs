// Runs compare.mjs for both packages several times, alternating between them,
// and prints the median of each row:  node bench/repeat.mjs [runs]
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const runs = Number(process.argv[2] ?? 5);
const compare = fileURLToPath(new URL('./compare.mjs', import.meta.url));
const results = { run: [], modebox: [] };
for (let i = 0; i < runs; i++) {
  for (const side of Object.keys(results)) {
    const out = execFileSync(process.execPath, ['--no-warnings', compare, side], { encoding: 'utf8' });
    results[side].push(JSON.parse(out));
  }
}

const median = (xs) => {
  const s = [...xs].sort((a, b) => a - b);
  const m = s.length >> 1;
  return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2;
};
const out = { runs };
for (const [side, rs] of Object.entries(results)) {
  out[side] = Object.fromEntries(Object.keys(rs[0]).map((k) => [k, median(rs.map((r) => r[k]))]));
}
console.log(JSON.stringify(out, null, 2));
