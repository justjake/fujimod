//! Driven by web/throughput.browser.ts. No USB hardware is opened.
use fujimod_usb_proxy::{Client, Operation};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires the isolated exporter started by web/throughput.browser.ts"]
fn throughput() {
    futures_lite::future::block_on(async {
        let link = std::env::var("FUJIMOD_USB_PROXY_TEST_LINK").expect("benchmark share link");
        let minimum: f64 = std::env::var("USB_PROXY_MIN_MBPS")
            .unwrap_or_else(|_| "20".into())
            .parse()
            .unwrap();
        let iterations: usize = std::env::var("USB_PROXY_BENCH_ITERATIONS")
            .unwrap_or_else(|_| "8".into())
            .parse()
            .unwrap();
        assert!(iterations > 0);
        assert!(minimum.is_finite() && minimum >= 0.0);
        let mut client = Client::connect(&link, "").await.unwrap();
        let devices = client.devices().await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].serial.as_deref(), Some("throughput-fixture"));
        let device = &devices[0].id;
        let timeout = Duration::from_secs(120);
        let opened = client
            .request(
                device,
                Operation::Open {
                    configuration: 1,
                    interface: 0,
                    alternate: 0,
                },
                timeout,
            )
            .await
            .unwrap();
        assert!(opened.error.is_none(), "{opened:?}");
        let mut rates = Vec::new();
        for length in [1024 * 1024, 8 * 1024 * 1024] {
            let data: Vec<u8> = (0..length).map(|i| i as u8).collect();
            for direction in ["upload", "download"] {
                // A full-sized warmup also grows SCTP's congestion window.
                let mut elapsed = Duration::ZERO;
                let mut samples = Vec::new();
                for index in 0..=iterations {
                    let operation = if direction == "upload" {
                        Operation::TransferOut {
                            endpoint: 1,
                            data: data.clone(),
                        }
                    } else {
                        Operation::TransferIn {
                            endpoint: 0x82,
                            length,
                        }
                    };
                    let started = Instant::now();
                    let completion = client.request(device, operation, timeout).await.unwrap();
                    let duration = started.elapsed();
                    assert!(completion.error.is_none(), "{completion:?}");
                    assert_eq!(completion.transferred, length);
                    if direction == "download" {
                        assert_eq!(completion.data, data);
                    }
                    if index > 0 {
                        elapsed += duration;
                        samples.push(duration.as_secs_f64());
                    }
                }
                let mbps = (length * iterations) as f64 / elapsed.as_secs_f64() / 1_000_000.0;
                println!(
                    "THROUGHPUT {}",
                    serde_json::json!({
                        "direction": direction, "bytes_per_transfer": length,
                        "iterations": iterations, "seconds": elapsed.as_secs_f64(), "MBps": mbps,
                        "samples_seconds": samples,
                    })
                );
                rates.push((direction, length, mbps));
            }
        }
        let closed = client
            .request(device, Operation::Close, timeout)
            .await
            .unwrap();
        assert!(closed.error.is_none(), "{closed:?}");
        for (direction, length, mbps) in rates {
            assert!(
                mbps >= minimum,
                "{direction} ({length} bytes): {mbps:.2} MB/s < {minimum} MB/s"
            );
        }
    });
}
