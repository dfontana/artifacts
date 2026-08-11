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
;; The state always carries :inventory and :inventory-count — build_state seeds
;; them — so no defensive defaults: a missing key is a key-shape regression that
;; must fail loudly, not be papered over with fabricated numbers.
(fn inv-add [st item-code qty]
  (let [new-st (copy st)
        inv (copy st.inventory)]
    (tset inv item-code (+ (or (. inv item-code) 0) qty))
    (tset new-st :inventory inv)
    (tset new-st :inventory-count (+ st.inventory-count qty))
    new-st))

;; Helper: remove qty of an item from inventory in model state (returns new
;; state). Symmetric with inv-add: clamps the slot at zero and drops it when it
;; empties, and never lets :inventory-count go negative. Shared by every action
;; that consumes an inventory item (deposit, recycle, use, delete, give, sell,
;; task-trade), so the model shrinks consistently wherever items leave the pack.
(fn inv-remove [st item-code qty]
  (let [new-st (copy st)
        inv (copy st.inventory)
        current (or (. inv item-code) 0)
        new-qty (- current qty)]
    (if (<= new-qty 0)
      (tset inv item-code nil)
      (tset inv item-code new-qty))
    (tset new-st :inventory inv)
    (tset new-st :inventory-count (math.max 0 (- st.inventory-count qty)))
    new-st))

;; Helper: spend gold in model state (returns new state). Symmetric with
;; inv-remove. If the model can't afford the spend it flags --pending-blocker
;; (drained by interp.fnl into a plan blocker, same mechanism as an unwinnable
;; fight), then clamps the balance at 0 so a guarded workflow still models a
;; non-negative gold total.
(fn gold-spend [st amount]
  (let [new-st (copy st)]
    (when (> amount st.gold)
      (tset new-st :--pending-blocker
            (.. "insufficient gold: need " amount " but have " st.gold)))
    (tset new-st :gold (math.max 0 (- st.gold amount)))
    new-st))

;; Helper: add gold in model state (returns new state). Bank/GE stock is not
;; modelled, so withdraw/sell trust the workflow the same way withdraw-item does.
(fn gold-earn [st amount]
  (let [new-st (copy st)]
    (tset new-st :gold (+ st.gold amount))
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

;; Gather's :sim adds the resource's REAL drops (e.g. mining copper_rocks yields
;; copper_ore), not the resource code itself — the earlier model was a lie that
;; broke has_item loops and gather-then-craft plans. Primary drops are rate 1, so
;; add-expected-drops adds an effectively-deterministic +1 of the real item; that
;; is why gather stays a HARD overflow blocker (no :probabilistic-drops flag —
;; `DYNAMIC_WORKFLOWS` §5.1 decides this explicitly), unlike :fight. It also gates
;; on skill: gathering a resource above the character's skill level is genuinely
;; infeasible, so it flags --pending-blocker (drained by interp.fnl, same
;; mechanism as an unwinnable fight), reading st.skills by the resource's skill.
(def-action :gather
  {:bucket :action
   :cost (fn [st _args]
           (host.cooldown_cost :gathering {:level (. (host.active_resource st.x st.y) :level)}))
   :sim  (fn [st _args]
           (let [res (host.active_resource st.x st.y)
                 have (. st.skills res.skill)]
             (if (< have res.level)
                 (let [new-st (copy st)]
                   (tset new-st :--pending-blocker
                         (.. "gathering " res.code " needs " res.skill " " res.level
                             ", have " have))
                   new-st)
                 (add-expected-drops st res.drops))))
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
           (inv-remove st code qty))
   :run  (fn [_char [code qty]]
           (host.deposit_item code qty))})

;; Withdraw from bank. The model adds items to inventory; bank stock is NOT
;; modelled, so the plan assumes the bank holds what the workflow withdraws —
;; a wrong assumption fails loudly at run time (server error), never silently
;; in the plan. Same server formula family as deposit: 3s per distinct type.
(def-action :withdraw-item
  {:bucket :action
   :cost (fn [_st _args]
           (host.cooldown_cost :deposit {:distinct_types 1}))
   :sim  (fn [st [code qty]]
           (inv-add st code qty))
   :run  (fn [_char [code qty]]
           (host.withdraw_item code qty))})

(def-action :deposit-all
  {:bucket :action
   :cost (fn [st _args]
           ;; 3s per distinct item type deposited.
           (host.cooldown_cost :deposit
                               {:distinct_types (inv-distinct-count st.inventory)}))
   :sim  (fn [st _args]
           (let [new-st (copy st)]
             (tset new-st :inventory {})
             (tset new-st :inventory-count 0)
             new-st))
   :run  (fn [_char _args]
           (host.deposit_all))})

(def-action :rest
  {:bucket :action
   :cost (fn [st _args]
           (host.cooldown_cost :rest {:hp_to_restore (- st.max-hp st.hp)}))
   :sim  (fn [st _args]
           (let [new-st (copy st)]
             (tset new-st :hp st.max-hp)
             new-st))
   :run  (fn [_char _args]
           (host.rest))})

(def-action :fight
  {:bucket :action
   ;; Drops are probabilistic, so an inventory overshoot in the plan is a soft
   ;; warning, not a hard blocker. The interpreter keys off this flag rather than
   ;; the action name, so any future action with probabilistic yield can opt in.
   :probabilistic-drops true
   ;; Cost: the deterministic (crits-off) simulator predicts the turn count, and
   ;; the fight cooldown is turns×2 reduced by haste.
   :cost (fn [st monster]
           (let [pred (host.simulate_fight st (host.monster_stats monster))]
             (host.cooldown_cost :fight {:turns pred.turns
                                         :haste st.combat.haste})))
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
                      (.. "would lose fight vs " monster " from " st.hp " HP"))
                (set s (add-expected-drops s m.drops)))
            s))
   ;; Run: the monster code is informational here — host.fight engages whatever
   ;; monster is on the current tile (and bails on a loss).
   :run (fn [_char _monster]
          (host.fight))})

;; Facets shared by the large flat-cooldown / unmodelled-effect action family
;; below, factored out the same way inv-add/inv-remove are — so "these all incur
;; the base 3s and don't touch the model" is stated once, not copy-pasted ~15×.
;; Each def-action still lists :cost and :sim explicitly, so def-action's
;; all-three-fields assertion is unaffected.
(local simple-cost (fn [_st _args] (host.cooldown_cost :simple {})))
(local neutral-sim (fn [st _args] st))

;; ─── skilling: crafting / recycling ──────────────────────────────────────────
;; craft's :sim now models the real RECIPE from host.recipe (the TTL-cached
;; /items data): it consumes each input and adds the crafted output, so a plan's
;; inventory prediction is accurate (and a backwards "craft X from raw mats"
;; planner has ground truth). `qty` is the number of crafted items requested;
;; batches = qty / recipe.output_quantity (usually 1), and each input is consumed
;; input.quantity * batches. A missing recipe (unknown/non-craftable code) or no
;; recipe data loaded fails loudly via host.recipe, same as monster_stats.
(def-action :craft
  {:bucket :action
   :cost (fn [_st [_code qty]]
           (host.cooldown_cost :craft {:quantity qty}))
   :sim  (fn [st [code qty]]
           (let [r (host.recipe code)]
             ;; Skill gate: a recipe carrying both skill+level is infeasible below
             ;; that skill level, so flag --pending-blocker (interp.fnl drains it),
             ;; same mechanism as gather/unwinnable-fight. Recipes without a
             ;; skill/level (rare) skip the gate.
             (if (and r.skill r.level (< (. st.skills r.skill) r.level))
                 (let [new-st (copy st)]
                   (tset new-st :--pending-blocker
                         (.. "crafting " code " needs " r.skill " " r.level
                             ", have " (. st.skills r.skill)))
                   new-st)
                 (let [batches (/ qty r.output_quantity)]
                   (var s st)
                   (each [_ inp (ipairs r.inputs)]
                     (set s (inv-remove s inp.code (* inp.quantity batches))))
                   (inv-add s code qty)))))
   :run  (fn [_char [code qty]]
           (host.craft code qty))})

;; recycle's :sim removes the recycled item but does NOT add salvage. Recycling
;; returns a *random* subset of the craft materials (only the returned COUNT is
;; deterministic: floor((#recipe-items - 1) / 5) + 1), so there's no faithful
;; static salvage to model — fabricating specific item codes would make the plan
;; lie. Honest net-consume is the safe direction (never over-promises inventory
;; the plan doesn't really have); the run pass reflects the real salvage live.
(def-action :recycle
  {:bucket :action
   :cost (fn [_st [_code qty]]
           (host.cooldown_cost :recycle {:quantity qty}))
   :sim  (fn [st [code qty]]
           (inv-remove st code qty))
   :run  (fn [_char [code qty]]
           (host.recycle code qty))})

;; ─── inventory: use / delete ─────────────────────────────────────────────────
;; Both consume the item from the pack (a use may also heal/buff, which the model
;; doesn't track). Flat 3s.
(def-action :use-item
  {:bucket :action
   :cost simple-cost
   :sim  (fn [st [code qty]] (inv-remove st code qty))
   :run  (fn [_char [code qty]] (host.use_item code qty))})

(def-action :delete-item
  {:bucket :action
   :cost simple-cost
   :sim  (fn [st [code qty]] (inv-remove st code qty))
   :run  (fn [_char [code qty]] (host.delete_item code qty))})

;; ─── equipment: equip / unequip ──────────────────────────────────────────────
;; Equipment slots aren't in the model state, so :sim is neutral (the run pass
;; reflects the real gear via the live view). Quantity is optional (defaults to
;; 1) — only stackable utility/consumable slots use > 1.
(def-action :equip
  {:bucket :action
   :cost simple-cost
   :sim  neutral-sim
   :run  (fn [_char [code slot qty]] (host.equip code slot (or qty 1)))})

(def-action :unequip
  {:bucket :action
   :cost simple-cost
   :sim  neutral-sim
   :run  (fn [_char [slot qty]] (host.unequip slot (or qty 1)))})

;; ─── bank / give gold ────────────────────────────────────────────────────────
;; Gold is on the model state surface (predicate_state), so :sim moves it the
;; same way inv-add/inv-remove move items. Depositing/giving more than the
;; model holds is genuinely infeasible, so gold-spend's --pending-blocker is
;; the correct signal there (same mechanism as an unwinnable fight).
(def-action :deposit-gold
  {:bucket :action
   :cost simple-cost
   :sim  (fn [st qty] (gold-spend st qty))
   :run  (fn [_char qty] (host.deposit_gold qty))})

(def-action :withdraw-gold
  {:bucket :action
   :cost simple-cost
   :sim  (fn [st qty] (gold-earn st qty))
   :run  (fn [_char qty] (host.withdraw_gold qty))})

(def-action :give-gold
  {:bucket :action
   :cost simple-cost
   :sim  (fn [st [qty _character]] (gold-spend st qty))
   :run  (fn [_char [qty character]] (host.give_gold qty character))})

;; ─── give item (to another character) ────────────────────────────────────────
;; Removes the given item from the pack. Uses the :deposit formula (3s per
;; distinct type, one here) — genuinely per-type, not the flat simple-cost.
(def-action :give-item
  {:bucket :action
   :cost (fn [_st _args] (host.cooldown_cost :deposit {:distinct_types 1}))
   :sim  (fn [st [code qty _character]] (inv-remove st code qty))
   :run  (fn [_char [code qty character]] (host.give_item code qty character))})

;; ─── NPC merchant ────────────────────────────────────────────────────────────
;; Buying adds the item to the pack AND decrements gold by price*qty; selling
;; removes the item and adds gold. The unit price is a sim-only third arg the
;; workflow author supplies (they already need to know it to gate the buy with
;; `gold_at_least`) — the server is authoritative on the real price, and :run
;; only forwards code/qty; the run pass reconciles against the live gold total
;; via host.view each iteration, same as every other model-vs-server field.
(def-action :npc-buy
  {:bucket :action
   :cost simple-cost
   :sim  (fn [st [code qty price]] (gold-spend (inv-add st code qty) (* price qty)))
   :run  (fn [_char [code qty _price]] (host.npc_buy code qty))})

(def-action :npc-sell
  {:bucket :action
   :cost simple-cost
   :sim  (fn [st [code qty price]] (gold-earn (inv-remove st code qty) (* price qty)))
   :run  (fn [_char [code qty _price]] (host.npc_sell code qty))})

;; ─── Grand Exchange ──────────────────────────────────────────────────────────
;; GE orders are addressed by an opaque server order id, and the order book is a
;; live player marketplace — listings are bought/sold/cancelled continuously, so
;; caching it client-side would be silently wrong. Instead ge-buy/ge-fill take
;; sim-only `code` + `price` hints (exactly the npc-buy/npc-sell pattern): the
;; author already knows both (they need `price` to gate the trade with
;; gold_at_least), :run forwards only id/qty, and the run pass reconciles gold
;; against the live view each iteration. ge-buy buys FROM a sell order (gain
;; item, spend gold); ge-fill sells INTO a buy order (lose item, gain gold).
(def-action :ge-buy
  {:bucket :action
   :cost simple-cost
   :sim  (fn [st [_id qty code price]] (gold-spend (inv-add st code qty) (* price qty)))
   :run  (fn [_char [id qty _code _price]] (host.ge_buy id qty))})

;; Cancel refunds items (sell order) or gold (buy order) — which, and how much,
;; depends on the order's server-side type/state, not modelled here — so :sim
;; stays neutral. The live view reflects the real refund on the run pass.
(def-action :ge-cancel
  {:bucket :action
   :cost simple-cost
   :sim  neutral-sim
   :run  (fn [_char id] (host.ge_cancel id))})

(def-action :ge-fill
  {:bucket :action
   :cost simple-cost
   :sim  (fn [st [_id qty code price]] (gold-earn (inv-remove st code qty) (* price qty)))
   :run  (fn [_char [id qty _code _price]] (host.ge_fill id qty))})

;; ─── tasks ───────────────────────────────────────────────────────────────────
;; Task board state isn't on the model surface, so the argless task actions are
;; neutral in :sim; task-trade removes the traded item from the pack.
(def-action :task-new
  {:bucket :action
   :cost simple-cost
   :sim  neutral-sim
   :run  (fn [_char _args] (host.task_new))})

(def-action :task-complete
  {:bucket :action
   :cost simple-cost
   :sim  neutral-sim
   :run  (fn [_char _args] (host.task_complete))})

(def-action :task-cancel
  {:bucket :action
   :cost simple-cost
   :sim  neutral-sim
   :run  (fn [_char _args] (host.task_cancel))})

(def-action :task-exchange
  {:bucket :action
   :cost simple-cost
   :sim  neutral-sim
   :run  (fn [_char _args] (host.task_exchange))})

(def-action :task-trade
  {:bucket :action
   :cost simple-cost
   :sim  (fn [st [code qty]] (inv-remove st code qty))
   :run  (fn [_char [code qty]] (host.task_trade code qty))})

;; ─── map transition ──────────────────────────────────────────────────────────
;; Moves between map layers from the current tile; the destination isn't known
;; client-side, so position is left unchanged in :sim.
(def-action :transition
  {:bucket :action
   :cost simple-cost
   :sim  neutral-sim
   :run  (fn [_char _args] (host.transition))})

;; Export. The helpers above are referenced lexically within this file; only the
;; action table needs to leave it.
{:actions actions}
