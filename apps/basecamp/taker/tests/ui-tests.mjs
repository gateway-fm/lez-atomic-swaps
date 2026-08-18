import { resolve } from "node:path";

const framework = process.env.LOGOS_QT_MCP || new URL("../result-mcp", import.meta.url).pathname;
const { test, run } = await import(resolve(framework, "test-framework/framework.mjs"));

test("taker: role route loads", async (app) => {
  await app.waitFor(async () => app.expectTexts(["LEZ / BTC Settlement Evidence"]), {
    timeout: 15000,
    interval: 500,
    description: "Taker UI to load",
  });
  await app.expectTexts(["Five effects. Two chains. One completed swap.", "Refresh BTC proof"]);
});

run();
