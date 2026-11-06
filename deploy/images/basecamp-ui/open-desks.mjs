// open-desks.mjs — opens both desks once Basecamp's launcher shows them, so
// each desk starts its role's LEZ signer in Basecamp's lez_core
// (apps/basecamp/common/lez_role_signer.h). Run once by entrypoint.sh, in the
// background, when a signer is configured.
const { test, run } = await import("/opt/qt-mcp/test-framework/framework.mjs");
test("open both desks for their LEZ signers", async (app) => {
  await app.waitFor(async () => app.expectTexts(["LEZ Maker", "LEZ Taker"]), { timeout: 180000 });
  await app.click("LEZ Maker");
  await new Promise((resolve) => setTimeout(resolve, 5000));
  await app.click("LEZ Taker");
});
await run();
