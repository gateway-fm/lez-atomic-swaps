#!/opt/homebrew/bin/bash
# mac-ui-install.sh <user dir> <desk> <tag> — Basecamp 0.3.2 on a fresh user
# dir: a first start creates the package downloader's instance directory, its
# repositories.json is written with the local catalog (download source http:
# what Settings → Package Repositories writes), then Applications → the desk's
# tile → Install (one click) → Launch, by real mouse clicks; AX text dumps and
# Basecamp's log under logs/.
set -u
U="${1:?}" DESK="${2:?}" T="${3:?}"
cd "${SCRATCH:?}" || exit 1
rm -rf "$U"; mkdir -p "$U"
scripts/mac-bc-start.sh "$U" "logs/mac-bc-$T-first.log"; sleep 30; scripts/quit-basecamp.sh | tail -1
D=$(ls -d "$U"/module_data/package_downloader/*)
jq -n '{defaultDisabled:false, defaultRemoved:false, downloadSource:"http", followIncludes:true,
        repositories:[{enabled:true, url:"https://localhost:8473/logos-repo.json"}], schemaVersion:1}' > "$D/repositories.json"
scripts/mac-bc-start.sh "$U" "logs/mac-bc-$T.log"; sleep 30
osascript -l JavaScript scripts/ax-click.js Applications; sleep 8
osascript -l JavaScript scripts/ax-mouse-click.js "$DESK"; sleep 8
osascript -l JavaScript scripts/ax-dump.js > "logs/ax-$T-dialog.txt"
osascript -l JavaScript scripts/ax-mouse-click.js Install; sleep 90
osascript -l JavaScript scripts/ax-dump.js > "logs/ax-$T-installed.txt"
osascript -l JavaScript scripts/ax-mouse-click.js Launch; sleep 25
osascript -l JavaScript scripts/ax-dump.js > "logs/ax-$T-launched.txt"
scripts/quit-basecamp.sh | tail -1
for m in "$U"/modules/* "$U"/plugins/*; do echo "${m##*/} $(cat $m/variant) $(jq -r .version $m/manifest.json)"; done
