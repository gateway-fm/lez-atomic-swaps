#!/usr/bin/env bash
# swap-through-ui.sh — one complete swap clicked through the two Basecamp
# desks; the happy scenario of scripts/ui-e2e.sh, kept for from-scratch.sh --swap.
exec bash "$(dirname "${BASH_SOURCE[0]}")/ui-e2e.sh" happy "$@"
