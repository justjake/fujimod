# Service/debug command catalog

The host core speaks the reverse-engineered ff80 protocol. These are command
mechanisms, not a universal list of supported body features. Firmware definitions
supply limits, memory maps, App80 codes and the meaning of unclassified fields.

## USB envelopes

| Envelope | API                         | Completion                                                                         |
| -------- | --------------------------- | ---------------------------------------------------------------------------------- |
| C0       | `Connection::begin_session` | Exact eight-byte reply; session ID and advertised debug packet size stay distinct. |
| C2       | `Session::ping`             | Two opaque reply bytes.                                                            |
| C4       | `Session::close`            | Consumes the session, including on failure; two opaque reply bytes on success.     |
| D0/D1    | Direct command methods      | One request/reply exchange. Common headers are validated before session reuse.     |
| E0       | `write_data`                | One payload transfer; no preparation or finalization is implicit.                  |
| E1       | `read_data`                 | One data request; offset is a protocol quantity, not an implicit CPU address.      |
| E8       | `empty_response`            | Four opaque bytes; no inference about camera execution or readiness.               |

No command retries, reset requests or service-mode transitions are sent as
transport cleanup. A failed, canceled or malformed exchange fences its session.
A validated device error is a completed exchange, independently of application
work the camera may have started.

## Firmware requests

| Command / selector          | Typed API                                    | Effects and interpretation                                                                                                                                                    |
| --------------------------- | -------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `150000`, selectors `1..40` | `device_info(InfoSelector)`                  | Known text selectors; the serial number is explicitly selected.                                                                                                               |
| `150000`, selector `f0001`  | `firmware_download_status`                   | One application-state read: Normal, Writing, Success, Failed(code), or Unknown(text). Writing is not a host wait instruction.                                                 |
| `150000`, selector `f0002`  | `clear_firmware_download_status`             | Explicitly clears firmware download state; separate from identification and status reads.                                                                                     |
| `1a0002`, function 1        | `configuration_geometry`                     | Explicit RAM/storage view. Capacity and page size come from the reply.                                                                                                        |
| `1a0002`, functions 2/3     | `read_configuration` / `write_configuration` | One bounded command at an explicit view/page/offset. A storage read can load firmware buffers; a write's effects depend on firmware and location. Persistence is not implied. |
| `200001`, functions 2/3     | `read_memory` / `write_memory`               | One bounded CPU-address request; explicit firmware limits, no chunking.                                                                                                       |
| `200001`, function 1        | `read_log`                                   | Treat as consuming retrieval: firmware can advance the log cursor and clear overrun state. Not an observational RAM read or a safely repeatable retry.                        |
| `200001`, function 5        | `send_event`                                 | Typed key, mode or command event. This can change camera state.                                                                                                               |
| `200001`, functions 6/7     | `submit_console` / `poll_console`            | Submission and one poll stay separate. Complete, Pending and Unknown progress are explicit; output is preserved as bytes.                                                     |
| `450006`                    | `diagnostic_query`                           | Known BulkSource selector or explicit research tag; raw fields and device status are retained.                                                                                |
| `550001`                    | `prepare_data_read`                          | Select pattern/echo diagnostic data; no E1 request is implicit.                                                                                                               |
| `760001`                    | `prepare_data_write`                         | Explicit buffer size and mode; no E0/E8 is implicit.                                                                                                                          |
| `580000`                    | `prepare_capabilities`                       | Select capability data and return its size. Reading/parsing the table and applying host timeouts are separate.                                                                |
| Unclassified commands       | `research_command`                           | Explicit command identifier, body and reply capacity; validates only the common header. Preserves status and bytes without guessing completion or command-specific semantics. |

All numeric identifiers in this document are hexadecimal. `150000` selectors
`1..40` mean the named power-of-two selectors, not every intervening number.

The research request body starts after the 16-byte common header and must contain
at least 16 bytes. Reserved header fields are generated by the codec. It sends
exactly one command, without padding, bulk data transfer, polling or cleanup.
Use typed APIs when their command semantics are established. An unknown reply's
command-specific fields still need firmware-specific validation; successful USB
completion cannot establish ownership or application completion.

Some statically identified operations remain deliberately unclassified: `1a0000` returns an
empty acknowledgment; `1a0001` returns a status byte whose
meaning needs a firmware definition; `200001` function 4 uses the log handler; `660002` prepares firmware download buffers but its complete
update lifecycle is not implemented. They can be inspected through the explicit
research API. No firmware-update workflow or universal calibration/reset codes
are inferred from those handlers.

## Nested commands and compositions

`fujimod-usb-app80` defines a distinct one-byte command identifier and bounded
32-byte arguments. Its `operations::Mailbox` supplies the ff80 configuration
carrier.
It stages arguments, publishes the command and reads one result, awaiting the
publish write's ff80 completion. Codes, argument/result codecs and effects come
from the selected firmware. The library has no default body or mailbox address.

`operations::memory` owns range chunking; `operations::data` supplies explicit
profile-qualified bulk windows and prepared uploads. These compositions use the
same direct commands and completion rules. Mod installation, allocation,
callback selection, machine-code execution, persistent settings and safe-exit
policies belong above this layer.

The upstream protocol reference is
[fffw's ff80 sources](https://github.com/tiredboffin/fffw/tree/bedc091e0b54a1a34aaf6929dd08e1db36d13b08/ff80).
The retained [MIT notice](LICENSE.fffw) accompanies that reference. Experimental
upstream names, such as `get_timeouts`, do not establish field semantics.

Firmware-qualified App80 catalogs use the shared `fujimod-catalog-service` data schema.
The command codec layer accepts enum inputs and resolves the selected table's
command byte and argument slots. Its mode rules preserve firmware order,
including duplicate identifiers and unreviewed entries. The ff80 `Service`
composition binds a prepared command to that same definition's mailbox.
Callback-kind words remain metadata; transport completion still comes from
awaiting ff80. Typed results retain all 33 raw bytes and explicit unknown values.
No universal service table is bundled with this mechanism.

Peripheral service codecs preserve their own operation enums separately from
the firmware-selected App80 command byte:

| Codec             | Operations and replies                                                                                                                                                                                                                                                                         |
| ----------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Register control  | Audio value, display forwarding/stub, power-register write, packed position pair and two no-op modes. Completion has no payload. Studied read modes leave output uninitialized and remain research operations.                                                                                 |
| I²C               | Read or write 1–10 bytes through typed ports; a separate single-byte write variant represents the write-only route. Read data is bounded by the requested length; all mailbox bytes remain available. A route may call firmware stubs, so completion does not certify a physical bus transfer. |
| Accumulator       | Explicit start or stop-and-read. Stop returns the low 16 bits of three counters, not full-width totals.                                                                                                                                                                                        |
| Peripheral report | Eight opaque result bytes. Callback and flag helpers can be stubbed or unreviewed; no hardware-state interpretation is supplied.                                                                                                                                                               |

| Media codec         | Operations and replies                                                                                                                                                                                   |
| ------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Display option/mode | Idle pins or forward a byte pair; select setup option 0/1. These are not UI-mode transitions. No result payload.                                                                                         |
| Sound               | Checked clip 0–22 or typed track/operation/format choices; canonical codes represent the reviewed mapping. Completion acknowledges submission, not playback completion.                                  |
| HDMI                | Exact 16-byte packet, three checked 12-bit values or explicit reset-stub request. Packet bytes are reversed into the parameter slots. Studied hardware callees are stubs.                                |
| Parameter service   | Explicit enabled/disabled choice; no implicit configuration save.                                                                                                                                        |
| Charge control      | Set control or enable it and set the register flag in one firmware command. Output byte 0/1 means success/failure, with unknown values preserved.                                                        |
| Charge status       | Separate presence and reported-state queries. Presence maps 1/2 to absent/present; state groups remain named for the register values instead of inferred charging meanings. Unknown values are retained. |

| Audio/storage codec | Operations and replies                                                                                                                                                                                                                                          |
| ------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Audio capture       | Checked 0–10 firmware chunks and explicit AV-output request 82/81. Firmware retains samples in its own RAM. Output 0/1 is success/failure; unchanged or other output remains unknown.                                                                           |
| Audio playback      | Copy the existing firmware capture buffer and play both lanes, zero the second lane or zero the first. Does not upload data. Output 0/1 is success/failure, with unknown values retained.                                                                       |
| Erase storage       | Typed choices for kind-1 regions, kinds 0/2, all three kinds, auxiliary region or profile reset. The first three also clear diagnostics; kinds 0/2 reset profiles. Persistent effects are part of the selected firmware command. Output 0/1 is success/failure. |
| Storage status      | Numbering-snapshot classifications 0, 4, 8, 16, 32. Zero occurs on both success and error paths; physical meanings remain unspecified. Other bytes are unknown.                                                                                                 |
| Format storage      | One firmware command formats its selected drive and awaits its internal callback. No payload result; the library adds no polling, reset or cleanup.                                                                                                             |
| Display setup       | One firmware command runs its internal display setup/request sequence. No arguments or payload result.                                                                                                                                                          |

| Storage/radio codec               | Operations and replies                                                                                                                                                                                                                                                                                         |
| --------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| NAND read blocks                  | `NandSpare` and a byte-sized block-group index; firmware reads eight blocks beginning at index × 8 into its RAM buffer. No USB download is composed. Output 0 is success, 1 is rejection/wait error, 2 is another error, and other bytes remain unknown.                                                       |
| Option 0x41                       | Query whether the firmware getter returns zero. Reply 1 maps to `Zero`, 0 to `NonZero`; it does not establish drive presence.                                                                                                                                                                                  |
| WLAN control                      | Explicit control-line disabled/enabled request. The enable path increments a firmware counter. No payload result or implied connection workflow.                                                                                                                                                               |
| Radio control                     | Primary mode/channel/encoded-power/flag or secondary mode/channel. Channel 0–14 and power 0–127 are checked quantities; mode and flag are enum choices. Firmware may stop LAN/restart the driver and ignores lower-level failures. No payload result.                                                          |
| Signal strength                   | Exact six-byte peer address; opaque output byte. A state-dependent firmware path may leave output unchanged or copy an uninitialized byte. No validated measurement or unit conversion.                                                                                                                        |
| iPerf                             | Uses existing camera configuration text; opaque low result byte. Service completion can precede running the test, so the read is a snapshot without an implicit poll.                                                                                                                                          |
| WPS                               | Explicit stop, start with stored mode/PIN and broadcast or checked exact peer, connect, or publish progress. Start returns a low callback-byte snapshot, including unknown/sentinel bytes. Progress publication also writes firmware configuration; it is not an observational read. No host polling is added. |
| Station                           | Stop, start from stored credentials, or scan with selector 0/1 (both invoke the same studied path). Start/scan return opaque low callback bytes; scan also returns the low count byte. Firmware waiting can outlast its USB deadline. No credentials or scan-result setup is composed.                         |
| AP reconfiguration                | Stop the server, read a six-byte configuration snapshot, or apply number/channel/radio/security/country fields. The command cannot supply legacy key data; only the three reviewed security fields without that requirement are typed. Apply can acknowledge before its work finishes.                         |
| Transfer prepare                  | Firmware preparation/release sequence in one service command. Output 0/2/3 distinguishes success, preparation error, and source status 7; other bytes remain unknown.                                                                                                                                          |
| Transfer select operation/channel | Typed lower-operation choices encode canonical selectors. Operation callback failure can leave the output unchanged. Output 0/1/unknown remains separate from host completion.                                                                                                                                 |
| Transfer readiness/start          | Separate firmware commands with 0/1/unknown results. No USB data transfer is composed, and stub success does not certify physical readiness.                                                                                                                                                                   |
| Transfer word/pair                | Bounded aligned transfer-word addresses and checked nine-bit pair inputs. Word payload is exposed only for status 0; pair results distinguish 0/3/4/6 and unknown bytes. Some lower callbacks receive null data. These are separate from debug memory commands.                                                |
| Transfer byte/bitfields           | One opaque byte or two 16-bit masks plus the byte. Byte replies preserve two raw bytes; firmware expands the masks internally. No host workflow or inferred bit meanings.                                                                                                                                      |
| Transfer halfword/options         | Full-width halfword address or typed option 0–7. Replies preserve raw values because studied stubs may leave the firmware output buffer uninitialized.                                                                                                                                                         |
| Transfer selected words/load      | Separate kind enums for 9–14, 7/8, 16/17 and load selectors 0/1/3/4. No payload result. Stock kind-16/17 success may omit signaling; host completion rules and deadlines still apply.                                                                                                                          |

The configuration service family has four codecs: flags (11 slots), mode
(2 slots), packed options (4 slots) and raw options (4 output slots). Flags
selector 0 submits a lower flags request then publishes two raw four-byte
buffers; selector 1 submits flags without output. Both take bank and option
0/1. Flags output retains each buffer's published 1,0,3,2 byte order. Mode
forwards flag 0/1 and selector 0–3; firmware maps the selector to bits 2/0/4/6
and first submits kind 3 with zero. Packed options forward flag/option 0/1
and two full bytes; firmware assembles `flag | option<<1 | upper<<10 | lower<<2`.
Raw options decode four big-endian bytes. Output slots start at `ff`, but a
studied lower stub can write uninitialized stack bytes over them: neither an
unchanged sentinel nor a different byte establishes validity. Read-flags is
mutating, and command completion does not establish persistence.

Peripheral battery control uses two definition-selected slots. Selector 0 resets
a pending flag; selector 1 invokes a callback and waits on the firmware event.
The second input is canonical zero: the reviewed reset helper ignores it. The
handler ignores the internal wait result, so no battery value or success-status
payload is inferred. Some handlers use literal configuration keys instead of
the passed parameter table; the caller's firmware definition supplies that slot
mapping just as it supplies mappings for table-based handlers.

Display initialization has one selector slot, canonically zero, and uses camera
state for timing, format, output and buffers. Display pattern uses five slots:
selector 0 resets the preset, 1 draws bars, and 2 takes `repeated` in slot 2,
`low` in slot 3 and `middle` in slot 4. Slot 1 is unused. Solid packs
`repeated<<24 | repeated<<16 | middle<<8 | low`. Firmware waits on its own events
for reset/bars; the host uses the ordinary ff80 completion and does not poll.
These commands have no payload result, and no host preparation is implied.

Adjustment configuration has five slots: action 0 reads configured state into
slot 4, action 1 applies profile 0–6, minutes and seconds from slots 1–3.
Durations are nonzero and at most 15360 seconds; the canonical maximum encodes
255/60. The wrapper sets internal service flags to `20` before apply. Its
configuration sequence also uses existing firmware state outside this mailbox.
Adjustment mode selection has one slot: 0 requires flags `8` and changes them
to `10`; canonical 1 requires `10` and changes them to `8`. A later request
failure does not undo that change. The firmware's global key tables, rather
than the dispatch row's null parameter pointer, determine these layouts.

Lens selection has five slots: 0 initializes, otherwise the selector denotes
index plus one; a lower index of `a0` has initialization semantics. Slot 1 is
read on the initialization path but ignored by the reviewed lower initializer;
the typed request sends zero. Firmware clears outputs 2–4, checks its own
available-count limit and writes 1 to output 2 on range rejection. Other output
bytes stay raw. Callback errors can still lead to normal command completion,
so output 2 equal to zero only means no range rejection was published.
Lens operations 2/3 clear their single output byte and wait for either callback
event; 4/5 have no payload and wait on a separate event. None defines a host
pending disposition, and the library does not reconstruct a lost callback status.

Lens operation 6 and both adjustment-output preparation commands take no mailbox
arguments. Preparation submits the firmware's `0204/0205` sequence and waits
inside firmware; it is not a host-side composition or USB data preparation API.
Completion-parameter publication uses two definition-selected literal keys:
flag 1/encoded 255 for parameter `80`, otherwise flag 0 and the low byte of
parameter+1. Replies preserve both fields, including unknown flag values.

Bounded position has two input bytes and one output flag, initialized to zero
by firmware. Callback result -97 writes 1 to a literal configuration key. The
codec contains no key address. Other callback
errors can leave the flag zero. Fixed position initializes a single output byte
and submits its internal parameter `ff`; no callback result code is published.
Primary/secondary parameter-callback commands take one input and return one raw
byte initialized to zero, independently of callback event status.

Option query uses input slot 0, high/low result bytes 1/2, and status 3. Status
0/1 reports success/failure; other bytes remain unknown. Firmware signals normal
completion even when its internal wait or callback reports failure. Raw data is
preserved in every case and never drives host completion logic. A studied lower
stub passes null to the callback, so no buffer-validity guarantee is inferred.
