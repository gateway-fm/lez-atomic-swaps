#!/usr/bin/env node

import { readFileSync, statSync } from "node:fs";
import { dirname, extname, isAbsolute, relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const repositoryRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const prototypeRoot = resolve(repositoryRoot, "apps/m6-prototypes");
const entrypoints = ["index.html", "maker.html", "taker.html"];
const errors = [];
const checked = new Set();

function report(file, message) {
  errors.push(`${relative(repositoryRoot, file)}: ${message}`);
}

function read(file, purpose = "referenced file") {
  try {
    if (!statSync(file).isFile()) {
      report(file, `${purpose} is not a regular file`);
      return null;
    }
    return readFileSync(file, "utf8");
  } catch (error) {
    report(file, `${purpose} is unavailable (${error.code ?? error.message})`);
    return null;
  }
}

function attribute(attributes, name) {
  const match = attributes.match(
    new RegExp(`(?:^|\\s)${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s>]+))`, "i"),
  );
  return match ? (match[1] ?? match[2] ?? match[3] ?? "") : null;
}

function stripMarkup(source) {
  return source
    .replace(/<script\b[^>]*>[\s\S]*?<\/script\s*>/gi, " ")
    .replace(/<style\b[^>]*>[\s\S]*?<\/style\s*>/gi, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/&(?:nbsp|ensp|emsp);/gi, " ")
    .replace(/\s+/g, " ")
    .trim();
}

function inspectSecrets(file, source) {
  const secretPatterns = [
    [/-----BEGIN (?:EC |RSA |OPENSSH )?PRIVATE KEY-----/i, "embedded PEM private key"],
    [/\b(?:private[_-]?key|secret[_-]?key|mnemonic|wallet[_-]?seed|seed[_-]?phrase)\s*(?:=|:)\s*["'][^"'\r\n]{8,}["']/i, "assigned private key, seed, or mnemonic"],
    [/\b(?:0x)?[0-9a-f]{64}\b/i, "apparent 256-bit hexadecimal secret"],
    [/\b[5KL][1-9A-HJ-NP-Za-km-z]{50,51}\b/, "apparent Bitcoin WIF private key"],
  ];
  for (const [pattern, description] of secretPatterns) {
    if (pattern.test(source)) report(file, description);
  }
}

function inspectNoEffectsCode(file, source) {
  const prohibited = [
    [/\bfetch\s*\(/, "fetch()"],
    [/\bXMLHttpRequest\b/, "XMLHttpRequest"],
    [/\bWebSocket\s*\(/, "WebSocket"],
    [/\bEventSource\s*\(/, "EventSource"],
    [/\bnavigator\s*\.\s*sendBeacon\s*\(/, "sendBeacon()"],
    [/\b(?:localStorage|sessionStorage|indexedDB)\b/, "persistent browser storage"],
    [/\bcaches\s*\.(?:open|match|keys|delete)\s*\(/, "Cache Storage"],
  ];
  for (const [pattern, description] of prohibited) {
    if (pattern.test(source)) report(file, `prohibited runtime effect API: ${description}`);
  }
}

function localReference(file, rawReference, context) {
  const reference = rawReference.trim();
  if (!reference || reference.startsWith("#")) return null;
  if (reference.startsWith("//") || /^[a-z][a-z0-9+.-]*:/i.test(reference)) {
    report(file, `${context} must be local, found ${JSON.stringify(reference)}`);
    return null;
  }

  let pathname;
  try {
    pathname = decodeURIComponent(reference.split(/[?#]/, 1)[0]);
  } catch {
    report(file, `${context} contains invalid URL encoding: ${JSON.stringify(reference)}`);
    return null;
  }
  if (!pathname) return null;
  if (pathname.includes("\\")) {
    report(file, `${context} uses a non-portable backslash path: ${JSON.stringify(reference)}`);
    return null;
  }

  const target = pathname.startsWith("/")
    ? resolve(prototypeRoot, `.${pathname}`)
    : resolve(dirname(file), pathname);
  const fromRoot = relative(prototypeRoot, target);
  if (fromRoot === ".." || fromRoot.startsWith(`..${process.platform === "win32" ? "\\" : "/"}`) || isAbsolute(fromRoot)) {
    report(file, `${context} escapes the prototype root: ${JSON.stringify(reference)}`);
    return null;
  }
  return target;
}

function collectMarkupReferences(file, source) {
  const references = [];
  const resourceAttributes = /\b(?:src|href|poster|action|formaction)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/gi;
  for (const match of source.matchAll(resourceAttributes)) {
    references.push([match[1] ?? match[2] ?? match[3] ?? "", "resource reference"]);
  }
  const srcsetAttributes = /\bsrcset\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>]+))/gi;
  for (const match of source.matchAll(srcsetAttributes)) {
    const srcset = match[1] ?? match[2] ?? match[3] ?? "";
    for (const candidate of srcset.split(",")) {
      const url = candidate.trim().split(/\s+/, 1)[0];
      if (url) references.push([url, "srcset reference"]);
    }
  }
  return references;
}

function collectCssReferences(source) {
  const references = [];
  for (const match of source.matchAll(/\burl\(\s*(?:"([^"]*)"|'([^']*)'|([^)'"\s]+))\s*\)/gi)) {
    references.push([match[1] ?? match[2] ?? match[3] ?? "", "CSS url() reference"]);
  }
  for (const match of source.matchAll(/@import\s+(?:url\(\s*)?(?:"([^"]*)"|'([^']*)'|([^\s;)]+))/gi)) {
    references.push([match[1] ?? match[2] ?? match[3] ?? "", "CSS @import reference"]);
  }
  return references;
}

function collectModuleReferences(source) {
  const references = [];
  const moduleReference = /\b(?:import\s*\(\s*|(?:import|export)\s+[^;]*?\bfrom\s*)["']([^"']+)["']/g;
  for (const match of source.matchAll(moduleReference)) {
    references.push([match[1], "JavaScript module reference"]);
  }
  return references;
}

function inspectHtml(file, source) {
  const visibleText = stripMarkup(source);
  if (!/\bprototype\b/i.test(visibleText) || !/\b(?:no|zero)\b.{0,160}\beffects?\b/is.test(visibleText)) {
    report(file, "missing visible prototype and no-effects disclosure");
  }
  if (/<style\b/i.test(source) || /\sstyle\s*=/i.test(source)) {
    report(file, "inline styles are prohibited");
  }
  if (/\son[a-z][a-z0-9_-]*\s*=/i.test(source) || /\bjavascript\s*:/i.test(source)) {
    report(file, "inline script handlers and javascript: URLs are prohibited");
  }
  for (const match of source.matchAll(/<script\b([^>]*)>([\s\S]*?)<\/script\s*>/gi)) {
    if (attribute(match[1], "src") === null) report(file, "inline scripts are prohibited");
    if (match[2].trim()) report(file, "script elements must not contain inline code");
  }
  for (const match of source.matchAll(/<[a-z][^>]*>/gi)) {
    if (attribute(match[0], "target")?.toLowerCase() !== "_blank") continue;
    const rel = attribute(match[0], "rel") ?? "";
    const tokens = new Set(rel.toLowerCase().split(/\s+/).filter(Boolean));
    if (!tokens.has("noopener") || !tokens.has("noreferrer")) {
      report(file, "target=_blank requires rel=\"noopener noreferrer\"");
    }
  }
  return collectMarkupReferences(file, source);
}

function inspectSvg(file, source) {
  if (/<script\b/i.test(source) || /\son[a-z][a-z0-9_-]*\s*=/i.test(source) || /\bjavascript\s*:/i.test(source)) {
    report(file, "active SVG script content is prohibited");
  }
  if (/<foreignObject\b/i.test(source)) report(file, "SVG foreignObject content is prohibited");
  return collectMarkupReferences(file, source);
}

function inspectFile(file) {
  if (checked.has(file)) return;
  checked.add(file);
  const source = read(file);
  if (source === null) return;

  inspectSecrets(file, source);
  const extension = extname(file).toLowerCase();
  let references = [];
  if (extension === ".html") references = inspectHtml(file, source);
  if (extension === ".css") references = collectCssReferences(source);
  if (extension === ".js" || extension === ".mjs") {
    inspectNoEffectsCode(file, source);
    references = collectModuleReferences(source);
  }
  if (extension === ".svg") references = inspectSvg(file, source);

  for (const [reference, context] of references) {
    const target = localReference(file, reference, context);
    if (target !== null) inspectFile(target);
  }
}

function inspectServerCsp() {
  const serverFile = resolve(prototypeRoot, "server.mjs");
  const source = read(serverFile, "prototype server declaration");
  if (source === null) return;

  const match = source.match(
    /["']Content-Security-Policy["']\s*:\s*(?:"([^"\r\n]+)"|'([^'\r\n]+)'|`([^`\r\n]+)`)/,
  );
  if (!match) {
    report(serverFile, "missing static Content-Security-Policy response header declaration");
    return;
  }
  const policy = match[1] ?? match[2] ?? match[3] ?? "";
  const directives = new Map();
  for (const declaration of policy.split(";")) {
    const [name, ...values] = declaration.trim().split(/\s+/).filter(Boolean);
    if (name) directives.set(name, values);
  }
  const required = new Map([
    ["default-src", ["'self'"]],
    ["img-src", ["'self'"]],
    ["style-src", ["'self'"]],
    ["script-src", ["'self'"]],
    ["connect-src", ["'none'"]],
    ["object-src", ["'none'"]],
    ["base-uri", ["'none'"]],
    ["form-action", ["'none'"]],
  ]);
  for (const [name, expected] of required) {
    const actual = directives.get(name);
    if (!actual || actual.length !== expected.length || actual.some((value, index) => value !== expected[index])) {
      report(serverFile, `CSP must declare ${name} ${expected.join(" ")}`);
    }
  }
  if (/\b(?:unsafe-inline|unsafe-eval)\b/.test(policy) || /(?:^|\s)\*(?:\s|$)/.test(policy)) {
    report(serverFile, "CSP must not allow unsafe inline/eval execution or wildcard sources");
  }
}

for (const entrypoint of entrypoints) inspectFile(resolve(prototypeRoot, entrypoint));
inspectServerCsp();

if (errors.length > 0) {
  process.stderr.write(`M6 prototype contract: RED (${errors.length} defect${errors.length === 1 ? "" : "s"})\n`);
  for (const error of errors.sort()) process.stderr.write(`- ${error}\n`);
  process.exitCode = 1;
} else {
  process.stdout.write(`M6 prototype contract: GREEN (${entrypoints.length} entrypoints, ${checked.size - entrypoints.length} local assets)\n`);
}
