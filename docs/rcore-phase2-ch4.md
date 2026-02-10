# RCore phase2 ch4 学习笔记（mmap/munmap + 用户指针安全访问）

> 目的：记录我在 ch4 的实现设计与踩坑点。
>
> ch4 的核心变化：进入“真正的虚拟内存时代”。用户程序传进来的指针是**用户虚拟地址**，内核不能再像 ch3 一样用 `unsafe {*ptr}` 直接读写，否则会出现：
>
> - 读写未映射地址（PageFault）
> - 权限不匹配（只读页写入）
> - 安全风险（用户传内核地址）

---

## 1. ch4 要求我做什么？

本阶段主要要实现四个系统调用：

1. `sys_get_time`：把当前时间写回用户传入的 `TimeVal*`。
2. `sys_trace`：支持 Read/Write/Syscall 三类请求。
3. `sys_mmap`：在当前进程地址空间新增一段映射（按 `prot` 设置权限）。
4. `sys_munmap`：删除映射并回收页帧，要求支持**部分取消映射**（可能会把一个区域切成两段）。

同时为了 `trace(Syscall)` 统计次数，需要维护每个任务的 syscall 调用计数。

---

## 2. 总体设计（模块怎么串起来）

我把 ch4 的新增逻辑拆成 3 条链路：

### 2.1 用户指针安全访问（基础设施）

- 位置：`os/src/mm/page_table.rs`
- 作用：给 `get_time`/`trace` 提供“可检查权限的用户内存访问”。

### 2.2 syscall 实现（调用基础设施 + 操作地址空间）

- 位置：`os/src/syscall/process.rs`
- 作用：在 syscall 层做参数检查，然后：
  - `get_time/trace` 调用“用户指针安全访问”函数
  - `mmap/munmap` 调用 task 层封装的 `current_mmap/current_munmap`

### 2.3 地址空间修改（真正干活）

- 位置：`os/src/mm/memory_set.rs`
- 作用：在当前任务的 `MemorySet` 上增加/删除 `MapArea`，并更新页表。

---

## 3. 用户指针安全访问：为什么要做？怎么做？

### 3.1 ch3 的写法为什么不行？

ch3 的 `sys_trace` 可以这么写：

- Read：`unsafe { *(id as *const u8) }`
- Write：`unsafe { *(id as *mut u8) = data as u8 }`

但 ch4 引入虚拟内存后，这里的 `id` 是**用户虚拟地址**：

- 内核不能直接解引用用户虚拟地址
- 必须通过“当前进程的页表”翻译到物理地址
- 必须检查 PTE 权限位：`U/R/W`

### 3.2 新增的几个关键函数（契约式理解）

下面这些函数在 `os/src/mm/page_table.rs`：

#### (1) `PageTable::translate_va(va)`

**输入**：`va`（虚拟地址）

**输出**：`Option<(ppn, offset, flags)>`

- `ppn`：物理页号（Physical Page Number）
- `offset`：页内偏移
- `flags`：该页的 PTEFlags（R/W/X/U...）

这一步就是“查页表”。

对应代码（`os/src/mm/page_table.rs`）：

```rust
/// Translate a virtual address to (physical page number, page offset, pte flags)
/// Return None if the mapping does not exist.
pub fn translate_va(&self, va: VirtAddr) -> Option<(PhysPageNum, usize, PTEFlags)> {
   let vpn = va.floor();
   let offset = va.page_offset();
   self.translate(vpn)
      .map(|pte| (pte.ppn(), offset, pte.flags()))
}
```

#### (2) `translated_read_u8(token, ptr)`

从用户虚拟地址读 1 字节，要求权限：

- 必须含 `U`（用户可访问）
- 必须含 `R`（可读）

失败返回 `None`。

对应代码（`os/src/mm/page_table.rs`）：

```rust
/// Read a byte from user virtual address with permission check.
pub fn translated_read_u8(token: usize, ptr: *const u8) -> Option<u8> {
   let page_table = PageTable::from_token(token);
   let va = VirtAddr::from(ptr as usize);
   let (ppn, off, flags) = page_table.translate_va(va)?;
   if !flags.contains(PTEFlags::U) || !flags.contains(PTEFlags::R) {
      return None;
   }
   Some(ppn.get_bytes_array()[off])
}
```

#### (3) `translated_write_u8(token, ptr, val)`

向用户虚拟地址写 1 字节，要求权限：

- 必须含 `U`
- 必须含 `W`

失败返回 `Err(())`。

对应代码（`os/src/mm/page_table.rs`）：

```rust
/// Write a byte to user virtual address with permission check.
pub fn translated_write_u8(token: usize, ptr: *mut u8, val: u8) -> Result<(), ()> {
   let page_table = PageTable::from_token(token);
   let va = VirtAddr::from(ptr as usize);
   let (ppn, off, flags) = page_table.translate_va(va).ok_or(())?;
   if !flags.contains(PTEFlags::U) || !flags.contains(PTEFlags::W) {
      return Err(());
   }
   ppn.get_bytes_array()[off] = val;
   Ok(())
}
```

#### (4) `copy_to_user(token, dst, src)`

把一段内核 buffer 拷贝到用户虚拟地址，特点：

- **逐页检查**：每页都要 `U|W`
- 支持跨页：比如 `TimeVal` 被拆在两页里也能写对

它做法大概是：

- while 还有没写完：
  - 翻译当前 `dst` 所在页
  - 算本页还能写多少（`PAGE_SIZE - page_off`）
  - 拷贝一段，推进指针

对应代码（`os/src/mm/page_table.rs`，核心循环）：

```rust
/// Copy bytes from kernel buffer to user virtual memory with permission check (U|W per page).
pub fn copy_to_user(token: usize, dst: *mut u8, src: &[u8]) -> Result<(), ()> {
   let page_table = PageTable::from_token(token);
   let mut start = dst as usize;
   let end = start + src.len();
   let mut copied = 0usize;
   while start < end {
      let start_va = VirtAddr::from(start);
      let vpn = start_va.floor();
      let pte = page_table.translate(vpn).ok_or(())?;
      let flags = pte.flags();
      if !flags.contains(PTEFlags::U) || !flags.contains(PTEFlags::W) {
         return Err(());
      }
      let ppn = pte.ppn();
      let page_off = start_va.page_offset();
      let page_left = PAGE_SIZE - page_off;
      let to_copy = (end - start).min(page_left);
      let dst_slice = &mut ppn.get_bytes_array()[page_off..page_off + to_copy];
      dst_slice.copy_from_slice(&src[copied..copied + to_copy]);
      start += to_copy;
      copied += to_copy;
   }
   Ok(())
}
```

---

## 4. `sys_get_time`：如何保证跨页也正确？

- 位置：`os/src/syscall/process.rs`
- 关键点：不能直接写 `*ts = tv`，必须用 `copy_to_user`。

实现思路：

1. `get_time_us()` 取到微秒
2. 构造 `TimeVal { sec, usec }`
3. 把 `TimeVal` 看成字节切片（`&[u8]`）
4. `copy_to_user(current_user_token(), ts as *mut u8, bytes)`

写成功返回 0，失败返回 -1。

对应代码（`os/src/syscall/process.rs`）：

```rust
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
   trace!("kernel: sys_get_time");
   let us = get_time_us();
   let tv = TimeVal {
      sec: us / 1_000_000,
      usec: us % 1_000_000,
   };
   // 通过逐字节拷贝，天然支持跨页
   let bytes = unsafe {
      core::slice::from_raw_parts(
         (&tv as *const TimeVal) as *const u8,
         core::mem::size_of::<TimeVal>(),
      )
   };
   match copy_to_user(current_user_token(), ts as *mut u8, bytes) {
      Ok(()) => 0,
      Err(()) => -1,
   }
}
```

---

## 5. `sys_trace`（ch4 版本）：Read/Write/Syscall

- 位置：`os/src/syscall/process.rs`
- 三种请求：

### 5.1 Read

调用 `translated_read_u8`：

- 检查 `U|R`
- 成功返回字节值
- 失败返回 -1

### 5.2 Write

调用 `translated_write_u8`：

- 检查 `U|W`
- 成功返回 0
- 失败返回 -1

### 5.3 Syscall

返回 `get_syscall_times(id)`。

对应代码（`os/src/syscall/process.rs`）：

```rust
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
   trace!("kernel: sys_trace");
   const TRACE_READ: usize = 0;
   const TRACE_WRITE: usize = 1;
   const TRACE_SYSCALL: usize = 2;
   match trace_request {
      TRACE_READ => translated_read_u8(current_user_token(), id as *const u8)
         .map(|v| v as isize)
         .unwrap_or(-1),
      TRACE_WRITE => translated_write_u8(current_user_token(), id as *mut u8, data as u8)
         .map(|_| 0)
         .unwrap_or(-1),
      TRACE_SYSCALL => get_syscall_times(id) as isize,
      _ => -1,
   }
}
```

---

## 6. syscall 次数统计：为什么放在 TCB？怎么更新？

### 6.1 数据结构

- 位置：`os/src/task/task.rs`
- 在 `TaskControlBlock` 里新增：

```rust
pub syscall_times: [u32; MAX_SYSCALL_NUM]
```

并在 `TaskControlBlock::new()` 初始化为 0。

对应代码（`os/src/task/task.rs` 的结构体字段 + 初始化）：

```rust
pub struct TaskControlBlock {
   // ...
   /// Per-task syscall counter
   pub syscall_times: [u32; MAX_SYSCALL_NUM],
}

// 在 new() 里：
syscall_times: [0; MAX_SYSCALL_NUM],
```

### 6.2 更新与查询

- 位置：`os/src/task/mod.rs`

```rust
pub fn update_syscall_times(syscall_id: usize)
pub fn get_syscall_times(syscall_id: usize) -> u32
```

- 更新逻辑：找到当前任务 `inner.current_task`，对应数组 +1
- 查询逻辑：返回对应计数

它与 `trace(Syscall)` 连接起来，让 `sys_trace` 可以查“当前任务的 syscall 次数”。

对应代码（`os/src/task/mod.rs`）：

```rust
/// Increment current task's syscall counter.
pub fn update_syscall_times(syscall_id: usize) {
   use crate::config::MAX_SYSCALL_NUM;
   if syscall_id >= MAX_SYSCALL_NUM {
      return;
   }
   let mut inner = TASK_MANAGER.inner.exclusive_access();
   let cur = inner.current_task;
   inner.tasks[cur].syscall_times[syscall_id] += 1;
}

/// Get current task's syscall counter value.
pub fn get_syscall_times(syscall_id: usize) -> u32 {
   use crate::config::MAX_SYSCALL_NUM;
   if syscall_id >= MAX_SYSCALL_NUM {
      return 0;
   }
   let inner = TASK_MANAGER.inner.exclusive_access();
   let cur = inner.current_task;
   inner.tasks[cur].syscall_times[syscall_id]
}
```

---

## 7. `sys_mmap`：参数校验 + 映射权限

- 位置：`os/src/syscall/process.rs`

### 7.1 参数校验（结合用例）

我最终对齐用例，做了以下检查：

- `len == 0` → -1
- `start` 必须页对齐（`start % PAGE_SIZE == 0`）否则 -1
- `start + len` 溢出 → -1
- `prot` 只能用低三位（`prot & !0x7 == 0`），否则 -1
- `prot` 不能全 0（否则没有任何 R/W/X 权限）

### 7.2 `prot -> MapPermission`

约定：

- bit0：R
- bit1：W
- bit2：X

转换时始终带 `U`：

```rust
let mut perm = MapPermission::U;
...
```

最终调用：

```rust
current_mmap(VirtAddr(start), VirtAddr(end), perm)
```

成功返回 0，失败返回 -1。

> 用例对齐点：`prot=2`（只写）允许 mmap 成功，但对该页进行读访问会触发 PageFault（用例中会验证）。

对应代码（`os/src/syscall/process.rs`）：

```rust
pub fn sys_mmap(_start: usize, _len: usize, _port: usize) -> isize {
   trace!("kernel: sys_mmap");
   let start = _start;
   let len = _len;
   let prot = _port;
   if len == 0 {
      return -1;
   }
   // start 必须页对齐
   if start % PAGE_SIZE != 0 {
      return -1;
   }
   let end = match start.checked_add(len) {
      Some(v) => v,
      None => return -1,
   };
   // prot 只允许低 3 位
   if prot & !0x7 != 0 {
      return -1;
   }

   // 计算映射权限：bit0=R bit1=W bit2=X
   let mut perm = MapPermission::U;
   if prot & 1 != 0 {
      perm |= MapPermission::R;
   }
   if prot & 2 != 0 {
      perm |= MapPermission::W;
   }
   if prot & 4 != 0 {
      perm |= MapPermission::X;
   }
   if perm == MapPermission::U {
      return -1;
   }
   let ok = current_mmap(VirtAddr(start), VirtAddr(end), perm);
   if ok { 0 } else { -1 }
}
```

---

## 8. `sys_munmap`：严格页对齐 + 支持部分取消映射

- 位置：`os/src/syscall/process.rs`

### 8.1 参数校验（用例强约束）

用例 `ch4_unmap2` 明确要求：

- `munmap(start, len + 1) == -1`
- `munmap(start + 1, len - 1) == -1`

所以 `munmap` 必须严格要求：

- `start % PAGE_SIZE == 0`
- `len % PAGE_SIZE == 0`

### 8.2 真正执行取消映射

Syscall 层只做检查，然后调用：

```rust
current_munmap(VirtAddr(start), VirtAddr(end))
```

对应代码（`os/src/syscall/process.rs`）：

```rust
pub fn sys_munmap(_start: usize, _len: usize) -> isize {
   trace!("kernel: sys_munmap");
   let start = _start;
   let len = _len;
   if len == 0 {
      return -1;
   }
   // start/len 必须页对齐且 len 为页大小整数倍
   if start % PAGE_SIZE != 0 || len % PAGE_SIZE != 0 {
      return -1;
   }
   let end = match start.checked_add(len) {
      Some(v) => v,
      None => return -1,
   };
   let ok = current_munmap(VirtAddr(start), VirtAddr(end));
   if ok { 0 } else { -1 }
}
```

---

## 9. `MemorySet::mmap_area/munmap_area`：地址空间层如何实现？

- 位置：`os/src/mm/memory_set.rs`

### 9.1 `is_range_free`：冲突检测

把 `[start, end)` 转成 VPN 范围，检查是否与任何已有 `MapArea` 重叠。

对应代码（`os/src/mm/memory_set.rs`）：

```rust
/// Check whether [start, end) (in VPN) overlaps with any existing area.
pub fn is_range_free(&self, start: VirtAddr, end: VirtAddr) -> bool {
   let start_vpn = start.floor();
   let end_vpn = end.ceil();
   self.areas.iter().all(|area| {
      let a_start = area.vpn_range.get_start();
      let a_end = area.vpn_range.get_end();
      end_vpn <= a_start || start_vpn >= a_end
   })
}
```

如果重叠，`mmap_area` 返回 false。

### 9.2 `mmap_area`：插入 framed area

- 先检查 start<end 且区间空闲
- `push(MapArea::new(..., MapType::Framed, perm))`
- `push` 内部会 `map_area.map(&mut page_table)`，为每个页分配 frame 并建立映射

对应代码（`os/src/mm/memory_set.rs`）：

```rust
/// Map a new framed area into this address space. Return false if overlaps.
pub fn mmap_area(&mut self, start: VirtAddr, end: VirtAddr, perm: MapPermission) -> bool {
   if start >= end {
      return false;
   }
   if !self.is_range_free(start, end) {
      return false;
   }
   self.push(MapArea::new(start, end, MapType::Framed, perm), None);
   true
}
```

### 9.3 `munmap_area`：支持 shrink/split（核心难点）

目标：取消映射 `[start,end)`，并支持“只取消其中一段”。

对每个可能重叠的 `MapArea`，分 4 类情况：

1. **完全覆盖**：直接 remove 该 area，并 `area.unmap(&mut page_table)`
2. **从左侧削掉**（start <= area.start && end < area.end）：调用 `shrink_left(end_vpn)`
3. **从右侧削掉**（start > area.start && end >= area.end）：调用 `shrink_to(start_vpn)`
4. **中间挖洞**：需要 split 成左右两段：
   - 先把原 area shrink 成左段 `[a_start, start_vpn)`
   - 再创建一个右段 `[end_vpn, a_end)` 的新 `MapArea`，并 map

最终只要触碰到了至少一段 area，就返回 true；如果区间完全不在任何映射内，返回 false。

对应代码（`os/src/mm/memory_set.rs`，核心逻辑）：

```rust
/// Unmap range [start, end) from this address space.
/// Support partial unmap by shrinking/splitting areas.
pub fn munmap_area(&mut self, start: VirtAddr, end: VirtAddr) -> bool {
   if start >= end {
      return false;
   }
   let start_vpn = start.floor();
   let end_vpn = end.ceil();

   let mut i = 0usize;
   let mut touched = false;
   while i < self.areas.len() {
      let a_start = self.areas[i].vpn_range.get_start();
      let a_end = self.areas[i].vpn_range.get_end();
      if end_vpn <= a_start || start_vpn >= a_end {
         i += 1;
         continue;
      }
      touched = true;
      // overlap exists
      if start_vpn <= a_start && end_vpn >= a_end {
         // remove whole area
         let mut area = self.areas.remove(i);
         area.unmap(&mut self.page_table);
         continue;
      }
      if start_vpn <= a_start && end_vpn < a_end {
         // shrink from left
         self.areas[i].shrink_left(&mut self.page_table, end_vpn);
         i += 1;
         continue;
      }
      if start_vpn > a_start && end_vpn >= a_end {
         // shrink from right
         self.areas[i].shrink_to(&mut self.page_table, start_vpn);
         i += 1;
         continue;
      }
      // split into two areas
      let right_start = end_vpn;
      let right_end = a_end;
      let right_map_type = self.areas[i].map_type;
      let right_map_perm = self.areas[i].map_perm;
      // left part shrink
      self.areas[i].shrink_to(&mut self.page_table, start_vpn);
      // create right part and map
      let r_start_va: VirtAddr = right_start.into();
      let r_end_va: VirtAddr = right_end.into();
      let mut right = MapArea::new(r_start_va, r_end_va, right_map_type, right_map_perm);
      right.map(&mut self.page_table);
      self.areas.push(right);
      i += 1;
   }
   touched
}
```

另外，为了给 syscall 层一个“好用的入口”，task 层额外封装了：

```rust
/// Map a new area into current task's address space.
pub fn current_mmap(start: crate::mm::VirtAddr, end: crate::mm::VirtAddr, perm: crate::mm::MapPermission) -> bool {
   let mut inner = TASK_MANAGER.inner.exclusive_access();
   let cur = inner.current_task;
   inner.tasks[cur].memory_set.mmap_area(start, end, perm)
}

/// Unmap an area from current task's address space.
pub fn current_munmap(start: crate::mm::VirtAddr, end: crate::mm::VirtAddr) -> bool {
   let mut inner = TASK_MANAGER.inner.exclusive_access();
   let cur = inner.current_task;
   inner.tasks[cur].memory_set.munmap_area(start, end)
}
```

---

## 10. 踩坑记录（我实际遇到的）

1. `munmap` 的 len 是否允许非页对齐：
   - 一开始我想“向上取整”更友好
   - 但用例 `ch4_unmap2` 明确要求 `len+1` 必须 -1
   - 所以最终采取严格校验

2. `mmap prot=2` 的语义：
   - 用例要求 mmap 成功，但读该页触发 fault
   - 所以内核只要按 `prot` 设置权限即可，不需要额外禁止 `W=1,R=0`

---

## 11. 如何验证我实现正确？

在 `os/` 目录运行（qemu 平台）：

```bash
make run BASE=2
```

Exit Code 为 0 且输出中包含：

- `Test 04_4 test OK!`
- `Test 04_5 ummap OK!`
- `Test 04_6 ummap2 OK!`
- `Test trace_1 OK!`

说明 ch4 相关用例通过。

> 末尾的 `All applications completed!` 是教学代码常见的“跑完所有 app 的退出方式”，不是失败。
