#!/bin/bash
# install-both.sh <dir> — both desks by name from the real LEZ catalog into one clean dir
# (logosctl --config-dir == Basecamp --user-dir), each installed twice so the doubled
# delivery_module plan ends on 0.2.1 (Phase 1 finding); then deps of both.
. "$(dirname "$0")/env.sh"
D="${1:?dir}"; V="$LV/scripts/verify.sh"
"$V" install "$D" lez_atomic_swap_maker
"$V" reinstall "$D" lez_atomic_swap_taker
"$V" reinstall "$D" lez_atomic_swap_maker
"$V" reinstall "$D" lez_atomic_swap_taker
"$V" record "$D" lez_atomic_swap_maker
