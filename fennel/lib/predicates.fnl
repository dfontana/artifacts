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

;; The model state always carries the predicate keys (predicate_state is the
;; single surface that builds it) — no `(or ... default)` fallbacks: a missing
;; key is a key-shape regression that must fail loudly, not read as a default.

(fn is-full [st]
  "True when total inventory QUANTITY >= capacity (the usual loop exit).
   `inventory-max-items` is a quantity cap (e.g. 100), not a slot count."
  (>= st.inventory-count st.inventory-max-items))

(fn hp-below [threshold st]
  "True when hp < threshold."
  (< st.hp threshold))

(fn is-at [x y st]
  "True when the character is at position (x, y)."
  (and (= st.x x) (= st.y y)))

(fn is-winnable [monster st]
  "The crits-off simulator says a fight with `monster` is winnable from the
   character's CURRENT hp. Identical in plan (model state) and run (live view):
   both carry the combat stats and hp host.simulate_fight needs."
  (= :win (. (host.simulate_fight st (host.monster_stats monster)) :result)))

(fn gold-at-least [amount st]
  "True when the character holds at least `amount` gold. Identical in plan (model
   state) and run (live view): both carry :gold via predicate_state."
  (>= st.gold amount))

(fn has-item [code qty st]
  "True when inventory holds at least `qty` of `code`. Identical in plan (model
   state, seeded from PlanSeed) and run (live view) now that predicate_state
   builds :inventory on BOTH sides. The `(or ... 0)` is CORRECT and deliberate
   here — unlike the banned fallback-defaults for state KEYS (see header): a
   state KEY missing is a shape regression, but an ITEM you don't carry is
   genuinely a real 0, so absence reads as zero rather than an error."
  (>= (or (. st.inventory code) 0) qty))

(fn skill-at-least [skill lvl st]
  "True when the character's `skill` level is at least `lvl`. Asserts the skill
   key exists so a typo'd skill name errors loudly (\"unknown skill 'mning'\")
   instead of reading as 0 — a missing skill is an author mistake, not a real
   zero (contrast has_item's deliberate item-absence-is-zero)."
  (let [have (. st.skills skill)]
    (assert (not= nil have) (.. "unknown skill '" (tostring skill) "'"))
    (>= have lvl)))

;; Export under Lua-safe keys (see header).
{:is_full is-full
 :hp_below hp-below
 :is_at is-at
 :is_winnable is-winnable
 :gold_at_least gold-at-least
 :has_item has-item
 :skill_at_least skill-at-least}
