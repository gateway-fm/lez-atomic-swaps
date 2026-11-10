// #91: with its Node unreachable a desk must not claim a connection, and it
// must notice when the Node comes back. Run by ui-e2e.sh's `node-offline`
// scenario inside the role's Basecamp, against the desk that Basecamp has
// open, in two phases:
//   verify-node-offline.mjs <role> down   the Nodes are held down in place
//                                         (scripts/node-hold.sh: Basecamp, the
//                                         desk and its signer keep running);
//                                         the desk must report its Node
//                                         unavailable and keep saying so
//   verify-node-offline.mjs <role> up     the Nodes were released; the desk
//                                         must pick its Node up again
//
// It reads the view's own state through the inspector rather than the strip's
// pixels: `nodeLink` is the property the status strip renders from.
import net from "node:net";
const framework = "/opt/qt-mcp/test-framework/framework.mjs";
const { test, run } = await import(framework);
const role = process.argv[2] === "taker" ? "taker" : "maker";
const phase = process.argv[3] === "up" ? "up" : "down";
if (process.env.BASECAMP_ROLE !== role) {
  throw new Error(`this is the ${process.env.BASECAMP_ROLE ?? "unknown"} Basecamp; run the ${role} check in lez-${role}-node`);
}
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
  const label = role === "maker" ? "LEZ Maker" : "LEZ Taker";
  await app.waitFor(async () => app.expectTexts([label]), { timeout: 40000, interval: 500, description: "package discovery" });
  const desk = role === "maker" ? "LEZ — Maker Desk" : "LEZ — Taker Desk";
  try { await app.expectTexts([desk]); } catch { await app.click(label); }
  await app.waitFor(async () => app.expectTexts([desk]), { timeout: 40000, interval: 500, description: "desk view" });
}
async function state(app) {
  const found = await app.findByProperty("objectName", `${role}BtcWallet`);
  const id = found.matches[0].id;
  const out = await app.inspector.send("evaluate", { objectId: id,
    expression: "JSON.stringify({link: root.nodeLink, mode: root.statusMode, title: root.statusTitle, detail: String(root.statusDetail).slice(0,80)})" });
  return JSON.parse(out.result);
}
// The framework takes its first argument (the role) as the test-name filter.
if (phase === "down") {
  test(`${role}: a Node held down is reported unavailable, never connected`, async (app) => {
    await openDesk(app);
    // The desk saw its Node until the hold: its next poll must find it gone.
    const seen = [];
    const started = Date.now();
    while (Date.now() - started < 45000) {
      const s = await state(app);
      seen.push(s);
      if (s.link === "down") break;
      await new Promise((r) => setTimeout(r, 1000));
    }
    const last = seen[seen.length - 1];
    console.log(`  first=${JSON.stringify(seen[0])}`);
    console.log(`  last =${JSON.stringify(last)}`);
    if (last.link !== "down") throw new Error(`never reported unavailable: ${JSON.stringify(last)}`);
    if (last.mode !== "error") throw new Error(`unavailable was not an error state: ${JSON.stringify(last)}`);
    // ... and must not claim a connection while the Node stays down.
    const watched = Date.now();
    while (Date.now() - watched < 15000) {
      const s = await state(app);
      if (s.link === "up" || /Node connected|Market/.test(s.title)) {
        throw new Error(`claimed a connection with the Node down: ${JSON.stringify(s)}`);
      }
      await new Promise((r) => setTimeout(r, 1000));
    }
  });
} else {
  test(`${role}: the Node coming back is picked up`, async (app) => {
    await openDesk(app);
    const deadline2 = Date.now() + 180000;
    while (Date.now() < deadline2) {
      const s = await state(app);
      if (s.link === "up") { console.log(`  recovered=${JSON.stringify(s)}`); return; }
      await new Promise((r) => setTimeout(r, 2000));
    }
    throw new Error(`never recovered: ${JSON.stringify(await state(app))}`);
  });
}
await run();
