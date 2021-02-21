use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync;
use std::sync::Arc;

use async_stream;
use async_stream::try_stream;
use criterion::measurement::WallTime;
use criterion::{
    criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use futures::TryStreamExt;
use futures_util::stream::StreamExt;
use hyper::client::HttpConnector;
use hyper::http::HeaderValue;
use hyper::server::accept;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use hyper::{Client, Uri};
use hyper_tls::HttpsConnector;
use native_tls::TlsConnector;
use rcgen::generate_simple_self_signed;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, Semaphore};
use tokio_rustls::TlsAcceptor;

async fn hello_world(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
    let mut resp: Response<Body> = Response::new("Hello, World".into());
    resp.headers_mut()
        .append("Connection", HeaderValue::from_static("close"));
    Ok(resp)
}

async fn make_requests(uri: &Uri, requests: usize, concurrent: i32, http2: bool) {
    let todo = Semaphore::new(requests);
    let (notify_done, mut done) = mpsc::channel::<()>(1);

    let td = Arc::new(todo);

    for _ in 0..concurrent {
        tokio::spawn(request_thread(
            uri.clone(),
            http2,
            td.clone(),
            notify_done.clone(),
        ));
    }
    done.recv().await;
}

async fn request_thread(uri: Uri, http2: bool, todo: Arc<Semaphore>, notify_done: Sender<()>) {
    // cannot use rustls here: https://github.com/ctz/hyper-rustls/issues/56
    let mut http = HttpConnector::new();
    http.enforce_http(false);

    let alpn = if http2 { "h2" } else { "http/1.1" };
    let tls = TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .request_alpns(&[alpn])
        .build()
        .unwrap();
    let https = HttpsConnector::from((http, tls.into()));
    let mut client_builder = Client::builder();
    if http2 {
        client_builder.http2_only(true);
    }
    let client = client_builder.build::<_, hyper::Body>(https);

    loop {
        match todo.try_acquire() {
            Ok(permit) => {
                match client.get(uri.clone()).await {
                    Ok(resp) => {
                        hyper::body::to_bytes(resp.into_body()).await.unwrap();
                        permit.forget();
                    }
                    Err(e) => (println!("erroring! {}", e)),
                };
            }
            Err(_) => {
                notify_done.try_send(()).ok();
                return;
            }
        }
    }
}

async fn run_http_server(port: u16) {
    let http_addr = SocketAddr::from(([127, 0, 0, 1], port));
    let make_http_svc =
        make_service_fn(|_conn| async { Ok::<_, Infallible>(service_fn(hello_world)) });
    Server::bind(&http_addr).serve(make_http_svc).await.unwrap();
}

async fn run_https_server(port: u16, protocol: &str) {
    let https_addr = SocketAddr::from(([127, 0, 0, 1], port));
    let make_https_svc =
        make_service_fn(|_conn| async { Ok::<_, Infallible>(service_fn(hello_world)) });

    let listener = TcpListener::bind(https_addr).await.unwrap();

    let cert = generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let mut tls_conf = rustls::ServerConfig::new(rustls::NoClientAuth::new());
    tls_conf
        .set_single_cert(
            vec![rustls::Certificate(cert.serialize_der().unwrap())],
            rustls::PrivateKey(cert.serialize_private_key_der()),
        )
        .unwrap();
    tls_conf.set_protocols(&[protocol.as_bytes().to_vec()]);
    let tls_acceptor = TlsAcceptor::from(sync::Arc::new(tls_conf));

    let tls_stream = try_stream! {
        loop {
            let (stream, _addr) = listener.accept().await?;
            yield stream;
        }
    }
    .and_then(move |s| tls_acceptor.accept(s))
    .filter(|x| match x {
        Ok(_) => futures::future::ready(true),
        Err(e) => {
            print!("tls error: {}", e);
            futures::future::ready(false)
        }
    });
    Server::builder(accept::from_stream(tls_stream))
        .serve(make_https_svc)
        .await
        .unwrap();
}

pub fn proxy_benchmark(
    rt: &mut Runtime,
    mut group: BenchmarkGroup<WallTime>,
    uri: Uri,
    http2: bool,
) {
    for requests in [100_usize].iter() {
        for concurrent in [1, 20, 99].iter() {
            group.sample_size(10);
            group.throughput(Throughput::Elements(*requests as u64));
            group.bench_with_input(
                BenchmarkId::new(requests.to_string(), concurrent),
                &(&uri, requests, concurrent),
                |b, &(uri, requests, concurrent)| {
                    b.iter(|| {
                        rt.block_on(make_requests(uri, *requests, *concurrent, http2));
                    });
                },
            );
        }
    }
    group.finish();
}

pub fn bench(c: &mut Criterion) {
    let mut rt = Runtime::new().unwrap();

    rt.spawn(run_http_server(3000));
    rt.spawn(run_https_server(3001, "h2"));
    rt.spawn(run_https_server(3002, "http/1.1"));

    proxy_benchmark(
        &mut rt,
        c.benchmark_group("proxy-plain"),
        Uri::from_static("http://127.0.0.1:8000/"),
        false,
    );
    proxy_benchmark(
        &mut rt,
        c.benchmark_group("proxy-https-h1"),
        Uri::from_static("https://127.0.0.1:8001/"),
        false,
    );
    proxy_benchmark(
        &mut rt,
        c.benchmark_group("proxy-https-h2"),
        Uri::from_static("https://127.0.0.1:8002/"),
        true,
    );
    proxy_benchmark(
        &mut rt,
        c.benchmark_group("self-plain"),
        Uri::from_static("http://127.0.0.1:3000/"),
        false,
    );
    proxy_benchmark(
        &mut rt,
        c.benchmark_group("self-https-h1"),
        Uri::from_static("https://127.0.0.1:3001/"),
        false,
    );
    proxy_benchmark(
        &mut rt,
        c.benchmark_group("self-https-h2"),
        Uri::from_static("https://127.0.0.1:3002/"),
        true,
    );
}

criterion_group!(benches, bench);
criterion_main!(benches);
