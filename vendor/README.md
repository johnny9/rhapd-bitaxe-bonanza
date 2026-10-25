# esp-hal ADC calibration backport

`esp-hal/` is the published `esp-hal` 1.0.0-rc.0 crate, retained under its
original MIT/Apache-2.0 licenses. Upstream source revision: `6b758d75b878d3ebf671a89c28de787551bdbdbc`.
The published crate checksum is `f3887eda2917deef3d99e7a5c324f9190714e99055361ad36890dffd0a995b49`.
License texts from that upstream revision's repository root are included as
`esp-hal/LICENSE-MIT` and `esp-hal/LICENSE-APACHE`.

Only `src/analog/adc/xtensa.rs` differs from the upstream source. The exact
backport is recorded in `esp-hal-adc.patch`. It applies the ESP32-S3 fixes from
https://github.com/esp-rs/esp-hal/pull/4286 without changing public APIs:

- Use zero-based eFuse unit 0 for ADC1 and unit 1 for ADC2. Previously both
  selected ADC2 parameters because the driver passed 1/2 to a 0/1 API.
- Write each ADC's DREF field to 4 before its calibration init code, as in
  ESP-IDF and the upstream fix.

Cargo's root `[patch.crates-io]` points here for both the application and
`esp-hal-embassy`. No global Cargo registry files are modified.

The fixed upstream 1.0.0-rc.1 release removes the internal Embassy integration
feature required by this project's esp-hal-embassy 0.9.1. The local backport
keeps the compatible dependency graph while correcting ADC calibration.

This does not apply any empirical DMM offset or board-specific voltage scale.
The existing 11 dB curve calibration and 16-sample averaging remain in use.
Hardware accuracy must be checked against a DMM after flashing this RHAP-D firmware. A future HAL/runtime migration can remove this backport once it includes
the upstream fix and passes board-control/USB/SWD regression checks.
