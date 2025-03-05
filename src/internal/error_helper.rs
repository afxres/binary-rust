pub(crate) fn error_allocator_max_capacity_overflow() -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(std::io::ErrorKind::StorageFull, "maximum capacity has been reached."))
}

pub(crate) fn error_allocator_allocate_failed() -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(std::io::ErrorKind::OutOfMemory, "out of memory."))
}

pub(crate) fn error_allocator_invalid() -> Box<dyn std::error::Error> {
    Box::<dyn std::error::Error>::from("allocator has been modified unexpectedly!")
}

pub(crate) fn error_not_enough_bytes() -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, "not enough bytes."))
}

pub(crate) fn error_not_enough_bytes_to_write() -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not enough bytes to write."))
}
