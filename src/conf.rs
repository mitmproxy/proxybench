use std::fmt::{Debug, Formatter};

pub const DEFAULT_PROXY: &str = "http://127.0.0.1:8080";
pub const DEFAULT_H2_WINDOW_SIZE: Option<u32> = Some((1 << 31) - 1);

#[derive(Clone, bon::Builder)]
pub struct Configuration {
    pub protocol: Protocol,
    #[builder(default = 0)]
    pub request_megabytes: usize,
    #[builder(default = 0)]
    pub response_megabytes: usize,
    #[builder(default = 1)]
    pub workers: usize,
    #[builder(default = 50)]
    pub requests: u64,
    pub proxy: Option<&'static str>,
}

impl Debug for Configuration {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:?} req={}mb resp={}mb, w={} reqs={} proxy={}",
            self.protocol,
            self.request_megabytes,
            self.response_megabytes,
            self.workers,
            self.requests,
            self.proxy
                .map(|x| x.rsplit_once(":").map(|(_, port)| port).unwrap_or(x))
                .unwrap_or("/")
        )
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Protocol {
    PlaintextHttp1,
    EncryptedHttp1,
    EncryptedHttp2,
}
