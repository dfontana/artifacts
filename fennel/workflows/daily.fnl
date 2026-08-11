;; daily.fnl — farm a resource, then hunt a monster: the COMPOSITION reference
;; (`plans/DYNAMIC_WORKFLOWS.md` §3). Living documentation — keep this short.
;;
;; This is exactly what farm.fnl/hunt.fnl are: a workflow MODULE
;; `{:doc :params :build}`. The only thing new here is what `build` does: it
;; calls `use_workflow` twice instead of constructing actions itself.
;;
;; `use_workflow` (`fennel.lib.interp`) does, in one call, what a hand-rolled
;; composition would otherwise spell out:
;;   1. `(require :workflows.<name>)` — the workflows package searcher
;;      (`src/lua.rs`) resolves `workflows.farm` to `fennel/workflows/farm.fnl`
;;      and evaluates it, exactly like any other require (memoized via
;;      `package.loaded`, so requiring the same sub-workflow twice is cheap).
;;   2. Validates + coerces the params table against the sub-workflow's OWN
;;      declared `:params` schema (`fennel.lib.params`) — the same shape
;;      validation a CLI/TUI invocation of `farm.fnl` on its own would get.
;;   3. Calls the sub-workflow's `build(coerced, ctx)` to get its AST.
;;   4. Wraps that AST in a `:group` node labelled with the sub-workflow's name
;;      and its coerced params (e.g. "farm target=copper_rocks"), so the
;;      composed skeleton renders as a legible outline instead of one long
;;      flattened step list.
;;
;; `ctx` (the seed-state snapshot) is threaded through unchanged to each
;; sub-workflow — `build` only ever reads `params` + `ctx`, composed or not.
(local {: seq : use_workflow} (require :fennel.lib.interp))

{:doc "Farm a resource, then hunt a monster — a composition example."
 :params {:resource {:type :resource :required true
                     :doc "resource tile code to farm first (e.g. copper_rocks)"}
          :monster {:type :monster :required true
                    :doc "monster code to hunt second (e.g. chicken)"}}
 :build (fn [p ctx]
          (seq
            (use_workflow :farm {:target p.resource} ctx)
            (use_workflow :hunt {:target p.monster} ctx)))}
