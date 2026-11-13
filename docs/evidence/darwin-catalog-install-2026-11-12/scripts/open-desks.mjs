// open-desks.mjs: open each desk (LOOK_DESKS, comma-separated launcher
// labels) in the native Basecamp through its QML inspector (the qt-mcp test
// framework at QT_MCP_FRAMEWORK), as a user clicks the app launcher, and save
// a screenshot and the visible tree to LOOK_OUT.
import { writeFileSync } from "node:fs";
const { test, run } = await import(process.env.QT_MCP_FRAMEWORK);
const out = process.env.LOOK_OUT; const desks = process.env.LOOK_DESKS.split(",");
test(`open ${desks.join(", ")}`, async (app) => {
  await app.waitFor(async () => app.expectTexts(desks), { timeout: 180000 });
  for (const desk of desks) {
    await app.click(desk);
    await new Promise((r) => setTimeout(r, Number(process.env.SETTLE_MS || 20000)));
    const shot = await app.screenshot();
    writeFileSync(`${out}/${desk.replace(/\W+/g, "-")}.png`, Buffer.from(shot.image, "base64"));
    const tree = await app.getTree({ maxDepth: 60 });
    writeFileSync(`${out}/${desk.replace(/\W+/g, "-")}-tree.json`, JSON.stringify(tree, null, 1));
  }
});
await run();
