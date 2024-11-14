use crate::conf::{Protocol, DEFAULT_H2_WINDOW_SIZE};
use anyhow::{ensure, Error, Result};
use http_body_util::BodyExt;
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::server::conn::{http1, http2};
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::{TokioExecutor, TokioIo, TokioTimer};
use rcgen::generate_simple_self_signed;
use rustls::pki_types::PrivatePkcs8KeyDer;
use std::io;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpListener;
use tokio::select;
use tokio::task::{JoinHandle, JoinSet};
use tokio_rustls::TlsAcceptor;

pub const HUNDRED_MEGABYTES: &[u8] = &[0u8; 1024 * 1024 * 100];
const OK: &[u8] = "OK".as_bytes();

pub async fn start_server(configuration: Protocol) -> Result<(SocketAddr, JoinHandle<()>)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let acceptor = match configuration {
        Protocol::PlaintextHttp1 => None,
        Protocol::EncryptedHttp1 => Some(tls_acceptor("http/1.1")),
        Protocol::EncryptedHttp2 => Some(tls_acceptor("h2")),
    };
    let addr = listener.local_addr()?;

    let fut = async move {
        let mut handlers: JoinSet<Result<()>> = JoinSet::new();
        loop {
            select! {
                res = listener.accept() => {
                    let (stream, _) = res?;
                    let acceptor = acceptor.clone();
                    handlers.spawn(async move {
                        if let Some(acceptor) = acceptor {
                            let stream = acceptor.accept(stream).await?;
                            serve(stream, configuration).await?;
                        } else {
                            serve(stream, configuration).await?;
                        }
                        Ok(()) as Result<()>
                    });
                }
                Some(res) = handlers.join_next() => {
                    match res? {
                        Ok(_) => (),
                        Err(e) => {
                            if e.source().is_some_and(
                                |e| e.downcast_ref::<io::Error>().is_some_and(
                                    |e| e.kind() == ErrorKind::UnexpectedEof)) {
                                continue;
                            }
                            return Err::<(), Error>(e);
                        }
                    }
                }
            }
        }
    };

    let handle = tokio::spawn(async move {
        if let Err(e) = fut.await {
            eprintln!("Server failed: {:?}", e);
            panic!("Server failed: {:?}", e);
        }
    });

    Ok((addr, handle))
}

fn tls_acceptor(alpn: &str) -> TlsAcceptor {
    let cert = generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let key = PrivatePkcs8KeyDer::from(cert.key_pair.serialize_der()).into();
    let mut config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.cert.der().clone()], key)
        .unwrap();
    config.alpn_protocols = vec![alpn.as_bytes().to_vec()];
    TlsAcceptor::from(Arc::new(config))
}

pub async fn serve<I: AsyncRead + AsyncWrite + Unpin>(
    stream: I,
    configuration: Protocol,
) -> hyper::Result<()> {
    let stream = TokioIo::new(stream);
    match configuration {
        Protocol::PlaintextHttp1 | Protocol::EncryptedHttp1 => {
            http1::Builder::new()
                .timer(TokioTimer::new())
                .serve_connection(stream, service_fn(request_handler))
                .await
        }
        Protocol::EncryptedHttp2 => {
            http2::Builder::new(TokioExecutor::new())
                .initial_connection_window_size(DEFAULT_H2_WINDOW_SIZE)
                .initial_stream_window_size(DEFAULT_H2_WINDOW_SIZE)
                .timer(TokioTimer::new())
                .serve_connection(stream, service_fn(request_handler))
                .await
        }
    }
}

async fn request_handler(
    request: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Error> {
    let size = request
        .headers()
        .get("x-response-megabytes")
        .and_then(|s| s.to_str().ok())
        .unwrap_or("0")
        .parse::<usize>()?;
    ensure!(size <= 100, "largest body size is 100mb");
    let body = if size == 0 {
        OK
    } else {
        &HUNDRED_MEGABYTES[..1024 * 1024 * size]
    };

    let _ = request.collect().await?;

    Ok(Response::new(Full::new(Bytes::from(body))))
}
