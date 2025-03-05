use crate::internal::endian;

pub(crate) fn ensure_length_prefix_length(number: usize) -> Result<(), Box<dyn std::error::Error>> {
    if number > i32::MAX as usize {
        return Err(Box::<dyn std::error::Error>::from("number > i32::MAX"));
    }
    Ok(())
}

pub(crate) fn encode_length_prefix_length(number: usize) -> usize {
    assert!(number <= i32::MAX as usize);
    if ((number as u32) >> 7) == 0 {
        return 1;
    } else {
        return 4;
    }
}

pub(crate) unsafe fn encode_length_prefix(buffer: *mut u8, number: usize, length: usize) {
    assert!(number <= i32::MAX as usize);
    assert!(length == 1 || length == 4);
    assert!(length >= encode_length_prefix_length(number));
    if length == 1 {
        unsafe { endian::encode_be_unaligned(buffer, number as u8) };
    } else {
        unsafe { endian::encode_be_unaligned(buffer, number as u32 | 0x8000_0000) };
    }
}
