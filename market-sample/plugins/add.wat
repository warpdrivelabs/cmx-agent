;; add.wat —— 演示插件 wasm-add 的模块（WebAssembly 文本格式，wasmer 可直接跑）
;; 导出 add(a: i32, b: i32) -> i32；清单 invoke 模式调用：wasmer run add.wat --invoke add -- 2 3
(module
  (func (export "add") (param i32 i32) (result i32)
    local.get 0
    local.get 1
    i32.add))
