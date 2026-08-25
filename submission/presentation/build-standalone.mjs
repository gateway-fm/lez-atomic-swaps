#!/usr/bin/env node
import fs from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const scriptDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(scriptDir, '..', '..');
const sourcePath = path.join(scriptDir, 'index.html');
const outputPath = path.resolve(
  process.argv[2] || path.join(repoRoot, 'media', 'lez-btc-m1-m3-m6-submission.html')
);

const mimeByExtension = {
  '.png': 'image/png',
  '.svg': 'image/svg+xml',
  '.jpg': 'image/jpeg',
  '.jpeg': 'image/jpeg',
  '.webp': 'image/webp'
};

let html = fs.readFileSync(sourcePath, 'utf8');
const css = fs.readFileSync(path.join(scriptDir, 'styles.css'), 'utf8');
const javascript = fs.readFileSync(path.join(scriptDir, 'deck.js'), 'utf8');

html = html.replace(
  /\s*<link rel="stylesheet" href="styles\.css">/,
  `\n  <style>\n${css}\n  </style>`
);

html = html.replace(
  /\s*<script src="deck\.js"><\/script>/,
  `\n  <script>\n${javascript}\n  </script>`
);

html = html.replace(/src="([^"]+)"/g, (fullMatch, reference) => {
  if (/^(?:data:|https?:)/.test(reference)) return fullMatch;
  const assetPath = path.resolve(scriptDir, reference);
  const extension = path.extname(assetPath).toLowerCase();
  const mime = mimeByExtension[extension];
  if (!mime || !fs.existsSync(assetPath)) {
    throw new Error(`Cannot embed presentation asset: ${reference}`);
  }
  const base64 = fs.readFileSync(assetPath).toString('base64');
  return `src="data:${mime};base64,${base64}"`;
});

fs.mkdirSync(path.dirname(outputPath), { recursive: true });
fs.writeFileSync(outputPath, html);
console.log(`Wrote standalone presentation: ${outputPath}`);
