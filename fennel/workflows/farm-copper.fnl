;; farm-copper.fnl — gather copper ore until inventory full, then bank it.
;; This file produces a workflow AST value when loaded; it does not execute.

(local COPPER_X 2)
(local COPPER_Y 0)
(local BANK_X 4)
(local BANK_Y 1)

(seq
  (action :travel-to [COPPER_X COPPER_Y])
  (repeat_until is_full :gathers
    (action :gather))
  (action :travel-to [BANK_X BANK_Y])
  (action :deposit-all))
