;; farm-chickens.fnl — fight chickens until inventory is full, then bank the
;; drops. The combat analogue of farm-copper.fnl. Loading this file produces a
;; workflow AST value; it does not execute.
;;
;; Nothing is hardcoded: the monster's tile and stats, and the bank tile, all
;; come from the live map + cached /monsters data via the host bridge.

(local MONSTER :chicken)

;; The chicken tile and the bank tile, resolved from map content (no baked-in
;; coordinates). host.find_tile returns {:x :y}.
(local target (host.find_tile :monster MONSTER))
(local bank (host.find_tile :bank :bank))

;; Predicates. Defined locally (not pulled from predicates.fnl) because Fennel
;; mangles `name?`-style identifiers on reference, so installed globals wouldn't
;; resolve here — same reason farm-copper defines its own `full?`.

;; True once we're carrying our capacity — the loop's exit condition.
(fn full? [st]
  (>= (or st.inventory-count 0) (or st.inventory-max-items 100)))

;; The simulator says this fight is winnable from the character's CURRENT hp.
;; Works identically in plan (model state) and run (live view): both carry the
;; combat stats and current hp that host.simulate_fight needs.
(fn winnable? [st]
  (= :win (. (host.simulate_fight st (host.monster_stats MONSTER)) :result)))

;; Rest only when the next fight would NOT be winnable from current hp. This is
;; the self-adjusting heal gate: it ties directly to the simulator instead of a
;; magic HP threshold, so we never engage a fight we'd lose (a loss respawns the
;; character at 1 HP).
(fn need-rest? [st]
  (not (winnable? st)))

(seq
  (action :travel-to [target.x target.y])
  (repeat_until full? :fights
    (when_pred need-rest? (action :rest))
    (action :fight MONSTER))
  (action :travel-to [bank.x bank.y])
  (action :deposit-all))
