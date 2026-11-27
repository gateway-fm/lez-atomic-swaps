// reset-chain.mjs — presses the Zcash Node desk's Reset chain: the desk stops
// zebrad_module, deletes its Regtest chain under cacheDir and starts a new
// one with the same settings. Run by scripts/node-e2e.py (docker exec) for a
// worn chain; the harness then waits for the new chain over its JSON-RPC.
const { test, run } = await import("/opt/qt-mcp/test-framework/framework.mjs");
test("reset the Zcash Regtest chain", async (app) => {
  await app.waitFor(async () => app.expectTexts(["Reset chain"]), { timeout: 60000 });
  await app.click("Reset chain");
});
await run();
