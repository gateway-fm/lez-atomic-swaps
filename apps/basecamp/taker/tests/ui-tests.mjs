import { resolve } from "node:path";

const framework = process.env.LOGOS_QT_MCP || new URL("../result-mcp", import.meta.url).pathname;
const { test, run } = await import(resolve(framework, "test-framework/framework.mjs"));

test("taker: role route loads", async (app) => {
  await app.waitFor(async () => app.expectTexts(["LEZ / BTC — Taker Desk"]), {
    timeout: 15000,
    interval: 500,
    description: "Taker UI to load",
  });
  await app.expectTexts([
    "Choose the Taker wallet",
    "Live BTC / LEZ order book",
    "Your Taker swaps",
    "Five effects. Two chains. One completed swap.",
    "Wallet balance proof",
    "Private negotiation Chat",
    "Session identity and conversation history are intentionally discarded when the app closes.",
  ]);
});

run();
