# rCore Phase2 ch5 学习汇报（spawn + stride 调度）

> 本文对应 rCore phase2 ch5 编程练习：迁移 ch4 的 `sys_get_time/sys_mmap/sys_munmap`，实现 `sys_spawn`（DIY 创建进程），实现 stride 调度与 `sys_set_priority`，并保持对前一章测例的前向兼容。

## 1. 我完成了什么（功能总结，≤200字）

本章我在 `ch5` 分支补齐并迁移了 `sys_get_time/sys_mmap/sys_munmap`，使其适配 ch5 的进程（TCB）结构与虚拟内存访问方式；实现 `sys_spawn(path)`（syscall id=400）直接创建子进程并执行目标程序，不再像 `fork` 那样复制父进程地址空间；实现 `sys_set_priority(prio)`（id=140）设置当前进程优先级（要求 prio>=2）。调度器从 FIFO 改为 stride 调度：每个 runnable 进程维护 `stride/pass/priority`，每次从就绪队列中选择 stride 最小者运行，并令其 `stride += pass`，从而实现按优先级比例的公平调度。最终在 `make run BASE=2` 下运行 `ch5_usertest` 通过。

## 2. 需求理解与接口约定

### 2.1 `spawn` 系统调用

- 原型：`sys_spawn(path: *const u8) -> isize`
- syscall id：400
- 功能：新建子进程，使其从入口开始执行 `path` 对应的用户程序
- 返回：成功返回子进程 pid；失败返回 -1
- 失败原因：文件名无效（找不到对应 app）等

关键点：**spawn 不必复制父进程地址空间**。它更像“直接 new 一个进程并装载 ELF”。

### 2.2 stride 调度 + `set_priority`

- `sys_set_priority(prio: isize) -> isize`（id=140）
  - 要求：`prio >= 2`
  - 返回：合法返回 prio；非法返回 -1
- stride 调度参数：
  - `priority`：进程优先级，初始为 16
  - `BIG_STRIDE`：大常数（我取 10000，避免过小带来除法误差）
  - `pass = BIG_STRIDE / priority`
  - `stride`：当前进程“已运行长度”，初始为 0

调度规则：每次从所有 runnable 进程里选择 `stride` 最小者运行；选中后执行 `stride += pass`。

### 2.3 前向兼容与 BASE=2

从 ch5 开始要求内核前向兼容：除被删除的 trace 测例以外，需要继续通过前一章测例。并且由于默认初始程序可能是 `ch5b_initproc`，需要用 `BASE=2` 加载完整应用集合。

## 3. 实现要点（按代码路径串起来）

### 3.1 `spawn`：不 fork，不 exec，而是直接 new

代码路径（以当前仓库为例）：

- `user` 侧：`spawn("ch5_spawn0\0")` 触发 `ecall`
- `os/src/syscall/mod.rs`：分发到 `sys_spawn`
- `os/src/syscall/process.rs::sys_spawn`：
  1) `translated_str(token, path)` 从用户指针取出路径字符串
  2) `get_app_data_by_name(path)` 查表拿到 ELF 数据
  3) `TaskControlBlock::new(elf)` 创建子进程（新地址空间）
  4) 建立 parent/children 关系
  5) `add_task(child)` 放入就绪队列
  6) 返回子 pid

对比 `fork`：`fork` 走 `MemorySet::from_existed_user` 复制父地址空间；而 `spawn` 走 `MemorySet::from_elf`，直接构造一个全新的地址空间。

### 3.2 `sys_set_priority`：只改当前进程控制块字段

实现要点：

- 参数校验：`prio < 2` 直接返回 -1
- 写入当前任务的 `inner.priority`
- 同步更新 `inner.pass = BIG_STRIDE / priority`

### 3.3 stride 调度：在 TaskManager::fetch 里选最小 stride

为了便于实现与调试，我采用“暴力扫一遍”的方式：

- 就绪队列仍然用 `VecDeque<Arc<TaskControlBlock>>` 保存
- `fetch()` 时线性扫描选 `stride` 最小的进程
- 将其从队列中 remove 出来，并执行：`stride = stride.wrapping_add(pass)`

> 实验测例规模不大，线性扫描足够；后续可优化为最小堆/优先队列。

### 3.4 迁移 `get_time/mmap/munmap`

- `sys_get_time`：调用 `get_time_us()`，组装 `TimeVal { sec, usec }`，再用 `translated_refmut` 写回用户指针。
- `sys_mmap/sys_munmap`：
  - 校验页对齐、长度为页大小整数倍、prot 合法等；
  - 调用 `MemorySet::mmap_area / munmap_area` 完成映射与取消映射；
  - `munmap_area` 支持完全覆盖、左/右裁剪、以及中间切分（split），并在 framed 情况下正确处理 `data_frames`。

## 4. 本章我学到的关键点

1) **spawn 的本质**：创建“新进程 + 新地址空间 + 从 ELF 启动”，不需要 fork 那种复制。
2) **stride 调度的落点**：调度器要能“在一组 runnable 中挑最小 stride”，以及在每次被选中后更新 stride。
3) **系统调用与进程结构耦合**：`waitpid/exit` 的 zombie 回收、`children` 列表、以及用户指针写回（exit_code）都是内核必须保证的契约。

## 5. 问答题：stride 溢出与比较器

### 5.1 8-bit stride 的反转现象

例子：两个进程 `pass = 10`，用 8-bit 无符号存 stride：

- `p1.stride = 255`
- `p2.stride = 250`

理论上下一次应选 stride 更小的 p1（因为 p2 运行后 stride 会增加）。但若 p2 运行一次：

- `p2.stride = 250 + 10 = 260`，在 8-bit 下溢出为 `4`

此时比较得到 `p2.stride(4) < p1.stride(255)`，调度器会错误地继续选 p2，这就是溢出导致的“比较反转”。

### 5.2 为什么要求 priority >= 2？（直观说明）

在不考虑溢出的理想情况，若所有进程 `priority >= 2`，则它们的 `pass <= BIG_STRIDE/2`。stride 调度每次只给“当前最小 stride”加上自己的 pass，意味着最小者最多向前跳 `BIG_STRIDE/2`。因此系统内的 `STRIDE_MAX - STRIDE_MIN` 不会无限增大，能被一个与 `BIG_STRIDE/2` 同阶的上界约束，从而在发生整数溢出时，我们仍能用“差值是否跨过半圈”的方式判断哪个 stride 更小。

### 5.3 一个可用于处理溢出的比较器写法（假设永不相等）

设：`MAX = 1<<bits`（例如 u64 的 MAX 很大），对于环形值比较，可用“差值是否小于半圈”判断：

```rust
use core::cmp::Ordering;

struct Stride(u64);

impl PartialOrd for Stride {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        // 约定永不相等
        let a = self.0;
        let b = other.0;
        let diff = a.wrapping_sub(b);
        // 当 diff 落在 (0, 2^63) 认为 a > b；落在 (2^63, 2^64) 认为 a < b
        if diff < (1u64 << 63) {
            Some(Ordering::Greater)
        } else {
            Some(Ordering::Less)
        }
    }
}

impl PartialEq for Stride {
    fn eq(&self, _other: &Self) -> bool {
        false
    }
}
```

解释：`a - b` 的环形差若在“半圈以内”，表示 a 确实比 b 大；若超过半圈，等价于 a 在 b 的“后面”（更小）。

> 本实验测例在 `u64` + 适度 `BIG_STRIDE` 下几乎不会溢出，上述比较器属于拓展理解。

## 6. 边界情况与自检清单

- `sys_spawn`：路径字符串必须能正确从用户态读取；找不到 app 返回 -1。
- `sys_set_priority`：`prio < 2` 返回 -1；更新后 pass 必须同步。
- `sys_mmap/sys_munmap`：页对齐、长度合法；`munmap` 支持 partial unmap；不能破坏已有映射。
- 调度器：ready_queue 为空时要能继续 idle；选取 stride 最小逻辑要确保不会 panic。

## 7. 荣誉准则（必须）

我保证本次实验提交的代码与报告由本人独立完成；在参考公开资料/讨论时，我仅借鉴思路并理解后自行实现，没有直接复制他人代码；如有引用会在报告中注明。

## 8. 个人反馈（可选）

- spawn 的加入让“进程创建”更直观：不需要先 fork 再 exec。
- stride 的理论很简单，但实现时最容易忽视的是：初始化 priority/pass、以及每次调度点更新 stride。
- 建议讲义配一张数据结构关系图（TCB/children/zombie/ready_queue），对初学者更友好。
