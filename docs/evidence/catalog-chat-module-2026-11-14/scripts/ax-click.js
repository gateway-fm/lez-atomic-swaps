// ax-click.js <name> — click the first AXButton named <name> in Basecamp's window (System Events).
function run(argv) {
  const se = Application("System Events");
  const p = se.processes.byName("LogosBasecamp.bin");
  p.frontmost = true;
  const w = p.windows[0];
  function find(e, d) {
    if (d > 25) return null;
    try { if (e.role() === "AXButton" && e.name() === argv[0]) return e; } catch (x) {}
    let kids = []; try { kids = e.uiElements(); } catch (x) {}
    for (let i = 0; i < kids.length; i++) { const f = find(kids[i], d + 1); if (f) return f; }
    return null;
  }
  const b = find(w, 0);
  if (!b) return "not found: " + argv[0];
  b.click();
  return "clicked " + argv[0] + " at " + new Date().toISOString();
}
