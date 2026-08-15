;; Test-only component that imports wasi:filesystem without a host grant.
(component
  (import "wasi:filesystem/preopens@0.2.12" (instance
    (export "get-directories" (func (result u32)))
  ))
)
