//! Driven by web/peer.browser.ts against its isolated WebUSB fixture.
use fujimod_usb_proxy::{Client, Operation, Recipient, RequestType, Setup};
use std::time::Duration;

#[test]
#[ignore = "requires the isolated exporter started by web/peer.browser.ts"]
fn binary_native_browser_transfers() {
    futures_lite::future::block_on(async {
        let link =
            std::env::var("FUJIMOD_USB_PROXY_TEST_LINK").expect("isolated fixture share link");
        let mut client = Client::connect(&link, "").await.unwrap();
        let devices = client.devices().await.unwrap();
        assert_eq!(devices.len(), 1);
        let device = &devices[0];
        assert_eq!(device.serial.as_deref(), Some("synthetic"));
        assert_eq!(device.product.as_deref(), Some("Synthetic service device"));
        let timeout = Duration::from_secs(30);
        let completion = client
            .request(
                &device.id,
                Operation::Open {
                    configuration: 1,
                    interface: 2,
                    alternate: 3,
                },
                timeout,
            )
            .await
            .unwrap();
        assert!(completion.error.is_none(), "{:?}", completion.error);
        for length in [1usize, 16000, 16001, 262145, 8 * 1024 * 1024] {
            let data: Vec<u8> = (0..length).map(|i| i as u8).collect();
            for kind in [0xe0u8, 0xe1] {
                let mut envelope = vec![0u8; 12];
                envelope[0] = kind;
                envelope[6..10].copy_from_slice(&(length as u32).to_be_bytes());
                envelope[10..12].copy_from_slice(&0x1234u16.to_be_bytes());
                let completion = client
                    .request(
                        &device.id,
                        Operation::ControlOut {
                            setup: Setup {
                                request_type: RequestType::Vendor,
                                recipient: Recipient::Other,
                                request: 0,
                                value: 0,
                                index: 0,
                            },
                            data: envelope,
                        },
                        timeout,
                    )
                    .await
                    .unwrap();
                assert!(completion.error.is_none(), "{:?}", completion.error);
                assert_eq!(completion.transferred, 12);
                let operation = if kind == 0xe0 {
                    Operation::TransferOut {
                        endpoint: 1,
                        data: data.clone(),
                    }
                } else {
                    Operation::TransferIn {
                        endpoint: 0x82,
                        length: length.div_ceil(64) * 64,
                    }
                };
                let completion = client
                    .request(&device.id, operation, timeout)
                    .await
                    .unwrap();
                assert!(completion.error.is_none(), "{:?}", completion.error);
                assert_eq!(completion.transferred, length);
                if kind == 0xe1 {
                    assert_eq!(completion.data, data);
                }
            }
        }
        assert!(
            client
                .request(&device.id, Operation::Close, timeout)
                .await
                .unwrap()
                .error
                .is_none()
        );
    });
}
