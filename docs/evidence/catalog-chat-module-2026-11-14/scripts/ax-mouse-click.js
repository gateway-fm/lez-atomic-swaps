// ax-mouse-click.js <name> — a real mouse click (CoreGraphics event) at the
// centre of the first AXButton named <name> in Basecamp's window: QML tiles
// and dialog buttons ignore the accessibility press action.
ObjC.import("CoreGraphics");
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
  const pos = b.position(), size = b.size();
  const pt = $.CGPointMake(pos[0] + size[0] / 2, pos[1] + size[1] / 2);
  for (const type of [$.kCGEventMouseMoved, $.kCGEventLeftMouseDown, $.kCGEventLeftMouseUp]) {
    $.CGEventPost($.kCGHIDEventTap, $.CGEventCreateMouseEvent(null, type, pt, $.kCGMouseButtonLeft));
    delay(0.15);
  }
  return "clicked " + argv[0] + " at " + pt.x + "," + pt.y + " " + new Date().toISOString();
}
