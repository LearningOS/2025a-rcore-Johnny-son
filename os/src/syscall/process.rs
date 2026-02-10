//! Process management syscalls
use crate::mm::{copy_to_user, translated_read_u8, translated_write_u8};
use crate::config::PAGE_SIZE;
use crate::mm::{MapPermission, VirtAddr};
use crate::task::{
    change_program_brk, current_mmap, current_munmap, current_user_token, exit_current_and_run_next,
    get_syscall_times, suspend_current_and_run_next,
};
use crate::timer::get_time_us;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(_exit_code: i32) -> ! {
    trace!("kernel: sys_exit");
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// YOUR JOB: get time with second and microsecond
/// HINT: You might reimplement it with virtual memory management.
/// HINT: What if [`TimeVal`] is splitted by two pages ?
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    let tv = TimeVal {
        sec: us / 1_000_000,
        usec: us % 1_000_000,
    };
    // 通过逐字节拷贝，天然支持跨页
    let bytes = unsafe {
        core::slice::from_raw_parts((&tv as *const TimeVal) as *const u8, core::mem::size_of::<TimeVal>())
    };
    match copy_to_user(current_user_token(), ts as *mut u8, bytes) {
        Ok(()) => 0,
        Err(()) => -1,
    }
}

/// TODO: Finish sys_trace to pass testcases
/// HINT: You might reimplement it with virtual memory management.
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
        TRACE_SYSCALL => {
            get_syscall_times(id) as isize
        }
        _ => -1,
    }
}

// YOUR JOB: Implement mmap.
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
    // prot 只允许低 3 位（测试 ch4_mmap3 依赖）
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
        // 没有任何 R/W/X 权限
        return -1;
    }
    let ok = current_mmap(VirtAddr(start), VirtAddr(end), perm);
    if ok { 0 } else { -1 }
}

// YOUR JOB: Implement munmap.
pub fn sys_munmap(_start: usize, _len: usize) -> isize {
    trace!("kernel: sys_munmap");
    let start = _start;
    let len = _len;
    if len == 0 {
        return -1;
    }
    // start/len 必须页对齐且 len 为页大小整数倍（测试 ch4_unmap2 依赖）
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
/// change data segment size
pub fn sys_sbrk(size: i32) -> isize {
    trace!("kernel: sys_sbrk");
    if let Some(old_brk) = change_program_brk(size) {
        old_brk as isize
    } else {
        -1
    }
}
