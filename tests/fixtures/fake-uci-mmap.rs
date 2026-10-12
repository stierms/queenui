//! Probe the actual QueenUI child limits without faulting large amounts of RAM.
//! The parent sends the budget and fixture directory after the UCI handshake,
//! so Windows has attached the process to its Job Object before any allocation.

use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead, Write},
    path::PathBuf,
};

const MAP_BYTES: usize = 200 * 1024 * 1024 * 1024;
const SMALL_BYTES: usize = 16 * 1024 * 1024;

#[cfg(target_os = "linux")]
mod platform {
    use super::*;
    use std::{ffi::c_void, os::fd::AsRawFd, ptr};
    extern "C" {
        fn mmap(
            addr: *mut c_void,
            len: usize,
            prot: i32,
            flags: i32,
            fd: i32,
            offset: i64,
        ) -> *mut c_void;
        fn munmap(addr: *mut c_void, len: usize) -> i32;
    }
    pub fn sparse(_file: &File) {}
    pub fn control(budget: usize) {
        unsafe {
            let map = mmap(ptr::null_mut(), budget + 4096, 3, 0x22, -1, 0);
            assert_ne!(
                map,
                !0usize as *mut c_void,
                "unlimited control allocation: {}",
                io::Error::last_os_error()
            );
            assert_eq!(munmap(map, budget + 4096), 0);
        }
    }
    pub fn probe(file: &File, budget: usize) -> String {
        unsafe {
            let map = mmap(ptr::null_mut(), MAP_BYTES, 1, 1, file.as_raw_fd(), 0);
            assert_ne!(
                map,
                !0usize as *mut c_void,
                "200 GiB shared read-only mmap: {}",
                io::Error::last_os_error()
            );
            // Fault only two shared zero pages to check the view is usable.
            assert_eq!(ptr::read_volatile(map.cast::<u8>()), 0);
            assert_eq!(ptr::read_volatile(map.cast::<u8>().add(MAP_BYTES - 1)), 0);
            let small = mmap(ptr::null_mut(), SMALL_BYTES, 3, 0x22, -1, 0);
            assert_ne!(small, !0usize as *mut c_void, "small private allocation");
            let large = mmap(ptr::null_mut(), budget + 4096, 3, 0x22, -1, 0);
            let error = io::Error::last_os_error();
            assert_eq!(
                large, !0usize as *mut c_void,
                "private allocation above the full budget must fail"
            );
            assert_eq!(error.raw_os_error(), Some(12), "expected ENOMEM");
            assert_eq!(munmap(small, SMALL_BYTES), 0);
            assert_eq!(munmap(map, MAP_BYTES), 0);
            format!("file-map=200GiB anonymous-small=ok anonymous-over-budget=denied error={error}")
        }
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::{ffi::c_void, os::windows::io::AsRawHandle, ptr};
    type Handle = *mut c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn DeviceIoControl(
            file: Handle,
            code: u32,
            input: *const c_void,
            input_bytes: u32,
            output: *mut c_void,
            output_bytes: u32,
            returned: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn CreateFileMappingW(
            file: Handle,
            security: *const c_void,
            protection: u32,
            high: u32,
            low: u32,
            name: *const u16,
        ) -> Handle;
        fn MapViewOfFile(
            mapping: Handle,
            access: u32,
            high: u32,
            low: u32,
            bytes: usize,
        ) -> *mut c_void;
        fn UnmapViewOfFile(base: *const c_void) -> i32;
        fn CloseHandle(handle: Handle) -> i32;
        fn VirtualAlloc(
            base: *mut c_void,
            bytes: usize,
            allocation: u32,
            protection: u32,
        ) -> *mut c_void;
        fn VirtualFree(base: *mut c_void, bytes: usize, free_type: u32) -> i32;
    }
    pub fn sparse(file: &File) {
        let mut returned = 0;
        // FSCTL_SET_SPARSE: set this before extending the file on NTFS.
        assert_ne!(
            unsafe {
                DeviceIoControl(
                    file.as_raw_handle(),
                    0x900c4,
                    ptr::null(),
                    0,
                    ptr::null_mut(),
                    0,
                    &mut returned,
                    ptr::null_mut(),
                )
            },
            0,
            "mark sparse file: {}",
            io::Error::last_os_error()
        );
    }
    pub fn control(budget: usize) {
        unsafe {
            let allocation = VirtualAlloc(ptr::null_mut(), budget + 4096, 0x3000, 4);
            assert!(
                !allocation.is_null(),
                "unlimited control commitment: {}",
                io::Error::last_os_error()
            );
            assert_ne!(VirtualFree(allocation, 0, 0x8000), 0);
        }
    }
    pub fn probe(file: &File, budget: usize) -> String {
        unsafe {
            // PAGE_READONLY and FILE_MAP_READ consume address space, not commit.
            let mapping =
                CreateFileMappingW(file.as_raw_handle(), ptr::null(), 2, 0, 0, ptr::null());
            assert!(
                !mapping.is_null(),
                "CreateFileMappingW: {}",
                io::Error::last_os_error()
            );
            let map = MapViewOfFile(mapping, 4, 0, 0, MAP_BYTES);
            assert!(
                !map.is_null(),
                "200 GiB MapViewOfFile: {}",
                io::Error::last_os_error()
            );
            assert_eq!(ptr::read_volatile(map.cast::<u8>()), 0);
            assert_eq!(ptr::read_volatile(map.cast::<u8>().add(MAP_BYTES - 1)), 0);
            // MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE. No pages are touched.
            let small = VirtualAlloc(ptr::null_mut(), SMALL_BYTES, 0x3000, 4);
            assert!(!small.is_null(), "small committed allocation");
            let large = VirtualAlloc(ptr::null_mut(), budget + 4096, 0x3000, 4);
            let error = io::Error::last_os_error();
            assert!(
                large.is_null(),
                "private commitment above the full budget must fail"
            );
            assert_ne!(VirtualFree(small, 0, 0x8000), 0);
            assert_ne!(UnmapViewOfFile(map), 0);
            assert_ne!(CloseHandle(mapping), 0);
            format!("file-map=200GiB anonymous-small=ok anonymous-over-budget=denied error={error}")
        }
    }
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|arg| arg == "--control") {
        platform::control(args[2].parse().expect("control budget"));
        println!("unlimited-control=ok");
        return;
    }
    let mut budget = None;
    let mut directory = None;
    let mut stdout = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let line = line.expect("fake UCI input");
        match line.as_str() {
            "uci" => {
                writeln!(stdout, "id name MemoryLimitProbe").unwrap();
                writeln!(stdout, "uciok").unwrap();
            }
            "isready" => writeln!(stdout, "readyok").unwrap(),
            line if line.starts_with("setoption name Budget value ") => {
                budget = Some(line[28..].parse::<usize>().expect("budget"));
            }
            line if line.starts_with("setoption name Directory value ") => {
                directory = Some(PathBuf::from(&line[31..]));
            }
            line if line.starts_with("go") => {
                let path = directory
                    .as_ref()
                    .expect("fixture directory")
                    .join(format!("mmap-{}.tb", std::process::id()));
                let file = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .expect("create sparse file");
                platform::sparse(&file);
                file.set_len(MAP_BYTES as u64).expect("extend sparse file");
                let result = platform::probe(&file, budget.expect("private memory budget"));
                drop(file);
                std::fs::remove_file(path).expect("remove sparse file");
                writeln!(stdout, "info string {result}").unwrap();
                writeln!(stdout, "bestmove e2e4").unwrap();
            }
            "quit" => break,
            _ => {}
        }
        stdout.flush().unwrap();
    }
}
