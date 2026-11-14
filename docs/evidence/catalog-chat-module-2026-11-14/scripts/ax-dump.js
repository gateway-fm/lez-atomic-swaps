function run() {
  const se = Application("System Events");
  const w = se.processes.byName("LogosBasecamp.bin").windows[0];
  const out = [];
  function walk(e, d) {
    if (d > 25 || out.length > 3000) return;
    let r = "", n = "", desc = "", v = "";
    try { r = e.role(); } catch (x) {}
    try { n = e.name() || ""; } catch (x) {}
    try { desc = e.description() || ""; } catch (x) {}
    try { v = e.value(); v = (v === null || v === undefined) ? "" : String(v); } catch (x) {}
    if (n || v || (desc && desc !== r.replace("AX","").toLowerCase())) out.push("  ".repeat(d) + r + " | " + n + " | " + desc + " | " + v);
    let kids = [];
    try { kids = e.uiElements(); } catch (x) {}
    for (let i = 0; i < kids.length; i++) walk(kids[i], d + 1);
  }
  walk(w, 0);
  return out.join("\n");
}
