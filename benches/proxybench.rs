use criterion::measurement::WallTime;
use criterion::{
    criterion_group, criterion_main, BenchmarkGroup, BenchmarkId, Criterion, Throughput,
};
use futures::TryStreamExt;
use hyper::client::HttpConnector;
use hyper::http::HeaderValue;
use hyper::server::accept;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Request, Response, Server};
use hyper::{Client, Uri};
use hyper_tls::HttpsConnector;
use native_tls::TlsConnector;
use rcgen::generate_simple_self_signed;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::runtime::Runtime;
use tokio::stream::StreamExt;
use tokio::sync::mpsc::Sender;
use tokio::sync::{mpsc, Semaphore};
use tokio_rustls::TlsAcceptor;

async fn hello_world(_req: Request<Body>) -> Result<Response<Body>, Infallible> {
    let mut resp: Response<Body> = Response::new("Hello, World".into());
    resp.headers_mut()
        .append("Connection", HeaderValue::from_static("close"));
    Ok(resp)
}

async fn make_requests(uri: &Uri, requests: usize, concurrent: i32) {
    let todo = Semaphore::new(requests);
    let (notify_done, mut done) = mpsc::channel::<()>(1);

    let td = Arc::new(todo);

    for _ in 0..concurrent {
        tokio::spawn(request_thread(uri.clone(), td.clone(), notify_done.clone()));
    }
    done.recv().await;
}

async fn request_thread(uri: Uri, todo: Arc<Semaphore>, mut notify_done: Sender<()>) {
    // cannot use rustls here: https://github.com/ctz/hyper-rustls/issues/56
    let mut http = HttpConnector::new();
    http.enforce_http(false);
    let tls = TlsConnector::builder()
        .danger_accept_invalid_certs(true)
        .danger_accept_invalid_hostnames(true)
        .build()
        .unwrap();
    let https = HttpsConnector::from((http, tls.into()));
    let client = Client::builder().build::<_, hyper::Body>(https);

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

async fn run_http_server() {
    let http_addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    let make_http_svc =
        make_service_fn(|_conn| async { Ok::<_, Infallible>(service_fn(hello_world)) });
    Server::bind(&http_addr).serve(make_http_svc).await.unwrap();
}

async fn run_https_server() {
    let https_addr = SocketAddr::from(([127, 0, 0, 1], 3001));
    let make_https_svc =
        make_service_fn(|_conn| async { Ok::<_, Infallible>(service_fn(hello_world)) });

    let tcp = TcpListener::bind(https_addr).await.unwrap();

    let cert = generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
    let mut tls_conf = rustls::ServerConfig::new(rustls::NoClientAuth::new());
    tls_conf
        .set_single_cert(
            vec![rustls::Certificate(cert.serialize_der().unwrap())],
            rustls::PrivateKey(cert.serialize_private_key_der()),
        )
        .unwrap();
    tls_conf.set_protocols(&[b"h2".to_vec(), b"http/1.1".to_vec()]);
    let tls_acceptor = TlsAcceptor::from(sync::Arc::new(tls_conf));
    let incoming_tls_stream = tcp
        .and_then(move |s| tls_acceptor.accept(s))
        .filter(|x| match x {
            Ok(_) => true,
            Err(e) => {
                print!("tls error: {}", e);
                false
            }
        });
    Server::builder(accept::from_stream(incoming_tls_stream))
        .serve(make_https_svc)
        .await
        .unwrap();
}

pub fn proxy_benchmark(rt: &mut Runtime, mut group: BenchmarkGroup<WallTime>, uri: Uri) {
    for requests in [100_usize].iter() {
        for concurrent in [1, 20].iter() {
            group.sample_size(10);
            group.throughput(Throughput::Elements(*requests as u64));
            group.bench_with_input(
                BenchmarkId::new(requests.to_string(), concurrent),
                &(&uri, requests, concurrent),
                |b, &(uri, requests, concurrent)| {
                    b.iter(|| {
                        rt.block_on(make_requests(uri, *requests, *concurrent));
                    });
                },
            );
        }
    }
    group.finish();
}

pub fn bench(c: &mut Criterion) {
    let mut rt = Runtime::new().unwrap();

    rt.spawn(run_http_server());
    rt.spawn(run_https_server());

    proxy_benchmark(
        &mut rt,
        c.benchmark_group("http"),
        Uri::from_static("http://127.0.0.1:8080/"),
    );
    proxy_benchmark(
        &mut rt,
        c.benchmark_group("https"),
        Uri::from_static("https://127.0.0.1:8080/"),
    );
    proxy_benchmark(
        &mut rt,
        c.benchmark_group("proxybench-http"),
        Uri::from_static("http://127.0.0.1:3000/"),
    );
    proxy_benchmark(
        &mut rt,
        c.benchmark_group("proxybench-https"),
        Uri::from_static("https://127.0.0.1:3001/"),
    );
}

criterion_group!(benches, bench);
criterion_main!(benches);
