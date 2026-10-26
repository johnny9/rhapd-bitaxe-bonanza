# RHAP-D for Bitaxe Bonanza 1004

Raw Hardware Access Protocol firmware for the ESP32-S3 on **Bitaxe Bonanza
1004**. It exposes board I2C, GPIO, calibrated voltage taps, fan control and
BZM2 ASIC serial traffic over two USB CDC ACM ports. A host performs ASIC
initialization, mining, voltage programming and thermal supervision.

This is a fork of [256 Foundation's Gamma RHAP-D](https://github.com/256foundation/rhapd-bitaxe-gamma),
based on `59e90f6`. The 1004 GPIO, ADC calibration and SWD code comes from
[bitaxe-raw's Bonanza port](https://github.com/bitaxeorg/bitaxe-raw/commit/ec84fc0355e165a2614fed6333b5cc16609a0977).
It uses RHAP's explicit response status byte and System page `0x09`.
**Board 1002 is unsupported.**

## Build and check

Install Rust with rustup and the `just` command runner, then:

```sh
just setup-tools
just checks
just build
```

The setup recipe selects the versioned ESP toolchain `esp-1.93.0.0` without
replacing another project's `esp` toolchain. `Cargo.lock` is tracked. The local
ADC HAL backport and its licenses/provenance are documented in [vendor/README.md](vendor/README.md).
`ESP_GCC` can select a compatible Xtensa GCC explicitly when it is installed
outside the selected Rust toolchain.

Host tests can run independently of the ESP toolchain:

```sh
tools/test-host
```

A local build also passes with ESP Rust `1.89.0.0` and ESP GCC
`14.2.0_20251107`:

```sh
ESP_GCC="$HOME/.espressif/tools/xtensa-esp-elf/esp-14.2.0_20251107/xtensa-esp-elf/bin/xtensa-esp32s3-elf-gcc" \
  cargo +esp build --release --locked
```

## Flashing

Connect only the target Bonanza over USB, then run `just flash`. Hold BOOT
while attaching power for the first installation from a different firmware.
For subsequent updates, `tools/board` selects the Bonanza RHAP-D product and
uses the RHAP reboot-to-bootloader command. It rejects multiple matching
boards and a running Bonanza alongside another ESP ROM bootloader.

The ESP32 firmware and RP2040 bridge are separate images. The bridge must
implement **1004 transport protocol 3.0 at 5 Mbaud**, including queue status
and coordinated stream reset. A matching source is
[johnny9/bonanza-bridge-fw, `fix/1004-5mbaud-queues`](https://github.com/johnny9/bonanza-bridge-fw/tree/fix/1004-5mbaud-queues),
revision `1840d61`; its [protocol specification](https://github.com/johnny9/bonanza-bridge-fw/blob/1840d61587e2e4beb7d406b23427305d3dba8e8f/docs/1004-protocol.md)
defines the diagnostic payloads. The ESP can program that bridge through the
SWD extension below. The display's separate SWDIO on GPIO16 is untouched.

## USB and RHAP

USB descriptors:

- VID/PID: `1209:6102`, following the Bitaxe allocation used by Gamma RHAP-D
- Manufacturer: `OSMU`
- Product: **`Bitaxe Bonanza RHAP-D`**
- Serial: the ESP32 MAC address as 12 hex characters
- Device release: `0x0101` for firmware `0.1.1`
- First serial port: control; second serial port: ASIC data

Open the control port with **DTR asserted**. Closing it/deasserting DTR or
removing USB returns board outputs to the safe state. Existing Bonanza
`bitaxe-raw` clients need the new product match and RHAP response decoder;
Gamma clients need a Bonanza board driver and the commands below.

Control requests and responses use total packet lengths in little-endian:

```text
Request:  length:u16 | id:u8 | bus:u8 | page:u8 | command:u8 | data...
Response: length:u16 | id:u8 | status:u8 | data...
```

Bus must be zero. Status is `0x00` success, `0x10` timeout, `0x11` invalid,
`0x12` buffer overflow, or `0xff` with an ASCII error message. Board interlock
and bridge faults use the message status. The ID is echoed, including errors
when it is available. Length includes the header and status byte. Partial
requests time out after 100 ms without progress; split/coalesced requests are
supported. Responses larger than one USB packet are split into 64-byte packets.

For example, successful GPIO-low data is `05 00 2a 00 00`; a timeout is
`04 00 2a 10`. Data bytes cannot be confused with status codes.

| Page | Function |
| --- | --- |
| `0x00` | Bonanza board identity and output status |
| `0x05` | RHAP I2C |
| `0x06` | Bonanza GPIO |
| `0x07` | Calibrated cumulative voltage taps |
| `0x08` | Direct ESP fan PWM and tach |
| `0x09` | RHAP reboot / reboot to bootloader |
| `0x0a` | RP2040 SWD programmer |
| `0x0b` | RP2040 transport diagnostics and stream reset |

The data port runs a fixed **5 Mbaud/8N1** ESP-to-RP2040 link; USB line coding
is ignored. Host TX uses byte pairs `[low-eight-bits, bit-eight (0 or 1)]`
for each ASIC 9N1 word. ASIC replies return raw low-eight-bit bytes. Data is
forwarded only while ASIC reset is released and SWD programming is inactive.

## Board wiring and commands

| Signal | ESP32 GPIO |
| --- | --- |
| ASIC reset, active low / trip, active high | 1 / 2 |
| D1 / D2 / D3 voltage taps | 5 / 4 / 3 |
| Fan PWM, active low / tach | 8 / 9 |
| TPS546 enable / PGOOD | 10 / 11 |
| RP2040 SWCLK / SWDIO | 14 / 15 |
| ASIC data TX / RX through RP2040 | 17 / 18 |
| Switched 5 V enable | 21 |
| RP2040 control TX / RX | 43 / 44 |
| I2C SDA / SCL | 47 / 48 |

**Board page `0x00`:** `0x01` returns schema `1`, board number `1004:u16LE`,
version length `u8`, then ASCII `version+gcommit`. `0x10` returns three bytes:
`[schema=1, flags, requested_raw_fan_duty]`. Flag bits 0..5 mean 5 V enabled,
reset asserted, trip input high, trip latched, TPS enable high, PGOOD high.

**I2C page `0x05`:** `0x10 [frequency:u32LE]`, `0x20 [address, write-bytes...]`,
`0x30 [address, read-length:u8]`, `0x40 [address, write-bytes..., read-length:u8]`.
Frequency replies with the configured Hz; write replies with byte count;
read/write-read reply with bytes. The host owns TPS546 settings, including the
1004 compensation configuration. This firmware does not apply Gamma defaults.

**GPIO page `0x06`:** omit data to read, or append one canonical `0`/`1` byte
to set. Replies contain the resulting pin level.

| Command | Signal |
| --- | --- |
| `0x00`, `0x02` | ASIC reset (`0` asserts reset) |
| `0x01` | 5 V enable |
| `0x03` | ASIC trip, read only |
| `0x04` | TPS546 enable |
| `0x05` | PGOOD, read only |

5 V must be enabled before releasing reset; enabling 5 V requires reset
asserted. Disabling 5 V asserts reset first. A trip latches reset asserted,
5 V off, TPS enable off and full fan. Once the input clears, explicitly assert
reset with 5 V off to acknowledge the trip latch. Disconnect and bridge faults
also restore these safe outputs. PMBus OPERATION remains host-owned.

**ADC page `0x07`:** `0x50`, `0x51`, `0x52` return D1/D2/D3 respectively as
`u16LE` millivolts; `0x53` returns all three in that order. Readings average
16 curve-calibrated ADC1 samples at 11 dB attenuation. These are cumulative
stack taps, without a divider. Individual chip supplies are D1, D2-D1, D3-D2,
and TPS VOUT-D3. GPIO2 is the trip input and is never configured as ADC.

**Fan page `0x08`:** `0x00 [raw-duty:u8]` sets inverted 8-bit PWM (zero is full
speed, 255 is stopped) and echoes duty. `0x01` returns RPM as `u32LE`.
`0x10 [percent:0..100]` sets speed and replies with zero; `0x20` returns RPM as
`u16LE`. PWM is 25 kHz. Tach sampling uses two pulses per revolution over a
500 ms interval. Trips prevent lowering fan speed. Page `0x09` is reserved
for RHAP System commands.

**System page `0x09`:** `0x01 DE AD BE EF` reboots; `0x02 B0 07 10 AD` enters
the ESP ROM bootloader. Both first shut down board outputs and request full
fan. As in Gamma RHAP-D, no reply is sent; observe USB re-enumeration.

## Bridge diagnostics and recovery

Page `0x0b` accepts empty-payload commands `0x01` GET_INFO, `0x02` GET_RX_STATS,
`0x03` GET_STATUS, `0x04` RESET_STREAM and `0x05` GET_QUEUES. Successful data
is the corresponding protocol-3 bridge payload inside a normal RHAP response.
The inner bridge UART framing is separate from USB RHAP framing.

Before reset release, the ESP pauses data forwarding, drains hardware TX,
checks protocol/board/capabilities/5 Mbaud identity, requests stream reset,
verifies the generation advance, and drains ESP RX. Failure leaves reset
asserted and power disabled. Retrying reset release retries preparation.
Repeated reset release while already running is idempotent.

RESET_STREAM is denied while reset is released or SWD programming is active.
The host must stop data writes, assert reset, clear its receive/parser/job
state across reset, and re-enumerate ASICs before resuming mining. Allow at
least **3 seconds** for reset/release requests, which combine several bounded
bridge transactions.

A separate task polls bridge transport status every 250 ms while reset is
released. A failed transaction (500 ms deadline), non-running phase, changed
generation or decreasing uptime shuts down the board. Recovery requires host
reinitialization. The RP2040 watchdog covers transport; the host remains
responsible for voltage, fan, temperature and mining supervision.

## RP2040 SWD programming

Page `0x0a` uses these commands. The host must disable TPS PMBus OPERATION and
enable before BEGIN. PGOOD must be low and trip clear. Flash capacity is 2 MiB.

| Command | Request | Successful data |
| --- | --- | --- |
| `0x00` | empty | schema/API `1,1`, page-size:u16LE, capacity:u32LE, GPIOs `14,15` |
| `0x01` | empty | RP2040 DPIDR:u32LE |
| `0x10` BEGIN | image-size:u32LE, CRC32:u32LE, first 264 image bytes | schema `1`, verified-count:u32LE, readback-CRC32:u32LE |
| `0x20` PAGE | offset:u32LE, 1..256 bytes | schema/count/CRC |
| `0x30` COMMIT | empty | final schema/count/CRC after full watchdog reset |
| `0x7f` ABORT | empty | zero |

The image header must contain a valid RP2040 boot2 CRC, SRAM stack pointer and
in-image Thumb reset vector. Pages arrive in order at 256-byte aligned offsets;
only the last may be short. Every page is read back and checked. COMMIT requires
the complete image size and CRC. During programming, power-on GPIO requests
and I2C are denied, ASIC traffic is discarded, and trip supervision stays live.
Abort/disconnect releases SWD without booting a partial image. Restart the whole
upload after uncertainty. See [flash_stub/README.md](flash_stub/README.md) for
the SRAM algorithm source and reproducible build command.

## Qualification

Firmware 0.1.1 uses a full RP2040 watchdog reset after COMMIT. The prior
SYSRESETREQ sequence returned control replies in hardware testing but ASIC
discovery failed until a full RP2040 reset. COMMIT clears both debug halts,
requests the complete watchdog reset domains, and verifies the reset reason
and that neither core remains halted. This improved the reset checks, but
**did not resolve the tested ESP-Miner restoration failure**: ASIC discovery
still required an additional RP2040 watchdog reset after restoring ESP-Miner.
The fork is not fully hardware-qualified.

On board 1004, revision `6e76283` passed 33 USB protocol, safe-output, fan,
bridge-control, SWD upload/readback and reboot checks. The 48,032-byte bridge
image was verified across 188 pages, with CRC32 `4cff7b3c`. See the
[hardware validation report](docs/hardware-validation-1004.md) for the failed
recovery qualification and remaining coverage. Native tests (22 Rust and four
Python), release compilation and Clippy passed. Earlier bitaxe-raw hardware
results do not qualify this RHAP fork.

The root [GPL-3.0 license](LICENSE) is retained from both upstream projects;
vendor code and the flash trampoline retain their own licenses.
