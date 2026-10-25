# RP2040 SRAM flash trampoline

This small MIT-licensed trampoline calls the RP2040 boot ROM's flash routines
from SRAM while XIP is disabled. No bridge firmware is embedded in the ESP32.
The host supplies the complete image through the RHAP programmer page (`0x0A`).

Rebuild the checked-in `stub.bin` with:

```bash
sh flash_stub/build.sh
```

Requires `arm-none-eabi-gcc` and `arm-none-eabi-objcopy`. The build does not use
Pico SDK libraries. `stub.ld` places the entry point at `0x20030000`; the ESP32
sets R0 to the request at `0x2003FF00`, MSP to `0x20042000`, and the Thumb PC
to the entry point. Source data is at `0x20032000`. Both cores are halted and
DMA is held in reset before this SRAM region is used.

The request contains five LE32 fields: status, operation, flash offset, length,
source address. Operation 1 erases exactly one aligned 4096-byte sector;
operation 2 programs exactly one aligned 256-byte page. Completion status is
`0xD00DFEED`, followed by a breakpoint. The ESP32 verifies every programmed
page through XIP before advancing upload state.

ROM API addresses/function IDs and alignment requirements are documented in the
[RP2040 datasheet](https://datasheets.raspberrypi.com/rp2040/rp2040-datasheet.pdf).
The image boot2 CRC follows the
[Raspberry Pi boot ROM](https://github.com/raspberrypi/pico-bootrom-rp2040/blob/master/bootrom/bootrom_main.c).
