use std::sync::Arc;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::Duration;
use criterion::{
    criterion_group, criterion_main, BenchmarkId, Criterion, Throughput,
};

use proxybench::server::{start_server, HUNDRED_MEGABYTES};
use tokio::runtime::Runtime;
use anyhow::{Context, Result};
use reqwest::Proxy;
use tokio::task::{JoinSet};
use proxybench::conf::{Configuration, DEFAULT_H2_WINDOW_SIZE, DEFAULT_PROXY};
use proxybench::conf::Protocol::{EncryptedHttp1, EncryptedHttp2, PlaintextHttp1};

async fn make_requests(url: &'static str, conf: &'static Configuration) {
    let todo = Arc::new(AtomicIsize::new(conf.requests as isize));
    let mut set = JoinSet::new();

    let client = {
        let mut builder = reqwest::ClientBuilder::new()
            .no_gzip()
            .no_deflate()
            .no_brotli()
            .no_zstd()
            .http2_initial_connection_window_size(DEFAULT_H2_WINDOW_SIZE)
            .http2_initial_stream_window_size(DEFAULT_H2_WINDOW_SIZE)
            .danger_accept_invalid_certs(true)
            .tls_built_in_root_certs(false)
            .pool_max_idle_per_host(conf.workers);
        builder = match conf.protocol {
            PlaintextHttp1 | EncryptedHttp1 => builder,
            EncryptedHttp2 => builder.http2_prior_knowledge()
        };
        builder = if let Some(proxy) = conf.proxy {
            builder.proxy(Proxy::all(proxy).unwrap())
        } else {
            builder.no_proxy()
        };
        builder.build().unwrap()
    };

    let body: &'static [u8] = if conf.request_megabytes == 0 {
        "".as_bytes()
    } else {
        &HUNDRED_MEGABYTES[..1024*1024*conf.request_megabytes]
    };

    for _ in 0..conf.workers {
        let client = client.clone();
        let todo = todo.clone();

        set.spawn(async move {
            while todo.fetch_sub(1, Ordering::Relaxed) > 0 {
                client.get(url)
                    .header("x-response-megabytes", conf.response_megabytes)
                    .body(body)
                    .send()
                    .await.context("request failed")?
                    .error_for_status().context("error status")?;
            }
            Ok(())
        });
    }

    set.join_all()
        .await
        .into_iter().collect::<Result<Vec<_>>>()
        .expect("client failed");
}

pub fn bench(c: &mut Criterion) {
    env_logger::init();
    let rt = Runtime::new().unwrap();

    let mut confs = vec![
        Configuration::builder().protocol(PlaintextHttp1).build(),
        Configuration::builder().protocol(EncryptedHttp1).build(),
        Configuration::builder().protocol(EncryptedHttp1).proxy(DEFAULT_PROXY).build(),
        Configuration::builder().protocol(EncryptedHttp2).build(),
        Configuration::builder().protocol(EncryptedHttp2).proxy(DEFAULT_PROXY).build(),
    ];
    for proto in &[EncryptedHttp1, EncryptedHttp2] {
        for proxy in &[None, Some(DEFAULT_PROXY)] {
            confs.push(
                Configuration::builder()
                    .maybe_proxy(*proxy)
                    .protocol(*proto)
                    .requests(1)
                    .request_megabytes(100)
                    .response_megabytes(100).build()
            );
        }
    }

    for conf in confs {
        let mut g = c.benchmark_group("proxy");
        g.sample_size(10);
        g.warm_up_time(Duration::from_millis(10));
        g.measurement_time(Duration::from_millis(100));
        g.noise_threshold(0.5);
        g.throughput(Throughput::Elements(conf.requests));

        let conf: &'static Configuration = Box::leak(Box::new(conf));

        g.bench_with_input(
            BenchmarkId::new("bench", format!("{:?}", conf)),
            &conf,
            |b, conf| {
                let (server_addr, handle) = rt.block_on(start_server(conf.protocol)).unwrap();

                let url: &'static str = {
                    let scheme = match conf.protocol {
                        PlaintextHttp1 => "http://",
                        EncryptedHttp1 | EncryptedHttp2 => "https://"
                    };
                    let url = format!("{scheme}{server_addr}/?response_megabytes={}", conf.response_megabytes);
                    Box::leak(Box::new(url))
                };

                b.to_async(&rt).iter(|| make_requests(url, conf));

                handle.abort();
            }
        );
    }
}

criterion_group!(benches, bench);
criterion_main!(benches);
