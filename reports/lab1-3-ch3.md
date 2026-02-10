# rCore Phase2 ch3 学习汇报（sys_trace）

> 本文对应 rCore phase2 ch3 编程练习：实现 `sys_trace` 系统调用，并统计每个进程的系统调用次数。

## 1. 我完成了什么（功能总结，≤200字）

本章我实现了 `sys_trace` 系统调用，并在内核中为每个进程维护系统调用调用次数统计。`sys_trace` 支持三类请求：从用户地址读取 1 字节、向用户地址写入 1 字节、查询指定 syscall id 的调用次数。为统计次数，我在系统调用统一入口处对当前进程的计数进行自增，并提供查询接口。进一步地，我尝试用 `BTreeMap`（Map/哈希表风格的稀疏结构）按需存储“出现过的 syscall 次数”，减少大数组的空洞浪费，并保证通过测例。

## 2. 需求理解与接口约定

### 2.1 `sys_trace` 语义

`sys_trace(trace_request, id, data)`：

- `trace_request = 0 (Read)`：读取用户态地址 `id` 处的 1 字节，返回该字节值；
- `trace_request = 1 (Write)`：向用户态地址 `id` 写入 `data` 的低 8 位，成功返回 0；
- `trace_request = 2 (Syscall)`：返回 **当前进程** 对应 `syscall id` 的调用次数；
- 其他 request：返回 -1。

> ch3 里对用户地址的读写常用 `unsafe` 直接解引用完成（便于理解），但这只是一种“教学过渡”。到 ch4/ch5 开启虚拟内存后，应使用页表翻译来安全访问用户内存。

### 2.2 syscall 次数统计的约定

- 统计粒度：按 **进程/任务（Task/TCB）** 维度统计；
- 更新时机：每次进入 syscall 分发（`syscall()`）时，对应 id 的计数 `+1`；
- 查询时机：`sys_trace(Syscall, id, _)` 读取当前进程的计数。

## 3. 实现要点（按代码路径串起来）

### 3.1 增加 syscall id 并分发

- 在 syscall 分发处增加 `SYSCALL_TRACE` 常量；
- 在 `syscall()` 的 `match` 中加入分支，跳转到 `sys_trace(...)`。

### 3.2 实现 `sys_trace`

按 request 三分支：

1) Read：将 `id` 视作用户态指针 `*const u8`，读出 1 字节返回；
2) Write：将 `id` 视作用户态指针 `*mut u8`，写入 `data as u8`，返回 0；
3) Syscall：返回某个 syscall id 的计数。

容易踩坑：

- **用户指针安全**：直接 `unsafe` 解引用会有安全隐患；
- **非法 id**：数组实现要检查范围，Map 实现要处理缺省值。

### 3.3 每进程维护 syscall 次数

核心接口：

- `update_syscall_times(syscall_id)`：当前任务的 `times[syscall_id] += 1`；
- `get_syscall_times(syscall_id)`：返回当前任务该 id 的次数。

更新点放在 syscall 统一入口的好处：覆盖全面、维护成本低。

## 4. 数据结构选择：大数组 vs Map（优化思考）

### 4.1 大数组

- 方案：`syscall_times: [u32; MAX_SYSCALL_NUM]`
- 优点：O(1)、实现简单
- 缺点：稀疏浪费空间

### 4.2 Map（BTreeMap）

- 方案：`BTreeMap<usize, u32>` 只记录出现过的 syscall
- 更新：`entry(id).and_modify(|v| *v += 1).or_insert(1)`
- 查询：`get(&id).copied().unwrap_or(0)`

在教学 OS 的 `no_std` 环境里，`BTreeMap` 通常比 `HashMap` 更省心（不需要处理哈希器/随机种子）。

## 5. 边界情况与自检

- 非法 `trace_request`：返回 -1。
- syscall id 越界：返回 0 或 -1（按约定）。
- 读写非法用户地址：开启 VM 后会 page fault；正规做法应返回错误码而不是让内核崩。

## 6. 荣誉准则（必须）

我保证本次实验提交的代码与报告由本人独立完成；在参考公开资料/讨论时，我仅借鉴思路并理解后自行实现，没有直接复制他人代码；如有引用会在报告中注明。

## 7. 个人反馈（可选）

- `sys_trace` 把“系统调用分发 + 用户地址访问 + 统计”串起来，很适合入门。
- 建议讲义明确提示：ch3 的 `unsafe` 访问是过渡，ch4 起要用页表翻译/权限检查。
