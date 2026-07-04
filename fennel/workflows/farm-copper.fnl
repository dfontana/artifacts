;; farm-copper.fnl — gather copper ore until inventory full, then bank it.
;; This file produces a workflow AST value when loaded; it does not execute.
;;
;; Nothing is hardcoded: the copper and bank tiles are resolved from live map
;; content via host.find_tile (same pattern as farm-chickens.fnl), so a map
;; patch moves the workflow instead of silently sending the bot to grass.

(local copper (host.find_tile :resource :copper_rocks))
(local bank (host.find_tile :bank :bank))

(seq
  (action :travel-to [copper.x copper.y])
  (repeat_until is_full :gathers
    (action :gather))
  (action :travel-to [bank.x bank.y])
  (action :deposit-all))
