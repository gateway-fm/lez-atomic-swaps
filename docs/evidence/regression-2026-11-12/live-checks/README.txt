#103 items 6 and 7, live checks on the devnet (lane 1, fast timing profile), 2026-11-11 UTC.

badge-item7.txt                 item 7 on images of caebd6d9: each desk's header badge and status strip,
                                read through the QML inspector (tools/badge-read.mjs), with that role's
                                Node held and released (scripts/node-hold.sh) and with the LEZ sequencer
                                stopped past the 60 s stall threshold and started again.
held-refund-item6.txt           item 6 on images of caebd6d9 (tools/held-refund.py): sequencer stopped right
                                after the Taker's Bitcoin lock, refund pressed after the cutoff. The views
                                carried last_outcome but NO waiting_for: the hold was the LEZ first-lock
                                safety read not answering / classifying uncertain, a branch that named no wait.
held-refund-item6-after-fix.txt the same on images of 36cfd863 (that branch now notes the wait):
                                waiting_for lez_finality_past_cutoff with the cutoff and no chain reading on
                                taker_swap_monitor_v1, taker_swap_list_v1 and maker_actor_monitor_v1; both desk
                                rows "Waiting for LEZ finality to pass the lock cutoff HH:MM", no action; after
                                the sequencer started, the refund completed and last_outcome went away.
smoke-36cfd863.log              LEZ_LANES=3 regress.sh smoke on 36cfd863.
tools/                          the readers and the driver used.
Times in the desk texts are the desk container's clock (UTC).
