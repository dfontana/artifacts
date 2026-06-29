;; predicates.fnl — common predicates over model state, shared by every workflow.
;; In :run pass, (host.view) supplies the real character snapshot.
;; In :plan pass, st is the pure model table.
;; All predicates must work identically on both shapes.
;;
;; Predicates are exported under Lua-identifier-safe names (no `-`/`?`): Fennel
;; mangles a hyphen/`?` in a *bare reference* to a different Lua symbol than the
;; one installed as a global, so `inventory-full?` referenced from a workflow
;; would silently not resolve. Underscore names round-trip cleanly (same reason
;; interp exports `repeat_until`/`when_pred`), so workflows can reuse these
;; directly instead of redefining their own copies.

(fn is-full [st]
  "True when inventory slots used >= capacity (the usual loop exit condition)."
  (>= (or st.inventory-count 0) (or st.inventory-max-items 10)))

(fn hp-below [threshold st]
  "True when hp < threshold."
  (< (or st.hp 100) threshold))

(fn is-at [x y st]
  "True when the character is at position (x, y)."
  (and (= st.x x) (= st.y y)))

(fn is-winnable [monster st]
  "The crits-off simulator says a fight with `monster` is winnable from the
   character's CURRENT hp. Identical in plan (model state) and run (live view):
   both carry the combat stats and hp host.simulate_fight needs."
  (= :win (. (host.simulate_fight st (host.monster_stats monster)) :result)))

;; Export under Lua-safe keys (see header).
{:is_full is-full
 :hp_below hp-below
 :is_at is-at
 :is_winnable is-winnable}
