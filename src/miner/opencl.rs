use super::{MiningBackend, Solution};
use anyhow::{anyhow, bail, Result};
use ethers::types::U256;
use std::{
    ffi::{c_char, c_int, c_void, CString},
    mem, ptr,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

type ClInt = c_int;
type ClUInt = u32;
type ClBool = ClUInt;
type ClDeviceType = u64;
type ClMemFlags = u64;
type ClPlatformId = *mut c_void;
type ClDeviceId = *mut c_void;
type ClContext = *mut c_void;
type ClCommandQueue = *mut c_void;
type ClProgram = *mut c_void;
type ClKernel = *mut c_void;
type ClMem = *mut c_void;

const CL_SUCCESS: ClInt = 0;
const CL_TRUE: ClBool = 1;
const CL_DEVICE_TYPE_GPU: ClDeviceType = 1 << 2;
const CL_MEM_READ_WRITE: ClMemFlags = 1 << 0;
const CL_MEM_READ_ONLY: ClMemFlags = 1 << 2;
const CL_PROGRAM_BUILD_LOG: ClUInt = 0x1183;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RawResult {
    found: u32,
    nonce_lo: u32,
    nonce_hi: u32,
    hash: [u32; 8],
}

pub struct OpenClMiner {
    batch_size: usize,
    context: ClContext,
    queue: ClCommandQueue,
    program: ClProgram,
    kernel: ClKernel,
    challenge_buf: ClMem,
    difficulty_buf: ClMem,
    result_buf: ClMem,
}

impl OpenClMiner {
    pub fn new(batch_size: usize) -> Result<Self> {
        unsafe {
            let device = first_gpu_device()?;
            let mut err = 0;
            let context = clCreateContext(ptr::null(), 1, &device, None, ptr::null_mut(), &mut err);
            check(err, "clCreateContext")?;

            let queue = clCreateCommandQueue(context, device, 0, &mut err);
            check(err, "clCreateCommandQueue")?;

            let source = CString::new(KERNEL_SOURCE).expect("kernel source has no nul bytes");
            let source_ptr = source.as_ptr();
            let program = clCreateProgramWithSource(context, 1, &source_ptr, ptr::null(), &mut err);
            check(err, "clCreateProgramWithSource")?;

            err = clBuildProgram(program, 1, &device, ptr::null(), None, ptr::null_mut());
            if err != CL_SUCCESS {
                let log = build_log(program, device);
                bail!("clBuildProgram: {err}\n{log}");
            }

            let kernel_name = CString::new("mine").unwrap();
            let kernel = clCreateKernel(program, kernel_name.as_ptr(), &mut err);
            check(err, "clCreateKernel")?;

            let challenge_buf = clCreateBuffer(
                context,
                CL_MEM_READ_ONLY,
                mem::size_of::<[u32; 8]>(),
                ptr::null_mut(),
                &mut err,
            );
            check(err, "challenge buffer")?;
            let difficulty_buf = clCreateBuffer(
                context,
                CL_MEM_READ_ONLY,
                mem::size_of::<[u32; 8]>(),
                ptr::null_mut(),
                &mut err,
            );
            check(err, "difficulty buffer")?;
            let result_buf = clCreateBuffer(
                context,
                CL_MEM_READ_WRITE,
                mem::size_of::<RawResult>(),
                ptr::null_mut(),
                &mut err,
            );
            check(err, "result buffer")?;

            Ok(Self {
                batch_size,
                context,
                queue,
                program,
                kernel,
                challenge_buf,
                difficulty_buf,
                result_buf,
            })
        }
    }

    pub fn search<F>(
        &mut self,
        challenge: [u8; 32],
        difficulty: U256,
        on_progress: &mut F,
    ) -> Result<Solution>
    where
        F: FnMut(u64, f64),
    {
        let challenge_words = challenge_words(challenge);
        let difficulty_words = difficulty_words(difficulty);

        unsafe {
            write_buffer(
                self.queue,
                self.challenge_buf,
                &challenge_words,
                "write challenge",
            )?;
            write_buffer(
                self.queue,
                self.difficulty_buf,
                &difficulty_words,
                "write difficulty",
            )?;
            set_kernel_arg(self.kernel, 0, &self.challenge_buf, "arg0")?;
            set_kernel_arg(self.kernel, 1, &self.difficulty_buf, "arg1")?;
            set_kernel_arg(self.kernel, 3, &self.result_buf, "arg3")?;
        }

        let mut base = initial_base_nonce();
        let mut total: u64 = 0;
        let mut window_hashes: u64 = 0;
        let mut started = Instant::now();

        loop {
            let mut result = RawResult::default();
            unsafe {
                write_buffer(self.queue, self.result_buf, &result, "clear result")?;
                set_kernel_arg(self.kernel, 2, &base, "arg2")?;
                let global_work_size = self.batch_size;
                check(
                    clEnqueueNDRangeKernel(
                        self.queue,
                        self.kernel,
                        1,
                        ptr::null(),
                        &global_work_size,
                        ptr::null(),
                        0,
                        ptr::null(),
                        ptr::null_mut(),
                    ),
                    "enqueue",
                )?;
                check(clFinish(self.queue), "finish")?;
                read_buffer(self.queue, self.result_buf, &mut result, "read result")?;
            }

            total = total.wrapping_add(self.batch_size as u64);
            window_hashes = window_hashes.wrapping_add(self.batch_size as u64);

            if result.found != 0 {
                let nonce = ((result.nonce_hi as u64) << 32) | result.nonce_lo as u64;
                return Ok(Solution {
                    backend: MiningBackend::OpenCl,
                    nonce,
                    hash: hash_words_to_hex(result.hash),
                    hashes: total,
                });
            }

            let elapsed = started.elapsed().as_secs_f64();
            if elapsed > 0.5 {
                on_progress(window_hashes, window_hashes as f64 / elapsed);
                started = Instant::now();
                window_hashes = 0;
            }

            base = base.wrapping_add(self.batch_size as u64);
        }
    }
}

impl Drop for OpenClMiner {
    fn drop(&mut self) {
        unsafe {
            if !self.result_buf.is_null() {
                clReleaseMemObject(self.result_buf);
            }
            if !self.difficulty_buf.is_null() {
                clReleaseMemObject(self.difficulty_buf);
            }
            if !self.challenge_buf.is_null() {
                clReleaseMemObject(self.challenge_buf);
            }
            if !self.kernel.is_null() {
                clReleaseKernel(self.kernel);
            }
            if !self.program.is_null() {
                clReleaseProgram(self.program);
            }
            if !self.queue.is_null() {
                clReleaseCommandQueue(self.queue);
            }
            if !self.context.is_null() {
                clReleaseContext(self.context);
            }
        }
    }
}

fn challenge_words(bytes: [u8; 32]) -> [u32; 8] {
    let mut words = [0u32; 8];
    for (i, chunk) in bytes.chunks_exact(4).enumerate() {
        words[i] = u32::from_le_bytes(chunk.try_into().unwrap());
    }
    words
}

fn difficulty_words(value: U256) -> [u32; 8] {
    let mut bytes = [0u8; 32];
    value.to_big_endian(&mut bytes);
    let mut words = [0u32; 8];
    for (i, chunk) in bytes.chunks_exact(4).enumerate() {
        words[i] = u32::from_be_bytes(chunk.try_into().unwrap());
    }
    words
}

fn hash_words_to_hex(words: [u32; 8]) -> String {
    let mut out = String::from("0x");
    for word in words {
        out.push_str(&format!("{word:08x}"));
    }
    out
}

fn initial_base_nonce() -> u64 {
    let unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    (unix << 32) ^ (Instant::now().elapsed().as_nanos() as u64)
}

unsafe fn first_gpu_device() -> Result<ClDeviceId> {
    let mut platform_count = 0;
    check(
        clGetPlatformIDs(0, ptr::null_mut(), &mut platform_count),
        "clGetPlatformIDs(count)",
    )?;
    if platform_count == 0 {
        bail!("OpenCL platform tidak ditemukan");
    }

    let mut platforms = vec![ptr::null_mut(); platform_count as usize];
    check(
        clGetPlatformIDs(platform_count, platforms.as_mut_ptr(), ptr::null_mut()),
        "clGetPlatformIDs",
    )?;

    for platform in platforms {
        let mut device_count = 0;
        let err = clGetDeviceIDs(
            platform,
            CL_DEVICE_TYPE_GPU,
            0,
            ptr::null_mut(),
            &mut device_count,
        );
        if err != CL_SUCCESS || device_count == 0 {
            continue;
        }

        let mut devices = vec![ptr::null_mut(); device_count as usize];
        check(
            clGetDeviceIDs(
                platform,
                CL_DEVICE_TYPE_GPU,
                device_count,
                devices.as_mut_ptr(),
                ptr::null_mut(),
            ),
            "clGetDeviceIDs(GPU)",
        )?;
        return Ok(devices[0]);
    }

    Err(anyhow!(
        "OpenCL GPU tidak ditemukan. Cek ROCm/OpenCL dengan clinfo."
    ))
}

unsafe fn write_buffer<T>(
    queue: ClCommandQueue,
    buffer: ClMem,
    value: &T,
    label: &str,
) -> Result<()> {
    check(
        clEnqueueWriteBuffer(
            queue,
            buffer,
            CL_TRUE,
            0,
            mem::size_of::<T>(),
            value as *const T as *const c_void,
            0,
            ptr::null(),
            ptr::null_mut(),
        ),
        label,
    )
}

unsafe fn read_buffer<T>(
    queue: ClCommandQueue,
    buffer: ClMem,
    value: &mut T,
    label: &str,
) -> Result<()> {
    check(
        clEnqueueReadBuffer(
            queue,
            buffer,
            CL_TRUE,
            0,
            mem::size_of::<T>(),
            value as *mut T as *mut c_void,
            0,
            ptr::null(),
            ptr::null_mut(),
        ),
        label,
    )
}

unsafe fn set_kernel_arg<T>(kernel: ClKernel, index: ClUInt, value: &T, label: &str) -> Result<()> {
    check(
        clSetKernelArg(
            kernel,
            index,
            mem::size_of::<T>(),
            value as *const T as *const c_void,
        ),
        label,
    )
}

fn check(err: ClInt, label: &str) -> Result<()> {
    if err == CL_SUCCESS {
        Ok(())
    } else {
        Err(anyhow!("{label}: OpenCL error {err}"))
    }
}

unsafe fn build_log(program: ClProgram, device: ClDeviceId) -> String {
    let mut size = 0;
    let _ = clGetProgramBuildInfo(
        program,
        device,
        CL_PROGRAM_BUILD_LOG,
        0,
        ptr::null_mut(),
        &mut size,
    );
    if size == 0 {
        return String::new();
    }
    let mut buf = vec![0u8; size];
    let _ = clGetProgramBuildInfo(
        program,
        device,
        CL_PROGRAM_BUILD_LOG,
        size,
        buf.as_mut_ptr() as *mut c_void,
        ptr::null_mut(),
    );
    String::from_utf8_lossy(&buf).trim_matches('\0').to_string()
}

#[link(name = "OpenCL")]
extern "C" {
    fn clGetPlatformIDs(
        num_entries: ClUInt,
        platforms: *mut ClPlatformId,
        num_platforms: *mut ClUInt,
    ) -> ClInt;
    fn clGetDeviceIDs(
        platform: ClPlatformId,
        device_type: ClDeviceType,
        num_entries: ClUInt,
        devices: *mut ClDeviceId,
        num_devices: *mut ClUInt,
    ) -> ClInt;
    fn clCreateContext(
        properties: *const isize,
        num_devices: ClUInt,
        devices: *const ClDeviceId,
        pfn_notify: Option<extern "C" fn(*const c_char, *const c_void, usize, *mut c_void)>,
        user_data: *mut c_void,
        errcode_ret: *mut ClInt,
    ) -> ClContext;
    fn clCreateCommandQueue(
        context: ClContext,
        device: ClDeviceId,
        properties: u64,
        errcode_ret: *mut ClInt,
    ) -> ClCommandQueue;
    fn clCreateProgramWithSource(
        context: ClContext,
        count: ClUInt,
        strings: *const *const c_char,
        lengths: *const usize,
        errcode_ret: *mut ClInt,
    ) -> ClProgram;
    fn clBuildProgram(
        program: ClProgram,
        num_devices: ClUInt,
        device_list: *const ClDeviceId,
        options: *const c_char,
        pfn_notify: Option<extern "C" fn(ClProgram, *mut c_void)>,
        user_data: *mut c_void,
    ) -> ClInt;
    fn clGetProgramBuildInfo(
        program: ClProgram,
        device: ClDeviceId,
        param_name: ClUInt,
        param_value_size: usize,
        param_value: *mut c_void,
        param_value_size_ret: *mut usize,
    ) -> ClInt;
    fn clCreateKernel(
        program: ClProgram,
        kernel_name: *const c_char,
        errcode_ret: *mut ClInt,
    ) -> ClKernel;
    fn clCreateBuffer(
        context: ClContext,
        flags: ClMemFlags,
        size: usize,
        host_ptr: *mut c_void,
        errcode_ret: *mut ClInt,
    ) -> ClMem;
    fn clSetKernelArg(
        kernel: ClKernel,
        arg_index: ClUInt,
        arg_size: usize,
        arg_value: *const c_void,
    ) -> ClInt;
    fn clEnqueueWriteBuffer(
        command_queue: ClCommandQueue,
        buffer: ClMem,
        blocking_write: ClBool,
        offset: usize,
        size: usize,
        ptr: *const c_void,
        num_events_in_wait_list: ClUInt,
        event_wait_list: *const c_void,
        event: *mut c_void,
    ) -> ClInt;
    fn clEnqueueReadBuffer(
        command_queue: ClCommandQueue,
        buffer: ClMem,
        blocking_read: ClBool,
        offset: usize,
        size: usize,
        ptr: *mut c_void,
        num_events_in_wait_list: ClUInt,
        event_wait_list: *const c_void,
        event: *mut c_void,
    ) -> ClInt;
    fn clEnqueueNDRangeKernel(
        command_queue: ClCommandQueue,
        kernel: ClKernel,
        work_dim: ClUInt,
        global_work_offset: *const usize,
        global_work_size: *const usize,
        local_work_size: *const usize,
        num_events_in_wait_list: ClUInt,
        event_wait_list: *const c_void,
        event: *mut c_void,
    ) -> ClInt;
    fn clFinish(command_queue: ClCommandQueue) -> ClInt;
    fn clReleaseMemObject(memobj: ClMem) -> ClInt;
    fn clReleaseKernel(kernel: ClKernel) -> ClInt;
    fn clReleaseProgram(program: ClProgram) -> ClInt;
    fn clReleaseCommandQueue(command_queue: ClCommandQueue) -> ClInt;
    fn clReleaseContext(context: ClContext) -> ClInt;
}

const KERNEL_SOURCE: &str = r#"
#pragma OPENCL EXTENSION cl_khr_int64_base_atomics : enable
typedef struct{uint found;uint nonce_lo;uint nonce_hi;uint hash[8];} Result;
__constant ulong RC[24]={0x0000000000000001UL,0x0000000000008082UL,0x800000000000808aUL,0x8000000080008000UL,0x000000000000808bUL,0x0000000080000001UL,0x8000000080008081UL,0x8000000000008009UL,0x000000000000008aUL,0x0000000000000088UL,0x0000000080008009UL,0x000000008000000aUL,0x000000008000808bUL,0x800000000000008bUL,0x8000000000008089UL,0x8000000000008003UL,0x8000000000008002UL,0x8000000000000080UL,0x000000000000800aUL,0x800000008000000aUL,0x8000000080008081UL,0x8000000000008080UL,0x0000000080000001UL,0x8000000080008008UL};
__constant int R[24]={1,3,6,10,15,21,28,36,45,55,2,14,27,41,56,8,25,43,62,18,39,61,20,44};
__constant int P[24]={10,7,11,17,18,3,5,16,8,21,24,4,15,23,19,13,12,2,20,14,22,9,6,1};
uint bswap32(uint v){return ((v&255U)<<24)|((v&65280U)<<8)|((v&16711680U)>>8)|((v&4278190080U)>>24);}
ulong rotl64(ulong x,int s){return rotate(x,(ulong)s);}
void keccakf(ulong st[25]){int i,j,r;ulong t,bc[5];for(r=0;r<24;r++){for(i=0;i<5;i++)bc[i]=st[i]^st[i+5]^st[i+10]^st[i+15]^st[i+20];for(i=0;i<5;i++){t=bc[(i+4)%5]^rotl64(bc[(i+1)%5],1);for(j=0;j<25;j+=5)st[j+i]^=t;}t=st[1];for(i=0;i<24;i++){j=P[i];bc[0]=st[j];st[j]=rotl64(t,R[i]);t=bc[0];}for(j=0;j<25;j+=5){for(i=0;i<5;i++)bc[i]=st[j+i];for(i=0;i<5;i++)st[j+i]^=(~bc[(i+1)%5])&bc[(i+2)%5];}st[0]^=RC[r];}}
int below(uint h[8],__global const uint *d){for(int i=0;i<8;i++){if(h[i]<d[i])return 1;if(h[i]>d[i])return 0;}return 0;}
__kernel void mine(__global const uint *challenge,__global const uint *difficulty,ulong base,__global Result *out){size_t gid=get_global_id(0);ulong nonce=base+(ulong)gid;ulong st[25];for(int i=0;i<25;i++)st[i]=0UL;st[0]=((ulong)challenge[1]<<32)|challenge[0];st[1]=((ulong)challenge[3]<<32)|challenge[2];st[2]=((ulong)challenge[5]<<32)|challenge[4];st[3]=((ulong)challenge[7]<<32)|challenge[6];uint lo=(uint)(nonce&0xffffffffUL);uint hi=(uint)(nonce>>32);st[7]=((ulong)bswap32(lo)<<32)|bswap32(hi);st[8]=1UL;st[16]=0x8000000000000000UL;keccakf(st);uint h[8];h[0]=bswap32((uint)(st[0]&0xffffffffUL));h[1]=bswap32((uint)(st[0]>>32));h[2]=bswap32((uint)(st[1]&0xffffffffUL));h[3]=bswap32((uint)(st[1]>>32));h[4]=bswap32((uint)(st[2]&0xffffffffUL));h[5]=bswap32((uint)(st[2]>>32));h[6]=bswap32((uint)(st[3]&0xffffffffUL));h[7]=bswap32((uint)(st[3]>>32));if(below(h,difficulty)){if(atomic_cmpxchg((volatile __global unsigned int *)&out->found,0U,1U)==0U){out->nonce_lo=lo;out->nonce_hi=hi;for(int i=0;i<8;i++)out->hash[i]=h[i];}}}
"#;
