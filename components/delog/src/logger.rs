use core::cmp;
use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};
use core::ptr;

/// Semi-abstract characterization of the deferred loggers that the `delog!` macro produces.
///
/// This trait is markes "unsafe" to signal that users should never (need to) "write their own",
/// but always go through the `delog!` macro.
///
/// The user has access to the global logger via `delog::trylogger()`, but only as TryLog/Log
/// implementation, not with this direct access to implementation details.
pub unsafe trait Delogger: log::Log + crate::TryLog {

    fn buffer(&self) -> &'static mut [u8];
    fn read(&self) -> &'static AtomicUsize;
    fn written(&self) -> &'static AtomicUsize;
    fn claimed(&self) -> &'static AtomicUsize;
    fn flush(&self, logs: &str);
    fn render(&self, args: &fmt::Arguments) -> &'static [u8];

    fn capacity(&self) -> usize { self.buffer().len() }

}

/// Fallible, panic-free version of the `log::Log` trait.
///
/// The intention is actually that implementors of this library also
/// implement `log::Log` in a panic-free fashion, and simply drop logs
/// that can't be logged. Because, if the user can handle the error, they
/// would be using the fallible macros, and if not, they most likely do **not**
/// want to crash.
pub trait TryLog: log::Log {
    fn try_log(&self, _: &log::Record) -> core::result::Result<(), ()>;
}

/// Generate a deferred logger with specified capacity and flushing mechanism.
#[macro_export]
macro_rules! delog {
    ($logger:ident, $size:expr, $immediate_size:expr, $flusher:ty) => {

        #[derive(Clone, Copy)]
        pub struct $logger {
            flusher: &'static $flusher,
        }

        // log::Log implementations are required to be Send + Sync
        unsafe impl Send for $logger {}
        unsafe impl Sync for $logger {}

        impl log::Log for $logger {
            /// log level is set via log::set_max_level, not here, hence always true
            fn enabled(&self, _: &log::Metadata) -> bool {
                true
            }

            /// reads out logs from circular buffer, and flushes via injected flusher
            fn flush(&self) {
                let mut buf = [0u8; $size] ;

                use $crate::Delogger;
                let logs: &str = unsafe { $crate::dequeue(*self, &mut buf) };

                use $crate::Flusher;
                self.flusher.flush(logs);
            }

            fn log(&self, record: &log::Record) {
                // use $crate::Delogger;
                unsafe { $crate::enqueue(*self, record) }
            }
        }

        impl $crate::TryLog for $logger {
            fn try_log(&self, record: &log::Record) -> core::result::Result<(), ()> {
                // use $crate::Delogger;
                unsafe { $crate::try_enqueue(*self, record) }
            }
        }

        impl $logger {
            pub fn init(level: log::LevelFilter, flusher: &'static $flusher) -> Result<(), ()> {
                use core::sync::atomic::{self, AtomicBool, AtomicUsize, Ordering};
                use core::mem::MaybeUninit;

                static INITIALIZED: AtomicBool = AtomicBool::new(false);
                if INITIALIZED
                    .compare_exchange_weak(false, true, Ordering::AcqRel, Ordering::Acquire).is_ok()
                {

                    let logger = Self { flusher };
                    Self::get().replace(logger);
                    $crate::trylogger().replace(Self::get().as_ref().unwrap());
                    log::set_logger(Self::get().as_ref().unwrap())
                        .map(|()| log::set_max_level(level))
                        .map_err(|_| ())
                } else {
                    Err(())
                }
            }

            fn get() -> &'static mut Option<$logger> {
                static mut LOGGER: Option<$logger> = None;
                unsafe { &mut LOGGER }
            }

            fn flush() {
                // gracefully degrade if we're not initialized yet
                if let Some(logger) = Self::get() {
                    log::Log::flush(logger)
                }
            }
        }

        unsafe impl $crate::Delogger for $logger {

            fn buffer(&self) -> &'static mut [u8] {
                static mut BUFFER: [u8; $size] = [0u8; $size];
                unsafe { &mut BUFFER }
            }

            fn flush(&self, logs: &str) {
                use $crate::Flusher;
                self.flusher.flush(logs)
            }

            fn read(&self) -> &'static core::sync::atomic::AtomicUsize {
                use core::sync::atomic::AtomicUsize;
                static READ: AtomicUsize = AtomicUsize::new(0);
                &READ
            }

            fn written(&self) -> &'static core::sync::atomic::AtomicUsize {
                use core::sync::atomic::AtomicUsize;
                static WRITTEN: AtomicUsize = AtomicUsize::new(0);
                &WRITTEN
            }

            fn claimed(&self) -> &'static core::sync::atomic::AtomicUsize {
                use core::sync::atomic::AtomicUsize;
                static CLAIMED: AtomicUsize = AtomicUsize::new(0);
                &CLAIMED
            }

            fn render(&self, args: &core::fmt::Arguments) -> &'static [u8] {
                static mut LOCAL_BUFFER: [u8; $size] = [0u8; $size];

                let local_buffer = unsafe { &mut LOCAL_BUFFER };
                $crate::render::render_arguments(local_buffer, *args)
            }
        }
    }
}

/// The core "write to circular buffer" method. Marked unsafe to discourage use!
///
/// Unfortunately exposed for all to see, as the `delog!` macro needs access to it to
/// implement the logger at call site. Hence marked as unsafe.
pub unsafe fn enqueue(delogger: impl Delogger, record: &log::Record) {
    crate::logger::try_enqueue(delogger, record).ok();
}

/// The fallible "write to circular buffer" method. Marked unsafe to discourage use!
///
/// Unfortunately exposed for all to see, as the `delog!` macro needs access to it to
/// implement the logger at call site. Hence marked as unsafe.
#[allow(unused_unsafe)]
pub unsafe fn try_enqueue(delogger: impl Delogger, record: &log::Record) -> core::result::Result<(), ()> {

    if record.target() == "!" {
        // todo: proper "fast path" / immediate mode
        println!("{}", record.args());
        return Ok(());
    }

    let written = delogger.written().load(Ordering::SeqCst);
    let buffer = delogger.buffer();
    let input = delogger.render(record.args());

    let buffer_len = buffer.len();
    let input_len = input.len();

    if input_len > buffer_len {
        // early exit to hint the optimizer that `buffer_len` can't be `0`
        return Err(());
    }

    // NOTE we use `UnsafeCell` instead of `AtomicUsize` because we want this operation to
    // return the same value when calling `log` consecutively
    let read = delogger.read().load(Ordering::SeqCst);

    if buffer_len >= input_len + written.wrapping_sub(read) {

        let w = written % buffer_len;

        // NOTE we use `ptr::copy_nonoverlapping` instead of `copy_from_slice` to avoid
        // panicking branches
        if w + input_len > buffer_len {
            // two memcpy-s
            let mid = buffer_len - w;
            // buffer[w..].copy_from_slice(&input[..mid]);
            unsafe {
                ptr::copy_nonoverlapping(input.as_ptr(), buffer.as_mut_ptr().add(w), mid);
                // buffer[..input_len - mid].copy_from_slice(&input[mid..]);
                ptr::copy_nonoverlapping(
                    input.as_ptr().add(mid),
                    buffer.as_mut_ptr(),
                    input_len - mid,
                );
            }
        } else {
            // single memcpy
            // buffer[w..w + input_len].copy_from_slice(&input);
            unsafe {
                ptr::copy_nonoverlapping(input.as_ptr(), buffer.as_mut_ptr().add(w), input_len);
            }
        }

        delogger.written().store(written.wrapping_add(input_len), Ordering::SeqCst);
        Ok(())
    } else {
        Err(())
    }
}

/// The core "read from circular buffer" method. Marked unsafe to discourage use!
///
/// Unfortunately exposed for all to see, as the `delog!` macro needs access to it to
/// implement the logger at call site. Hence marked as unsafe.
#[allow(unused_unsafe)]
pub unsafe fn dequeue<'b>(delogger: impl Delogger, buf: &'b mut [u8]) -> &'b str
{
    // we control the inputs, so we know this is a valid string
    unsafe { core::str::from_utf8_unchecked(drain_as_bytes(delogger, buf)) }
}

/// Copy out the contents of the `Logger` ring buffer into the given buffer,
/// updating `read` to make space for new log data
fn drain_as_bytes<'b>(delogger: impl Delogger, buf: &'b mut [u8]) -> &'b [u8] {
    unsafe {
        let read = delogger.read().load(Ordering::SeqCst);
        let written = delogger.written().load(Ordering::SeqCst);
        let p = delogger.buffer().as_ptr();

        // early exit to hint the compiler that `n` is not `0`
        let capacity = delogger.buffer().len();
        if capacity == 0 {
            return &[];
        }

        if written > read {
            // number of bytes to copy
            let available = cmp::min(buf.len(), written.wrapping_sub(read));

            let r = read % capacity;

            // NOTE `ptr::copy_nonoverlapping` instead of `copy_from_slice` to avoid panics
            if r + available > capacity {
                // two memcpy-s
                let mid = capacity - r;
                // buf[..mid].copy_from_slice(&buffer[r..]);
                ptr::copy_nonoverlapping(p.add(r), buf.as_mut_ptr(), mid);
                // buf[mid..mid + c].copy_from_slice(&buffer[..available - mid]);
                ptr::copy_nonoverlapping(p, buf.as_mut_ptr().add(mid), available - mid);
            } else {
                // single memcpy
                // buf[..c].copy_from_slice(&buffer[r..r + c]);
                ptr::copy_nonoverlapping(p.add(r), buf.as_mut_ptr(), available);
            }

            delogger.read().store(read.wrapping_add(available), Ordering::SeqCst);

            // &buf[..c]
            buf.get_unchecked(..available)
        } else {
            &[]
        }
    }
}
