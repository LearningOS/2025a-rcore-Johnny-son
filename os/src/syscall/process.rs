//! Process management syscalls
use crate::{
    task::{exit_current_and_run_next, get_syscall_times, suspend_current_and_run_next},
    timer::get_time_us,
};

/// TraceRequest 的类型定义
const TRACE_READ: usize = 0;
const TRACE_WRITE: usize = 1;
const TRACE_SYSCALL: usize = 2;

#[repr(C)]
#[derive(Debug)]
pub struct TimeVal {
    pub sec: usize,
    pub usec: usize,
}

/// task exits and submit an exit code
pub fn sys_exit(exit_code: i32) -> ! {
    trace!("[kernel] Application exited with code {}", exit_code);
    exit_current_and_run_next();
    panic!("Unreachable in sys_exit!");
}

/// current task gives up resources for other tasks
pub fn sys_yield() -> isize {
    trace!("kernel: sys_yield");
    suspend_current_and_run_next();
    0
}

/// get time with second and microsecond
pub fn sys_get_time(ts: *mut TimeVal, _tz: usize) -> isize {
    trace!("kernel: sys_get_time");
    let us = get_time_us();
    unsafe {
        *ts = TimeVal {
            sec: us / 1_000_000,
            usec: us % 1_000_000,
        };
    }
    0
}

/// sys_trace 系统调用
/// - request = 0 (Read): 读取地址 id 处的一个字节，返回该字节的值
/// - request = 1 (Write): 向地址 id 写入 data 的低8位，成功返回0
/// - request = 2 (Syscall): 返回系统调用 id 的调用次数
pub fn sys_trace(trace_request: usize, id: usize, data: usize) -> isize {
    trace!("kernel: sys_trace request={}, id={:#x}, data={}", trace_request, id, data);
    match trace_request {
        TRACE_READ => {
            // 从用户态地址读取一个字节
            let addr = id as *const u8;
            unsafe { (*addr) as isize }
        }
        TRACE_WRITE => {
            // 向用户态地址写入一个字节
            let addr = id as *mut u8;
            unsafe {
                *addr = data as u8;
            }
            0
        }
        TRACE_SYSCALL => {
            // 返回系统调用的调用次数
            get_syscall_times(id) as isize
        }
        _ => -1,
    }
}
