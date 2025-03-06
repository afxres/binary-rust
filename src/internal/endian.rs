#![allow(dead_code)]

pub(crate) unsafe fn encode_le_unaligned<T>(target: *mut u8, item: T) {
    match std::mem::size_of::<T>() {
        1 => unsafe { target.write_unaligned(*(std::ptr::from_ref(&item).cast::<u8>())) },
        2 => unsafe { target.cast::<u16>().write_unaligned((*(std::ptr::from_ref(&item).cast::<u16>())).to_le()) },
        4 => unsafe { target.cast::<u32>().write_unaligned((*(std::ptr::from_ref(&item).cast::<u32>())).to_le()) },
        8 => unsafe { target.cast::<u64>().write_unaligned((*(std::ptr::from_ref(&item).cast::<u64>())).to_le()) },
        _ => panic!("not supported"),
    }
}

pub(crate) unsafe fn encode_be_unaligned<T>(target: *mut u8, item: T) {
    match std::mem::size_of::<T>() {
        1 => unsafe { target.write_unaligned(*(std::ptr::from_ref(&item).cast::<u8>())) },
        4 => unsafe { target.cast::<u32>().write_unaligned((*(std::ptr::from_ref(&item).cast::<u32>())).to_be()) },
        _ => panic!("not supported"),
    }
}

pub(crate) unsafe fn decode_be_unaligned<T>(target: *const u8) -> T {
    match std::mem::size_of::<T>() {
        1 => unsafe { std::ptr::from_ref(&target.read_unaligned()).cast::<T>().read_unaligned() },
        4 => unsafe { std::ptr::from_ref(&target.cast::<u32>().read_unaligned().to_be()).cast::<T>().read_unaligned() },
        _ => panic!("not supported"),
    }
}
