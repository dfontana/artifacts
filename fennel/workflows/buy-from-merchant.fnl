;; buy-from-merchant.fnl — travel to an NPC merchant and buy an item one unit at
;; a time while the character can still afford it. Demonstrates gold on the model
;; surface: the plan pass decrements gold each iteration and the loop terminates
;; once the balance drops below the item price; the run pass reads the live gold
;; total each iteration and stops at the same point.
;;
;; A workflow MODULE: loading yields `{:doc :params :build}`. The npc, item, and
;; unit price are all parameters now — the merchant tile is resolved from live
;; map content via host.find_tile. (Price stays a param until M3 makes it
;; derivable from the NPC catalog.)

(local {: seq : action : repeat_until : when_pred} (require :fennel.lib.interp))
(local {: gold_at_least} (require :fennel.lib.predicates))

{:doc "Buy an item from an NPC merchant, one unit at a time, until broke."
 :params {:npc {:type :npc :required true
                :doc "npc merchant code (e.g. timber_merchant)"}
          :item {:type :item :required true
                 :doc "item code to buy (e.g. ash_wood)"}
          :price {:type :number :required true
                  :doc "unit price in gold (sim hint + gold gate)"}}
 :build (fn [p _ctx]
          (let [merchant (host.find_tile :npc p.npc)]
            (seq
              (action :travel-to [merchant.x merchant.y])
              ;; Buy one at a time until the balance can no longer cover the price.
              (repeat_until (fn [st] (not (gold_at_least p.price st))) :buys
                ;; Conditional purchase: only buy when the gold is actually there,
                ;; making "check they can afford it" explicit and guarding the buy.
                (when_pred (fn [st] (gold_at_least p.price st))
                  (action :npc-buy [p.item 1 p.price]))))))}
