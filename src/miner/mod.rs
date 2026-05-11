pub mod cpu;
pub mod opencl;

#[derive(Debug, Clone, Copy)]
pub enum MiningBackend {
    Cpu,
    OpenCl,
}

impl MiningBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cpu => "cpu",
            Self::OpenCl => "opencl",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Solution {
    pub backend: MiningBackend,
    pub nonce: u64,
    pub hash: String,
    pub hashes: u64,
}
