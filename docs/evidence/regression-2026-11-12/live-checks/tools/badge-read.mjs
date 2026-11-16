// Reads a desk's header badge and status strip through the QML inspector,
// polling until the badge matches the expected pattern (#103 item 7 check).
//   node badge-read.mjs <role> <regex> [timeout seconds]
import net from "node:net";
const { test, run } = await import("/opt/qt-mcp/test-framework/framework.mjs");
const role = process.argv[2] === "taker" ? "taker" : "maker";
const want = new RegExp(process.argv[3] ?? ".");
const timeout = Number(process.argv[4] ?? 120) * 1000;
const port = Number(process.env.QML_INSPECTOR_PORT || 3768);
for (const end = Date.now() + 60000; Date.now() < end;) {
  const ok = await new Promise((r) => { const s = net.createConnection({ host: "127.0.0.1", port });
    s.once("connect", () => { s.destroy(); r(true); }); s.once("error", () => r(false)); });
  if (ok) break; await new Promise((r) => setTimeout(r, 500));
}
test(`${role}: badge`, async (app) => {
  const label = role === "maker" ? "LEZ Maker" : "LEZ Taker";
  const desk = role === "maker" ? "LEZ — Maker Desk" : "LEZ — Taker Desk";
  try { await app.expectTexts([desk]); } catch { await app.click(label); }
  await app.waitFor(async () => app.expectTexts([desk]), { timeout: 40000, interval: 500, description: "desk view" });
  const until = Date.now() + timeout; let last = null;
  while (Date.now() < until) {
    const wallet = await app.findByProperty("objectName", `${role}BtcWallet`);
    const out = await app.inspector.send("evaluate", { objectId: wallet.matches[0].id,
      expression: "JSON.stringify({badge: root.nodeBadge().text, strip_title: root.statusTitle, strip_detail: root.statusDetail, nodeLink: root.nodeLink, health_degraded: root.nodeHealth ? root.nodeHealth.degraded : null})" });
    last = JSON.parse(out.result);
    const shown = await app.findByProperty("objectName", `${role}Connection`);
    const props = await app.getProperties(shown.matches[0].id);
    last.badge_label = props.properties.find((e) => e.name === "text")?.value;
    if (want.test(last.badge_label) && last.badge === last.badge_label) { console.log(`  READ ${new Date().toISOString()} ${role} ${JSON.stringify(last)}`); return; }
    await new Promise((r) => setTimeout(r, 1000));
  }
  throw new Error(`badge never matched ${want}: ${JSON.stringify(last)}`);
});
await run();
