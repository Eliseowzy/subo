//! Windows 的作业对象（Job Object）：子进程连同它拉起的进程都挂在一个作业里，
//! 对应 Unix 上的进程组 + 资源限额。
//!
//! - 杀进程树：`TerminateJobObject`，xelatex 拉起的 xdvipdfmx、biber 一起结束；
//!   作业句柄关掉时（`KILL_ON_JOB_CLOSE`）残留的进程也会被系统收走，服务进程意外退出同样适用。
//! - 内存：读作业里所有进程提交内存的峰值。Unix 上量的是引擎进程的常驻内存，这里量的是整棵进程树，更严一点。
//! - 输出大小：Windows 没有 `RLIMIT_FSIZE`，改看作业累计写出的字节数。正常一遍编译远到不了上限，
//!   到了基本是死循环在往外写，和 Unix 上拦的是同一种情况。
//!
//! 进程先起来、再挂进作业，中间有一小段空档：这段时间里拉起的孙进程不在作业里。
//! TeX 都是跑到最后才调 xdvipdfmx，实际碰不到。

use std::mem::{size_of, zeroed};
use std::os::windows::io::AsRawHandle;
use std::process::Child;
use std::ptr::{null, null_mut};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAndIoAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject, JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

pub struct Job(HANDLE);

// SAFETY: 作业句柄是内核对象句柄，可以跨线程使用
unsafe impl Send for Job {}

impl Job {
    /// 建一个作业并把 `child` 挂进去。失败（比如被安全软件放进了不允许嵌套的作业）返回 `None`，
    /// 调用方退回只杀直接子进程。
    pub fn attach(child: &Child) -> Option<Job> {
        // SAFETY: 参数都是合法的空指针或已初始化的结构，大小如实传入；失败时句柄由 Job 的 Drop 关掉
        unsafe {
            let h = CreateJobObjectW(null(), null());
            if h.is_null() {
                return None;
            }
            let job = Job(h);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let set = SetInformationJobObject(
                h,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            );
            if set == 0 || AssignProcessToJobObject(h, child.as_raw_handle() as HANDLE) == 0 {
                return None;
            }
            Some(job)
        }
    }

    pub fn kill(&self) {
        // SAFETY: 句柄在 Job 存活期间有效
        unsafe { TerminateJobObject(self.0, 1) };
    }

    /// 作业里所有进程提交内存的峰值（字节）
    pub fn peak_memory(&self) -> Option<u64> {
        // SAFETY: 输出缓冲区足够大，大小如实传入
        unsafe {
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = zeroed();
            let ok = QueryInformationJobObject(
                self.0,
                JobObjectExtendedLimitInformation,
                &mut info as *mut _ as *mut _,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                null_mut(),
            );
            (ok != 0).then_some(info.PeakJobMemoryUsed as u64)
        }
    }

    /// 作业里所有进程累计写出的字节数（文件、管道都算）
    pub fn bytes_written(&self) -> Option<u64> {
        // SAFETY: 同上
        unsafe {
            let mut info: JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION = zeroed();
            let ok = QueryInformationJobObject(
                self.0,
                JobObjectBasicAndIoAccountingInformation,
                &mut info as *mut _ as *mut _,
                size_of::<JOBOBJECT_BASIC_AND_IO_ACCOUNTING_INFORMATION>() as u32,
                null_mut(),
            );
            (ok != 0).then_some(info.IoInfo.WriteTransferCount)
        }
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: 句柄只在这里关一次
        unsafe { CloseHandle(self.0) };
    }
}
