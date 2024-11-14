use anyhow::Result;
use proxybench::conf::Protocol;
use proxybench::server::start_server;
use tokio::signal;

#[tokio::main]
pub async fn main() -> Result<()> {
    for &conf in &[
        Protocol::PlaintextHttp1,
        Protocol::EncryptedHttp1,
        Protocol::EncryptedHttp2,
    ] {
        let (addr, _) = start_server(conf).await.unwrap();
        println!("Listening on {addr}... ({conf:?})");
    }
    signal::ctrl_c().await?;
    Ok(())
}
