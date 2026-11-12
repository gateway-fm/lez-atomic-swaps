// look.mjs <out dir> <desk...>: open each desk in the native Basecamp through
// its QML inspector (qt-mcp) and save a screenshot and the visible texts.
import { writeFileSync } from "node:fs";
const { test, run } = await import("$BASECAMP/qt-mcp/test-framework/framework.mjs");
const out = process.env.LOOK_OUT; const desks = process.env.LOOK_DESKS.split(",");
test(`look at ${desks.join(", ")}`, async (app) => {
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
