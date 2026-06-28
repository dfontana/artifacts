;; interp.fnl — the three interpreters: estimate, simulate, run.
;; All three consume the same workflow AST. The AST is a table (not closures),
;; so it is introspectable — this is what makes offline planning possible.
;;
;; Accumulator `acc` is a Lua table mutated in place via tset throughout.
;; estimate-node returns only the new model state; acc is always mutated by ref.

;; Workflow AST node types:
;;   {:type :seq    :steps [...]}
;;   {:type :action :op <name> :args [...]}
;;   {:type :repeat-until :pred <pred-fn> :label <label> :steps [...]}
;;   {:type :repeat-n :n <int> :steps [...]}
;;   {:type :when   :pred <pred-fn> :steps [...]}

(local MAX-ITERS 10000)

;; ─── helpers ────────────────────────────────────────────────────────────────

(fn get-action [op]
  (let [a (. _G :_artifacts_actions)]
    (assert a "actions table not set; call set-actions first")
    (let [spec (. a op)]
      (assert spec (.. "unknown action: " op))
      spec)))

(fn acc-add-action [acc bucket]
  (tset acc :actions (+ (or acc.actions 0) 1))
  (let [bc (or acc.bucket-cost {})]
    (tset acc :bucket-cost bc)
    (tset bc bucket (+ (or (. bc bucket) 0) 1))))

;; ─── estimate pass ──────────────────────────────────────────────────────────
;; acc is mutated in place; estimate-node returns new-st only.

(fn estimate-node [node st acc]
  "Walk one AST node. Mutates acc in place. Returns new-st."
  (match node.type
    :seq
    (do
      (var s st)
      (each [_ child (ipairs node.steps)]
        (set s (estimate-node child s acc)))
      s)

    :action
    (let [spec (get-action node.op)
          cost (spec.cost st (table.unpack node.args))
          new-st (spec.sim st (table.unpack node.args))
          bk (or node.bucket spec.bucket :action)]
      (tset acc :seconds (+ (or acc.seconds 0) cost))
      (acc-add-action acc bk)
      new-st)

    :repeat-until
    (do
      (var s st)
      (var iters 0)
      (while (and (not (node.pred s)) (< iters MAX-ITERS))
        (each [_ child (ipairs node.steps)]
          (set s (estimate-node child s acc)))
        (set iters (+ iters 1)))
      (when (>= iters MAX-ITERS)
        (error "estimate: repeat-until did not terminate within MAX-ITERS"))
      (let [assumptions (or acc.assumptions {})]
        (tset acc :assumptions assumptions)
        (tset assumptions (or node.label :loop) iters))
      s)

    :repeat-n
    (do
      (var s st)
      (for [_ 1 node.n]
        (each [_ child (ipairs node.steps)]
          (set s (estimate-node child s acc))))
      s)

    :when
    (if (node.pred st)
      (do
        (var s st)
        (each [_ child (ipairs node.steps)]
          (set s (estimate-node child s acc)))
        s)
      st)

    _
    (error (.. "estimate: unknown node type: " (tostring node.type)))))

(fn estimate [wf st]
  "Return {:seconds N :actions N :bucket-cost {:action N ...} :assumptions {...}}."
  (let [acc {:seconds 0 :actions 0 :bucket-cost {} :assumptions {}}]
    (estimate-node wf st acc)
    acc))

;; ─── simulate pass ──────────────────────────────────────────────────────────
;; v1: simulate = deterministic estimate (stochastic combat stubbed).
;; Returns {:feasible bool :estimate {...} :gathers N}.

(fn simulate [wf st _trials]
  (let [result (estimate wf st)]
    {:feasible true
     :estimate result
     :gathers (or (. result.assumptions :gathers) 0)}))

;; ─── run pass ───────────────────────────────────────────────────────────────

(fn run-node [node]
  "Execute one AST node against the real character via host fns."
  (match node.type
    :seq
    (each [_ child (ipairs node.steps)]
      (run-node child))

    :action
    (let [spec (get-action node.op)]
      (spec.run nil (table.unpack node.args)))

    :repeat-until
    (do
      (var done false)
      (var iters 0)
      (while (and (not done) (< iters MAX-ITERS))
        (each [_ child (ipairs node.steps)]
          (run-node child))
        (set iters (+ iters 1))
        (let [v (host.view)]
          (set done (node.pred v))))
      (when (>= iters MAX-ITERS)
        (error "run: repeat-until did not terminate within MAX-ITERS")))

    :repeat-n
    (for [_ 1 node.n]
      (each [_ child (ipairs node.steps)]
        (run-node child)))

    :when
    (let [v (host.view)]
      (when (node.pred v)
        (each [_ child (ipairs node.steps)]
          (run-node child))))

    _
    (error (.. "run: unknown node type: " (tostring node.type)))))

(fn run [wf]
  "Execute a workflow against the real character."
  (run-node wf))

;; ─── workflow AST constructors ───────────────────────────────────────────────
;; These build tables, not closures. Loading a workflow produces a value.

(fn seq [...]
  {:type :seq :steps [...]})

(fn action [op ...]
  {:type :action :op op :args [...]})

(fn repeat_until [pred label ...]
  {:type :repeat-until :pred pred :label label :steps [...]})

(fn repeat-n [n ...]
  {:type :repeat-n :n n :steps [...]})

(fn when_pred [pred ...]
  {:type :when :pred pred :steps [...]})

;; ─── global action table registration ───────────────────────────────────────

(fn set_actions [actions-tbl]
  (tset _G :_artifacts_actions actions-tbl))

;; Export.
{:estimate estimate
 :simulate simulate
 :run run
 :seq seq
 :action action
 :repeat_until repeat_until
 :repeat_n repeat-n
 :when_pred when_pred
 :set_actions set_actions}
