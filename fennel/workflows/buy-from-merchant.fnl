;; buy-from-merchant.fnl — travel to the timber merchant and buy ash_wood while
;; the character can still afford it. Demonstrates gold on the model surface:
;; the plan pass decrements gold each iteration and the loop terminates once the
;; balance drops below the item price; the run pass reads the live gold total
;; each iteration and stops at the same point. Nothing is hardcoded except the
;; item/price — the merchant tile is resolved from live map content.

(local {: seq : action : repeat_until : when_pred} (require :fennel.lib.interp))
(local {: gold_at_least} (require :fennel.lib.predicates))

;; ash_wood costs 10 gold at the timber merchant (currency: gold).
(local ITEM :ash_wood)
(local PRICE 10)

(local merchant (host.find_tile :npc :timber_merchant))

(seq
  (action :travel-to [merchant.x merchant.y])
  ;; Buy one at a time until the balance can no longer cover the price.
  (repeat_until (fn [st] (not (gold_at_least PRICE st))) :buys
    ;; Conditional purchase: only buy when the gold is actually there. This makes
    ;; "check they have the gold for the item" explicit and guards the buy.
    (when_pred (fn [st] (gold_at_least PRICE st))
      (action :npc-buy [ITEM 1 PRICE]))))
