// #112: a stalled LEZ finality must be visible on the desk, not only as each
// swap's "waiting for finalized chain evidence". Run by ui-e2e.sh's
// `lez-finality-stall` scenario inside the role's Basecamp, against the desk
// that Basecamp has open, in two phases:
//   verify-lez-chain.mjs <role> stalled   the Node reports lez_chain degraded
//                                         (lez_finality_stalled); the desk
//                                         must show the line above the swaps
//   verify-lez-chain.mjs <role> clear     finality moves again; the line goes
//
// It reads the view's own state through the inspector: the banner's
// `visible` and the line it renders (root.lezChainLine(), from the Node's
// health, which the desk reads every 10 s).
import net from "node:net";
const framework = "/opt/qt-mcp/test-framework/framework.mjs";
const { test, run } = await import(framework);
const role = process.argv[2] === "taker" ? "taker" : "maker";
const phase = process.argv[3] === "clear" ? "clear" : "stalled";
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
  const wallet = await app.findByProperty("objectName", `${role}BtcWallet`);
  const out = await app.inspector.send("evaluate", { objectId: wallet.matches[0].id,
    expression: "JSON.stringify({line: root.lezChainLine(), chain: root.lezChain})" });
  const desk = JSON.parse(out.result);
  const banner = await app.findByProperty("objectName", `${role}LezChainBanner`);
  if (banner.error || banner.matches?.length !== 1) throw new Error(`expected one ${role}LezChainBanner, got ${JSON.stringify(banner)}`);
  const properties = await app.getProperties(banner.matches[0].id);
  desk.visible = properties.properties.find((entry) => entry.name === "visible")?.value === true;
  return desk;
}
// The framework takes its first argument (the role) as the test-name filter.
test(`${role}: the desk ${phase === "stalled" ? "shows" : "no longer shows"} the LEZ finality stall`, async (app) => {
  await openDesk(app);
  const until = Date.now() + 90000;
  let last = null;
  while (Date.now() < until) {
    last = await state(app);
    const showing = last.visible && /^LEZ finality has not advanced for .* — swaps wait until it does$/.test(last.line);
    if (phase === "stalled" ? showing : !last.visible && last.line === "") {
      console.log(`  ${role} desk: ${phase === "stalled" ? JSON.stringify(last.line) : "no LEZ chain line"} · lez_chain=${JSON.stringify(last.chain)}`);
      return;
    }
    await new Promise((r) => setTimeout(r, 2000));
  }
  throw new Error(`the ${role} desk did not reach "${phase}": ${JSON.stringify(last)}`);
});
await run();
