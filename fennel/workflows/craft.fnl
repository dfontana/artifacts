;; craft.fnl — "craft me an X": the flagship GENERATOR workflow
;; (`plans/DYNAMIC_WORKFLOWS.md` §4.2), and the proof that a generator is just
;; a workflow module whose `build` COMPUTES. All of the machinery — bank-first
;; withdrawal, recursive crafting, gathering, fighting, gold-bounded NPC
;; buying, capacity chunking — lives in `fennel.lib.acquire`; this file only
;; points it at the requested item. The plan pass validates the generated AST
;; exactly as it validates a hand-written one, so a wrong generation is caught
;; as a blocker before anything runs.
;;
;;   artifacts plan fennel/workflows/craft.fnl <character> item=copper_dagger
;;
;; (Despite the name, `item` need not be craftable — acquire routes any
;; obtainable item through its policy: gather it, hunt it, or buy it.)

(local acquire (require :fennel.lib.acquire))

{:doc "Acquire the materials for any craftable item and craft it."
 :params {:item {:type :item :required true
                 :doc "item code to obtain (e.g. copper_dagger)"}
          :qty {:type :number :default 1
                :doc "how many to end up holding"}}
 :build (fn [p ctx] (acquire.item p.item p.qty ctx {}))}
