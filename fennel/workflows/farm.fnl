;; farm.fnl — gather a resource until inventory is full, then bank it.
;;
;; A workflow MODULE: loading this file yields `{:doc :params :build}`, not a
;; bare AST. `build` runs at load time (plan or run), receiving the coerced
;; params and a read-only seed-state `ctx`; it returns the workflow AST. The
;; resource is a parameter now (`target`), so one skeleton farms any resource —
;; `target=copper_rocks`, `target=ash_tree`, … — resolved from live map content
;; via host.find_tile, so a map patch moves the workflow instead of silently
;; sending the bot to grass.

(local {: seq : action : repeat_until} (require :fennel.lib.interp))
(local {: is_full} (require :fennel.lib.predicates))

{:doc "Gather a resource until inventory is full, then bank it."
 :params {:target {:type :resource :required true
                   :doc "resource tile code to farm (e.g. copper_rocks)"}}
 :build (fn [p _ctx]
          (let [tile (host.find_tile :resource p.target)
                bank (host.find_tile :bank :bank)]
            (seq
              (action :travel-to [tile.x tile.y])
              (repeat_until is_full :gathers
                (action :gather))
              (action :travel-to [bank.x bank.y])
              (action :deposit-all))))}
