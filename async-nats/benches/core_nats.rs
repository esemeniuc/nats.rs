use std::time::Duration;

use bytes::Bytes;
use criterion::{criterion_group, criterion_main, Criterion};
use futures::stream::StreamExt;

static MSG: &[u8] = &[22; 32768];

pub fn publish(c: &mut Criterion) {
    let messages_amount = 500_000;
    let server = nats_server::run_basic_server();
    let mut throughput_group = c.benchmark_group("async-nats: publish throughput");
    throughput_group.sample_size(10);
    throughput_group.warm_up_time(std::time::Duration::from_secs(1));

    for &size in [32, 1024, 8192].iter() {
        throughput_group.throughput(criterion::Throughput::Bytes(size as u64 * messages_amount));
        throughput_group.bench_with_input(
            criterion::BenchmarkId::from_parameter(size),
            &size,
            |b, _| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let nc =
                    rt.block_on(async { async_nats::connect(server.client_url()).await.unwrap() });

                b.to_async(rt).iter(move || {
                    let nc = nc.clone();
                    async move {
                        publish_messages(nc, Bytes::from_static(&MSG[..size]), messages_amount)
                            .await
                    }
                });
            },
        );
    }
    throughput_group.finish();

    let mut messages_group = c.benchmark_group("async-nats: publish messages amount");
    messages_group.sample_size(10);
    messages_group.warm_up_time(std::time::Duration::from_secs(1));

    for &size in [32, 1024, 8192].iter() {
        messages_group.throughput(criterion::Throughput::Elements(messages_amount));
        messages_group.bench_with_input(
            criterion::BenchmarkId::from_parameter(size),
            &size,
            |b, _| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let nc = rt.block_on(async {
                    let nc = async_nats::connect(server.client_url()).await.unwrap();
                    nc.publish("data".to_string(), "data".into()).await.unwrap();
                    nc.flush().await.unwrap();
                    nc
                });

                b.to_async(rt).iter(move || {
                    let nc = nc.clone();
                    async move {
                        publish_messages(nc, Bytes::from_static(&MSG[..size]), messages_amount)
                            .await
                    }
                });
            },
        );
    }
    messages_group.finish();
}

pub fn subscribe(c: &mut Criterion) {
    let server = nats_server::run_basic_server();
    let messages_per_subscribe = 500_000;

    let mut subscribe_amount_group = c.benchmark_group("subscribe amount");
    subscribe_amount_group.sample_size(10);

    for &size in [32, 1024, 8192].iter() {
        let url = server.client_url();
        subscribe_amount_group.throughput(criterion::Throughput::Elements(messages_per_subscribe));
        subscribe_amount_group.bench_with_input(
            criterion::BenchmarkId::from_parameter(size),
            &size,
            move |b, _| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let url = url.clone();
                let (nc, handle) = rt.block_on(async move {
                    let nc = async_nats::ConnectOptions::new()
                        .connect(url.clone())
                        .await
                        .unwrap();
                    let (started, ready) = tokio::sync::oneshot::channel();
                    let handle = tokio::task::spawn({
                        async move {
                            let client = async_nats::ConnectOptions::new()
                                .connect(url)
                                .await
                                .unwrap();

                            started.send(()).unwrap();
                            loop {
                                client
                                    .publish("bench".to_string(), Bytes::from_static(&MSG[..size]))
                                    .await
                                    .ok();
                            }
                        }
                    });
                    nc.publish("data".to_string(), "data".into()).await.unwrap();
                    nc.flush().await.unwrap();
                    ready.await.unwrap();
                    (nc, handle)
                });

                b.to_async(rt).iter(move || {
                    let nc = nc.clone();
                    async move { subscribe_messages(nc, messages_per_subscribe).await }
                });
                handle.abort();
            },
        );
    }
    subscribe_amount_group.finish();
}

pub fn request(c: &mut Criterion) {
    let server = nats_server::run_basic_server();
    let messages_per_run = 10_000;

    let mut subscribe_amount_group = c.benchmark_group("request amount");
    subscribe_amount_group.sample_size(10);

    for &size in [32, 1024, 8192].iter() {
        let url = server.client_url();
        subscribe_amount_group.throughput(criterion::Throughput::Elements(messages_per_run));
        subscribe_amount_group.bench_with_input(
            criterion::BenchmarkId::from_parameter(size),
            &size,
            move |b, _| {
                let rt = tokio::runtime::Runtime::new().unwrap();
                let url = url.clone();
                let (nc, handle) = rt.block_on(async move {
                    let nc = async_nats::ConnectOptions::new()
                        .connect(url.clone())
                        .await
                        .unwrap();
                    let (started, ready) = tokio::sync::oneshot::channel();
                    let handle = tokio::task::spawn({
                        async move {
                            let client = async_nats::ConnectOptions::new()
                                .connect(url)
                                .await
                                .unwrap();

                            let mut subscription = client.subscribe("bench".into()).await.unwrap();
                            tokio::time::sleep(Duration::from_secs(3)).await;
                            started.send(()).unwrap();

                            while let Some(request) = subscription.next().await {
                                client
                                    .publish(request.reply.unwrap(), "".into())
                                    .await
                                    .unwrap();
                                client.flush().await.unwrap();
                            }
                        }
                    });
                    nc.flush().await.unwrap();
                    ready.await.unwrap();
                    (nc, handle)
                });
                b.to_async(rt).iter(move || {
                    let nc = nc.clone();
                    async move { requests(nc, Bytes::from_static(&MSG[..size]), messages_per_run).await }
                });
                handle.abort();
            },
        );
    }
    subscribe_amount_group.finish();
}

async fn requests(nc: async_nats::Client, msg: Bytes, amount: u64) {
    for _i in 0..amount {
        nc.request("bench".into(), msg.clone()).await.unwrap();
    }
    nc.flush().await.unwrap();
}

async fn publish_messages(nc: async_nats::Client, msg: Bytes, amount: u64) {
    for _i in 0..amount {
        nc.publish("bench".into(), msg.clone()).await.unwrap();
    }
    nc.flush().await.unwrap();
}

async fn subscribe_messages(nc: async_nats::Client, amount: u64) {
    let mut sub = nc.subscribe("bench".into()).await.unwrap();
    for _ in 0..amount {
        sub.next().await.unwrap();
    }
}

criterion_group!(benches, publish, subscribe);
criterion_main!(benches);
