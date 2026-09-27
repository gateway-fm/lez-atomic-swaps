// #91: with its Node unreachable a desk must not claim a connection, and it
// must notice when the Node comes back. Run by ui-e2e.sh's `node-offline`
// scenario, which stops the Nodes before it and starts them while it waits.
//
// It reads the view's own state through the inspector rather than the strip's
// pixels: `nodeLink` is the property the status strip renders from.
import { spawn } from "node:child_process";
import net from "node:net";
import { cpSync, mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
const framework = "/opt/qt-mcp/test-framework/framework.mjs";
const { test, run } = await import(framework);
const role = process.argv[2] === "taker" ? "taker" : "maker";
const dir = mkdtempSync(join(tmpdir(), `lez-status-${role}-`));
cpSync("/var/lez-assets/both-user", dir, { recursive: true });
process.env.BASECAMP_USER_DIR = dir;
spawn(`/usr/local/bin/lez-${role}-ui`, ["-platform", "offscreen"],
  { stdio: ["ignore", "ignore", "inherit"], env: { ...process.env, QT_QPA_PLATFORM: "offscreen" } });
const port = Number(process.env.QML_INSPECTOR_PORT || 3768);
const deadline = Date.now() + 120000;
while (Date.now() < deadline) {
  const ok = await new Promise((r) => {
    const s = net.createConnection({ host: "127.0.0.1", port });
    s.once("connect", () => { s.destroy(); r(true); });
    s.once("error", () => r(false));
  });
  if (ok) break;
  await new Promise((r) => setTimeout(r, 500));
}
async function openDesk(app) {
  const label = role === "maker" ? "LEZ / BTC Maker" : "LEZ / BTC Taker";
  await app.waitFor(async () => app.expectTexts([label]), { timeout: 40000, interval: 500, description: "package discovery" });
  await app.click(label);
  const desk = role === "maker" ? "LEZ / BTC — Maker Desk" : "LEZ / BTC — Taker Desk";
  await app.waitFor(async () => app.expectTexts([desk]), { timeout: 40000, interval: 500, description: "desk view" });
}
async function state(app) {
  const found = await app.findByProperty("objectName", `${role}BtcWallet`);
  const id = found.matches[0].id;
  const out = await app.inspector.send("evaluate", { objectId: id,
    expression: "JSON.stringify({link: root.nodeLink, mode: root.statusMode, title: root.statusTitle, detail: String(root.statusDetail).slice(0,80)})" });
  return JSON.parse(out.result);
}
test(`${role}: an unreachable Node is never reported as connected`, async (app) => {
  await openDesk(app);
  const seen = [];
  const started = Date.now();
  while (Date.now() - started < 40000) {
    const s = await state(app);
    seen.push(s);
    if (s.link === "up" || /Node connected|Market/.test(s.title)) {
      throw new Error(`claimed a connection with the Node down: ${JSON.stringify(s)}`);
    }
    if (s.link === "down") break;
    await new Promise((r) => setTimeout(r, 1000));
  }
  const last = seen[seen.length - 1];
  console.log(`  first=${JSON.stringify(seen[0])}`);
  console.log(`  last =${JSON.stringify(last)}`);
  if (last.link !== "down") throw new Error(`never reported unavailable: ${JSON.stringify(last)}`);
  if (last.mode !== "error") throw new Error(`unavailable was not an error state: ${JSON.stringify(last)}`);
});
test(`${role}: the Node coming back is picked up`, async (app) => {
  console.log("  (waiting for the Nodes to be started from the host)");
  const deadline2 = Date.now() + 180000;
  while (Date.now() < deadline2) {
    const s = await state(app);
    if (s.link === "up") { console.log(`  recovered=${JSON.stringify(s)}`); return; }
    await new Promise((r) => setTimeout(r, 2000));
  }
  throw new Error(`never recovered: ${JSON.stringify(await state(app))}`);
});
await run();
