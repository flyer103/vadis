# ADR-002 — 插件运行时采用 Cordis 语义（revertible effects + reactive coeffects）

- 状态：accepted
- 日期：2026-09-19
- 依据：Yifan Shi, Wei Zhang, Tianyi Cui, *A Programming Paradigm for Spatiotemporal Composability*,
  arXiv:2608.25512（§3 机制、§5.1 核心库、§5.2 声明式 loader 与 HMR）

## 背景

插件系统要长期承载"成本/效果实验"：插件会被频繁加载、卸载、并存（A/B、shadow）、并相互依赖。
朴素钩子式插件有两个已知缺陷：**卸载不干净**（副作用残留，无法判定某实验的真实影响）与**依赖靠人工
排序/全局约定**（加一个插件要改别人的代码）。论文把这两个问题分别形式化为 temporal composability
（可逆副作用）与 spatial composability（被动声明的依赖解析）。

## 决策

采用论文的 **context paradigm** 作为运行时内核，取其中四组原语：

| 论文原语 | 本项目用法 |
|---|---|
| `ctx.effect(cb) → dispose`（LIFO 逆累加） | 每个 transform/服务注册必须自带逆；卸载插件 = 回滚其全部 effect |
| `ctx.set/get(key)` + `notify → refresh` | 类型化服务槽；**提供者下线时依赖者先停用**，再撤绑定 |
| `fiber.inject`（coeffect 声明） | 插件声明依赖；未满足时停在加载等待，不乱序报错 |
| `ctx.isolate(key, realm)` / `ctx.intercept(key, md)` | 同 key 多 realm（同策略两版本并存做 shadow）；不换绑定只改用法（采样/超时） |
| entries + keyed diff | 声明式 `plugins:` 列表；per-field 最小操作（config 自 diff / disabled 卸载 / id 变更重建） |

## Rust 落地取舍（重要）

论文的 HMR 依赖动态模块加载。本项目：

- **tier-A（编译期链接）插件没有模块级 HMR**——代码变更 = 重建 + 重启。只做**配置级协调**
  （config / 权重 / 规则 TOML 立即生效）。
- **tier-B（进程外 UDS/WASM）插件具备模块级重载**——实验类与模型决策类插件强制走 tier-B。
- 为压低重启代价：cache 账本、粘性表、trace 缓冲由状态服务落盘并在重启后交接（论文 §1.2.3 指出的
  "重启丢弃进程内状态"问题在本项目由状态服务化解）。

## 后果

- 插件必须实现 `Effect` 逆（Rust 侧是显式 `undo` 闭包 + RAII 的组合）；review 时要检查"卸载后
  是否真的回到加载前状态"，并用测试断言（加载→卸载→状态等价）。
- 插件间依赖写在 manifest 的 `inject` 里，而不是代码里硬编顺序；加载顺序由运行时解析。
- realm 隔离让 A/B 从"两次实验"变成"一次并存对比"，这是本项目实验效率的关键。
