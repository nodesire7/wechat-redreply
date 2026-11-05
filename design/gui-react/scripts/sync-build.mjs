import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
const from = new URL('../dist/index.html', import.meta.url);
const to = new URL('../../../assets/gui/next.html', import.meta.url);
writeFileSync(fileURLToPath(to), readFileSync(fileURLToPath(from), 'utf8').replace(/\r\n?/g, '\n'));
console.log('Embedded GUI updated: assets/gui/next.html');
