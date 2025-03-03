pub fn error_allocator_max_capacity_overflow() -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(std::io::ErrorKind::StorageFull, "maximum capacity has been reached."))
}

pub fn error_allocator_allocate_failed() -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(std::io::ErrorKind::OutOfMemory, "out of memory."))
}

pub fn error_not_enough_bytes() -> Box<dyn std::error::Error> {
    Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, "not enough bytes."))
}
