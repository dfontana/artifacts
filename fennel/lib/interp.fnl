;; interp.fnl — the two interpreters: plan and run.
;; Both consume the same workflow AST. The AST is a table (not closures),
;; so it is introspectable — this is what makes offline planning possible.
;;
;; `plan` walks the workflow against a seed model state (ideally a live
;; character's current state) to predict cost AND feasibility with no I/O;
;; `run` executes it for real. There is one offline pass, not two: an earlier
;; split (estimate = cost, simulate = a deterministic wrapper that always said
;; "feasible") only duplicated the same walk.
;;
;; Accumulator `acc` is a Lua table mutated in place via tset throughout.
;; plan-node returns only the new model state; acc is always mutated by ref.

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

;; Record a reason the workflow can't be carried out as written, and flip the
;; plan to infeasible. Blockers are human-readable so the CLI can print them.
(fn acc-add-blocker [acc msg]
  (let [bs (or acc.blockers [])]
    (tset acc :blockers bs)
    (table.insert bs msg)
    (tset acc :feasible false)))

;; Record a non-fatal risk (e.g. probabilistic fight drops that *might* overflow
;; inventory). Unlike a blocker this does NOT flip feasibility — it's advisory.
(fn acc-add-warning [acc msg]
  (let [ws (or acc.warnings [])]
    (tset acc :warnings ws)
    (table.insert ws msg)))

;; ─── plan pass ───────────────────────────────────────────────────────────────
;; acc is mutated in place; plan-node returns new-st only. Threading the model
;; state through each action's :sim is what makes feasibility checkable: the
;; same walk that sums cost also watches the evolving state for blockers.

(fn plan-node [node st acc]
  "Walk one AST node. Mutates acc in place. Returns new-st."
  ;; Thread state through a node's children, returning the final state.
  (fn walk [steps st]
    (var s st)
    (each [_ child (ipairs steps)]
      (set s (plan-node child s acc)))
    s)
  (match node.type
    :seq
    (walk node.steps st)

    :action
    (let [spec (get-action node.op)
          cost (spec.cost st (table.unpack node.args))
          new-st (spec.sim st (table.unpack node.args))
          bk (or node.bucket spec.bucket :action)]
      (tset acc :seconds (+ (or acc.seconds 0) cost))
      (acc-add-action acc bk)
      ;; A :sim may flag a hard blocker on the state (e.g. an unwinnable fight);
      ;; drain it here so the blocker is recorded against the plan.
      (when new-st.--pending-blocker
        (acc-add-blocker acc new-st.--pending-blocker)
        (tset new-st :--pending-blocker nil))
      ;; Feasibility: an action that ADDS to inventory must never push the model
      ;; past capacity. This catches e.g. a fixed gather count that exceeds the
      ;; room the character has. We only flag the action that actually added
      ;; items (an additive step) so a later travel/deposit doesn't inherit the
      ;; blame for an earlier overshoot. Actions whose yield is probabilistic
      ;; (declared via spec.probabilistic-drops, e.g. :fight) only warn on an
      ;; overshoot, since the expected total is fractional; deterministic adds
      ;; are a hard blocker.
      (let [prev (or st.inventory-count 0)
            cnt (or new-st.inventory-count 0)
            cap (or new-st.inventory-max-items 0)]
        (when (and (> cap 0) (> cnt cap) (> cnt prev))
          (if spec.probabilistic-drops
              (acc-add-warning acc
                (.. node.op " drops may overflow inventory: ~" (math.floor cnt)
                    " expected vs capacity " cap))
              (acc-add-blocker acc
                (.. "inventory overflow after " node.op ": holds " cnt
                    " but capacity is " cap)))))
      new-st)

    :repeat-until
    (do
      (var s st)
      (var iters 0)
      (while (and (not (node.pred s)) (< iters MAX-ITERS))
        (set s (walk node.steps s))
        (set iters (+ iters 1)))
      ;; A loop the model can't exit is infeasible, not a hard crash: record it
      ;; and move on so plan still returns a full report.
      (when (>= iters MAX-ITERS)
        (acc-add-blocker acc
          (.. "loop '" (tostring (or node.label :loop))
              "' did not terminate within " MAX-ITERS " iterations")))
      (let [assumptions (or acc.assumptions {})]
        (tset acc :assumptions assumptions)
        (tset assumptions (or node.label :loop) iters))
      s)

    :repeat-n
    (do
      (var s st)
      (for [_ 1 node.n]
        (set s (walk node.steps s)))
      s)

    :when
    (if (node.pred st)
      (walk node.steps st)
      st)

    _
    (error (.. "plan: unknown node type: " (tostring node.type)))))

(fn plan [wf st]
  "Predict cost AND feasibility of a workflow from a seed model state (ideally a
   live character's current state). Returns
   {:seconds N :actions N :bucket-cost {:action N ...} :assumptions {...}
    :feasible bool :blockers [...]}."
  (let [acc {:seconds 0 :actions 0 :bucket-cost {} :assumptions {}
             :feasible true :blockers [] :warnings []}]
    (plan-node wf st acc)
    acc))

;; ─── run pass ───────────────────────────────────────────────────────────────

(fn run-node [node]
  "Execute one AST node against the real character via host fns."
  (fn run-steps [steps]
    (each [_ child (ipairs steps)]
      (run-node child)))
  (match node.type
    :seq
    (run-steps node.steps)

    :action
    (let [spec (get-action node.op)]
      (spec.run nil (table.unpack node.args)))

    :repeat-until
    (do
      (var done false)
      (var iters 0)
      (while (and (not done) (< iters MAX-ITERS))
        (run-steps node.steps)
        (set iters (+ iters 1))
        (let [v (host.view)]
          (set done (node.pred v))))
      (when (>= iters MAX-ITERS)
        (error "run: repeat-until did not terminate within MAX-ITERS")))

    :repeat-n
    (for [_ 1 node.n]
      (run-steps node.steps))

    :when
    (let [v (host.view)]
      (when (node.pred v)
        (run-steps node.steps)))

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
{:plan plan
 :run run
 :seq seq
 :action action
 :repeat_until repeat_until
 :repeat_n repeat-n
 :when_pred when_pred
 :set_actions set_actions}
