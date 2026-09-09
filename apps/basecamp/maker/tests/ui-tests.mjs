import { resolve } from "node:path";

const framework = process.env.LOGOS_QT_MCP || new URL("../result-mcp", import.meta.url).pathname;
const { test, run } = await import(resolve(framework, "test-framework/framework.mjs"));

test("maker: role console loads", async (app) => {
  await app.waitFor(async () => app.expectTexts(["LEZ / BTC — Maker Desk"]), {
    timeout: 15000,
    interval: 500,
    description: "Maker UI to load",
  });
  await app.expectTexts([
    "Compose an offer",
    "Open offers",
    "My orders",
    "Activity",
    "Private negotiation Chat",
  ]);
});

run();
