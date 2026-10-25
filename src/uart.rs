use crate::control::{gpio, programmer, CommandError};
use core::sync::atomic::{AtomicU32, Ordering};
use embassy_futures::select::{select, Either};
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel, signal::Signal,
};
use embassy_time::{with_timeout, Duration, Timer};
use embassy_usb::class::cdc_acm::{CdcAcmClass, Receiver, Sender};
use embedded_io_async::Write;
use esp_hal::{
    uart::{Uart, UartRx, UartTx},
    Async,
};

// One control caller. Tokens distinguish acknowledgements after cancellation.
static NEXT_ID: AtomicU32 = AtomicU32::new(0);
static REQUEST: Channel<CriticalSectionRawMutex, (u32, bool), 1> = Channel::new();
static REPLY: Signal<CriticalSectionRawMutex, (u32, bool)> = Signal::new();
const IO_TIMEOUT: Duration = Duration::from_millis(100);

async fn request(paused: bool) -> Result<(), CommandError> {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    with_timeout(Duration::from_millis(500), async {
        REQUEST.send((id, paused)).await;
        loop {
            let (reply_id, success) = REPLY.wait().await;
            if reply_id == id {
                return if success {
                    Ok(())
                } else {
                    Err(CommandError::Fault)
                };
            }
        }
    })
    .await
    .map_err(|_| CommandError::Timeout)?
}

pub async fn pause() -> Result<(), CommandError> {
    request(true).await
}
pub async fn resume() -> Result<(), CommandError> {
    request(false).await
}

fn enabled() -> bool {
    gpio::data_enabled() && !programmer::is_active()
}

#[embassy_executor::task]
pub async fn usb_task(
    class: CdcAcmClass<'static, crate::UsbDriver>,
    uart: Uart<'static, Async>,
) -> ! {
    let (mut usb_tx, mut usb_rx, _) = class.split_with_control();
    let (mut uart_rx, mut uart_tx) = uart.split();
    let mut paused = true;
    loop {
        let (id, pause) = if paused {
            REQUEST.receive().await
        } else {
            match select(
                REQUEST.receive(),
                pipe(&mut usb_tx, &mut usb_rx, &mut uart_rx, &mut uart_tx),
            )
            .await
            {
                Either::First(request) => request,
                Either::Second(()) => {
                    gpio::shutdown();
                    paused = true;
                    continue;
                }
            }
        };
        paused = true;
        let success = if pause {
            // esp-hal's flush also waits for the last byte's shift register.
            matches!(
                with_timeout(IO_TIMEOUT, uart_tx.flush_async()).await,
                Ok(Ok(()))
            )
        } else {
            let mut discard = [0; 128];
            // ASIC reset is still asserted. Discard ESP RX only after reset ACK.
            let result = with_timeout(IO_TIMEOUT, async {
                while uart_rx.read_ready() {
                    uart_rx.read_buffered(&mut discard).map_err(|_| ())?;
                    Timer::after_micros(10).await;
                }
                Ok::<(), ()>(())
            })
            .await;
            matches!(result, Ok(Ok(())))
        };
        if success && !pause {
            paused = false;
        }
        if !success {
            gpio::shutdown();
        }
        REPLY.signal((id, success));
    }
}

async fn pipe(
    usb_tx: &mut Sender<'static, crate::UsbDriver>,
    usb_rx: &mut Receiver<'static, crate::UsbDriver>,
    uart_rx: &mut UartRx<'static, Async>,
    uart_tx: &mut UartTx<'static, Async>,
) {
    // Independent directions; only reset, disconnect, or error cancels writes.
    let _ = select(to_asic(usb_rx, uart_tx), from_asic(usb_tx, uart_rx)).await;
}

async fn to_asic(usb: &mut Receiver<'static, crate::UsbDriver>, uart: &mut UartTx<'static, Async>) {
    let mut buf = [0; 64];
    usb.wait_connection().await;
    loop {
        let Ok(n) = usb.read_packet(&mut buf).await else {
            return;
        };
        if enabled()
            && !matches!(
                with_timeout(IO_TIMEOUT, uart.write_all(&buf[..n])).await,
                Ok(Ok(()))
            )
        {
            return;
        }
    }
}

async fn from_asic(usb: &mut Sender<'static, crate::UsbDriver>, uart: &mut UartRx<'static, Async>) {
    let mut buf = [0; 1024];
    loop {
        let Ok(n) = uart.read_async(&mut buf).await else {
            return;
        };
        if !enabled() {
            continue;
        }
        for packet in buf[..n].chunks(64) {
            if !enabled() {
                break;
            }
            if !matches!(
                with_timeout(IO_TIMEOUT, usb.write_packet(packet)).await,
                Ok(Ok(()))
            ) {
                return;
            }
        }
    }
}
