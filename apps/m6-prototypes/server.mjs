#!/usr/bin/env node

import { createReadStream } from "node:fs";
import { createServer } from "node:http";
import { extname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("./", import.meta.url));
const host = "127.0.0.1";
const portText = process.env.M6_PROTOTYPE_PORT ?? "0";
if (!Number.isInteger(Number(portText)) || String(Number(portText)) !== portText || Number(portText) > 65535) {
  throw new Error("M6_PROTOTYPE_PORT must be an integer from 0 through 65535");
}
const requestedPort = Number(portText);
const files = new Set([
  "/index.html",
  "/maker.html",
  "/taker.html",
  "/styles.css",
  "/prototype.js",
  "/assets/lez-orbit.svg",
  "/assets/maker-console.svg",
  "/assets/taker-route.svg",
]);
const contentTypes = {
  ".html": "text/html; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".svg": "image/svg+xml; charset=utf-8",
};

const server = createServer((request, response) => {
  const method = request.method ?? "";
  if (method !== "GET" && method !== "HEAD") {
    response.writeHead(405, { Allow: "GET, HEAD", "Content-Type": "text/plain; charset=utf-8" });
    response.end("Method not allowed\n");
    return;
  }

  const url = new URL(request.url ?? "/", `http://${host}`);
  const pathname = url.pathname === "/" ? "/index.html" : url.pathname;
  if (!files.has(pathname)) {
    response.writeHead(404, { "Content-Type": "text/plain; charset=utf-8" });
    response.end("Prototype file not found\n");
    return;
  }

  const file = resolve(root, `.${pathname}`);
  response.writeHead(200, {
    "Content-Type": contentTypes[extname(file)] ?? "application/octet-stream",
    "Cache-Control": "no-store",
    "Content-Security-Policy": "default-src 'self'; img-src 'self'; style-src 'self'; script-src 'self'; connect-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'",
    "Referrer-Policy": "no-referrer",
    "X-Content-Type-Options": "nosniff",
  });
  if (method === "HEAD") {
    response.end();
    return;
  }
  createReadStream(file).on("error", () => response.destroy()).pipe(response);
});

server.listen(requestedPort, host, () => {
  const address = server.address();
  if (!address || typeof address === "string") throw new Error("missing listener address");
  process.stdout.write(`M6 prototypes: http://${host}:${address.port}/\n`);
  process.stdout.write("Sample state only; no runtime network or chain effects.\n");
});
