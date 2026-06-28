;; actions.fnl — CRITICAL INVARIANT: every action is defined EXACTLY ONCE.
;; Each action record has :cost (pure prediction), :sim (pure state advance),
;; and :run (real execution via host). All three interpreters read from this
;; single table. Divergence between :sim and :run means plans silently lie.
;; def-action enforces all three fields at load time.

(local actions {})

(fn def-action [op spec]
  (assert spec.cost (.. "action " op " missing :cost"))
  (assert spec.sim  (.. "action " op " missing :sim"))
  (assert spec.run  (.. "action " op " missing :run"))
  (tset actions op spec))

;; Helper: Manhattan distance between two [x y] positions.
(fn manhattan [[x1 y1] [x2 y2]]
  (+ (math.abs (- x2 x1)) (math.abs (- y2 y1))))

;; Helper: add item to inventory in model state (returns new state).
(fn inv-add [st item-code qty]
  (let [new-st (collect [k v (pairs st)] (values k v))
        inv (collect [k v (pairs (or st.inventory {}))] (values k v))]
    (tset inv item-code (+ (or (. inv item-code) 0) qty))
    (tset new-st :inventory inv)
    (tset new-st :inventory-count (+ (or st.inventory-count 0) qty))
    new-st))

;; Helper: set position in model state.
(fn set-pos [st [x y]]
  (let [new-st (collect [k v (pairs st)] (values k v))]
    (tset new-st :x x)
    (tset new-st :y y)
    new-st))

;; Helper: count distinct non-nil inventory items.
(fn inv-distinct-count [inv]
  (var n 0)
  (each [_ _ (pairs inv)]
    (set n (+ n 1)))
  n)

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
           (let [new-st (collect [k v (pairs st)] (values k v))
                 inv (collect [k v (pairs (or st.inventory {}))] (values k v))
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
           (let [new-st (collect [k v (pairs st)] (values k v))]
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
   :run  (fn [_char [code qty]]
           (host.gather))})   ;; placeholder — wire to host.craft when added

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
   :cost (fn [_st _args]
           ;; Approximate: 5 turns × 2s = 10s. Sim pass can refine.
           (host.cooldown_cost :fight {:turns 5}))
   :sim  (fn [st _args] st)   ;; stub: combat sim yields a result in simulate pass
   :run  (fn [_char _args]
           (host.fight))})

;; Export.
{:actions actions
 :manhattan manhattan
 :inv-add inv-add
 :set-pos set-pos
 :inv-distinct-count inv-distinct-count}
