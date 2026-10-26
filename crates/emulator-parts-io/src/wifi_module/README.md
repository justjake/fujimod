# MT5931 SDIO endpoint

`Protocol::Mt5931` extends the bounded ROM-download transport with the MT5931
HIF command ABI. The board supplies a permanent unicast MAC address; the part
has no RF peers. The model consumes SDIO register and FIFO transactions only.
Operations are planned without effects and committed by the host at its
virtual transfer deadline. The endpoint has no program inspection interface.

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

Implemented effects include ordered RX1 packets, TC4 TX release accounting,
interrupt status/masks, direct interrupt control and the configured MAC query.
Queue capacity is bounded and overflow refuses. Host inspection and binary
checkpoint serialization do not consume data. Host PIO and SDMA share the same
packet and commit its dequeue only when the transfer completes. The retained
host convention drains a function block with zero empty-latch words after the
wire bytes; that convention and transfer latency remain declared model inputs.

NIC capability command `0x80` refuses before any command, credit or RX effect.
The reference header describes a 12-byte body, or 16 bytes when both optional
RF/PHY and calibration fields are compiled in. An extended 20-byte capability
body is not defined there. Its layout and values require protocol evidence;
zero-filling extra capability words is not supported. Ownership handoff/wakeup timing also refuses. Other command IDs, network data,
scanning and association remain unmodeled and refuse explicitly.
