use crate::internal::{endian, error_helper};

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
        unsafe { endian::encode_be_unaligned(buffer, &(number as u8)) };
    } else {
        unsafe { endian::encode_be_unaligned(buffer, &(number as u32 | 0x8000_0000)) };
    }
}

pub(crate) unsafe fn decode_length_prefix(buffer: *const u8, offset: &mut usize, limits: usize) -> Result<usize, Box<dyn std::error::Error>> {
    assert!(limits >= *offset);
    if limits == *offset {
        return Err(error_helper::error_not_enough_bytes());
    }
    let source = unsafe { buffer.add(*offset) };
    let header = unsafe { endian::decode_be_unaligned::<u8>(source) } as u32;
    *offset += 1;
    if (header & 0x80) == 0 {
        return Ok(header as usize);
    }
    assert!(limits >= *offset);
    if limits < *offset + 3 {
        return Err(error_helper::error_not_enough_bytes());
    }
    let result = unsafe { endian::decode_be_unaligned::<u32>(source) };
    *offset += 3;
    return Ok((result & 0x7FFF_FFFF) as usize);
}
