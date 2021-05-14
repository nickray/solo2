use std::{fs::File, io::Write};
pub use embedded_hal::blocking::rng;
use littlefs2::{
    const_ram_storage,
};
use littlefs2::fs::{Allocation, Filesystem};
use trussed::types::{LfsResult, LfsStorage};

use trussed::platform::{
    ui,
    reboot,
    consent,
};
use trussed::{platform, store};
use ctap_types::consts;

pub use generic_array::{
    GenericArray,
    typenum::{U16, U512},
};

use generic_array::typenum::{U256, U1022};


const SOLO_STATE: &'static str = "solo-state.bin";

#[macro_use]
extern crate delog;
generate_macros!();

#[derive(Debug)]
pub struct Flusher {}

impl delog::Flusher for Flusher {
    fn flush(&self, logs: &str) {
        println!("{}", logs);
    }
}

delog!(Delogger, 16*1024, 3*1024, Flusher);
static FLUSHER: Flusher = Flusher {};

#[allow(non_camel_case_types)]
pub mod littlefs_params {
    use super::*;
    pub const READ_SIZE: usize = 16;
    pub const WRITE_SIZE: usize = 512;
    pub const BLOCK_SIZE: usize = 512;

    pub const BLOCK_COUNT: usize = 256;
    // no wear-leveling for now
    pub const BLOCK_CYCLES: isize = -1;

    pub type CACHE_SIZE = U512;
    pub type LOOKAHEADWORDS_SIZE = U16;
    /// TODO: We can't actually be changed currently
    pub type FILENAME_MAX_PLUS_ONE = U256;
    pub type PATH_MAX_PLUS_ONE = U256;
    pub const FILEBYTES_MAX: usize = littlefs2::ll::LFS_FILE_MAX as _;
    /// TODO: We can't actually be changed currently
    pub type ATTRBYTES_MAX = U1022;
}

pub struct FileFlash {
    state: [u8; 128 * 1024],
}
impl FileFlash {
    pub fn new() -> Self {
        let mut state = [0u8; 128 * 1024];

        if let Ok(contents) = std::fs::read(SOLO_STATE) {
            println!("loaded {}", SOLO_STATE);
            state.copy_from_slice( contents.as_slice() );
            Self {state}
        } else {
            println!("No state yet, creating");
            Self {state}
        }
    }
}

impl littlefs2::driver::Storage for FileFlash {
    const READ_SIZE: usize = littlefs_params::READ_SIZE;
    const WRITE_SIZE: usize = littlefs_params::WRITE_SIZE;
    const BLOCK_SIZE: usize = littlefs_params::BLOCK_SIZE;

    const BLOCK_COUNT: usize = littlefs_params::BLOCK_COUNT;
    const BLOCK_CYCLES: isize = littlefs_params::BLOCK_CYCLES;

    type CACHE_SIZE = littlefs_params::CACHE_SIZE;
    type LOOKAHEADWORDS_SIZE = littlefs_params::LOOKAHEADWORDS_SIZE;
    type FILENAME_MAX_PLUS_ONE = littlefs_params::FILENAME_MAX_PLUS_ONE;
    type PATH_MAX_PLUS_ONE = littlefs_params::PATH_MAX_PLUS_ONE;
    const FILEBYTES_MAX: usize = littlefs_params::FILEBYTES_MAX;
    type ATTRBYTES_MAX = littlefs_params::ATTRBYTES_MAX;


    fn read(&self, off: usize, buf: &mut [u8]) -> LfsResult<usize> {
        for i in 0 .. buf.len() {
            buf[i] = self.state[i + off];
        }
        Ok(buf.len())
    }

    fn write(&mut self, off: usize, data: &[u8]) -> LfsResult<usize> {
        for i in 0 .. data.len() {
            self.state[i + off] = data[i];
        }
        let mut buffer = File::create(SOLO_STATE).unwrap();
        buffer.write(&self.state).unwrap();

        Ok(data.len())
    }

    fn erase(&mut self, off: usize, len: usize) -> LfsResult<usize> {
        for i in 0 .. len {
            self.state[i + off] = 0;
        }
        let mut buffer = File::create(SOLO_STATE).unwrap();
        buffer.write(&self.state).unwrap();
        Ok(len)
    }

}

// 8KB of RAM
const_ram_storage!(
    name=VolatileStorage,
    trait=LfsStorage,
    erase_value=0x00,
    read_size=1,
    write_size=1,
    cache_size_ty=consts::U128,
    // this is a limitation of littlefs
    // https://git.io/JeHp9
    block_size=128,
    // block_size=128,
    block_count=8192/128,
    lookaheadwords_size_ty=consts::U8,
    filename_max_plus_one_ty=consts::U256,
    path_max_plus_one_ty=consts::U256,
    result=LfsResult,
);

// minimum: 2 blocks
// TODO: make this optional
const_ram_storage!(ExternalStorage, 1024);

store!(Store,
    Internal: FileFlash,
    External: ExternalStorage,
    Volatile: VolatileStorage
);



// #[derive(Default)]
// pub struct Rng {
//     count: u64,
// }

// impl rng::Read for Rng {
//     type Error = core::convert::Infallible;
//     fn read(&mut self, buffer: &mut [u8]) -> core::result::Result<(), Self::Error> {
//         // bad
//         for i in 0 .. buffer.len() {
//             self.count += 1;
//             buffer[i] = (self.count & 0xff) as u8;
//         }
//         Ok(())
//     }
// }


#[derive(Default)]
pub struct UserInterface {
}

impl trussed::platform::UserInterface for UserInterface
{
    fn check_user_presence(&mut self) -> consent::Level {
        consent::Level::Normal
    }

    fn set_status(&mut self, status: ui::Status) {

        println!("Set status: {:?}", status);

    }

    fn refresh(&mut self) {

    }

    fn uptime(&mut self) -> core::time::Duration {
        core::time::Duration::from_millis(1000)
    }

    fn reboot(&mut self, to: reboot::To) -> ! {
        println!("Restart!  ({:?})", to);
        std::process::exit(25);
    }

}

platform!(Board,
    R: chacha20::ChaCha8Rng,
    S: Store,
    UI: UserInterface,
);

fn main () {

    Delogger::init_default(delog::LevelFilter::Debug, &FLUSHER).ok();
    info_now!("starting to log");

    let filesystem = FileFlash::new();

    static mut INTERNAL_STORAGE: Option<FileFlash> = None;
    unsafe { INTERNAL_STORAGE = Some(filesystem); }
    static mut INTERNAL_FS_ALLOC: Option<Allocation<FileFlash>> = None;
    unsafe { INTERNAL_FS_ALLOC = Some(Filesystem::allocate()); }

    static mut EXTERNAL_STORAGE: ExternalStorage = ExternalStorage::new();
    static mut EXTERNAL_FS_ALLOC: Option<Allocation<ExternalStorage>> = None;
    unsafe { EXTERNAL_FS_ALLOC = Some(Filesystem::allocate()); }

    static mut VOLATILE_STORAGE: VolatileStorage = VolatileStorage::new();
    static mut VOLATILE_FS_ALLOC: Option<Allocation<VolatileStorage>> = None;
    unsafe { VOLATILE_FS_ALLOC = Some(Filesystem::allocate()); }


    let store = Store::claim().unwrap();

    let result = store.mount(
        unsafe { INTERNAL_FS_ALLOC.as_mut().unwrap() },
        // unsafe { &mut INTERNAL_STORAGE },
        unsafe { INTERNAL_STORAGE.as_mut().unwrap() },
        unsafe { EXTERNAL_FS_ALLOC.as_mut().unwrap() },
        unsafe { &mut EXTERNAL_STORAGE },
        unsafe { VOLATILE_FS_ALLOC.as_mut().unwrap() },
        unsafe { &mut VOLATILE_STORAGE },
        // to trash existing data, set to true
        false,
    );

    if result.is_err() {
        println!("Not yet formatted!  Formatting..");
        store.mount(
            unsafe { INTERNAL_FS_ALLOC.as_mut().unwrap() },
            // unsafe { &mut INTERNAL_STORAGE },
            unsafe { INTERNAL_STORAGE.as_mut().unwrap() },
            unsafe { EXTERNAL_FS_ALLOC.as_mut().unwrap() },
            unsafe { &mut EXTERNAL_STORAGE },
            unsafe { VOLATILE_FS_ALLOC.as_mut().unwrap() },
            unsafe { &mut VOLATILE_STORAGE },
            // to trash existing data, set to true
            true,
        ).unwrap();
    }


    use trussed::service::SeedableRng;
    let rng = chacha20::ChaCha8Rng::from_seed([0u8; 32]);
    let pc_interface: UserInterface = Default::default();

    let board = Board::new(rng, store, pc_interface);
    let mut trussed_service = trussed::service::Service::new(board);

    println!("hello trussed");

    use usb_device::prelude::*;
    use usb_device::class_prelude::*;
    use usbip_device::UsbIpBus;

    static mut USB_BUS: Option<UsbBusAllocator<UsbIpBus>> = None;
    unsafe { USB_BUS.replace(UsbBusAllocator::new(UsbIpBus::new())) };
    let usb_bus = unsafe { USB_BUS.as_ref().unwrap() };

    use trussed::Interchange;
    let (contact_requester, _contact_responder) = apdu_dispatch::interchanges::Contact::claim()
        .expect("could not setup ccid ApduInterchange");

    let (hid_requester, hid_responder) = ctaphid_dispatch::types::HidInterchange::claim()
        .expect("could not setup HidInterchange");

    let mut ctaphid = usbd_ctaphid::CtapHid::new(&usb_bus, hid_requester, 100);
    // let mut ccid = usbd_ccid::Ccid::new(&usb_bus, contact_requester);

    let mut usb_device = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x1209, 0xcc1d))
        .manufacturer("SoloKeys")
        .product("Solo 2 PC")
        .serial_number("TEST")
        // .max_packet_size_0(64)
        // .composite_with_iads()
        .build();

    let (piv_trussed_requester, _piv_trussed_responder) = trussed::pipe::TrussedInterchange::claim()
        .expect("could not setup PIV TrussedInterchange");

    let piv_trussed = trussed::ClientImplementation::new(
        piv_trussed_requester,
        &mut trussed_service,
    );
    let _piv = piv_authenticator::Authenticator::new(piv_trussed);

    loop {
        // usb_device.poll(&mut [&mut ccid]);
        usb_device.poll(&mut [&mut ctaphid]);
        Delogger::flush();
    }


}
