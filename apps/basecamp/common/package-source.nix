# The source tree one role package is built from.
#
# The Logos module builder copies a package's QML view directory into the
# package as-is. Both desks use the shared UI kit in common/qml, so the kit is
# merged into the role's src/qml here, in a derived source that the aggregate
# flake and the per-role catalog flakes all build from; the catalog's
# lgx-portable output therefore ships the kit (#50).
#
# qmllint then resolves every import and type the view names against that
# directory and the pinned Qt, so a package whose view would fail to compile
# in Basecamp fails to build instead of failing to load. Only unresolved
# imports and types, and syntax errors, are refused; style warnings are not.
{ pkgs, role, roleSource, commonSource }:
pkgs.runCommand "lez-${role}-ui-source" {
  nativeBuildInputs = [ pkgs.qt6.qtdeclarative pkgs.python3 ];
  LC_ALL = "C.UTF-8";
} ''
  cp -r ${roleSource} $out
  chmod -R u+w $out
  cp ${commonSource}/qml/*.qml $out/src/qml/
  # the interfaces the desk calls in Basecamp (dependency_overrides): lez_core
  # for the role's signer, lez_swap_node for the role's Node
  mkdir -p $out/deps
  cp ${commonSource}/deps/lez_core.lidl $out/deps/
  cp ${commonSource}/deps/lez_swap_node.lidl $out/deps/
  qmllint --bare \
    --qmldirs ${pkgs.qt6.qtdeclarative}/lib/qt-6/qml \
    --qmldirs $out/src/qml \
    --json $out/src/qml/qmllint.json \
    $out/src/qml/*.qml || true
  python3 - $out/src/qml/qmllint.json <<'PY'
  import json, sys
  report = json.load(open(sys.argv[1]))
  unresolved = [
      f"{entry['filename'].rsplit('/', 1)[-1]}:{warning.get('line')}: {warning['message']}"
      for entry in report["files"]
      for warning in entry.get("warnings", [])
      if warning.get("id") in ("import", "missing-type", "unresolved-type", "type")
      or warning.get("type") in ("critical", "error")
  ]
  if unresolved:
      sys.exit("the packaged view would not compile in Basecamp:\n" + "\n".join(unresolved))
  print(f"packaged view resolves every import and type ({len(report['files'])} files)")
  PY
  rm $out/src/qml/qmllint.json
''
