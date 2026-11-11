// Prints `lez_swap_node.node_stats(<role>)` as the role's own Basecamp answers
// it: the call goes through the desk's QML bridge (logos.callModule), Basecamp's
// LogosAPI and the lez_swap_node host that supervises the role's Node, the
// path the desk's own calls take. Run inside the role's Basecamp:
//   docker exec lez-<role>-node node /ui-tests/node-stats.mjs <role>
// The last line of the output is the call's result, as one JSON line after
// "node_stats: ".
import net from "node:net";
const framework = "/opt/qt-mcp/test-framework/framework.mjs";
const { test, run } = await import(framework);
const role = process.argv[2] === "taker" ? "taker" : "maker";
if (process.env.BASECAMP_ROLE !== role) {
  throw new Error(`this is the ${process.env.BASECAMP_ROLE ?? "unknown"} Basecamp; run the ${role} call in lez-${role}-node`);
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
// The framework takes its first argument (the role) as the test-name filter.
test(`${role}: node_stats through the role's Basecamp`, async (app) => {
  await openDesk(app);
  const wallet = await app.findByProperty("objectName", `${role}BtcWallet`);
  if (wallet.error || wallet.matches?.length !== 1) throw new Error(`expected one ${role}BtcWallet, got ${JSON.stringify(wallet)}`);
  const out = await app.inspector.send("evaluate", { objectId: wallet.matches[0].id,
    expression: `(function () { var r = logos.callModule("lez_swap_node", "node_stats", [${JSON.stringify(role)}]); return typeof r === "string" ? r : JSON.stringify(r); })()` });
  if (out.error) throw new Error(`evaluate: ${JSON.stringify(out.error)}`);
  const text = String(out.result ?? "");
  let stats = JSON.parse(text);
  if (typeof stats === "string") stats = JSON.parse(stats);
  if (typeof stats?.result === "string") stats = JSON.parse(stats.result);
  if (stats?.role !== role) throw new Error(`not a node_stats answer for ${role}: ${text}`);
  console.log(`node_stats: ${JSON.stringify(stats)}`);
});
await run();
