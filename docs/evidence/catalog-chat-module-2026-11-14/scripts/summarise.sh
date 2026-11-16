#!/opt/homebrew/bin/bash
# summarise.sh <install log> — per `package install`: the plan (name toVersion
# repository); per `package deps`: name required -> installed status.
set -u
awk '/\$ logosctl package install/{getline; print "PLAN"; print; next} /\$ logosctl --json package deps/{getline; print "DEPS"; print}' "$1" |
while IFS= read -r line; do
  case "$line" in
    PLAN) mode=plan ;;
    DEPS) mode=deps ;;
    *) if [[ $mode == plan ]]; then
         jq -r '"plan: " + ([.changes[] | "\(.name) \(.toVersion) (\(.repository | sub("https://";"") | split("/")[0]))"] | join("; ")) + "  status=" + .status' <<<"$line" 2>/dev/null || echo "plan: $line"
       else
         jq -r '"deps: " + ([.[] | "\(.name) \(.requiredVersion // "-") -> \(.version) \(.status)"] | join("; "))' <<<"$line" 2>/dev/null || echo "deps: $line"
       fi ;;
  esac
done
grep -A20 '^# installed' "$1" | grep -v '^\[' | grep -v '^{' | sed '/^$/d'
