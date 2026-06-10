------------------------------ MODULE crabcraft ------------------------------
(* Model of crabcraft's deployment / reconciliation / adoption / GC logic    *)
(* (host/gateway.lua reconcile + host/worker.lua lifecycle), checked with    *)
(* TLC. One workload "w"; two workers n1 (slot s1) and n2 (slot s2).         *)
(*                                                                           *)
(* Ground truth (survives everything): dsk (meta+wasm on the floppy), run   *)
(* (live instance), alive. Gateway state: reg is DURABLE (v0.2.3+); plc/pst *)
(* /gv/galive are in-memory and lost on gateway reboot. chan[s] models the  *)
(* per-pair FIFO rednet link as a single-slot mailbox (gateway never sends  *)
(* a second message to a slot before the view refreshes in practice; the    *)
(* guards enforce it here).                                                  *)
(*                                                                           *)
(* Faults (each budgeted): worker crash (+recovery restart from disk),      *)
(* gateway reboot, assign failure, message loss, workload removal.          *)
(*                                                                           *)
(* Checked:                                                                  *)
(*   THEOREM-ish 1 (safety):  NoStuck - in any quiet state (views synced,   *)
(*     channels empty) that is not converged, some reconcile rule applies.  *)
(*   THEOREM-ish 2 (liveness): Spec => <>[]Converged under weak fairness on *)
(*     the system actions (env actions are unfair and budgeted, so they     *)
(*     eventually stop).                                                     *)
EXTENDS Naturals

Slots == {"s1", "s2"}
NodeOf(s) == IF s = "s1" THEN "n1" ELSE "n2"
Nodes == {"n1", "n2"}

VARIABLES dsk, run, alive,                    \* worker ground truth
          reg, plc, pst, gv, galive, hbm,     \* gateway (hbm: contradicting-heartbeat count)
          chan,                               \* network (per-slot mailbox)
          bCrash, bBoot, bFail, bRem, bLoss   \* fault budgets

vars == <<dsk, run, alive, reg, plc, pst, gv, galive, hbm, chan,
          bCrash, bBoot, bFail, bRem, bLoss>>

Init ==
  /\ dsk = [s \in Slots |-> FALSE] /\ run = [s \in Slots |-> FALSE]
  /\ alive = [n \in Nodes |-> TRUE]
  /\ reg = TRUE /\ plc = "na" /\ pst = "na"
  /\ gv = [s \in Slots |-> "f"]
  /\ galive = [n \in Nodes |-> FALSE]
  /\ hbm = 0
  /\ chan = [s \in Slots |-> "none"]
  /\ bCrash = 1 /\ bBoot = 1 /\ bFail = 1 /\ bRem = 1 /\ bLoss = 1

Truth(s) == IF run[s] THEN "r" ELSE IF dsk[s] THEN "l" ELSE "f"

(* ---- heartbeats: worker truth -> gateway view ----------------------------- *)
HB(n) ==
  /\ alive[n]
  /\ ( \/ ~galive[n]
       \/ \E s \in Slots : NodeOf(s) = n /\ gv[s] # Truth(s)
       \/ (plc # "na" /\ NodeOf(plc) = n /\ Truth(plc) = "f" /\ hbm < 2) )
  /\ galive' = [galive EXCEPT ![n] = TRUE]
  /\ gv' = [s \in Slots |-> IF NodeOf(s) = n THEN Truth(s) ELSE gv[s]]
  /\ pst' = IF plc # "na" /\ NodeOf(plc) = n /\ Truth(plc) = "r" THEN "r" ELSE pst
  \* contradiction = slot EMPTY ("f"); right-workload-still-loading ("l")
  \* neither confirms nor contradicts (real boots take minutes)
  /\ hbm' = IF plc = "na" \/ NodeOf(plc) # n THEN hbm
             ELSE IF Truth(plc) = "r" THEN 0
             ELSE IF Truth(plc) = "f" /\ hbm < 2 THEN hbm + 1
             ELSE hbm
  /\ UNCHANGED <<dsk, run, alive, reg, plc, chan, bCrash, bBoot, bFail, bRem, bLoss>>

(* gateway notices a worker stopped heartbeating *)
Lost(n) ==
  /\ ~alive[n] /\ galive[n]
  /\ galive' = [galive EXCEPT ![n] = FALSE]
  /\ plc' = IF plc # "na" /\ NodeOf(plc) = n THEN "na" ELSE plc
  /\ pst' = IF plc # "na" /\ NodeOf(plc) = n THEN "na" ELSE pst
  /\ hbm' = IF plc # "na" /\ NodeOf(plc) = n THEN 0 ELSE hbm
  /\ UNCHANGED <<dsk, run, alive, reg, gv, chan, bCrash, bBoot, bFail, bRem, bLoss>>

(* ---- reconcile rules (the logic under test) -------------------------------- *)
Place(s) ==
  /\ reg /\ plc = "na" /\ galive[NodeOf(s)] /\ gv[s] = "f" /\ chan[s] = "none"
  /\ plc' = s /\ pst' = "a" /\ hbm' = 0
  /\ gv' = [gv EXCEPT ![s] = "l"]
  /\ chan' = [chan EXCEPT ![s] = "assign"]
  /\ UNCHANGED <<dsk, run, alive, reg, galive, bCrash, bBoot, bFail, bRem, bLoss>>

(* adopt an already-RUNNING copy (v0.2.3: running state required) *)
Adopt(s) ==
  /\ reg /\ plc = "na" /\ galive[NodeOf(s)] /\ gv[s] = "r"
  /\ plc' = s /\ pst' = "r" /\ hbm' = 0
  /\ UNCHANGED <<dsk, run, alive, reg, gv, galive, chan, bCrash, bBoot, bFail, bRem, bLoss>>

(* age-out: an assigning placement whose slot view no longer matches *)
StuckA ==
  /\ plc # "na" /\ pst = "a" /\ galive[NodeOf(plc)] /\ hbm = 2 /\ chan[plc] = "none"
  /\ plc' = "na" /\ pst' = "na" /\ hbm' = 0
  /\ UNCHANGED <<dsk, run, alive, reg, gv, galive, chan, bCrash, bBoot, bFail, bRem, bLoss>>

(* PROPOSED FIX under test: same age-out for RUNNING placements whose view
   stopped matching (the stale-drain / adopted-corpse class) *)
StuckR ==
  /\ plc # "na" /\ pst = "r" /\ galive[NodeOf(plc)] /\ hbm = 2 /\ chan[plc] = "none"
  /\ plc' = "na" /\ pst' = "na" /\ hbm' = 0
  /\ UNCHANGED <<dsk, run, alive, reg, gv, galive, chan, bCrash, bBoot, bFail, bRem, bLoss>>

(* GC: drain unknown workloads and duplicate copies that are not the placement *)
GC(s) ==
  /\ galive[NodeOf(s)] /\ gv[s] # "f"
  /\ (~reg \/ (plc # "na" /\ plc # s))
  /\ chan[s] = "none"
  /\ chan' = [chan EXCEPT ![s] = "drain"]
  /\ gv' = [gv EXCEPT ![s] = "f"]
  /\ UNCHANGED <<dsk, run, alive, reg, plc, pst, hbm, galive, bCrash, bBoot, bFail, bRem, bLoss>>

(* ---- delivery ----------------------------------------------------------------- *)
(* the ok-reply confirms the placement immediately (FIX under test: without
   pst' here, StuckA can age out a healthy assignment seen through a stale
   view and the cluster livelocks - found by TLC) *)
DelOk(s) ==
  /\ alive[NodeOf(s)] /\ chan[s] = "assign"
  /\ dsk' = [dsk EXCEPT ![s] = TRUE] /\ run' = [run EXCEPT ![s] = TRUE]
  /\ chan' = [chan EXCEPT ![s] = "none"]
  /\ pst' = IF plc = s THEN "r" ELSE pst
  /\ hbm' = IF plc = s THEN 0 ELSE hbm
  /\ UNCHANGED <<alive, reg, plc, gv, galive, bCrash, bBoot, bFail, bRem, bLoss>>

(* failed start clears the slot (v0.2.3) and the fail-reply clears the placement *)
DelFail(s) ==
  /\ bFail > 0 /\ alive[NodeOf(s)] /\ chan[s] = "assign"
  /\ dsk' = [dsk EXCEPT ![s] = FALSE] /\ run' = [run EXCEPT ![s] = FALSE]
  /\ chan' = [chan EXCEPT ![s] = "none"]
  /\ plc' = IF plc = s THEN "na" ELSE plc
  /\ pst' = IF plc = s THEN "na" ELSE pst
  /\ hbm' = IF plc = s THEN 0 ELSE hbm
  /\ bFail' = bFail - 1
  /\ UNCHANGED <<alive, reg, gv, galive, bCrash, bBoot, bRem, bLoss>>

DelDrain(s) ==
  /\ alive[NodeOf(s)] /\ chan[s] = "drain"
  /\ dsk' = [dsk EXCEPT ![s] = FALSE] /\ run' = [run EXCEPT ![s] = FALSE]
  /\ chan' = [chan EXCEPT ![s] = "none"]
  /\ UNCHANGED <<alive, reg, plc, pst, gv, hbm, galive, bCrash, bBoot, bFail, bRem, bLoss>>

(* ---- faults (budgeted, unfair) -------------------------------------------------- *)
Crash(n) ==
  /\ bCrash > 0 /\ alive[n]
  /\ alive' = [alive EXCEPT ![n] = FALSE]
  /\ run' = [s \in Slots |-> IF NodeOf(s) = n THEN FALSE ELSE run[s]]
  /\ chan' = [s \in Slots |-> IF NodeOf(s) = n THEN "none" ELSE chan[s]]
  /\ bCrash' = bCrash - 1
  /\ UNCHANGED <<dsk, reg, plc, pst, gv, hbm, galive, bBoot, bFail, bRem, bLoss>>

Recover(n) ==
  /\ ~alive[n]
  /\ alive' = [alive EXCEPT ![n] = TRUE]
  /\ run' = [s \in Slots |-> IF NodeOf(s) = n THEN dsk[s] ELSE run[s]]
  /\ UNCHANGED <<dsk, reg, plc, pst, gv, hbm, galive, chan, bCrash, bBoot, bFail, bRem, bLoss>>

GReboot ==
  /\ bBoot > 0
  /\ plc' = "na" /\ pst' = "na" /\ hbm' = 0
  /\ gv' = [s \in Slots |-> "f"]
  /\ galive' = [n \in Nodes |-> FALSE]
  /\ bBoot' = bBoot - 1
  /\ UNCHANGED <<dsk, run, alive, reg, chan, bCrash, bFail, bRem, bLoss>>

Remove ==
  /\ bRem > 0 /\ reg
  /\ reg' = FALSE /\ plc' = "na" /\ pst' = "na" /\ hbm' = 0 /\ bRem' = bRem - 1
  /\ UNCHANGED <<dsk, run, alive, gv, galive, chan, bCrash, bBoot, bFail, bLoss>>

MsgLoss(s) ==
  /\ bLoss > 0 /\ chan[s] # "none"
  /\ chan' = [chan EXCEPT ![s] = "none"]
  /\ bLoss' = bLoss - 1
  /\ UNCHANGED <<dsk, run, alive, reg, plc, pst, gv, hbm, galive, bCrash, bBoot, bFail, bRem>>

(* ---- composition --------------------------------------------------------------- *)
SysNext ==
  \/ \E n \in Nodes : HB(n) \/ Lost(n) \/ Recover(n)
  \/ \E s \in Slots : Place(s) \/ Adopt(s) \/ GC(s)
                    \/ DelOk(s) \/ DelFail(s) \/ DelDrain(s)
  \/ StuckA \/ StuckR

EnvNext ==
  \/ \E n \in Nodes : Crash(n)
  \/ \E s \in Slots : MsgLoss(s)
  \/ GReboot \/ Remove

Next == SysNext \/ EnvNext

RunningCopies == (IF run["s1"] THEN 1 ELSE 0) + (IF run["s2"] THEN 1 ELSE 0)

Converged ==
  IF reg
  THEN /\ plc # "na" /\ pst = "r" /\ run[plc] /\ dsk[plc]
       /\ RunningCopies = 1
       /\ \A s \in Slots : (s # plc) => (~run[s] /\ ~dsk[s])
  ELSE \A s \in Slots : ~run[s] /\ ~dsk[s]

(* fairness on system actions only: faults may stop, the system must not *)
Spec == Init /\ [][Next]_vars /\ WF_vars(SysNext)

Convergence == <>[]Converged

Quiet ==
  /\ \A n \in Nodes : alive[n] /\ galive[n]
  /\ \A s \in Slots : gv[s] = Truth(s) /\ chan[s] = "none"

ReconcileCanAct ==
  \/ \E s \in Slots : reg /\ plc = "na" /\ gv[s] = "f"
  \/ \E s \in Slots : reg /\ plc = "na" /\ gv[s] = "r"
  \/ (plc # "na" /\ hbm = 2)
  \/ (plc # "na" /\ ~run[plc] /\ alive[NodeOf(plc)])  \* heartbeats will raise hbm
  \/ \E s \in Slots : gv[s] # "f" /\ (~reg \/ (plc # "na" /\ plc # s))

NoStuck == (Quiet /\ ~Converged) => ReconcileCanAct

===============================================================================
