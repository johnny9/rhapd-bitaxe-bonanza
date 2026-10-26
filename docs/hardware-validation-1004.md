# Board 1004 hardware validation

Tested 2026-10-25 (UTC evidence dated 2026-10-26), on the USB-connected
`bonanza1004.local`, using RHAP firmware 0.1.1 at
`6e762835fa53c993d6ce713ad72ebefde8d865b3`.

**Result: partial pass; recovery qualification failed.** Board 1002 is unsupported.

## Passed

- 33 hardware checks: USB identity and framing, malformed-request rejection,
  parser recovery, safe startup/disconnect/reboot outputs, regulator reads,
  power-off ADC readings, fan PWM/tach, bridge identity/control, coordinated
  stream reset, SWD probes, programming interlocks, pre-write abort,
  out-of-order page rejection, complete page readback, CRC and COMMIT.
- SWD programmed the production bridge image `0.1.0-1004.2+ge73b18a`:
  48,032 bytes, 188 pages, CRC32 `4cff7b3c`, about 32 seconds for page upload.
  SHA256: `8d85b9a520aec86a2ea2c0bad9e28b65671bf72b2c5e6e3a9e44a222fae310a8`.
- COMMIT confirmed a forced RP2040 watchdog reset and neither core halted.
  Bridge control replies and RUNNING status returned.
- Original ESP32 flash was backed up and restored. All 16 MiB matched its
  pre-test MD5. Original ESP-Miner image: `1e40c1f4`, `ota_1`, SHA256
  `95ac6a7d7d9fca8a3ce9543e32d0d08bea184769c9cdb7e07322de4b26b70ebc`.
- 22 Rust tests, four Python tests, release build and Clippy passed.

The initial fan test sampled the same tach count after only 0.8 seconds of
settling. Repeating with two seconds passed: full/half/full RPM 3599/2039/3539.

## Failed recovery qualification

After restoring ESP-Miner, ASIC discovery failed with `BZM initialization
failed`. In the initial 0.1.0 attempt, startup logs showed
`chain_missing ASIC=0x0a reg=0x0b`. ESP32 restart and bridge RESET_STREAM
alone did not recover that attempt. An additional full RP2040 watchdog reset
while ASIC outputs were off, followed by ESP32 restart, recovered mining.

Version 0.1.1 replaced SYSRESETREQ after COMMIT with a full RP2040 watchdog
reset, confirmed its reset reason and checked that both cores were running.
The complete SWD upload and control checks passed again, but restoration still
failed ASIC discovery. The extra RP2040 reset recovered mining again.

The precise failure cause and reset timing dependency remain unresolved.
Successful control replies and a RUNNING bridge status do not establish
working ASIC traffic. Neither tested version is fully hardware-qualified.

## Remaining coverage

Mining through a RHAP host driver, ASIC traffic saturation, powered ADC accuracy
against a meter, physical trip assertion, mid-page disconnect/power loss,
and long thermal qualification were not tested. Full flash backups and raw
logs remain local because the backups include device configuration.
