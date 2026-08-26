.[1].cryptarchia_info.height >= .[0].cryptarchia_info.height
and .[1].cryptarchia_info.slot >= .[0].cryptarchia_info.slot
and (
  .[1].cryptarchia_info.height > .[0].cryptarchia_info.height
  or .[1].cryptarchia_info.slot > .[0].cryptarchia_info.slot
)
