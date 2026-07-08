;; params.fnl — parameter schema validation + coercion for workflow modules.
;;
;; A workflow module declares a `:params` table (name -> spec) and receives a
;; coerced params table as the first argument to its `build`. This file owns the
;; DIVISION OF LABOR between shape and semantics:
;;
;;   • SHAPE validation lives here: unknown keys, missing required params, a
;;     non-numeric :number, a :bool that isn't "true"/"false", an :enum value
;;     outside its :options. All catchable with no game data at all.
;;   • SEMANTIC validation does NOT live here: whether "copper_rocks" is a real
;;     resource or "chicken" a real monster is enforced downstream by the loud
;;     host lookups (host.find_tile, host.monster_stats, host.recipe, …) that
;;     fire during `build`/`plan`. A second registry of valid codes here would
;;     be a competing source of truth that could drift from the cached
;;     reference data — so the game-code types stay opaque strings.
;;
;; `raw` values reach `coerce` either as STRINGS (the CLI passes `key=value`
;; pairs verbatim) or ALREADY TYPED (a TUI form or a composing workflow may hand
;; over a number/bool directly). Strings are coerced by :type; already-typed
;; values are type-checked, never re-coerced.
;;
;; Exports use Lua-identifier-safe underscore names (validate_schema, coerce),
;; the same convention predicates.fnl/interp.fnl follow: a hyphen or `?` in a
;; bare cross-file reference mangles to a different Lua symbol than the one that
;; was installed, so it would silently fail to resolve.

;; The nine declared param types. :string plus the five game-code types
;; (:item :resource :monster :npc :skill) all carry plain strings; :number,
;; :bool, and :enum are the only ones that coerce/constrain a value.
(local KNOWN-TYPES
  {:string true :number true :bool true :enum true
   :item true :resource true :monster true :npc true :skill true})

(fn contains? [tbl v]
  (accumulate [found false _ x (ipairs tbl) &until found]
    (= x v)))

;; ─── schema validation ───────────────────────────────────────────────────────

(fn validate-schema [schema]
  "Assert a param SCHEMA is well-formed and return it: each spec declares a
   known :type, an :enum carries a non-empty :options, and no spec sets both
   :required and a :default (a required param cannot also have a fallback). Loud
   by design — a malformed schema fails at workflow load, never mid-run."
  (assert (= :table (type schema))
          "workflow :params must be a table of {name -> spec}")
  (each [name spec (pairs schema)]
    (assert (= :table (type spec))
            (.. "param '" name "' spec must be a table, got a " (type spec)))
    (let [ty spec.type]
      (assert ty (.. "param '" name "' has no :type"))
      (assert (. KNOWN-TYPES ty)
              (.. "param '" name "' has unknown :type " (tostring ty)
                  " (expected one of :string :number :bool :enum :item"
                  " :resource :monster :npc :skill)"))
      (when (= ty :enum)
        (assert (and spec.options (> (length spec.options) 0))
                (.. "param '" name "' is :enum but declares no (non-empty)"
                    " :options list")))
      (assert (not (and spec.required (not= nil spec.default)))
              (.. "param '" name "' sets both :required and :default"
                  " (a required param takes no default)"))))
  schema)

;; ─── coercion ────────────────────────────────────────────────────────────────

;; A human-readable listing of the declared params, appended to every coerce
;; error. This doubles as the CLI's per-workflow help, so it lists each param's
;; name, type, required/default disposition, and doc.
(fn describe-params [schema]
  (let [names []]
    (each [name _ (pairs schema)]
      (table.insert names name))
    (table.sort names)
    (let [lines ["declared params:"]]
      (if (= 0 (length names))
          (table.insert lines "  (none)")
          (each [_ name (ipairs names)]
            (let [spec (. schema name)
                  disposition (if spec.required " (required)"
                                  (not= nil spec.default)
                                  (.. " (default " (tostring spec.default) ")")
                                  " (optional)")
                  doc (if spec.doc (.. " — " spec.doc) "")]
              (table.insert lines
                (.. "  " name " : " (tostring spec.type) disposition doc)))))
      (table.concat lines "\n"))))

;; Coerce/type-check ONE non-nil raw value against its spec. `fail` raises a loud
;; error that already carries the declared-param listing.
(fn coerce-one [name spec raw-val fail]
  (match spec.type
    :number
    (match (type raw-val)
      :number raw-val
      :string (or (tonumber raw-val)
                  (fail (.. "param '" name "': expected a number, got '"
                            raw-val "'")))
      other (fail (.. "param '" name "': expected a number, got a " other)))
    :bool
    (match (type raw-val)
      :boolean raw-val
      :string (match raw-val
                :true true
                :false false
                _ (fail (.. "param '" name "': expected 'true' or 'false', got '"
                            raw-val "'")))
      other (fail (.. "param '" name "': expected a bool, got a " other)))
    :enum
    (let [s (tostring raw-val)]
      (if (contains? spec.options s)
          s
          (fail (.. "param '" name "': '" s "' is not one of ["
                    (table.concat spec.options ", ") "]"))))
    ;; :string and the game-code types (:item :resource :monster :npc :skill):
    ;; the value stays a string; semantic validity is a downstream host lookup.
    _
    (if (= :string (type raw-val))
        raw-val
        (fail (.. "param '" name "': expected a string, got a "
                  (type raw-val))))))

(fn coerce [schema raw]
  "Coerce a RAW params table (CLI strings and/or already-typed values) against
   SCHEMA. Applies defaults, rejects unknown keys and missing required params,
   and coerces/type-checks each supplied value by its :type. Returns the typed
   params table handed to `build`. Every error message ends with the declared
   params — the CLI's per-workflow help."
  (assert (= :table (type schema)) "coerce: schema must be a table")
  (let [raw (or raw {})
        out {}
        fail (fn [msg] (error (.. msg "\n" (describe-params schema))))]
    ;; Unknown keys: a typo'd CLI k=v or a stale composing call.
    (each [k _ (pairs raw)]
      (when (= nil (. schema k))
        (fail (.. "unknown param '" (tostring k) "'"))))
    ;; Each declared param: coerce a supplied value, else apply the default, else
    ;; error if it is required. Defaults go through the SAME coerce-one path as
    ;; supplied values: a string default for a :number coerces, a typed default
    ;; is type-checked, and an :enum default must be a member of :options — so an
    ;; author typo fails loudly at load, not far downstream inside `build`.
    (each [name spec (pairs schema)]
      (let [supplied (. raw name)]
        (if (not= nil supplied)
            (tset out name (coerce-one name spec supplied fail))
            spec.required
            (fail (.. "missing required param '" name "'"))
            (not= nil spec.default)
            (tset out name (coerce-one name spec spec.default fail)))))
    out))

;; Export under Lua-safe keys (see header).
{:validate_schema validate-schema
 :coerce coerce}
