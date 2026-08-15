;; Test-only component that burns fuel in a tight loop.
(component
  (core module $m
    (func (export "burn")
      (loop $l (br $l)))
  )
  (core instance $i (instantiate $m))
  (func (export "burn") (canon lift (core func $i "burn")))
)
