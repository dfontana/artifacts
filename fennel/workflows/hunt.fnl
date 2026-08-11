;; hunt.fnl — fight a monster until inventory is full, then bank the drops.
;; The combat analogue of farm.fnl — a separate file, not a `:type :enum`
;; switch on farm, because the fight loop's rest gate makes the body genuinely
;; different (see `need-rest` below).
;;
;; A workflow MODULE: loading yields `{:doc :params :build}`. `build` receives
;; the coerced params and a read-only seed-state `ctx` and returns the AST. The
;; monster is a parameter (`target`); its tile and stats come from the live map
;; + cached /monsters data via the host bridge, nothing hardcoded.

(local {: seq : action : repeat_until : when_pred} (require :fennel.lib.interp))
(local {: is_full : is_winnable} (require :fennel.lib.predicates))

{:doc "Fight a monster until inventory is full, then bank the drops."
 :params {:target {:type :monster :required true
                   :doc "monster code to hunt (e.g. chicken)"}}
 :build (fn [p _ctx]
          (let [monster p.target
                target (host.find_tile :monster monster)
                bank (host.find_tile :bank :bank)
                ;; Rest only when the next fight would NOT be winnable from
                ;; current hp — the self-adjusting heal gate, tied to the
                ;; simulator (is_winnable) rather than a magic HP threshold, so
                ;; we never engage a fight we'd lose (a loss respawns at 1 HP).
                need-rest (fn [st] (not (is_winnable monster st)))]
            (seq
              (action :travel-to [target.x target.y])
              (repeat_until is_full :fights
                (when_pred need-rest (action :rest))
                (action :fight monster))
              (action :travel-to [bank.x bank.y])
              (action :deposit-all))))}
