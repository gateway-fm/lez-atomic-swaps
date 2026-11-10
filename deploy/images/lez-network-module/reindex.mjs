// reindex.mjs — presses the LEZ Network desk's Re-index: the desk has
// lez_indexer_module delete its database (reset_storage) and start again, so
// the indexer re-reads the zone from the node, which keeps running. Run by
// deploy/scripts/repair-indexer.sh (docker exec); the script then waits for
// the indexer to catch up over its JSON-RPC.
const { test, run } = await import("/opt/qt-mcp/test-framework/framework.mjs");
test("re-index the LEZ indexer from the node", async (app) => {
  await app.waitFor(async () => app.expectTexts(["Re-index"]), { timeout: 60000 });
  await app.click("Re-index");
});
await run();
