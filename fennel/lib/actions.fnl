;; actions.fnl — CRITICAL INVARIANT: every action is defined EXACTLY ONCE.
;; Each action record has :cost (pure prediction), :sim (pure state advance),
;; and :run (real execution via host). Both interpreters read from this single
;; table — plan uses :cost + :sim, run uses :run. Divergence between :sim and
;; :run means plans silently lie. def-action enforces all three fields at load
;; time.

(local actions {})

(fn def-action [op spec]
  (assert spec.cost (.. "action " op " missing :cost"))
  (assert spec.sim  (.. "action " op " missing :sim"))
  (assert spec.run  (.. "action " op " missing :run"))
  (tset actions op spec))

;; Helper: shallow copy of a table (model state is treated as immutable in sim).
(fn copy [t]
  (collect [k v (pairs t)] (values k v)))

;; Helper: add item to inventory in model state (returns new state).
(fn inv-add [st item-code qty]
  (let [new-st (copy st)
        inv (copy (or st.inventory {}))]
    (tset inv item-code (+ (or (. inv item-code) 0) qty))
    (tset new-st :inventory inv)
    (tset new-st :inventory-count (+ (or st.inventory-count 0) qty))
    new-st))

;; Helper: set position in model state.
(fn set-pos [st [x y]]
  (let [new-st (copy st)]
    (tset new-st :x x)
    (tset new-st :y y)
    new-st))

;; Helper: count distinct non-nil inventory items.
(fn inv-distinct-count [inv]
  (accumulate [n 0 _ _ (pairs inv)] (+ n 1)))

;; Helper: add a monster's EXPECTED drops to the model inventory. Drops are
;; probabilistic — each entry hits with chance 1/rate for min..max quantity — so
;; the expected yield per win is fractional. This is what lets a fight loop's
;; `is_full` predicate eventually terminate in the plan pass; the fractional
;; total also drives the (soft) overflow warning in interp.fnl.
(fn add-expected-drops [st drops]
  (var s st)
  (each [_ d (ipairs (or drops []))]
    (let [expected (* (/ 1 d.rate) (/ (+ d.min d.max) 2))]
      (set s (inv-add s d.code expected))))
  s)

(def-action :gather
  {:bucket :action
   :cost (fn [st _args]
           (host.cooldown_cost :gathering {:level (host.resource_level st.tile)}))
   :sim  (fn [st _args]
           (inv-add st (. (host.gather_yield st.tile) :code) 1))
   :run  (fn [_char _args]
           (host.gather))})

(def-action :travel-to
  {:bucket :action
   ;; path_hops uses A* when a map is loaded; falls back to Manhattan otherwise.
   :cost (fn [st [x y]]
           (host.cooldown_cost :movement {:tiles (host.path_hops st.x st.y x y)}))
   :sim  (fn [st [x y]]
           (set-pos st [x y]))
   :run  (fn [_char [x y]]
           (host.move x y))})

(def-action :deposit-item
  {:bucket :action
   :cost (fn [_st _args]
           (host.cooldown_cost :deposit {:distinct_types 1}))
   :sim  (fn [st [code qty]]
           (let [new-st (copy st)
                 inv (copy (or st.inventory {}))
                 current (or (. inv code) 0)
                 new-qty (- current qty)]
             (if (<= new-qty 0)
               (tset inv code nil)
               (tset inv code new-qty))
             (tset new-st :inventory inv)
             (tset new-st :inventory-count
                   (math.max 0 (- (or st.inventory-count 0) qty)))
             new-st))
   :run  (fn [_char [code qty]]
           (host.deposit_item code qty))})

(def-action :deposit-all
  {:bucket :action
   :cost (fn [st _args]
           ;; 3s per distinct item type deposited.
           (host.cooldown_cost :deposit
                               {:distinct_types (inv-distinct-count (or st.inventory {}))}))
   :sim  (fn [st _args]
           (let [new-st (copy st)]
             (tset new-st :inventory {})
             (tset new-st :inventory-count 0)
             new-st))
   :run  (fn [_char _args]
           (host.deposit_all))})

(def-action :craft
  {:bucket :action
   :cost (fn [_st [_code qty]]
           (host.cooldown_cost :crafting {:quantity qty}))
   :sim  (fn [st _args] st)   ;; stub: crafting sim not in scope for v1
   ;; No host.craft is registered yet; fail loudly rather than silently
   ;; gathering. Wire this to host.craft when crafting lands in the run pass.
   :run  (fn [_char [_code _qty]]
           (error "craft :run not implemented (no host.craft registered)"))})

(def-action :rest
  {:bucket :action
   :cost (fn [st _args]
           (host.cooldown_cost :rest
                               {:hp_to_restore (- (or st.max-hp 100) (or st.hp 100))}))
   :sim  (fn [st _args]
           (let [new-st (collect [k v (pairs st)] (values k v))]
             (tset new-st :hp (or st.max-hp 100))
             new-st))
   :run  (fn [_char _args]
           (host.rest))})

(def-action :fight
  {:bucket :action
   ;; Cost: the deterministic (crits-off) simulator predicts the turn count, and
   ;; the fight cooldown is turns×2 reduced by haste.
   :cost (fn [st monster]
           (let [pred (host.simulate_fight st (host.monster_stats monster))]
             (host.cooldown_cost :fight {:turns pred.turns
                                         :haste (or st.combat.haste 0)})))
   ;; Sim: advance HP by the predicted loss; on a predicted win add expected
   ;; drops to the model inventory. A predicted LOSS is a hard blocker — surfaced
   ;; via the `--pending-blocker` marker that interp.fnl drains (a loss respawns
   ;; the character at 1 HP, so it must never be planned-through).
   :sim (fn [st monster]
          (let [m (host.monster_stats monster)
                pred (host.simulate_fight st m)]
            (var s (copy st))
            (tset s :hp pred.hp_remaining)
            (if (= pred.result :lose)
                (tset s :--pending-blocker
                      (.. "would lose fight vs " monster " from " (or st.hp 0) " HP"))
                (set s (add-expected-drops s m.drops)))
            s))
   ;; Run: the monster code is informational here — host.fight engages whatever
   ;; monster is on the current tile (and bails on a loss).
   :run (fn [_char _monster]
          (host.fight))})

;; Export. The helpers above are referenced lexically within this file; only the
;; action table needs to leave it.
{:actions actions}
