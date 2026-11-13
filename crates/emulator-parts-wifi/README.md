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

An `access_point` option configures one network: SSID, BSSID, channel, RCPI,
gateway, two DNS servers, a DNS name, and a TCP destination plus host loopback
port. An optional `wpa2_passphrase` (8 to 63 bytes) makes it a WPA2-PSK
network; without it the network is open. Without an access point, commands that
need a peer are accepted only where the stock driver has no peer exchange; scan
and join produce no RF traffic.

## Initialization, scan and join

The endpoint accepts the boot sequence above the upload: F2, C1 query and set,
0x80 capability query, 0x31 and 0x36 configuration, F1 boot parameters, and the
later 0x06, 0x13 and request-flag reads. Every accepted command returns its
traffic-class credit and, where stock waits, its command-result event. The
stock request-flag window at `0x10` is 0x58 bytes: WHISR, the two TX status
words, per-FIFO receive counts at 0x0c/0x0e, 16 RX0 lengths at 0x10 and 16 RX1
lengths at 0x30. WHISR bits 0-2 are read-clear in the default register mode,
and the same window read drains WTSR0/WTSR1; stock adds those credits to its
own traffic-class balance. Command 0x20 answers event 0x18 with its flag byte
cleared, which stock consumes as the channel-privilege grant.

Scan requests (CID 0x1E and 0x04) produce a beacon and probe response for the
configured SSID and channel, followed by event 0x0F with a 0x60-byte body and
event 0x15, whose body starts with the scan request's sequence byte, then the
sparse-valid flag, band and channel. Management authentication and association
requests for the configured BSSID receive responses; a secured access point
advertises RSN and, after association, starts the authenticator's 4-way
handshake over the EAPOL frames the chip transports. Data frames
use the 16-byte HIF TX header and may arrive through WTDR0 or WTDR1; commands
and management frames use WTDR1. Received frames carry the 12-byte receive
header; events remain the eight-byte WIFI_EVENT_T packets required by the
stock reply dispatcher, padded with the same four-byte trailing convention as
frames. Stock reads a whole receive batch listed by the request-flag window in
one WRDR read, so the endpoint drains every packet whose padded length fits the
requested extent.

NIC capability command `0x80` refuses before any command, credit or RX effect
unless the board supplies an explicit 20-byte capability body. The reference
header describes a 12-byte body, or 16 bytes when both optional RF/PHY and
calibration fields are compiled in. An extended 20-byte body is not defined
there; the fixture exists so stock initialization can be exercised without
claiming recovered capability values. Ownership handoff/wakeup timing is
stored as WHLPCR bits 8 and 9 without a timing model.

## WPA2-PSK

With a configured passphrase the beacons and probe responses carry an RSN
element selecting CCMP with PSK authentication, and the association response
adds the same element before the endpoint sends EAPOL-Key message 1/4. The
endpoint is the authenticator: it derives the PMK from the passphrase and SSID
with PBKDF2-HMAC-SHA1 (4096 iterations), checks the station's message 2/4 MIC
against the PTK from the IEEE 802.11i PRF, and answers with message 3/4 whose
key data holds the RSN element and a group-key KDE wrapped with AES key wrap
(RFC 3394). A message 4/4 with the right MIC and replay counter completes the
handshake. A wrong passphrase fails the MIC check and the endpoint
deauthenticates the station. Station data frames are dropped until the
handshake completes, and the chip model still performs CCMP on the wire, so the
endpoint neither encrypts nor decrypts data frames. Nonces and the GTK are
fixed per exchange so a checkpoint resumes the same handshake.

## Virtual network

The data path is bridged to a deterministic IPv4 network in
`virtual_network.rs`: a DHCP server, a gateway, two DNS servers, an optional
NTP server, and a TCP relay to one host loopback port. Packet headers and
checksums use [smoltcp](https://crates.io/crates/smoltcp) 0.12's wire layer
(0BSD). The server state is our own and serializes with the device; host TCP
streams are host resources and are not checkpointed, so a restored connection
is reset.

The DHCP lease is the gateway subnet's `.100` host (skipping configured
addresses), with the gateway as router and the two configured addresses as DNS.
DNS answers the configured name with the configured host address, resolves an
optional NTP name to its server, and returns NXDOMAIN for other names. A
configured NTP server answers client-mode requests on port 123 with a
stratum-2 server reply whose time is the configured UTC instant plus the
network's elapsed milliseconds. ARP answers every configured local address.
TCP connects to `127.0.0.1:<local_port>` when the station sends a SYN to
`<host>:<host_port>`; a connection is polled from RX FIFO reads and from the
endpoint's data path. Unsupported command IDs and malformed register/FIFO
extents refuse before effects.
