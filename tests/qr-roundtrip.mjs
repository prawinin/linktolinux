import assert from 'node:assert/strict';
import { readFile, mkdtemp, writeFile, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import ts from 'typescript';
const compiled = ts.transpileModule(await readFile(new URL('../src/lib/qr.ts', import.meta.url), 'utf8'), {compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ES2022}}).outputText;
const { generateQrMatrix } = await import(`data:text/javascript;base64,${Buffer.from(compiled).toString('base64')}`);
const directory = await mkdtemp(join(tmpdir(), 'linktolinux-qr-'));
try {
 for (const content of ['hello', 'http://192.0.2.10:1718/download/companion.apk']) {
  const matrix = generateQrMatrix(content, 'M');
  const scale = 8, size = (matrix.length + 8) * scale;
  const pixels = Array.from({length:size},(_,y)=>Array.from({length:size},(_,x)=>matrix[Math.floor(y/scale)-4]?.[Math.floor(x/scale)-4] ? 0 : 255));
  const path = join(directory,'qr.pgm');
  await writeFile(path,`P2\n${size} ${size}\n255\n${pixels.map(row=>row.join(' ')).join('\n')}\n`);
  const decoded = execFileSync('zbarimg',['--quiet','--raw',path],{timeout:5000,encoding:'utf8'}).trim();
  assert.equal(decoded, content);
 }
 console.log('QR round trips: 2 passed');
} finally { await rm(directory,{recursive:true,force:true}); }
