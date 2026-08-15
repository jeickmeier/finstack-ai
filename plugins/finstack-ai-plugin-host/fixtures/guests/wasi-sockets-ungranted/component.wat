;; Test-only component that imports wasi:sockets without a host grant.
(component
  (import "wasi:sockets/instance-network@0.2.12" (instance
    (export "instance-network" (func (result u32)))
  ))
)
