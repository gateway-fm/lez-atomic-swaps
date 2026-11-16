// Prints one swap row of a desk as the view model holds it (#103 item 6 check).
//   node row-read.mjs <role> <swap id prefix>
import net from "node:net";
const { test, run } = await import("/opt/qt-mcp/test-framework/framework.mjs");
const role = process.argv[2] === "taker" ? "taker" : "maker";
const id = process.argv[3] ?? "";
const port = Number(process.env.QML_INSPECTOR_PORT || 3768);
for (const end = Date.now() + 60000; Date.now() < end;) {
  const ok = await new Promise((r) => { const s = net.createConnection({ host: "127.0.0.1", port });
    s.once("connect", () => { s.destroy(); r(true); }); s.once("error", () => r(false)); });
  if (ok) break; await new Promise((r) => setTimeout(r, 500));
}
test(`${role}: row`, async (app) => {
  const label = role === "maker" ? "LEZ Maker" : "LEZ Taker";
  const desk = role === "maker" ? "LEZ — Maker Desk" : "LEZ — Taker Desk";
  try { await app.expectTexts([desk]); } catch { await app.click(label); }
  await app.waitFor(async () => app.expectTexts([desk]), { timeout: 40000, interval: 500, description: "desk view" });
  await new Promise((r) => setTimeout(r, 4000));
  const wallet = await app.findByProperty("objectName", `${role}BtcWallet`);
  const out = await app.inspector.send("evaluate", { objectId: wallet.matches[0].id,
    expression: `JSON.stringify((root.btcMarket.swaps ?? []).filter(function(s){return String(s.ui_swap_id).indexOf(${JSON.stringify(id)})===0}).map(function(s){return {ui_swap_id:s.ui_swap_id,state:s.state,state_label:s.state_label,progress_detail:s.progress_detail,can_act:s.can_act,action_label:s.action_label}}))` });
  console.log(`  ROW ${new Date().toISOString()} ${role} ${out.result}`);
});
await run();
