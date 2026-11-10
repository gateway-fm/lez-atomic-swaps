// open-desks.mjs — opens this Basecamp's desks once its launcher shows them:
// each desk starts its role's LEZ signer in Basecamp's lez_core
// (apps/basecamp/common/lez_role_signer.h) and has Basecamp's lez_swap_node
// launch its role's Node (lez_role_node.h); the LEZ Network desk starts the LEZ
// network node and/or indexer (lez_network_host.h). Run once by entrypoint.sh,
// in the background, when any of them is configured. A role Basecamp
// (BASECAMP_ROLE=maker|taker) has the one desk, "network" the LEZ Network desk,
// "both" the two role desks.
const { test, run } = await import("/opt/qt-mcp/test-framework/framework.mjs");
const desks = { maker: ["LEZ Maker"], taker: ["LEZ Taker"], network: ["LEZ Network"] }[process.env.BASECAMP_ROLE] ??
  ["LEZ Maker", "LEZ Taker"];
test(`open ${desks.join(" and ")} for their signers and Nodes`, async (app) => {
  await app.waitFor(async () => app.expectTexts(desks), { timeout: 180000 });
  for (const [index, desk] of desks.entries()) {
    if (index > 0) await new Promise((resolve) => setTimeout(resolve, 5000));
    await app.click(desk);
  }
});
await run();
