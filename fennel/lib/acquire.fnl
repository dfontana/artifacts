;; acquire.fnl — the acquisition library: goal-directed workflow GENERATION
;; (`plans/DYNAMIC_WORKFLOWS.md` §4.1). The flagship generator, and the
;; reusable core for future ones (achievements, gear-up, task grinding).
;;
;; CONTRACT: `(item code qty ctx opts)` returns an AST `:group` node whose
;; steps end with ≥ `qty` of `code` in the character's inventory — with ONE
;; documented exception: when capacity CHUNKING splits a gather/fight loop
;; into bank trips (see below), the full-trip loads end up *banked*, and only
;; the final partial trip is held. Anything that later CONSUMES the material
;; (a craft's inputs) draws inventory first and withdraws the banked
;; remainder — which can itself overflow capacity, and that is deliberate:
;; per §4.1-3, when a single consumer's inputs exceed capacity outright we do
;; NOT try to be clever, we let the plan pass's overflow blocker be the
;; backstop.
;;
;; PURITY: `item` is a pure function of (args, ctx, reference data). The only
;; host fns it calls are the always-registered, pure-once-loaded lookups
;; (item_sources, recipe, find_tile, monster_stats, simulate_fight) — never a
;; live/run-only fn, never I/O. Same purity contract as a predicate, so
;; generation behaves identically in a plan and a run invocation, and is
;; testable against fixtures.
;;
;; PROJECTION MODEL: one mutable generator-state table per top-level call
;; (`new-gen`), threaded through the recursion:
;;   - pos      — the itinerary position; every emitted travel updates it, and
;;                EVERY find_tile call passes it as the explicit anchor (M3's
;;                third arg), so "nearest X" tracks where the character WILL
;;                be, not where it started.
;;   - inv      — projected inventory {code → qty}, seeded from ctx.inventory;
;;                every emitted action updates it (gather/fight/withdraw/buy
;;                add; craft consumes inputs + adds output; deposit removes).
;;   - count    — projected total item count, for capacity checks against
;;                ctx.inventory-max-items.
;;   - bank     — projected bank {code → qty}, seeded from ctx.bank; withdraws
;;                decrement, chunk-deposits increment.
;;   - gold     — projected gold, seeded from ctx.gold; buys decrement.
;;   - budget   — the remaining opts.gold_budget, decremented across buys
;;                within one top-level call.
;;   - visiting — the in-progress item set (+ `chain`, its ordered mirror) for
;;                recipe-cycle detection: a cycle errors loudly, naming the
;;                chain.
;; Gather/fight loops are projected as yielding EXACTLY `need` of the target
;; item (rate-1 primary drops make that the truthful common case; the plan
;; pass models rare-rate loops exactly anyway) — which is fine, because…
;;
;; …THE PLAN PASS IS THE VALIDATOR. Generation may be imperfect: a projection
;; that drifts from what the sims compute produces a *wrong plan*, and a wrong
;; plan is caught as a blocker before anything runs (the design's central
;; argument, §1). This library aims for truthful common-case emission, not a
;; second simulator.
;;
;; No pairs()-order dependence anywhere: sources arrive pre-sorted from Rust
;; (`SourceIndex`), recipe inputs are an array, and the projection tables are
;; only ever *looked up*, never iterated for emission — identical inputs
;; produce an identical AST.

(local {: group : action : repeat_until : when_pred} (require :fennel.lib.interp))
(local {: has_item : is_full : is_winnable} (require :fennel.lib.predicates))

;; Source ranking tried in order for each item; first ELIGIBLE wins
;; (overridable per call via opts.policy, e.g. [:buy :craft]).
(local DEFAULT-POLICY [:craft :gather :fight :buy])

;; ─── helpers ─────────────────────────────────────────────────────────────────

(fn shallow-copy [t]
  (collect [k v (pairs (or t {}))] (values k v)))

;; Deterministic number rendering for labels/errors: integers never pick up a
;; Lua 5.4 ".0" suffix.
(fn fmt-n [n]
  (if (= n (math.floor n)) (string.format "%d" (math.floor n)) (tostring n)))

(fn new-gen [ctx opts]
  {:pos {:x ctx.x :y ctx.y}
   :inv (shallow-copy ctx.inventory)
   :count (. ctx :inventory-count)
   :cap (. ctx :inventory-max-items)
   :bank (shallow-copy ctx.bank)
   :gold ctx.gold
   :budget (or opts.gold_budget ctx.gold)
   :visiting {}
   :chain []})

(fn inv-get [gen code] (or (. gen.inv code) 0))
(fn bank-get [gen code] (or (. gen.bank code) 0))

(fn inv-add [gen code qty]
  (tset gen.inv code (+ (inv-get gen code) qty))
  (tset gen :count (+ gen.count qty)))

(fn inv-take [gen code qty]
  (tset gen.inv code (- (inv-get gen code) qty))
  (tset gen :count (- gen.count qty)))

;; Every "nearest" lookup anchors on the projected itinerary position, never
;; the (frozen) plan seed / live start (§3.3, §5.7).
(fn find-anchored [gen kind code]
  (host.find_tile kind code {:x gen.pos.x :y gen.pos.y}))

;; Emit a travel-to unless the itinerary is already there; update pos. A
;; same-tile travel would be a wasted action in the plan and a benign 490
;; no-op live — skipping it keeps generated plans tight.
(fn emit-travel [gen steps x y]
  (when (or (not= gen.pos.x x) (not= gen.pos.y y))
    (table.insert steps (action :travel-to [x y]))
    (tset gen.pos :x x)
    (tset gen.pos :y y)))

;; Travel to the nearest bank and withdraw `qty` of `code`, updating the
;; projections. Shared by step-2 ("bank first") and the craft emitter's
;; banked-remainder draw — the one "ensure it is IN the pack" path.
(fn emit-bank-withdraw [gen steps code qty]
  (let [tile (find-anchored gen :bank :bank)]
    (emit-travel gen steps tile.x tile.y)
    (table.insert steps (action :withdraw-item [code qty]))
    (tset gen.bank code (- (bank-get gen code) qty))
    (inv-add gen code qty)))

;; ─── loop emission (with capacity chunking, §4.1-3) ──────────────────────────
;; Emit the travel + acquisition loop(s) that add `need` of `code` at the site
;; tile. `make-loop` builds one repeat-until node from (pred label) — the
;; gather and fight bodies differ, the chunking around them doesn't.
;;
;; When the target would overflow ctx.inventory-max-items, the loop splits
;; into full-inventory trips: fill up (`is_full` exit) → bank the trip's load
;; (deposit-item, crediting the projected bank) → travel back — then one final
;; partial loop to the absolute `has_item` target. The banked loads are drawn
;; back by whatever consumes them (see the header's chunking note).
(fn emit-chunked-loop [gen steps code need site label-base make-loop]
  (emit-travel gen steps site.x site.y)
  (var remaining need)
  (var trip 0)
  (while (> (+ gen.count remaining) gen.cap)
    (set trip (+ trip 1))
    (let [load (- gen.cap gen.count)]
      ;; A zero-capacity trip can never make progress: the pack is already
      ;; full of OTHER items this acquisition won't deposit. Erroring here
      ;; beats generating an infinite trip list.
      (when (<= load 0)
        (error (.. "acquire " code ": inventory already full ("
                   (fmt-n gen.count) "/" (fmt-n gen.cap)
                   ") — no room to chunk-gather; bank something first")))
      (table.insert steps (make-loop is_full (.. label-base "-trip-" trip)))
      ;; The full-trip loop filled the pack with `load` of `code` (projection;
      ;; the plan models the real drops) — bank it and return to the site.
      (let [bank-tile (find-anchored gen :bank :bank)]
        (emit-travel gen steps bank-tile.x bank-tile.y)
        (table.insert steps (action :deposit-item [code load]))
        (tset gen.bank code (+ (bank-get gen code) load))
        (emit-travel gen steps site.x site.y))
      (set remaining (- remaining load))))
  (when (> remaining 0)
    ;; TARGET is the ABSOLUTE projected inventory quantity to reach, so the
    ;; has_item exit is correct whatever the pack already holds.
    (let [target (+ (inv-get gen code) remaining)]
      (table.insert steps (make-loop (fn [st] (has_item code target st)) label-base))
      (inv-add gen code remaining))))

;; ─── the per-item algorithm (§4.1) ───────────────────────────────────────────
;; Recursive: craft sources recurse on their inputs in recipe order (an array —
;; deterministic), post-order, leaves first. Mutates `gen`; appends to `steps`;
;; wraps each item's steps in a labelled :group.

(fn ensure [code qty ctx opts gen]
  (when (. gen.visiting code)
    (error (.. "acquire " code ": recipe cycle — "
               (table.concat gen.chain " -> ") " -> " code)))
  (tset gen.visiting code true)
  (table.insert gen.chain code)
  (let [steps []
        policy (or opts.policy DEFAULT-POLICY)]
    (var need (- qty (inv-get gen code)))
    ;; 2. Bank first: withdraw whatever the (projected) bank already holds.
    (when (> need 0)
      (let [take (math.min need (bank-get gen code))]
        (when (> take 0)
          (emit-bank-withdraw gen steps code take)
          (set need (- need take)))))
    ;; 3. Remaining need: first eligible source in policy order wins. Every
    ;; REJECTED candidate contributes a reason, so a no-route error names them
    ;; all (a primary UX surface — informative and deterministic).
    (when (> need 0)
      (let [srcs (host.item_sources code)
            reasons []]
        (var handled false)
        (each [_ kind (ipairs policy) &until handled]
          (match kind
            :craft
            (when srcs.craftable
              (let [r (host.recipe code)
                    have (or (. ctx.skills r.skill) 0)]
                (if (and r.skill r.level (< have r.level))
                    (table.insert reasons
                      (.. "craft: needs " r.skill " " (fmt-n r.level)
                          " (have " (fmt-n have) ")"))
                    (let [batches (math.ceil (/ need r.output_quantity))]
                      ;; Post-order: acquire every input (leaves first) …
                      (each [_ inp (ipairs r.inputs)]
                        (let [req (* inp.quantity batches)
                              sub (ensure inp.code req ctx opts gen)]
                          (when (> (length sub.steps) 0)
                            (table.insert steps sub))
                          ;; …then draw any chunk-banked remainder back into
                          ;; the pack before consuming. May overflow capacity:
                          ;; the plan pass's overflow blocker is the designed
                          ;; backstop for inputs that outsize the pack (§4.1-3).
                          (let [short (- req (inv-get gen inp.code))]
                            (when (> short 0)
                              (assert (>= (bank-get gen inp.code) short)
                                      (.. "acquire " code ": internal projection "
                                          "shortfall on input " inp.code))
                              (emit-bank-withdraw gen steps inp.code short)))))
                      ;; …then craft at the recipe's workshop.
                      (let [shop (find-anchored gen :workshop r.skill)
                            out (* batches r.output_quantity)]
                        (emit-travel gen steps shop.x shop.y)
                        (table.insert steps (action :craft [code out]))
                        (each [_ inp (ipairs r.inputs)]
                          (inv-take gen inp.code (* inp.quantity batches)))
                        (inv-add gen code out))
                      (set handled true)))))

            :gather
            (do
              (var found nil)
              (each [_ res (ipairs srcs.resources) &until found]
                (let [have (or (. ctx.skills res.skill) 0)]
                  (if (>= have res.level)
                      (set found res)
                      (table.insert reasons
                        (.. "gather: " res.code " needs " res.skill " "
                            (fmt-n res.level) " (have " (fmt-n have) ")")))))
              (when found
                (emit-chunked-loop gen steps code need
                                   (find-anchored gen :resource found.code)
                                   (.. code "-gathers")
                                   (fn [pred label]
                                     (repeat_until pred label (action :gather))))
                (set handled true)))

            :fight
            (do
              (var found nil)
              (each [_ m (ipairs srcs.monsters) &until found]
                ;; Winnable from FULL hp — the acquisition assumes the rest
                ;; gate below tops the character up between fights.
                (let [pred (host.simulate_fight
                             {:hp (. ctx :max-hp) :combat (. ctx :combat)}
                             (host.monster_stats m.code))]
                  (if (= pred.result :win)
                      (set found m)
                      (table.insert reasons (.. "fight: would lose vs " m.code)))))
              (when found
                (let [monster found.code
                      ;; hunt.fnl's gate verbatim: rest only when the next
                      ;; fight would NOT be winnable from current hp.
                      need-rest (fn [st] (not (is_winnable monster st)))]
                  (emit-chunked-loop gen steps code need
                                     (find-anchored gen :monster monster)
                                     (.. code "-hunts")
                                     (fn [pred label]
                                       (repeat_until pred label
                                         (when_pred need-rest (action :rest))
                                         (action :fight monster)))))
                (set handled true)))

            :buy
            (do
              (var found nil)
              (each [_ n (ipairs srcs.npcs) &until found]
                (if (not= n.currency :gold)
                    ;; Never auto-chosen (§4.1); surfaced so the author can
                    ;; hand-write the item-priced trade.
                    (table.insert reasons
                      (.. "buy: " n.npc " sells for " n.currency
                          " (non-gold), not auto-chosen"))
                    (let [total (* n.buy_price need)]
                      (if (> total gen.budget)
                          (table.insert reasons
                            (.. "buy: " (fmt-n total)
                                " gold needed exceeds budget " (fmt-n gen.budget)))
                          (set found n)))))
              (when found
                (let [tile (find-anchored gen :npc found.npc)
                      total (* found.buy_price need)]
                  (emit-travel gen steps tile.x tile.y)
                  (table.insert steps (action :npc-buy [code need found.buy_price]))
                  (inv-add gen code need)
                  (tset gen :gold (- gen.gold total))
                  (tset gen :budget (- gen.budget total)))
                (set handled true)))

            _ (error (.. "acquire: unknown policy kind '" (tostring kind) "'"))))

        ;; 4. No route: name the item and every rejected source with its
        ;; reason — the CLI/TUI surface this verbatim.
        (when (not handled)
          (error (.. "acquire " code ": no route — "
                     (if (> (length reasons) 0)
                         (table.concat reasons "; ")
                         "no known sources"))))))
    (tset gen.visiting code nil)
    (table.remove gen.chain)
    (group (.. "acquire " (fmt-n qty) "× " code) (table.unpack steps))))

;; ─── entry point ─────────────────────────────────────────────────────────────

(fn item [code qty ctx opts]
  "Build an AST :group whose steps end with ≥ `qty` of `code` in inventory
   (chunked overflow banked — see the header). `ctx` is the read-only seed
   snapshot `build` receives. `opts` (all defaulted): `policy` — the source
   ranking ([:craft :gather :fight :buy]); `gold_budget` — cap on auto-chosen
   purchases (ctx.gold); `keep_in_bank` — finish by banking the target."
  (let [opts (or opts {})
        gen (new-gen ctx opts)
        node (ensure code qty ctx opts gen)]
    (when opts.keep_in_bank
      (let [amount (math.min qty (inv-get gen code))]
        (when (> amount 0)
          (let [tile (find-anchored gen :bank :bank)]
            (emit-travel gen node.steps tile.x tile.y)
            (table.insert node.steps (action :deposit-item [code amount]))
            (inv-take gen code amount)
            (tset gen.bank code (+ (bank-get gen code) amount))))))
    node))

;; Export (underscore-safe, same convention as every lib).
{:item item}
