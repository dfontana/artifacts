;; predicates.fnl — common predicates over model state.
;; In :run pass, (host.view) is called for the real character snapshot.
;; In :plan pass, st is the pure model table.
;; All predicates must work identically on both shapes.

(fn inventory-full? [st]
  "True when inventory slots used >= max."
  (>= (or st.inventory-count 0) (or st.inventory-max-items 10)))

(fn hp-below? [threshold st]
  "True when hp < threshold."
  (< (or st.hp 100) threshold))

(fn at? [x y st]
  "True when character is at position (x, y)."
  (and (= st.x x) (= st.y y)))

;; Export.
{:inventory-full? inventory-full?
 :hp-below? hp-below?
 :at? at?}
