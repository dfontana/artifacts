;; farm-chickens.fnl — fight chickens until inventory is full, then bank the
;; drops. The combat analogue of farm-copper.fnl. Loading this file produces a
;; workflow AST value; it does not execute.
;;
;; Nothing is hardcoded: the monster's tile and stats, and the bank tile, all
;; come from the live map + cached /monsters data via the host bridge. The loop
;; conditions reuse the shared predicates from predicates.fnl (is_full,
;; is_winnable) rather than redefining them here.

(local MONSTER :chicken)

;; The chicken tile and the bank tile, resolved from map content (no baked-in
;; coordinates). host.find_tile returns {:x :y}.
(local target (host.find_tile :monster MONSTER))
(local bank (host.find_tile :bank :bank))

;; Rest only when the next fight would NOT be winnable from current hp. This is
;; the self-adjusting heal gate: it ties directly to the simulator (is_winnable)
;; instead of a magic HP threshold, so we never engage a fight we'd lose (a loss
;; respawns the character at 1 HP). It's the one workflow-specific bit of glue —
;; the negation of a shared predicate, bound to this workflow's MONSTER.
(fn need-rest [st]
  (not (is_winnable MONSTER st)))

(seq
  (action :travel-to [target.x target.y])
  (repeat_until is_full :fights
    (when_pred need-rest (action :rest))
    (action :fight MONSTER))
  (action :travel-to [bank.x bank.y])
  (action :deposit-all))
