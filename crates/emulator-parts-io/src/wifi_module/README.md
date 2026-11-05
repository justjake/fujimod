# MT5931 SDIO endpoint

`Protocol::Mt5931` extends the bounded ROM-download transport with the MT5931
HIF command ABI and one optional virtual access point. The board supplies a
permanent unicast MAC address. The model consumes SDIO register and FIFO
transactions only. Operations are planned without effects and committed by the
host at its virtual transfer deadline. The endpoint has no program inspection
interface.

Protocol references are the MediaTek driver published in
[Galland/MTK5931 at 462989f3998e9818f5be76d5e43bac08720098c7](https://github.com/Galland/MTK5931/tree/462989f3998e9818f5be76d5e43bac08720098c7/wifi/mtk_5931):

- [`include/nic/mt5931_reg.h`](https://github.com/Galland/MTK5931/blob/462989f3998e9818f5be76d5e43bac08720098c7/wifi/mtk_5931/include/nic/mt5931_reg.h)
  defines WCIR, WHLPCR, WHCR, WHISR, WHIER, WTSR0/1, WTDR1 (`0x2c`),
  WRDR1 (`0x34`), WRPLR (`0x50`) and the ROM-download registers.
- [`include/nic_cmd_event.h`](https://github.com/Galland/MTK5931/blob/462989f3998e9818f5be76d5e43bac08720098c7/wifi/mtk_5931/include/nic_cmd_event.h)
  defines the eight-byte command/event headers, F2 oscillator configuration,
  C1 basic configuration and event 9. Reserved and pooled padding bytes are
  opaque. A basic-configuration query returns the declared MAC and the default
  native-frame/checksum configuration.
- [`nic/nic_tx.c`](https://github.com/Galland/MTK5931/blob/462989f3998e9818f5be76d5e43bac08720098c7/wifi/mtk_5931/nic/nic_tx.c)
  and `include/nic/hal.h` define command traffic class 4, read-to-clear TX
  release counts, RX length halves and interrupt read-clear/W1C selection.
- [`common/wlan_lib.c`](https://github.com/Galland/MTK5931/blob/462989f3998e9818f5be76d5e43bac08720098c7/wifi/mtk_5931/common/wlan_lib.c)
  implements `wlanSetMcuOscStableTime` without a reply and
  `wlanQueryPermanentAddress` through RX1 with event 9.

This is an original Rust protocol implementation; driver source is not
incorporated. The cited distribution retains its MediaTek attribution and
license notices.

Implemented effects include ordered RX1 packets, TX release accounting for the
traffic class named by each packet, interrupt status/masks, direct interrupt
control, the configured MAC query and boot configuration commands. Queue
capacity is bounded and overflow refuses. Host inspection and binary
checkpoint serialization do not consume data. Host PIO and SDMA share the same
packet and commit its dequeue only when the transfer completes. The retained
host convention drains a function block with zero empty-latch words after the
wire bytes; that convention and transfer latency remain declared model inputs.

An `access_point` option configures one open network: SSID, BSSID, channel,
RCPI, gateway, two DNS servers, a DNS name, and a TCP destination plus host
loopback port. Without it, commands that need a peer are accepted only where
the stock driver has no peer exchange; scan and join produce no RF traffic.

## Initialization, scan and join

The endpoint accepts the boot sequence above the upload: F2, C1 query and set,
0x80 capability query, 0x31 and 0x36 configuration, F1 boot parameters, and the
later 0x06, 0x13 and request-flag reads. Every accepted command returns its
traffic-class credit and, where stock waits, its command-result event. The
stock request-flag window at `0x10` is 0x58 bytes: WHISR, the two TX status
words, then reserved receive and mailbox space.

Scan requests (CID 0x1E and 0x04) produce a beacon and probe response for the
configured SSID and channel, followed by event 0x0F with a 0x60-byte body and
event 0x15 with the selected channel. Management authentication and association
requests for the configured BSSID receive open-network responses. Data frames
use the 16-byte HIF TX header and the 12-byte receive header; events remain the
eight-byte WIFI_EVENT_T packets required by the stock reply dispatcher.

NIC capability command `0x80` refuses before any command, credit or RX effect
unless the board supplies an explicit 20-byte capability body. The reference
header describes a 12-byte body, or 16 bytes when both optional RF/PHY and
calibration fields are compiled in. An extended 20-byte body is not defined
there; the fixture exists so stock initialization can be exercised without
claiming recovered capability values. Ownership handoff/wakeup timing is
stored as WHLPCR bits 8 and 9 without a timing model.

## Virtual network

The data path is bridged to a deterministic IPv4 network in
`virtual_network.rs`: a DHCP server, a gateway, two DNS servers, and a TCP
relay to one host loopback port. Packet headers and checksums use
[smoltcp](https://crates.io/crates/smoltcp) 0.12's wire layer (0BSD). The server
state is our own and serializes with the device; host TCP streams are host
resources and are not checkpointed, so a restored connection is reset.

The DHCP lease is the gateway subnet's `.100` host (skipping configured
addresses), with the gateway as router and the two configured addresses as DNS.
DNS answers the configured name with the configured host address and returns
NXDOMAIN for other names. ARP answers every configured local address. TCP
connects to `127.0.0.1:<local_port>` when the station sends a SYN to
`<host>:<host_port>`; a connection is polled from RX FIFO reads and from the
endpoint's data path. WPA2/EAPOL remains unmodeled. Unsupported command IDs and
malformed register/FIFO extents refuse before effects.
