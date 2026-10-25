#!/bin/sh
set -eu
cd "$(dirname "$0")"
arm-none-eabi-gcc -mcpu=cortex-m0plus -mthumb -Os -ffreestanding -fno-builtin \
  -fno-unwind-tables -fno-asynchronous-unwind-tables -nostdlib \
  -Wl,-T,stub.ld -Wl,--build-id=none -Wl,--gc-sections -o stub.elf stub.c
arm-none-eabi-objcopy -O binary stub.elf stub.bin
