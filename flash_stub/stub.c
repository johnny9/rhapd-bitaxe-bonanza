/* SPDX-License-Identifier: MIT
 * RP2040 ROM API trampoline. Entry/context addresses are defined by stub.ld.
 * ROM table/function IDs: RP2040 datasheet, section 2.8.3.
 * Interrupts stay disabled while XIP is unavailable. Only this SRAM code and
 * boot ROM execute; the ESP32 halts both Cortex-M0+ cores before loading it.
 */
#include <stdint.h>
#include <stddef.h>

typedef struct {
    volatile uint32_t status;
    uint32_t operation; /* 1 = erase one sector, 2 = program pages */
    uint32_t offset;
    uint32_t length;
    uint32_t source;
} request_t;

static void *rom_function(char a, char b) {
    typedef void *(*lookup_t)(uint16_t *, uint32_t);
    lookup_t lookup = (lookup_t)(uintptr_t)*(volatile uint16_t *)0x18;
    uint16_t *table = (uint16_t *)(uintptr_t)*(volatile uint16_t *)0x14;
    return lookup(table, (uint8_t)a | ((uint32_t)(uint8_t)b << 8));
}

__attribute__((section(".entry"), noreturn))
void run(request_t *request) {
    __asm volatile("cpsid i" ::: "memory");
    void (*connect)(void) = rom_function('I', 'F');
    void (*exit_xip)(void) = rom_function('E', 'X');
    void (*erase)(uint32_t, size_t, uint32_t, uint8_t) = rom_function('R', 'E');
    void (*program)(uint32_t, const uint8_t *, size_t) = rom_function('R', 'P');
    void (*flush)(void) = rom_function('F', 'C');
    void (*enter_xip)(void) = rom_function('C', 'X');
    request->status = 1;
    if (!connect || !exit_xip || !erase || !program || !flush || !enter_xip) {
        request->status = 0xE001;
    } else {
        connect();
        exit_xip();
        if (request->operation == 1 && request->offset % 4096 == 0 && request->length == 4096) {
            erase(request->offset, 4096, 4096, 0x20);
            request->status = 0xD00DFEED;
        } else if (request->operation == 2 && request->offset % 256 == 0 && request->length == 256) {
            program(request->offset, (const uint8_t *)(uintptr_t)request->source, 256);
            request->status = 0xD00DFEED;
        } else {
            request->status = 0xE002;
        }
        flush();
        enter_xip();
    }
    __asm volatile("bkpt #0" ::: "memory");
    for (;;) {}
}
